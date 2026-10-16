//! Thinking-parameter capability probe and its on-disk cache.
//!
//! Providers spell "think harder" differently. For a model whose thinking support is unknown,
//! [`SamplingClient::probe_thinking`](crate::client::SamplingClient::probe_thinking) sends a few
//! tiny requests, each with a different parameter shape, and records the first shape the
//! provider clearly accepts plus the effort levels it takes. The result is cached per
//! provider base URL + model in `~/.supercharge/cache/thinking-probe.json`, and chat requests
//! are rewritten to that shape by [`apply_shape_to_chat_body`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The request parameter a provider uses to enable thinking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingShape {
    /// Chat Completions `reasoning_effort: "<level>"`.
    ReasoningEffort,
    /// `reasoning: { effort: "<level>" }` (OpenRouter style).
    ReasoningObject,
    /// `thinking: { type: "enabled", budget_tokens: N }`.
    ThinkingBudget,
    /// `enable_thinking: true` (Qwen style).
    EnableThinking,
    /// `include_reasoning: true`.
    IncludeReasoning,
    /// Responses API `reasoning: { effort }`.
    ResponsesReasoning,
    /// No thinking parameter is accepted.
    Unsupported,
}

impl ThinkingShape {
    /// Whether the shape carries discrete effort levels.
    pub fn has_levels(self) -> bool {
        matches!(
            self,
            Self::ReasoningEffort | Self::ReasoningObject | Self::ResponsesReasoning
        )
    }
}

/// Cached probe outcome for one provider + model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingProbeResult {
    pub shape: ThinkingShape,
    /// Accepted effort ids, strongest first (`xhigh`, `high`, `medium`, `low`; `none` when it can be disabled).
    pub efforts: Vec<String>,
    /// Unix seconds when the probe ran.
    #[serde(default)]
    pub probed_at: u64,
}

impl ThinkingProbeResult {
    pub fn supported(&self) -> bool {
        self.shape != ThinkingShape::Unsupported
    }
}

/// Normalized cache key for a provider base URL and model.
pub fn cache_key(base_url: &str, model: &str) -> String {
    format!("{}|{}", base_url.trim().trim_end_matches('/').to_ascii_lowercase(), model.trim())
}

fn cache_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SUPERCHARGE_THINKING_PROBE_CACHE") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".supercharge").join("cache").join("thinking-probe.json"))
}

