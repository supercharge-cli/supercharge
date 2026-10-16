use agent_client_protocol as acp;
use serde::Serialize;
use supercharge_sampling_types::{ReasoningEffort, ReasoningEffortOption};

use crate::session::unified_list::SessionKind;

/// The `reasoning_effort` select values when the model has no catalog list:
/// `off`, `low`, `medium`, `high`, `xhigh`. `off` is [`ReasoningEffort::None`].
pub(crate) const SELECTABLE_REASONING_EFFORTS: [ReasoningEffort; 5] = [
    ReasoningEffort::None,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Xhigh,
];

pub(crate) const CONFIG_ID_MODEL: &str = "model";
pub(crate) const CONFIG_ID_REASONING_EFFORT: &str = "reasoning_effort";
pub(crate) const CONFIG_ID_PERMISSION_MODE: &str = "permission_mode";

pub(crate) fn permission_mode_option(
    current: &str,
    auto_available: bool,
) -> acp::SessionConfigOption {
    let mut values = vec![acp::SessionConfigSelectOption::new("ask", "Ask")];
    if auto_available {
        values.push(acp::SessionConfigSelectOption::new("auto", "Auto"));
    }
    values.push(acp::SessionConfigSelectOption::new(
        "always-approve",
        "Always Approve",
    ));
    acp::SessionConfigOption::select(
        CONFIG_ID_PERMISSION_MODE,
        "Permission Mode",
        current.to_owned(),
        values,
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionConfigOption {
    pub id: String,
    pub category: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GrokSessionDetail {
    pub session_id: String,
    pub kind: String,
    pub cwd: String,
    pub current_model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl GrokSessionDetail {
    pub(crate) fn build(
        session_id: String,
        cwd: String,
        current_model_id: String,
        title: Option<String>,
    ) -> Self {
        Self {
            session_id,
            kind: SessionKind::Build.as_ref().to_string(),
            cwd,
            current_model_id,
            title,
        }
    }
}

fn effort_label(effort: ReasoningEffort) -> String {
    match effort {
        ReasoningEffort::None => "Off",
        ReasoningEffort::Minimal => "Minimal",
        ReasoningEffort::Low => "Low",
        ReasoningEffort::Medium => "Medium",
        ReasoningEffort::High => "High",
        ReasoningEffort::Xhigh => "X-High",
        ReasoningEffort::Max => "Max",
    }
    .to_string()
}

/// Select-value id for an effort: `off` for [`ReasoningEffort::None`], otherwise the canonical name.
pub(crate) fn effort_option_id(effort: ReasoningEffort) -> String {
    match effort {
        ReasoningEffort::None => "off".to_string(),
        other => other.as_ref().to_string(),
    }
}

fn effort_option(effort: ReasoningEffort) -> ReasoningEffortOption {
    ReasoningEffortOption {
        id: effort_option_id(effort),
        value: effort,
        label: effort_label(effort),
        description: None,
        default: false,
    }
}

/// The built-in session-picker modes used when the model has no server list:
/// off / low / medium / high / xhigh.
pub(crate) fn legacy_session_effort_options() -> Vec<ReasoningEffortOption> {
    SELECTABLE_REASONING_EFFORTS
        .iter()
        .map(|&effort| effort_option(effort))
        .collect()
}

/// Effort options for a provider model from its supported canonical ids
/// (`low`, `medium`, `high`, `xhigh`), weakest first, with `off` prepended.
/// Unknown ids are skipped; an empty input yields no options.
pub fn effort_options_from_ids(ids: &[&str]) -> Vec<ReasoningEffortOption> {
    let mut efforts: Vec<ReasoningEffort> = ids
        .iter()
        .filter_map(|id| id.parse::<ReasoningEffort>().ok())
        .filter(|e| *e != ReasoningEffort::None)
        .collect();
    if efforts.is_empty() {
        return Vec::new();
    }
    efforts.sort_by_key(|e| *e as u8);
    efforts.dedup();
    std::iter::once(ReasoningEffort::None)
        .chain(efforts)
        .map(effort_option)
        .collect()
}

fn model_display_name(model: &acp::ModelInfo) -> String {
    if model.name.is_empty() {
        model.model_id.0.to_string()
    } else {
        model.name.clone()
    }
}

pub(crate) fn build_session_config_options(
    available_models: &[acp::ModelInfo],
    current_model_id: &acp::ModelId,
    effort_options: &[ReasoningEffortOption],
    current_effort: Option<ReasoningEffort>,
) -> Vec<SessionConfigOption> {
    let mut options = Vec::with_capacity(available_models.len() + effort_options.len());

    for model in available_models {
        options.push(SessionConfigOption {
            id: model.model_id.0.to_string(),
            category: "model".to_string(),
            label: model_display_name(model),
            description: None,
            selected: model.model_id == *current_model_id,
        });
    }

    for effort in effort_options {
        options.push(SessionConfigOption {
            id: effort.id.clone(),
            category: "mode".to_string(),
            label: effort.label.clone(),
            description: effort.description.clone(),
            selected: Some(effort.value) == current_effort,
        });
    }

    options
}

pub(crate) fn build_acp_config_options(
    available_models: &[acp::ModelInfo],
    current_model_id: &acp::ModelId,
    effort_options: &[ReasoningEffortOption],
    current_effort: Option<ReasoningEffort>,
) -> Vec<acp::SessionConfigOption> {
    let mut options = Vec::new();

    if !available_models.is_empty() {
        let values: Vec<acp::SessionConfigSelectOption> = available_models
            .iter()
            .map(|model| {
                acp::SessionConfigSelectOption::new(
                    model.model_id.0.to_string(),
                    model_display_name(model),
                )
            })
            .collect();
        // Keep the real model even if the catalog doesn't list it.
        let current_value = current_model_id.0.to_string();
        options.push(
            acp::SessionConfigOption::select(CONFIG_ID_MODEL, "Model", current_value, values)
                .category(acp::SessionConfigOptionCategory::Model),
        );
    }

    if !effort_options.is_empty() {
        // Keep the real effort even if it isn't a listed option (e.g. none/max);
        // fall back to the model default when none is set.
        let current_value = match current_effort {
            Some(effort) => effort_options
                .iter()
                .find(|option| option.value == effort)
                .map(|option| option.id.clone())
                .unwrap_or_else(|| effort.as_ref().to_string()),
            None => {
                let Some(option) = effort_options
                    .iter()
                    .find(|option| option.default)
                    .or(effort_options.first())
                else {
                    return options;
                };
                option.id.clone()
            }
        };
        let values: Vec<acp::SessionConfigSelectOption> = effort_options
            .iter()
            .map(|option| {
                let mut value =
                    acp::SessionConfigSelectOption::new(option.id.clone(), option.label.clone());
                if let Some(description) = &option.description {
                    value = value.description(description.clone());
                }
                value
            })
            .collect();
        options.push(
            acp::SessionConfigOption::select(
                CONFIG_ID_REASONING_EFFORT,
                "Reasoning Effort",
                current_value,
                values,
            )
            .category(acp::SessionConfigOptionCategory::ThoughtLevel),
        );
    }

    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &'static str, name: &str) -> acp::ModelInfo {
        acp::ModelInfo::new(acp::ModelId::new(id), name.to_string())
    }

    #[test]
    fn options_have_one_selected_model_and_a_mode_per_effort() {
        let models = [
            model("Supercharge AI-build", "Supercharge AI"),
            model("Supercharge AI-4.5", "Supercharge Core"),
        ];
        let current = acp::ModelId::from("Supercharge AI-build");
        let opts = build_session_config_options(
            &models,
            &current,
            &legacy_session_effort_options(),
            Some(ReasoningEffort::High),
        );

        let model_opts: Vec<_> = opts.iter().filter(|o| o.category == "model").collect();
        assert_eq!(model_opts.len(), 2);
        let selected_models: Vec<_> = model_opts.iter().filter(|o| o.selected).collect();
        assert_eq!(selected_models.len(), 1);
        let Some(selected_model) = selected_models.first() else {
            panic!("expected one selected model: {selected_models:?}");
        };
        assert_eq!(selected_model.id, "Supercharge AI-build");

        let mode_opts: Vec<_> = opts.iter().filter(|o| o.category == "mode").collect();
        assert_eq!(mode_opts.len(), SELECTABLE_REASONING_EFFORTS.len());
        let selected_modes: Vec<_> = mode_opts.iter().filter(|o| o.selected).collect();
        assert_eq!(selected_modes.len(), 1);
        let Some(selected_mode) = selected_modes.first() else {
            panic!("expected one selected mode: {selected_modes:?}");
        };
        assert_eq!(selected_mode.id, "high");
        assert_eq!(selected_mode.label, "High");
    }

    #[test]
    fn off_is_selectable_and_maps_to_none() {
        let ids: Vec<String> = legacy_session_effort_options()
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, ["off", "low", "medium", "high", "xhigh"]);
        let models = [model("Supercharge AI-build", "Supercharge AI")];
        let current = acp::ModelId::from("Supercharge AI-build");
        let opts = build_session_config_options(
            &models,
            &current,
            &legacy_session_effort_options(),
            Some(ReasoningEffort::None),
        );
        let selected: Vec<_> = opts
            .iter()
            .filter(|o| o.category == "mode" && o.selected)
            .collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "off");
    }

    #[test]
    fn provider_effort_ids_are_filtered_and_get_off() {
        let ids: Vec<String> = effort_options_from_ids(&["xhigh", "high", "medium", "low", "bogus"])
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, ["off", "low", "medium", "high", "xhigh"]);
        let ids: Vec<String> = effort_options_from_ids(&["high", "medium", "low"])
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, ["off", "low", "medium", "high"]);
        assert!(effort_options_from_ids(&[]).is_empty());
    }

    #[test]
    fn acp_reasoning_effort_select_uses_off_id() {
        let models = [model("m", "M")];
        let current = acp::ModelId::from("m");
        let opts = build_acp_config_options(
            &models,
            &current,
            &effort_options_from_ids(&["high", "medium", "low"]),
            Some(ReasoningEffort::None),
        );
        let effort = opts
            .iter()
            .find(|o| o.id.0.as_ref() == CONFIG_ID_REASONING_EFFORT)
            .expect("reasoning_effort option");
        let acp::SessionConfigKind::Select(select) = &effort.kind else {
            panic!("expected select");
        };
        assert_eq!(select.current_value.0.as_ref(), "off");
    }

    #[test]
    fn no_mode_options_when_model_lacks_effort_support() {
        let models = [model("Supercharge AI-build", "Supercharge AI")];
        let current = acp::ModelId::from("Supercharge AI-build");
        let opts = build_session_config_options(&models, &current, &[], None);
        assert_eq!(opts.len(), 1);
        assert!(opts.iter().all(|o| o.category == "model"));
    }

    #[test]
    fn model_label_falls_back_to_id_when_name_empty() {
        let models = [model("Supercharge AI-build", "")];
        let current = acp::ModelId::from("Supercharge AI-build");
        let opts = build_session_config_options(&models, &current, &[], None);
        let Some(first) = opts.first() else {
            panic!("expected one option: {opts:?}");
        };
        assert_eq!(first.label, "Supercharge AI-build");
    }

    #[test]
    fn session_config_option_serializes_camel_case() {
        let opt = SessionConfigOption {
            id: "Supercharge AI-build".to_string(),
            category: "model".to_string(),
            label: "Supercharge AI".to_string(),
            description: None,
            selected: true,
        };
        let v = serde_json::to_value(&opt).expect("serialize");
        assert_eq!(
            v.get("id").and_then(|x| x.as_str()),
            Some("Supercharge AI-build")
        );
        assert_eq!(v.get("category").and_then(|x| x.as_str()), Some("model"));
        assert_eq!(
            v.get("label").and_then(|x| x.as_str()),
            Some("Supercharge AI")
        );
        assert_eq!(v.get("selected").and_then(|x| x.as_bool()), Some(true));
        assert!(v.get("description").is_none());
    }

    #[test]
    fn grok_session_detail_serializes_camel_case() {
        let detail = GrokSessionDetail::build(
            "sess-1".to_string(),
            "/Users/me/xai".to_string(),
            "Supercharge AI-build".to_string(),
            None,
        );
        let v = serde_json::to_value(&detail).expect("serialize");
        assert_eq!(v.get("sessionId").and_then(|x| x.as_str()), Some("sess-1"));
        assert_eq!(v.get("kind").and_then(|x| x.as_str()), Some("build"));
        assert_eq!(v.get("cwd").and_then(|x| x.as_str()), Some("/Users/me/xai"));
        assert_eq!(
            v.get("currentModelId").and_then(|x| x.as_str()),
            Some("Supercharge AI-build")
        );
        assert!(v.get("title").is_none());
    }

    #[test]
    fn acp_config_options_map_model_and_effort_selectors() {
        let models = [
            model("grok-build", "Supercharge AI"),
            model("grok-4.5", "Grok 4.5"),
        ];
        let efforts = [ReasoningEffortOption {
            id: "high".to_string(),
            value: ReasoningEffort::High,
            label: "High".to_string(),
            description: None,
            default: false,
        }];

        let options = build_acp_config_options(
            &models,
            &acp::ModelId::from("grok-4.5"),
            &efforts,
            Some(ReasoningEffort::High),
        );

        let expected = vec![
            acp::SessionConfigOption::select(
                CONFIG_ID_MODEL,
                "Model",
                "grok-4.5",
                vec![
                    acp::SessionConfigSelectOption::new("grok-build", "Supercharge AI"),
                    acp::SessionConfigSelectOption::new("grok-4.5", "Grok 4.5"),
                ],
            )
            .category(acp::SessionConfigOptionCategory::Model),
            acp::SessionConfigOption::select(
                CONFIG_ID_REASONING_EFFORT,
                "Reasoning Effort",
                "high",
                vec![acp::SessionConfigSelectOption::new("high", "High")],
            )
            .category(acp::SessionConfigOptionCategory::ThoughtLevel),
        ];
        assert_eq!(options, expected);
    }

    #[test]
    fn acp_config_options_effort_current_preserves_unlisted_value() {
        let models = [model("grok-4.5", "Grok 4.5")];
        let efforts = [ReasoningEffortOption {
            id: "high".to_string(),
            value: ReasoningEffort::High,
            label: "High".to_string(),
            description: None,
            default: false,
        }];
        let options = build_acp_config_options(
            &models,
            &acp::ModelId::from("grok-4.5"),
            &efforts,
            Some(ReasoningEffort::Low),
        );
        let effort = options
            .iter()
            .find(|o| o.id.0.as_ref() == CONFIG_ID_REASONING_EFFORT)
            .expect("effort selector present when the model supports effort");
        match &effort.kind {
            acp::SessionConfigKind::Select(select) => {
                assert_eq!(select.current_value.0.as_ref(), "low");
            }
            _ => panic!("effort must be a select"),
        }
    }

    #[test]
    fn permission_mode_options_respect_gate_and_current_state() {
        for (auto_available, current, expected) in [
            (false, "ask", vec!["ask", "always-approve"]),
            (true, "auto", vec!["ask", "auto", "always-approve"]),
            (
                true,
                "always-approve",
                vec!["ask", "auto", "always-approve"],
            ),
        ] {
            let option = permission_mode_option(current, auto_available);
            assert_eq!(option.id.0.as_ref(), CONFIG_ID_PERMISSION_MODE);
            match option.kind {
                acp::SessionConfigKind::Select(select) => {
                    assert_eq!(select.current_value.0.as_ref(), current);
                    let acp::SessionConfigSelectOptions::Ungrouped(values) = select.options else {
                        panic!("permission values must be ungrouped");
                    };
                    assert_eq!(
                        values
                            .iter()
                            .map(|o| o.value.0.as_ref())
                            .collect::<Vec<_>>(),
                        expected
                    );
                }
                _ => panic!("permission mode must be a select"),
            }
        }
    }

    #[test]
    fn acp_config_options_model_current_preserves_unlisted_value() {
        let models = [
            model("grok-build", "Supercharge AI"),
            model("grok-4.5", "Grok 4.5"),
        ];
        let options =
            build_acp_config_options(&models, &acp::ModelId::from("stale-model"), &[], None);
        let model = options
            .iter()
            .find(|o| o.id.0.as_ref() == CONFIG_ID_MODEL)
            .expect("model selector present");
        match &model.kind {
            acp::SessionConfigKind::Select(select) => {
                assert_eq!(select.current_value.0.as_ref(), "stale-model");
            }
            _ => panic!("model must be a select"),
        }
    }
}
