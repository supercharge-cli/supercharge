//! BYOK provider errors and session usage must never enter a purchase flow.

use super::*;

#[test]
fn byok_provider_failures_remain_visible_and_keep_setup_and_usage_available() {
    use crate::app::acp_handler::apply_session_event_for_test;
    use supercharge_shell::extensions::notification::{RetryState, SessionUpdate};

    for (status, reason) in [
        (402, "Request failed (402): fake provider balance exhausted"),
        (
            403,
            "Request failed (403): fake provider has run out of credits",
        ),
        (
            429,
            "Request failed (429): subscription:free-usage-exhausted",
        ),
    ] {
        let mut app = test_app_with_agent();
        let id = AgentId(0);
        let effects = dispatch(Action::SendPrompt("hello fake provider".into()), &mut app);
        assert!(matches!(effects.as_slice(), [Effect::SendPrompt { .. }]));
        let prompt_id = test_agent(&app, id).session.current_prompt_id.clone();
        {
            let agent = app.agents.get_mut(&id).unwrap();
            let retry = if status == 429 {
                RetryState::Exhausted {
                    attempts: 2,
                    reason: reason.into(),
                    is_rate_limited: true,
                }
            } else {
                RetryState::Failed {
                    error_type: "api".into(),
                    message: reason.into(),
                }
            };
            apply_session_event_for_test(
                &SessionUpdate::RetryState(retry),
                &mut agent.session,
                &mut agent.scrollback,
            );
        }
        let effects = dispatch(
            Action::TaskComplete(TaskResult::PromptResponse {
                agent_id: id,
                result: Err(reason.into()),
                http_status: Some(status),
                prompt_id,
            }),
            &mut app,
        );
        assert!(
            effects.is_empty(),
            "provider {status} must schedule no RPC: {effects:?}"
        );
        let agent = test_agent(&app, id);
        assert!(
            agent.question_view.is_none(),
            "provider {status} created a purchase question"
        );
        assert!(agent.active_modal.is_none());
        assert!(agent.session.state.is_idle());
        assert!(app.has_access());
        let visible_errors: Vec<_> = (0..agent.scrollback.len())
            .filter_map(|idx| match &agent.scrollback.entry(idx)?.block {
                RenderBlock::SessionEvent(event) => Some(event.event.message()),
                _ => None,
            })
            .collect();
        assert!(
            !visible_errors.is_empty(),
            "provider {status} error disappeared"
        );
        let text = visible_errors.join("\n");
        if status != 429 {
            assert!(text.contains(&status.to_string()), "{text}");
            assert!(text.contains("fake provider"), "{text}");
        } else {
            assert!(text.to_lowercase().contains("rate limit"), "{text}");
        }
        for forbidden in [
            "Buy more credits",
            "Upgrade tier",
            "pay-as-you-go",
            "supercharge.ai",
            "grok.com",
        ] {
            assert!(!text.contains(forbidden), "provider {status}: {text}");
        }

        app.screen_mode = crate::app::ScreenMode::Minimal;
        assert!(matches!(
            dispatch(Action::SendPrompt("/usage".into()), &mut app).as_slice(),
            [Effect::FetchSessionUsage { .. }]
        ));
        let effects = dispatch(Action::SendPrompt("/setup".into()), &mut app);
        assert!(
            effects.is_empty(),
            "setup only opens the provider picker: {effects:?}"
        );
        assert!(matches!(
            test_agent(&app, id).active_modal,
            Some(crate::views::modal::ActiveModal::ProviderSetup { .. })
        ));
    }
}

fn complete_session_usage(
    app: &mut AppView,
    session_id: &str,
    usage: supercharge_shell::extensions::notification::PromptUsage,
) -> Vec<Effect> {
    dispatch(
        Action::TaskComplete(TaskResult::SessionUsageComplete {
            agent_id: AgentId(0),
            session_id: session_id.to_string().into(),
            usage: Box::new(usage),
            nonce: Default::default(),
        }),
        app,
    )
}

fn fail_session_usage(app: &mut AppView, session_id: &str, error: &str) -> Vec<Effect> {
    dispatch(
        Action::TaskComplete(TaskResult::SessionUsageFailed {
            agent_id: AgentId(0),
            session_id: session_id.to_string().into(),
            error: error.into(),
            nonce: Default::default(),
        }),
        app,
    )
}

#[test]
fn show_usage_schedules_session_fetch_only() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    app.usage_visible = false;
    assert!(matches!(
        dispatch(Action::ShowUsage, &mut app).as_slice(),
        [Effect::FetchSessionUsage { agent_id, .. }] if *agent_id == AgentId(0)
    ));
}

#[test]
fn show_usage_without_session_explains_unavailability_without_rpc() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    app.agents.get_mut(&AgentId(0)).unwrap().session.session_id = None;
    let effects = dispatch(Action::ShowUsage, &mut app);
    assert!(last_system_text(&app, AgentId(0)).contains("unavailable"));
    assert!(effects.is_empty());
}

#[test]
fn session_usage_keeps_tokens_and_cost_without_billing() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    let usage = supercharge_shell::extensions::notification::PromptUsage {
        totals: supercharge_shell::extensions::notification::PromptUsageModel {
            input_tokens: 1_000,
            output_tokens: 100,
            total_tokens: 1_100,
            model_calls: 3,
            cost_usd_ticks: Some(5_000_000_000),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(complete_session_usage(&mut app, "test-session", usage).is_empty());
    let text = last_system_text(&app, AgentId(0));
    assert!(
        text.contains("Session usage") && text.contains("$0.5000"),
        "{text}"
    );
    assert!(text.contains("1,000") && text.contains("100"), "{text}");
}

#[test]
fn session_usage_complete_drops_stale_session() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    assert!(complete_session_usage(&mut app, "old-session", Default::default()).is_empty());
    assert_eq!(agent_scrollback_len(&app), before);
}

#[test]
fn session_usage_failure_remains_visible_without_billing() {
    let mut app = test_app_with_agent();
    app.screen_mode = crate::app::ScreenMode::Minimal;
    assert!(fail_session_usage(&mut app, "test-session", "fake provider unavailable").is_empty());
    assert!(
        last_system_text(&app, AgentId(0))
            .contains("Couldn't load session usage: fake provider unavailable")
    );
}

#[test]
fn session_usage_failed_drops_stale_session() {
    let mut app = test_app_with_agent();
    let before = agent_scrollback_len(&app);
    assert!(fail_session_usage(&mut app, "old-session", "boom").is_empty());
    assert_eq!(agent_scrollback_len(&app), before);
}