fn cache() -> &'static Mutex<Option<HashMap<String, ThinkingProbeResult>>> {
    static CACHE: OnceLock<Mutex<Option<HashMap<String, ThinkingProbeResult>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn with_cache<R>(f: impl FnOnce(&mut HashMap<String, ThinkingProbeResult>) -> R) -> R {
    let mut guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| {
        cache_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    f(map)
}

/// Cached probe result for this provider + model, if any.
pub fn lookup(base_url: &str, model: &str) -> Option<ThinkingProbeResult> {
    let key = cache_key(base_url, model);
    with_cache(|m| m.get(&key).cloned())
}

/// Record a probe result in memory and on disk (best effort).
pub fn store(base_url: &str, model: &str, result: ThinkingProbeResult) {
    let key = cache_key(base_url, model);
    let snapshot = with_cache(|m| {
        m.insert(key, result);
        m.clone()
    });
    if let Some(path) = cache_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&snapshot) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

/// Clear the in-memory cache (tests).
#[doc(hidden)]
pub fn reset_memory_cache() {
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Thinking budget used for budget-style providers.
pub fn budget_for_effort(effort: &str) -> u64 {
    match effort {
        "minimal" | "low" => 2_048,
        "medium" => 8_192,
        "xhigh" | "max" => 32_000,
        _ => 16_384,
    }
}

/// The JSON parameters for `shape` at `effort` (`"none"` disables thinking where possible).
pub fn shape_params(shape: ThinkingShape, effort: &str) -> Map<String, Value> {
    let mut m = Map::new();
    let off = effort == "none";
    match shape {
        ThinkingShape::ReasoningEffort => {
            m.insert("reasoning_effort".into(), json!(effort));
        }
        ThinkingShape::ReasoningObject => {
            m.insert("reasoning".into(), json!({ "effort": effort }));
        }
        ThinkingShape::ResponsesReasoning => {
            m.insert("reasoning".into(), json!({ "effort": effort }));
        }
        ThinkingShape::ThinkingBudget => {
            let v = if off {
                json!({ "type": "disabled" })
            } else {
                json!({ "type": "enabled", "budget_tokens": budget_for_effort(effort) })
            };
            m.insert("thinking".into(), v);
        }
        ThinkingShape::EnableThinking => {
            m.insert("enable_thinking".into(), json!(!off));
        }
        ThinkingShape::IncludeReasoning => {
            m.insert("include_reasoning".into(), json!(!off));
        }
        ThinkingShape::Unsupported => {}
    }
    m
}

/// Rewrite a serialized Chat Completions body so its `reasoning_effort` uses the cached shape.
/// Returns true when the body changed.
pub fn apply_shape_to_chat_body(body: &mut Value, result: &ThinkingProbeResult) -> bool {
    let Some(obj) = body.as_object_mut() else {
        return false;
    };
    let Some(effort) = obj
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    match result.shape {
        ThinkingShape::ReasoningEffort | ThinkingShape::ResponsesReasoning => {
            // Clamp to a level the provider accepted.
            if !result.efforts.is_empty() && !result.efforts.iter().any(|e| *e == effort) {
                if effort == "none" {
                    obj.remove("reasoning_effort");
                    return true;
                }
                if let Some(best) = result.efforts.iter().find(|e| e.as_str() != "none") {
                    obj.insert("reasoning_effort".into(), json!(best));
                    return true;
                }
            }
            false
        }
        ThinkingShape::Unsupported => {
            obj.remove("reasoning_effort");
            true
        }
        shape => {
            obj.remove("reasoning_effort");
            if shape == ThinkingShape::ThinkingBudget && effort != "none" {
                let budget = budget_for_effort(&effort);
                let max = obj.get("max_tokens").and_then(Value::as_u64);
                if max.is_some_and(|m| m <= budget) {
                    obj.insert("max_tokens".into(), json!(budget + 4_096));
                }
            }
            obj.extend(shape_params(shape, &effort));
            true
        }
    }
}

/// How a probe response reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// 2xx with reasoning output or reasoning tokens.
    Clear,
    /// 2xx without evidence (the parameter may have been ignored).
    Tentative,
    /// 4xx naming the parameter, or a generic 400/422.
    Rejected,
    /// Network/auth/server failure: tells us nothing.
    Inconclusive,
}

/// Classify one probe response.
pub fn classify_probe_response(status: u16, body: &str, param_names: &[&str]) -> ProbeVerdict {
    if (200..300).contains(&status) {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let reasoning_tokens = [
            "/usage/completion_tokens_details/reasoning_tokens",
            "/usage/output_tokens_details/reasoning_tokens",
            "/usage/reasoning_tokens",
        ]
        .iter()
        .filter_map(|p| v.pointer(p).and_then(Value::as_u64))
        .any(|n| n > 0);
        let has_reasoning_text = [
            "/choices/0/message/reasoning_content",
            "/choices/0/message/reasoning",
            "/choices/0/message/thinking",
        ]
        .iter()
        .filter_map(|p| v.pointer(p).and_then(Value::as_str))
        .any(|s| !s.trim().is_empty())
            || v.get("output")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|i| i.get("type").and_then(Value::as_str) == Some("reasoning"))
                });
        return if reasoning_tokens || has_reasoning_text {
            ProbeVerdict::Clear
        } else {
            ProbeVerdict::Tentative
        };
    }
    // A 400/422 on a request that differs from a plain one only by this parameter is a rejection.
    if status == 400 || status == 422 {
        let _ = param_names;
        return ProbeVerdict::Rejected;
    }
    ProbeVerdict::Inconclusive
}

/// Effort levels to try after a level-carrying shape is accepted with `low`.
pub const EXTRA_LEVELS: [&str; 2] = ["xhigh", "none"];

/// Chat Completions shapes, in probe order.
pub const CHAT_SHAPES: [ThinkingShape; 5] = [
    ThinkingShape::ReasoningEffort,
    ThinkingShape::ReasoningObject,
    ThinkingShape::ThinkingBudget,
    ThinkingShape::EnableThinking,
    ThinkingShape::IncludeReasoning,
];

