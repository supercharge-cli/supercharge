use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::commands::{AgentCommand, help_text, parse_command};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionControl {
    pub stopped: bool,
    pub requested_model: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedControls {
    sessions: BTreeMap<String, SessionControl>,
    #[serde(default)]
    emergency_stopped: bool,
}

#[derive(Debug)]
pub struct ControlStore {
    path: PathBuf,
    state: PersistedControls,
}

static GLOBAL: OnceLock<Arc<Mutex<ControlStore>>> = OnceLock::new();

impl ControlStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let path = path.into();
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            PersistedControls::default()
        };
        Ok(Self { path, state })
    }

    fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&self.state)?)?;
        std::fs::rename(temporary, &self.path)?;
        Ok(())
    }

    fn entry_mut(&mut self, session_key: &str) -> &mut SessionControl {
        self.state
            .sessions
            .entry(session_key.to_owned())
            .or_default()
    }

    pub fn is_stopped(&self, session_key: &str) -> bool {
        self.state
            .sessions
            .get(session_key)
            .is_some_and(|entry| entry.stopped)
    }

    pub fn is_emergency_stopped(&self) -> bool {
        self.state.emergency_stopped
    }

    pub fn set_emergency_stopped(&mut self, stopped: bool) -> Result<(), std::io::Error> {
        self.state.emergency_stopped = stopped;
        self.save()
    }

    pub fn status(&self, session_key: &str) -> SessionControl {
        self.state
            .sessions
            .get(session_key)
            .cloned()
            .unwrap_or_default()
    }

    pub fn apply(
        &mut self,
        session_key: &str,
        command: AgentCommand,
    ) -> Result<String, std::io::Error> {
        let entry = self.entry_mut(session_key);
        entry.updated_at = now_seconds();
        let response = match command {
            AgentCommand::New | AgentCommand::Start => {
                entry.stopped = false;
                "Session resumed."
            }
            AgentCommand::Stop => {
                entry.stopped = true;
                "Session stopped. Use /start or /new to resume it."
            }
            AgentCommand::Status => {
                return Ok(format!(
                    "Session status: {}. Requested model: {}.",
                    if entry.stopped { "stopped" } else { "running" },
                    entry.requested_model.as_deref().unwrap_or("daemon default")
                ));
            }
            AgentCommand::Help => return Ok(help_text().to_owned()),
            AgentCommand::Setup => {
                return Ok(
                    "Chat provider is configured in Vectra instance settings \
(provider, model, API key, base URL). In the Supercharge TUI use /setup."
                        .to_owned(),
                );
            }
            AgentCommand::ISetup => {
                return Ok(
                    "Image generation is configured in Vectra instance settings \
(Image generation: provider, model, format, optional key). \
In the Supercharge TUI use /isetup and /imodels. \
Then send /imagine <description>."
                        .to_owned(),
                );
            }
            AgentCommand::IModels(model) => {
                if let Some(model) = model {
                    if model.len() > 200 || model.chars().any(char::is_whitespace) {
                        return Ok(
                            "Image model IDs must be a single value of at most 200 characters."
                                .to_owned(),
                        );
                    }
                    if let Err(error) = persist_image_model(&model) {
                        return Ok(format!(
                            "Could not write [image_provider].model: {error}. Set it in instance settings."
                        ));
                    }
                    return Ok(format!(
                        "Image model set to `{model}`. Restart the daemon or start a new session so /imagine uses it."
                    ));
                }
                return Ok(
                    "Set an image model with /imodels <id> (any custom slug), or configure Image generation in Vectra instance settings / TUI /isetup."
                        .to_owned(),
                );
            }
            AgentCommand::Format(choice) => {
                return Ok(apply_format_command(choice.as_deref()));
            }
            AgentCommand::Model(model) => {
                if let Some(model) = model {
                    if model.len() > 128 || model.chars().any(char::is_whitespace) {
                        return Ok(
                            "Model IDs must be a single value of at most 128 characters."
                                .to_owned(),
                        );
                    }
                    entry.requested_model = Some(model);
                    "Requested model recorded for this session; restart the provider bridge to apply a process-wide model change."
                } else {
                    return Ok(format!(
                        "Requested model: {}.",
                        entry.requested_model.as_deref().unwrap_or("daemon default")
                    ));
                }
            }
            AgentCommand::Update(_) | AgentCommand::Cmd(_) => {
                unreachable!("update and cmd are handled in handle_text")
            }
        };
        self.save()?;
        Ok(response.to_owned())
    }
}

