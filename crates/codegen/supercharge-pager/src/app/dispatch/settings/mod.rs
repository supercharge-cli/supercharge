pub(in crate::app::dispatch) mod setters;
pub(in crate::app::dispatch) mod ui;

use crate::app::actions::Effect;
use crate::app::app_view::AppView;
use supercharge_shell::agent::config::Feature;

/// One `[features]` write finished. The row that issued it settles and may issue the intent queued behind it;
/// `subagent_model_inheritance` is the only row today.
pub(in crate::app::dispatch) fn handle_feature_override_persisted(
    app: &mut AppView,
    feature: Feature,
    result: Result<Option<bool>, String>,
) -> Vec<Effect> {
    let persisted = match result {
        Ok(saved) => Some(saved),
        Err(error) => {
            tracing::warn!(target: "settings", key = feature.key(), %error, "setting persist failed");
            app.show_toast(&format!("✗ Could not save {}: {error}", feature.key()));
            None
        }
    };
    let effects = if feature == app.subagent_model_inheritance.feature {
        setters::settle_subagent_model_inheritance_write(app, persisted)
    } else {
        vec![]
    };
    ui::refresh_open_settings_modals(app);
    effects
}