/// Most requests a single probe sends.
pub const MAX_PROBE_REQUESTS: usize = 6;

/// Build the effort list for an accepted shape given which extra levels passed.
pub fn efforts_for(shape: ThinkingShape, xhigh_ok: bool, none_ok: bool) -> Vec<String> {
    if !shape.has_levels() {
        return vec!["high".into()];
    }
    let mut v: Vec<String> = Vec::new();
    if xhigh_ok {
        v.push("xhigh".into());
    }
    v.extend(["high", "medium", "low"].map(String::from));
    if none_ok {
        v.push("none".into());
    }
    v
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_render_expected_params() {
        assert_eq!(Value::Object(shape_params(ThinkingShape::ReasoningObject, "low")), json!({"reasoning":{"effort":"low"}}));
        assert_eq!(
            Value::Object(shape_params(ThinkingShape::ThinkingBudget, "medium")),
            json!({"thinking":{"type":"enabled","budget_tokens":8192}})
        );
        assert_eq!(Value::Object(shape_params(ThinkingShape::EnableThinking, "none")), json!({"enable_thinking":false}));
        assert_eq!(Value::Object(shape_params(ThinkingShape::IncludeReasoning, "high")), json!({"include_reasoning":true}));
    }

    #[test]
    fn rewrites_chat_body_to_cached_shape() {
        let r = ThinkingProbeResult { shape: ThinkingShape::ThinkingBudget, efforts: vec!["high".into()], probed_at: 0 };
        let mut body = json!({"model":"m","reasoning_effort":"high","max_tokens":1000});
        assert!(apply_shape_to_chat_body(&mut body, &r));
        assert_eq!(body, json!({"model":"m","max_tokens":20480,"thinking":{"type":"enabled","budget_tokens":16384}}));

        let r = ThinkingProbeResult { shape: ThinkingShape::ReasoningEffort, efforts: efforts_for(ThinkingShape::ReasoningEffort, false, false), probed_at: 0 };
        let mut body = json!({"reasoning_effort":"xhigh"});
        assert!(apply_shape_to_chat_body(&mut body, &r));
        assert_eq!(body, json!({"reasoning_effort":"high"}));

        let r = ThinkingProbeResult { shape: ThinkingShape::Unsupported, efforts: vec![], probed_at: 0 };
        let mut body = json!({"reasoning_effort":"low"});
        assert!(apply_shape_to_chat_body(&mut body, &r));
        assert_eq!(body, json!({}));
    }

    #[test]
    fn classifies_probe_responses() {
        assert_eq!(classify_probe_response(200, r#"{"usage":{"completion_tokens_details":{"reasoning_tokens":12}}}"#, &[]), ProbeVerdict::Clear);
        assert_eq!(classify_probe_response(200, r#"{"choices":[{"message":{"reasoning_content":"hmm"}}]}"#, &[]), ProbeVerdict::Clear);
        assert_eq!(classify_probe_response(200, r#"{"choices":[{"message":{"content":"hi"}}]}"#, &[]), ProbeVerdict::Tentative);
        assert_eq!(classify_probe_response(400, "unknown parameter reasoning_effort", &["reasoning_effort"]), ProbeVerdict::Rejected);
        assert_eq!(classify_probe_response(500, "", &[]), ProbeVerdict::Inconclusive);
        assert_eq!(classify_probe_response(401, "", &[]), ProbeVerdict::Inconclusive);
    }

    #[test]
    fn cache_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("tp-cache-{}", std::process::id()));
        let path = dir.join("probe.json");
        // SAFETY: test-only env mutation scoped to this process's probe cache.
        unsafe { std::env::set_var("SUPERCHARGE_THINKING_PROBE_CACHE", &path) };
        reset_memory_cache();
        let r = ThinkingProbeResult { shape: ThinkingShape::EnableThinking, efforts: vec!["high".into()], probed_at: 1 };
        store("https://Api.Example/v1/", "qwen3-max", r.clone());
        reset_memory_cache();
        assert_eq!(lookup("https://api.example/v1", "qwen3-max"), Some(r));
        let _ = std::fs::remove_dir_all(dir);
        unsafe { std::env::remove_var("SUPERCHARGE_THINKING_PROBE_CACHE") };
        reset_memory_cache();
    }
}
