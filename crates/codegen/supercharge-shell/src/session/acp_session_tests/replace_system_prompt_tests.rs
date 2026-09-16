//! Actor-path coverage for `handle_replace_system_prompt`, the resident-reconnect `systemPromptOverride` sync.
//! The head swap itself is unit-tested in `supercharge_chat_state` (`conversation_util` and the actor tests).
//! These tests cover only what is unique to the `SessionActor` path: the end-to-end swap and the `preserve_inherited_system` skip.

use supercharge_sampling_types::conversation::ConversationItem;

use super::support::create_test_actor;
use super::{PersistenceMsg, SessionActor};

#[tokio::test(flavor = "current_thread")]
async fn ponytail_toggle_is_idempotent_and_preserves_user_slash_text() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let actor = actor_with_history(vec![
                ConversationItem::system("base policy"),
                ConversationItem::user("/setup custom-key"),
                ConversationItem::assistant("unchanged reply"),
            ])
            .await;
            let before = actor.chat_state_handle.get_conversation().await;
            let turns = serde_json::to_value(&before[1..]).unwrap();
            for _ in 0..2 {
                actor.refresh_ponytail_system_prompt(true).await;
                let conv = actor.chat_state_handle.get_conversation().await;
                assert_eq!(
                    head_text(&conv).unwrap(),
                    format!("base policy\n\n{}", supercharge_shared::ponytail::SYSTEM_BLOCK),
                );
                assert_eq!(serde_json::to_value(&conv[1..]).unwrap(), turns);
            }
            actor.refresh_ponytail_system_prompt(false).await;
            let conv = actor.chat_state_handle.get_conversation().await;
            assert_eq!(head_text(&conv).as_deref(), Some("base policy"));
            assert_eq!(serde_json::to_value(&conv[1..]).unwrap(), turns);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ponytail_toggle_preserves_inherited_system_verbatim() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let inherited = format!("parent policy\n\n{}", supercharge_shared::ponytail::SYSTEM_BLOCK);
            let mut actor = actor_with_history(vec![
                ConversationItem::system(inherited.clone()),
                ConversationItem::user("/skill argument"),
            ])
            .await;
            actor.startup_hints.preserve_inherited_system = true;
            for enabled in [false, true] {
                actor.refresh_ponytail_system_prompt(enabled).await;
                let conv = actor.chat_state_handle.get_conversation().await;
                assert_eq!(head_text(&conv).as_deref(), Some(inherited.as_str()));
            }
        })
        .await;
}

fn head_text(conv: &[ConversationItem]) -> Option<String> {
    match conv.first() {
        Some(ConversationItem::System(sys)) => Some(sys.content.to_string()),
        _ => None,
    }
}

async fn actor_with_history(history: Vec<ConversationItem>) -> SessionActor {
    let (gateway_tx, _grx) =
        tokio::sync::mpsc::unbounded_channel::<supercharge_acp_lib::AcpClientMessage>();
    let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.chat_state_handle.replace_conversation(history);
    actor
}

#[tokio::test(flavor = "current_thread")]
async fn handle_replace_system_prompt_replaces_head_and_preserves_turns() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = actor_with_history(vec![
                ConversationItem::system("composer default"),
                ConversationItem::user("hi"),
                ConversationItem::assistant("yo"),
            ])
            .await;

            actor
                .handle_replace_system_prompt("client override".to_string())
                .await;

            let conv = actor.chat_state_handle.get_conversation().await;
            assert_eq!(head_text(&conv).as_deref(), Some("client override"));
            assert_eq!(conv.len(), 3, "must not wipe user/assistant turns");
            assert!(matches!(conv.get(1), Some(ConversationItem::User(_))));
            assert!(matches!(conv.get(2), Some(ConversationItem::Assistant(_))));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn handle_replace_system_prompt_skips_on_preserve_inherited_system() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut actor = actor_with_history(vec![
                ConversationItem::system("parent verbatim"),
                ConversationItem::user("hi"),
            ])
            .await;
            // Verbatim mirror-fork: the inherited cache prefix must survive.
            actor.startup_hints.preserve_inherited_system = true;

            actor
                .handle_replace_system_prompt("client override".to_string())
                .await;

            let conv = actor.chat_state_handle.get_conversation().await;
            assert_eq!(
                head_text(&conv).as_deref(),
                Some("parent verbatim"),
                "preserve_inherited_system must not overwrite the inherited head"
            );
        })
        .await;
}
