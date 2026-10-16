use agent_client_protocol::{self as acp};

use crate::agent::handlers::model_switch::{self, ConfigNotice};
use crate::agent::mvp_agent::MvpAgent;
use crate::agent::session_config::{
    CONFIG_ID_MODEL, CONFIG_ID_PERMISSION_MODE, CONFIG_ID_REASONING_EFFORT,
};
use crate::session::persistence::PersistenceMsg;

async fn persist_permission_mode(
    agent: &MvpAgent,
    session_id: &acp::SessionId,
    mode: &str,
) -> Result<(), acp::Error> {
    let handle = agent
        .resident_handle(session_id)
        .ok_or_else(|| acp::Error::invalid_params().data("session not found"))?;
    let (respond_to, response) = tokio::sync::oneshot::channel();
    handle
        .persistence_tx
        .send(PersistenceMsg::PermissionMode {
            mode: mode.to_owned(),
            respond_to,
        })
        .map_err(|_| acp::Error::internal_error().data("permission persistence unavailable"))?;
    response
        .await
        .map_err(|_| acp::Error::internal_error().data("permission persistence unavailable"))?
        .map_err(|_| acp::Error::internal_error().data("permission persistence failed"))
}

pub(crate) async fn apply(
    agent: &MvpAgent,
    args: acp::SetSessionConfigOptionRequest,
) -> Result<acp::SetSessionConfigOptionResponse, acp::Error> {
    let acp::SetSessionConfigOptionRequest {
        session_id,
        config_id,
        value,
        ..
    } = args;

    match config_id.0.as_ref() {
        CONFIG_ID_MODEL => {
            let value_id = value.as_value_id().ok_or_else(|| {
                acp::Error::invalid_params().data("model requires a string value")
            })?;
            let request = acp::SetSessionModelRequest::new(
                session_id.clone(),
                acp::ModelId::new(value_id.0.clone()),
            );
            agent.set_model_gated(request).await?;
        }
        CONFIG_ID_REASONING_EFFORT => {
            let value_id = value.as_value_id().ok_or_else(|| {
                acp::Error::invalid_params().data("reasoning_effort requires a string value")
            })?;
            // The selector is resolved inside `apply_reasoning_effort` under the config lock, so
            // it is validated against the model the effort will actually run on.
            model_switch::apply_reasoning_effort(
                agent,
                session_id.clone(),
                value_id.0.as_ref(),
                ConfigNotice::Send,
            )
            .await?;
        }
        CONFIG_ID_PERMISSION_MODE => {
            let value_id = value.as_value_id().ok_or_else(|| {
                acp::Error::invalid_params().data("permission_mode requires a string value")
            })?;
            let requested = value_id.0.as_ref();
            if !matches!(requested, "ask" | "auto" | "always-approve") {
                return Err(acp::Error::invalid_params().data("unknown permission mode"));
            }
            if requested == "auto" && !crate::util::config::auto_permission_mode_enabled_from_disk()
            {
                return Err(
                    acp::Error::invalid_params().data("auto permission mode is unavailable")
                );
            }
            if requested == "always-approve"
                && supercharge_workspace::permission::resolution::yolo_disabled_by_policy()
                    .is_some()
            {
                return Err(acp::Error::invalid_params().data("always-approve disabled by policy"));
            }
            let _config_guard = agent.config_mutation_lock(&session_id).lock_owned().await;
            let previous = agent.permission_mode_for_session(&session_id, None).await?;
            if requested == "ask" {
                persist_permission_mode(agent, &session_id, requested).await?;
            }
            agent
                .permission_mode_for_session(&session_id, Some(requested))
                .await?;
            if requested != "ask"
                && let Err(error) = persist_permission_mode(agent, &session_id, requested).await
            {
                let _ = agent
                    .permission_mode_for_session(&session_id, Some(&previous))
                    .await;
                return Err(error);
            }
            let options = agent.acp_config_options_for_session(&session_id).await?;
            if !options.iter().any(|option| {
                option.id.0.as_ref() == CONFIG_ID_PERMISSION_MODE
                    && matches!(&option.kind, acp::SessionConfigKind::Select(select) if select.current_value.0.as_ref() == requested)
            }) {
                return Err(acp::Error::invalid_params().data("permission mode changed while applying"));
            }
            agent
                .gateway
                .forward_fire_and_forget(acp::SessionNotification::new(
                    session_id,
                    acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(
                        options.clone(),
                    )),
                ));
            return Ok(acp::SetSessionConfigOptionResponse::new(options));
        }
        crate::agent::handlers::context_window::CONFIG_ID_CONTEXT_WINDOW => {
            let value_id = value.as_value_id().ok_or_else(|| {
                acp::Error::invalid_params().data("context_window requires a string value")
            })?;
            let parsed = crate::agent::handlers::context_window::parse_context_window(
                value_id.0.as_ref(),
            )
            .map_err(|e| acp::Error::invalid_params().data(e))?;
            crate::agent::handlers::context_window::apply_override(agent, &session_id, parsed)
                .await?;
        }
        other => {
            return Err(
                acp::Error::invalid_params().data(format!("unknown config option: {other}"))
            );
        }
    }

    let options = agent.acp_config_options_for_session(&session_id).await?;
    Ok(acp::SetSessionConfigOptionResponse::new(options))
}
