//! `supercharge acp` — wire Supercharge into ACP clients (Zed).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::{Map, Value};

const AGENT_NAME: &str = "Supercharge";

#[derive(Debug, clap::Args, Clone)]
pub struct AcpArgs {
    #[command(subcommand)]
    pub command: AcpCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum AcpCommand {
    /// Write ACP client settings so Supercharge appears in the editor agent picker
    Setup {
        /// Editor to configure (`zed`)
        #[arg(long, value_name = "EDITOR", default_value = "zed")]
        editor: String,
        /// Print the settings snippet instead of writing a file
        #[arg(long)]
        print: bool,
    },
}

pub fn run(args: AcpArgs) -> Result<()> {
    match args.command {
        AcpCommand::Setup { editor, print } => setup(&editor, print),
    }
}

fn setup(editor: &str, print: bool) -> Result<()> {
    let editor = editor.trim().to_ascii_lowercase();
    if editor != "zed" {
        anyhow::bail!("unsupported editor `{editor}`; currently only `--editor zed` is supported");
    }
    let snippet = supercharge_agent_server_entry();
    if print {
        println!("{}", serde_json::to_string_pretty(&snippet)?);
        return Ok(());
    }
    let path = zed_settings_path()?;
    write_zed_settings(&path, snippet)?;
    println!("Wrote Supercharge ACP agent to {}", path.display());
    println!("In Zed: open the Agent Panel (cmd-? / ctrl-?) and start a Supercharge thread.");
    println!(
        "If you are not signed in yet, authenticate in the thread or run `supercharge login`."
    );
    Ok(())
}

fn supercharge_command() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| dunce::canonicalize(p).ok())
        .map(|p| p.display().to_string())
        .filter(|p| {
            let name = Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            name.starts_with("supercharge") || name == "sc" || name.starts_with("supercharge-pager")
        })
        .unwrap_or_else(|| "supercharge".to_string())
}

fn supercharge_agent_server_entry() -> Value {
    serde_json::json!({
        "type": "custom",
        "command": supercharge_command(),
        "args": ["agent", "stdio"]
    })
}

fn zed_settings_path() -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("ZED_CONFIG_DIR") {
        return Ok(PathBuf::from(custom).join("settings.json"));
    }
    #[cfg(windows)]
    {
        let appdata = std::env::var("APPDATA").context("APPDATA is not set")?;
        return Ok(PathBuf::from(appdata).join("Zed").join("settings.json"));
    }
    #[cfg(not(windows))]
    {
        let config = if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".config")
        };
        Ok(config.join("zed").join("settings.json"))
    }
}

fn write_zed_settings(path: &Path, agent: Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if looks_like_jsonc(&raw) {
            let next = insert_agent_server_jsonc(&raw, AGENT_NAME, &agent)?;
            fs::write(path, next).with_context(|| format!("failed to write {}", path.display()))?;
            return Ok(());
        }
        let mut root = if raw.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            serde_json::from_str(raw.trim())
                .with_context(|| format!("{} is not valid JSON", path.display()))?
        };
        merge_agent_server(&mut root, agent)?;
        let serialized = serde_json::to_string_pretty(&root)?;
        fs::write(path, format!("{serialized}\n"))
            .with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(());
    }

    let mut root = Value::Object(Map::new());
    merge_agent_server(&mut root, agent)?;
    let serialized = serde_json::to_string_pretty(&root)?;
    fs::write(path, format!("{serialized}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn merge_agent_server(root: &mut Value, agent: Value) -> Result<()> {
    let obj = root
        .as_object_mut()
        .context("Zed settings.json must be a JSON object")?;
    let servers = obj
        .entry("agent_servers")
        .or_insert_with(|| Value::Object(Map::new()));
    let servers = servers
        .as_object_mut()
        .context("`agent_servers` in Zed settings.json must be a JSON object")?;
    servers.insert(AGENT_NAME.to_string(), agent);
    Ok(())
}

fn looks_like_jsonc(raw: &str) -> bool {
    raw.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with("//") || t.starts_with("/*")
    })
}

fn insert_agent_server_jsonc(raw: &str, name: &str, agent: &Value) -> Result<String> {
    let entry = format!(
        "    \"{}\": {}",
        name,
        serde_json::to_string_pretty(agent)?.replace('\n', "\n    ")
    );
    if let Some(start) = raw.find(&format!("\"{name}\"")) {
        // Replace existing Supercharge object: from the key through its matching braces.
        let rest = &raw[start..];
        let brace = rest
            .find('{')
            .context("existing agent entry has no object")?;
        let obj_start = start + brace;
        let obj_end = matching_brace_end(raw, obj_start)?;
        let mut out = String::with_capacity(raw.len() + entry.len());
        out.push_str(&raw[..start]);
        out.push_str(entry.trim_start());
        out.push_str(&raw[obj_end + 1..]);
        return Ok(out);
    }
    let marker = "\"agent_servers\"";
    let idx = raw
        .find(marker)
        .context("settings.json has comments but no agent_servers object; use --print")?;
    let after = &raw[idx + marker.len()..];
    let rel_brace = after.find('{').context("agent_servers is not an object")?;
    let insert_at = idx + marker.len() + rel_brace + 1;
    let mut out = String::with_capacity(raw.len() + entry.len() + 2);
    out.push_str(&raw[..insert_at]);
    out.push('\n');
    out.push_str(&entry);
    out.push(',');
    out.push_str(&raw[insert_at..]);
    Ok(out)
}

fn matching_brace_end(s: &str, open: usize) -> Result<usize> {
    let bytes = s.as_bytes();
    anyhow::ensure!(bytes.get(open) == Some(&b'{'), "expected '{{'");
    let mut depth = 0i32;
    let mut i = open;
    while let Some(byte) = bytes.get(i).copied() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            b'"' => {
                i += 1;
                while let Some(byte) = bytes.get(i).copied() {
                    if byte == b'\\' {
                        i += 2;
                        continue;
                    }
                    if byte == b'"' {
                        break;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    anyhow::bail!("unbalanced braces in settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_agent_into_empty_settings() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_zed_settings(&path, supercharge_agent_server_entry()).unwrap();
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["agent_servers"]["Supercharge"]["args"][1], "stdio");
        assert_eq!(parsed["agent_servers"]["Supercharge"]["args"][0], "agent");
    }

    #[test]
    fn preserves_existing_settings_and_other_agents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"theme":"dark","agent_servers":{"Other":{"type":"custom","command":"x"}}}"#,
        )
        .unwrap();
        write_zed_settings(&path, supercharge_agent_server_entry()).unwrap();
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["theme"], "dark");
        assert_eq!(parsed["agent_servers"]["Other"]["command"], "x");
        assert_eq!(parsed["agent_servers"]["Supercharge"]["type"], "custom");
    }

    #[test]
    fn jsonc_insert_preserves_comments() {
        let raw = r#"// Zed settings
{
  "theme": "dark",
  "agent_servers": {
    "Other": {
      "type": "registry"
    }
  }
}
"#;
        let next = insert_agent_server_jsonc(
            raw,
            "Supercharge",
            &serde_json::json!({"type":"custom","command":"supercharge","args":["agent","stdio"]}),
        )
        .unwrap();
        assert!(next.contains("// Zed settings"));
        assert!(next.contains("\"Supercharge\""));
        assert!(next.contains("\"Other\""));
    }
}
