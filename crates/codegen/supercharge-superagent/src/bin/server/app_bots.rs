use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const MAIN_BOT_ID: &str = "main";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotDefinition {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub instructions: String,
    pub default_folder: String,
    pub revision: u64,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BotCatalog {
    pub instance_id: String,
    pub capability: String,
    pub site: String,
    pub bots: BTreeMap<String, BotDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSyncRequest {
    pub instance_id: String,
    pub capability: String,
    pub site: String,
    pub bots: Vec<BotDefinition>,
}

pub fn default_main() -> BotDefinition {
    BotDefinition {
        id: MAIN_BOT_ID.to_owned(),
        name: "Main".to_owned(),
        icon: "🤖".to_owned(),
        instructions: String::new(),
        default_folder: "/home/user/workspace".to_owned(),
        revision: 1,
        archived: false,
    }
}

impl BotCatalog {
    pub fn load(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw).map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn get(&self, id: &str) -> Option<BotDefinition> {
        if id == MAIN_BOT_ID {
            Some(self.bots.get(id).cloned().unwrap_or_else(default_main))
        } else {
            self.bots.get(id).filter(|bot| !bot.archived).cloned()
        }
    }

    pub fn sync(
        &mut self,
        request: BotSyncRequest,
        path: &Path,
        expected_instance_id: &str,
    ) -> Result<(), String> {
        if expected_instance_id.is_empty()
            || expected_instance_id != request.instance_id
            || request.capability.len() < 32
            || request.capability.len() > 128
            || !request.site.starts_with("https://")
            || request.site.len() > 300
            || request.site.contains(['\r', '\n'])
        {
            return Err("invalid bot catalogue identity".to_owned());
        }
        if !self.instance_id.is_empty() && self.instance_id != request.instance_id {
            return Err("bot catalogue instance changed".to_owned());
        }
        let site = reqwest::Url::parse(&request.site).map_err(|_| "invalid bot service origin")?;
        if site.scheme() != "https"
            || !site.username().is_empty()
            || site.password().is_some()
            || site.host_str().is_none()
            || site.path() != "/"
            || site.query().is_some()
            || site.fragment().is_some()
        {
            return Err("invalid bot service origin".into());
        }
        let mut next = self.bots.clone();
        let mut seen = std::collections::HashSet::new();
        for bot in request.bots {
            if !seen.insert(bot.id.clone()) {
                return Err("duplicate bot identity".to_owned());
            }
            if bot.id.is_empty()
                || bot.id.len() > 80
                || !bot
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                || bot.name.trim().is_empty()
                || bot.name.len() > 160
                || bot.instructions.len() > 16_000
                || bot.icon.len() > 180_000
                || bot.revision == 0
                || validate_folder(&bot.default_folder).is_err()
            {
                return Err("invalid bot definition".to_owned());
            }
            if let Some(previous) = self.bots.get(&bot.id) {
                if bot.revision < previous.revision {
                    continue;
                }
                if bot.revision == previous.revision
                    && (bot.name != previous.name
                        || bot.icon != previous.icon
                        || bot.instructions != previous.instructions
                        || bot.default_folder != previous.default_folder
                        || bot.archived != previous.archived)
                {
                    return Err("conflicting bot catalogue revision".to_owned());
                }
            }
            next.insert(bot.id.clone(), bot);
        }
        if !next.get(MAIN_BOT_ID).is_some_and(|main| !main.archived) {
            return Err("missing or invalid Main bot".to_owned());
        }
        let updated = Self {
            instance_id: request.instance_id,
            capability: request.capability,
            site: request.site,
            bots: next,
        };
        updated.save(path)?;
        *self = updated;
        Ok(())
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let raw = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        supercharge_superagent::atomic_state::write_private(path, &raw)
            .map_err(|error| error.to_string())
    }
}

pub fn validate_folder(raw: &str) -> Result<PathBuf, String> {
    let configured = std::env::var("SUPERAGENT_SHARE_WORKSPACE_ROOT")
        .unwrap_or_else(|_| "/home/user/workspace".to_owned());
    let root = Path::new(&configured);
    let folder = Path::new(raw);
    if raw.len() > 256
        || !folder.is_absolute()
        || !folder.starts_with(root)
        || raw.contains(['\0', '\n', '\r', '\\'])
        || folder.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || raw
            .split('/')
            .skip(1)
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err("bot folder must be inside the sandbox workspace".to_owned());
    }
    let checked_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut prefix = root.to_path_buf();
    if let Ok(relative) = folder.strip_prefix(root) {
        for component in relative.components() {
            prefix.push(component);
            if let Ok(canonical) = prefix.canonicalize() {
                if !canonical.starts_with(&checked_root) {
                    return Err("bot folder resolves outside the sandbox workspace".to_owned());
                }
            }
        }
    }
    Ok(folder.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_main_exists_without_catalogue() {
        let empty = BotCatalog::default();
        let main = empty.get(MAIN_BOT_ID).unwrap();
        assert_eq!(main.default_folder, "/home/user/workspace");
        assert!(empty.get("unknown").is_none());
    }

    #[test]
    fn paths_reject_escape_and_accept_unique_bot_folder() {
        assert!(validate_folder("/home/user/workspace/bots/a").is_ok());
        for invalid in [
            "/home/user/workspace/../secret",
            "/tmp/x",
            "/home/user/workspace//bots",
            "/home/user/workspace/./bots",
        ] {
            assert!(validate_folder(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn sync_is_atomic_and_rejects_stale_revision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bots.json");
        let mut catalog = BotCatalog::default();
        let main = default_main();
        let request = || BotSyncRequest {
            instance_id: "inst-1".to_owned(),
            capability: "s".repeat(40),
            site: "https://example.test".to_owned(),
            bots: vec![main.clone()],
        };
        catalog.sync(request(), &path, "inst-1").unwrap();
        assert_eq!(
            BotCatalog::load(&path)
                .unwrap()
                .get(MAIN_BOT_ID)
                .unwrap()
                .name,
            "Main"
        );
        let mut conflicting = request();
        conflicting.bots[0].name = "Changed".to_owned();
        assert!(catalog.sync(conflicting, &path, "inst-1").is_err());
        assert_eq!(catalog.get(MAIN_BOT_ID).unwrap().name, "Main");
    }
}