pub fn init_global(path: impl Into<PathBuf>) -> Result<(), std::io::Error> {
    let store = ControlStore::load(path)?;
    let _ = GLOBAL.set(Arc::new(Mutex::new(store)));
    Ok(())
}

pub fn global() -> Option<Arc<Mutex<ControlStore>>> {
    GLOBAL.get().cloned()
}

pub async fn handle_text(session_key: &str, text: &str) -> Result<Option<String>, std::io::Error> {
    handle_text_in(session_key, text, None).await
}

pub async fn handle_text_in(session_key: &str, text: &str, cwd: Option<&std::path::Path>) -> Result<Option<String>, std::io::Error> {
    let Some(command) = parse_command(text) else {
        return Ok(None);
    };
    if let AgentCommand::Update(action) = command {
        return Ok(Some(crate::self_update::handle(action).await));
    }
    if let AgentCommand::Cmd(command) = command {
        return Ok(Some(run_workspace_command(&command, cwd).await));
    }
    let Some(store) = global() else {
        return Ok(Some("Control store is not initialized.".to_owned()));
    };
    let mut store = store.lock().await;
    Ok(Some(store.apply(session_key, command)?))
}

pub async fn is_stopped(session_key: &str) -> bool {
    let Some(store) = global() else {
        return false;
    };
    store.lock().await.is_stopped(session_key)
}

pub async fn is_emergency_stopped() -> bool {
    let Some(store) = global() else {
        return false;
    };
    store.lock().await.is_emergency_stopped()
}

pub async fn set_emergency_stopped(stopped: bool) -> Result<(), std::io::Error> {
    let Some(store) = global() else {
        return Err(std::io::Error::other("control store is not initialized"));
    };
    store.lock().await.set_emergency_stopped(stopped)
}

async fn run_workspace_command(command: &str, cwd: Option<&std::path::Path>) -> String {
    let command = command.trim();
    if command.is_empty() {
        return "Usage: /cmd <command>  (runs in /home/user/workspace)".to_owned();
    }
    if command.len() > 4000 {
        return "Command is too long (max 4000 characters).".to_owned();
    }
    let cwd = cwd.unwrap_or_else(|| if std::path::Path::new("/home/user/workspace").is_dir() {
        std::path::Path::new("/home/user/workspace")
    } else {
        std::path::Path::new("/home/user")
    });
    let mut shell = tokio::process::Command::new("bash");
    shell
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match tokio::time::timeout(std::time::Duration::from_secs(120), shell.output()).await {
        Ok(Ok(output)) => {
            let mut text = String::new();
            if !output.stdout.is_empty() {
                text.push_str(&String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            let code = output.status.code().unwrap_or(-1);
            if text.trim().is_empty() {
                format!("exit {code} (no output)")
            } else {
                let clipped: String = text.chars().take(8000).collect();
                format!("exit {code}\n{clipped}")
            }
        }
        Ok(Err(error)) => format!("Command failed: {error}"),
        Err(_) => "Command timed out after 120s.".to_owned(),
    }
}

fn apply_format_command(choice: Option<&str>) -> String {
    let current = read_chat_format()
        .map(|format| format.display_name())
        .unwrap_or("Chat Completions");
    let Some(raw) = choice.map(str::trim).filter(|value| !value.is_empty()) else {
        return format!(
            "API format: {current}.\n\
Usage: /format chat_completions | messages | responses\n\
Aliases: openai, anthropic.\n\
This updates the saved provider without repeating instance settings. Restart the daemon if a running session still uses the old protocol."
        );
    };
    let Some(format) = crate::provider::parse_format(raw) else {
        return "Unknown format. Use chat_completions, messages, or responses.".to_owned();
    };
    if let Err(error) = persist_chat_format(format.clone()) {
        return format!(
            "Could not write api_backend: {error}. Set API format in instance settings."
        );
    }
    format!(
        "API format set to {} (`{}`). Restart the daemon or start a new session so inference uses it.",
        format.display_name(),
        format.api_backend()
    )
}

fn provider_config_path() -> PathBuf {
    std::env::var("SUPERAGENT_PROVIDER_CONFIG_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user/.supercharge/provider-config.toml"))
}

fn read_chat_format() -> Option<crate::provider::ProviderFormat> {
    let path = provider_config_path();
    let raw = std::fs::read_to_string(path).ok()?;
    let value = toml::from_str::<toml::Value>(&raw).ok()?;
    let models = value.get("model")?.as_table()?;
    for entry in models.values() {
        if let Some(backend) = entry.get("api_backend").and_then(toml::Value::as_str) {
            return crate::provider::parse_format(backend);
        }
    }
    None
}

fn persist_chat_format(format: crate::provider::ProviderFormat) -> Result<(), String> {
    let path = provider_config_path();
    let mut value = if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        toml::from_str::<toml::Value>(&raw).map_err(|error| error.to_string())?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    let table = value
        .as_table_mut()
        .ok_or_else(|| "provider-config.toml is not a table".to_owned())?;
    let models = table
        .entry("model".to_owned())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let models = models
        .as_table_mut()
        .ok_or_else(|| "model is not a table".to_owned())?;
    if models.is_empty() {
        return Err("no saved provider model to change. Save instance settings first.".to_owned());
    }
    for (_, entry) in models.iter_mut() {
        if let Some(model) = entry.as_table_mut() {
            model.insert(
                "api_backend".to_owned(),
                toml::Value::String(format.api_backend().to_owned()),
            );
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        toml::to_string_pretty(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let home = PathBuf::from("/home/user/.superagent");
    let env_path = home.join("daemon.env");
    if env_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&env_path) {
            let mut lines = Vec::new();
            let mut wrote = false;
            for line in raw.lines() {
                if line.starts_with("SUPERAGENT_PROVIDER_FORMAT=") {
                    lines.push(format!(
                        "SUPERAGENT_PROVIDER_FORMAT={}",
                        format.api_backend()
                    ));
                    wrote = true;
                } else {
                    lines.push(line.to_owned());
                }
            }
            if !wrote {
                lines.push(format!(
                    "SUPERAGENT_PROVIDER_FORMAT={}",
                    format.api_backend()
                ));
            }
            lines.push(String::new());
            let _ = std::fs::write(env_path, lines.join("\n"));
        }
    }
    let profile_path = home.join("profile-default.json");
    if profile_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&profile_path) {
            if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(obj) = json.as_object_mut() {
                    obj.insert(
                        "format".to_owned(),
                        serde_json::Value::String(format.api_backend().to_owned()),
                    );
                    let _ = std::fs::write(
                        profile_path,
                        serde_json::to_string_pretty(&json).unwrap_or(raw),
                    );
                }
            }
        }
    }
    Ok(())
}

