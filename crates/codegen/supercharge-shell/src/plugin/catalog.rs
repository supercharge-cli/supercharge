//! Built-in packages are embedded in the binary, never fetched from a moving Git ref.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub endpoint: String,
    pub authentication: String,
    pub source: String,
}

pub fn list() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            name: "microsoft-learn".into(),
            version: "1.0.0".into(),
            description: "Research official Microsoft documentation".into(),
            endpoint: "https://learn.microsoft.com/api/mcp".into(),
            authentication: "none".into(),
            source: "starter:microsoft-learn".into(),
        },
        CatalogEntry {
            name: "github".into(),
            version: "1.0.0".into(),
            description: "GitHub repository tools and release briefs".into(),
            endpoint: "https://api.githubcopilot.com/mcp/".into(),
            authentication: "oauth_or_bearer".into(),
            source: "starter:github".into(),
        },
    ]
}

/// Resolve only known embedded packages. All installation still goes through the acquisition gate.
pub fn materialize(name: &str) -> Result<PathBuf, String> {
    let files: &[(&str, &str)] = match name {
        "microsoft-learn" => &[
            (
                "plugin.json",
                include_str!("starter/microsoft-learn/plugin.json"),
            ),
            ("mcp.json", include_str!("starter/microsoft-learn/mcp.json")),
            (
                "skills/documentation-research/SKILL.md",
                include_str!("starter/microsoft-learn/skills/documentation-research/SKILL.md"),
            ),
        ],
        "github" => &[
            ("plugin.json", include_str!("starter/github/plugin.json")),
            ("mcp.json", include_str!("starter/github/mcp.json")),
            (
                "skills/release-brief/SKILL.md",
                include_str!("starter/github/skills/release-brief/SKILL.md"),
            ),
        ],
        _ => return Err("Unknown starter plugin".into()),
    };
    let home = supercharge_config::user_supercharge_home().ok_or("Supercharge home unavailable")?;
    let root = home.join("bundled-plugins").join("1.0.0").join(name);
    for (relative, content) in files {
        let path = root.join(relative);
        let parent = path.parent().ok_or("Invalid bundled path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        file.persist(&path).map_err(|e| e.to_string())?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_packages_have_pinned_identity_and_valid_configs() {
        for (name, manifest, mcp) in [
            (
                "github",
                include_str!("starter/github/plugin.json"),
                include_str!("starter/github/mcp.json"),
            ),
            (
                "microsoft-learn",
                include_str!("starter/microsoft-learn/plugin.json"),
                include_str!("starter/microsoft-learn/mcp.json"),
            ),
        ] {
            let manifest: supercharge_agent::plugins::manifest::PluginManifest =
                serde_json::from_str(manifest).unwrap();
            manifest.validate().unwrap();
            assert_eq!(manifest.name, name);
            assert_eq!(manifest.version.as_deref(), Some("1.0.0"));
            let config: supercharge_config_types::McpConfig = serde_json::from_str(mcp).unwrap();
            assert_eq!(config.mcp_servers.len(), 1);
        }
        assert!(super::materialize("../escape").is_err());
    }
}

pub fn list_available() -> anyhow::Result<Vec<CatalogEntry>> {
    let remote = match super::remote_catalog::list() {
        Ok(remote) => remote,
        Err(error) if super::remote_catalog::is_implicit_default() => {
            tracing::warn!(%error, "Hosted plugin catalog unavailable; showing bundled plugins");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let mut bundled = list();
    bundled.retain(|p| !remote.iter().any(|r| r.name == p.name));
    bundled.extend(remote);
    Ok(bundled)
}
