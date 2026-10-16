//! Per-model context-window override: `[model."<id>"] context_window = N` in config.toml,
//! set from the `/context <tokens>` command, the `context_window` session config option,
//! or the `_supercharge/session/context_window` extension method.

use agent_client_protocol as acp;

use crate::agent::handlers::model_switch::{self, ConfigNotice, SwitchEffort};
use crate::agent::mvp_agent::MvpAgent;

/// Extension method (wire name `_supercharge/session/context_window`).
pub(crate) const CONTEXT_WINDOW_METHOD: &str = "supercharge/session/context_window";
/// Session config option id.
pub(crate) const CONFIG_ID_CONTEXT_WINDOW: &str = "context_window";
/// Select value that clears the override.
pub(crate) const AUTO: &str = "auto";
/// Preset windows offered by the `context_window` select.
pub(crate) const PRESETS: [u64; 6] = [128_000, 200_000, 256_000, 400_000, 1_000_000, 2_000_000];

const MIN_WINDOW: u64 = 1_000;
const MAX_WINDOW: u64 = 100_000_000;

/// Parse a window: `1000000`, `1m`, `1M`, `256k`, `1.5m`, `200_000`. `auto`/`reset` → `Ok(None)`.
pub fn parse_context_window(input: &str) -> Result<Option<u64>, String> {
    let s = input.trim().replace(['_', ','], "").to_ascii_lowercase();
    if s.is_empty() {
        return Err("expected a token count like 1m, 256k or 200000".into());
    }
    if s == AUTO || s == "reset" || s == "default" {
        return Ok(None);
    }
    let (num, mult) = match s.strip_suffix('m') {
        Some(n) => (n, 1_000_000f64),
        None => match s.strip_suffix('k') {
            Some(n) => (n, 1_000f64),
            None => (s.as_str(), 1f64),
        },
    };
    let value: f64 = num
        .parse()
        .map_err(|_| format!("invalid context window '{}'", input.trim()))?;
    let tokens = (value * mult).round();
    if !(tokens >= MIN_WINDOW as f64 && tokens <= MAX_WINDOW as f64) {
        return Err(format!(
            "context window must be between {MIN_WINDOW} and {MAX_WINDOW} tokens"
        ));
    }
    Ok(Some(tokens as u64))
}

/// `1000000` → `1M`, `256000` → `256k`.
pub fn format_context_window(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens % 100_000 == 0 {
        let m = tokens as f64 / 1_000_000.0;
        if m.fract() == 0.0 {
            format!("{}M", m as u64)
        } else {
            format!("{m:.1}M")
        }
    } else if tokens >= 1_000 && tokens % 1_000 == 0 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Set or clear `context_window` under `[model."<id>"]` in the TOML document.
/// Clearing removes the key and drops the table when nothing else is left in it.
pub(crate) fn set_override_in_document(
    document: &mut toml_edit::DocumentMut,
    model_id: &str,
    value: Option<u64>,
) -> Result<(), String> {
    let model = document
        .entry("model")
        .or_insert_with(|| {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            toml_edit::Item::Table(t)
        })
        .as_table_mut()
        .ok_or("[model] is not a table")?;
    match value {
        Some(tokens) => {
            let entry = model
                .entry(model_id)
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .ok_or_else(|| format!("[model.\"{model_id}\"] is not a table"))?;
            entry.insert("context_window", toml_edit::value(tokens as i64));
        }
        None => {
            if let Some(entry) = model.get_mut(model_id).and_then(|i| i.as_table_mut()) {
                entry.remove("context_window");
                if entry.is_empty() {
                    model.remove(model_id);
                }
            }
        }
    }
    Ok(())
}

fn persist_override(model_id: &str, value: Option<u64>) -> Result<(), String> {
    let path = crate::util::config::user_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = crate::util::config::read_to_string_or_empty(&path).map_err(|e| e.to_string())?;
    let mut document = if content.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| e.to_string())?
    };
    set_override_in_document(&mut document, model_id, value)?;
    crate::util::config::atomic_write_string(&path, &document.to_string()).map_err(|e| e.to_string())
}

/// The session's model and its effective window, plus whether a config override is set.
pub(crate) fn current_window(agent: &MvpAgent, model_id: &str) -> (Option<u64>, Option<u64>) {
    let effective = agent
        .models_manager
        .models()
        .get(model_id)
        .map(|e| e.info.context_window.get());
    let overridden = agent
        .cfg
        .borrow()
        .config_models
        .get(model_id)
        .and_then(|o| o.context_window);
    (effective, overridden)
}