fn persist_image_model(model: &str) -> Result<(), String> {
    let path = PathBuf::from("/home/user/.supercharge/config.toml");
    let mut value = if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        raw.parse::<toml::Value>().unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()))
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    let table = value.as_table_mut().ok_or_else(|| "config.toml is not a table".to_owned())?;
    let image = table
        .entry("image_provider".to_owned())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let image_table = image
        .as_table_mut()
        .ok_or_else(|| "image_provider is not a table".to_owned())?;
    image_table.insert("model".to_owned(), toml::Value::String(model.to_owned()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, toml::to_string_pretty(&value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::ControlStore;
    use crate::commands::AgentCommand;
    use tempfile::tempdir;

    #[test]
    fn control_state_survives_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("controls.json");
        let mut store = ControlStore::load(&path).unwrap();
        assert!(
            store
                .apply("session", AgentCommand::Stop)
                .unwrap()
                .contains("stopped")
        );
        assert!(ControlStore::load(&path).unwrap().is_stopped("session"));
        let mut restored = ControlStore::load(&path).unwrap();
        restored.set_emergency_stopped(true).unwrap();
        assert!(ControlStore::load(&path).unwrap().is_emergency_stopped());
    }

    #[test]
    fn format_command_requires_a_saved_model_table() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("provider-config.toml");
        unsafe {
            std::env::set_var("SUPERAGENT_PROVIDER_CONFIG_PATH", &path);
        }
        let msg = super::apply_format_command(Some("responses"));
        assert!(msg.contains("no saved provider") || msg.contains("Could not write"));
        std::fs::write(
            &path,
            "[model.custom]\nmodel = \"m\"\nbase_url = \"https://example.com/v1\"\napi_backend = \"chat_completions\"\n",
        )
        .unwrap();
        let ok = super::apply_format_command(Some("responses"));
        assert!(ok.contains("Responses"), "{ok}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("api_backend = \"responses\""));
        let listed = super::apply_format_command(None);
        assert!(listed.contains("Responses"), "{listed}");
    }
}