/// Apply (or clear, with `None`) the override for the session's current model: persist it,
/// rebuild the catalog (so `availableModels[]._meta.totalContextTokens` changes), then re-apply
/// the model to the session so its window, `context_usage.maxTokens` and auto-compact threshold follow.
/// Returns the effective window afterwards.
pub(crate) async fn apply_override(
    agent: &MvpAgent,
    session_id: &acp::SessionId,
    value: Option<u64>,
) -> Result<u64, acp::Error> {
    let handle = agent
        .session_handle_waiting_for_load(session_id)
        .await
        .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
    let model_id = handle.model_id.clone();
    let key = model_id.0.to_string();
    persist_override(&key, value)
        .map_err(|e| acp::Error::internal_error().data(format!("saving config failed: {e}")))?;
    {
        let mut cfg = agent.cfg.borrow_mut();
        match value {
            Some(tokens) => {
                cfg.config_models.entry(key.clone()).or_default().context_window = Some(tokens);
            }
            None => {
                // An entry left with only defaults is harmless: `apply` changes nothing.
                if let Some(o) = cfg.config_models.get_mut(&key) {
                    o.context_window = None;
                }
            }
        }
    }
    let cfg = agent.cfg.borrow().clone();
    agent.models_manager.apply_config(cfg);
    model_switch::apply(
        agent,
        acp::SetSessionModelRequest::new(session_id.clone(), model_id.clone()),
        SwitchEffort::Preserve,
        ConfigNotice::Send,
    )
    .await?;
    current_window(agent, &key)
        .0
        .ok_or_else(|| acp::Error::internal_error().data("model missing from catalog"))
}

/// `context_window` select: `auto` plus presets, plus the current value when it isn't a preset.
pub(crate) fn config_option(effective: u64, overridden: Option<u64>) -> acp::SessionConfigOption {
    let mut values = vec![acp::SessionConfigSelectOption::new(
        AUTO,
        format!("Auto ({})", format_context_window(effective)),
    )];
    let mut windows: Vec<u64> = PRESETS.to_vec();
    if let Some(v) = overridden
        && !windows.contains(&v)
    {
        windows.push(v);
        windows.sort_unstable();
    }
    for w in windows {
        values.push(acp::SessionConfigSelectOption::new(
            w.to_string(),
            format_context_window(w),
        ));
    }
    let current = overridden.map_or_else(|| AUTO.to_string(), |v| v.to_string());
    acp::SessionConfigOption::select(CONFIG_ID_CONTEXT_WINDOW, "Context Window", current, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_suffixes_and_reset() {
        assert_eq!(parse_context_window("1M"), Ok(Some(1_000_000)));
        assert_eq!(parse_context_window("1m"), Ok(Some(1_000_000)));
        assert_eq!(parse_context_window("256k"), Ok(Some(256_000)));
        assert_eq!(parse_context_window("1.5m"), Ok(Some(1_500_000)));
        assert_eq!(parse_context_window("200_000"), Ok(Some(200_000)));
        assert_eq!(parse_context_window("reset"), Ok(None));
        assert_eq!(parse_context_window("auto"), Ok(None));
        assert!(parse_context_window("10").is_err());
        assert!(parse_context_window("lots").is_err());
    }

    #[test]
    fn formats_windows() {
        assert_eq!(format_context_window(1_000_000), "1M");
        assert_eq!(format_context_window(256_000), "256k");
        assert_eq!(format_context_window(1_500_000), "1.5M");
        assert_eq!(format_context_window(123_456), "123456");
    }

    #[test]
    fn document_override_set_and_clear() {
        let mut doc: toml_edit::DocumentMut = "[model.\"a/b\"]\nname = \"A\"\n".parse().unwrap();
        set_override_in_document(&mut doc, "a/b", Some(1_000_000)).unwrap();
        set_override_in_document(&mut doc, "c", Some(256_000)).unwrap();
        let v: toml::Value = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(v["model"]["a/b"]["context_window"].as_integer(), Some(1_000_000));
        assert_eq!(v["model"]["c"]["context_window"].as_integer(), Some(256_000));
        set_override_in_document(&mut doc, "a/b", None).unwrap();
        set_override_in_document(&mut doc, "c", None).unwrap();
        let v: toml::Value = toml::from_str(&doc.to_string()).unwrap();
        assert!(v["model"]["a/b"].get("context_window").is_none());
        assert_eq!(v["model"]["a/b"]["name"].as_str(), Some("A"));
        assert!(v["model"].get("c").is_none());
    }

    #[test]
    fn select_has_auto_presets_and_custom_current() {
        let opt = config_option(1_000_000, Some(300_000));
        let acp::SessionConfigKind::Select(select) = &opt.kind else {
            panic!("select expected");
        };
        assert_eq!(select.current_value.0.as_ref(), "300000");
        let json = serde_json::to_value(&opt).unwrap();
        let text = json.to_string();
        assert!(text.contains("\"auto\"") && text.contains("Auto (1M)"));
        assert!(text.contains("\"1000000\"") && text.contains("\"300000\""));
        let opt = config_option(256_000, None);
        let acp::SessionConfigKind::Select(select) = &opt.kind else {
            panic!("select expected");
        };
        assert_eq!(select.current_value.0.as_ref(), "auto");
    }
}
