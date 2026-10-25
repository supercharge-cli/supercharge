//! Final shape pass for Chat Completions messages so strict OpenAI-compatible hosts accept the request.

use crate::types::{ChatContentBlock, ChatRequestMessage, MessageContent, Role};

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn has_text(content: &MessageContent) -> bool {
    match content {
        MessageContent::Text(t) => !t.trim().is_empty(),
        MessageContent::Blocks(blocks) => blocks.iter().any(|b| match b {
            ChatContentBlock::Text { text } => !text.trim().is_empty(),
            ChatContentBlock::ImageUrl { .. } => true,
        }),
    }
}

fn into_blocks(content: MessageContent) -> Vec<ChatContentBlock> {
    match content {
        MessageContent::Text(text) => vec![ChatContentBlock::Text { text }],
        MessageContent::Blocks(blocks) => blocks,
    }
}

fn merge_content(a: MessageContent, b: MessageContent) -> MessageContent {
    match (a, b) {
        (MessageContent::Text(a), MessageContent::Text(b)) => {
            if a.is_empty() {
                MessageContent::Text(b)
            } else if b.is_empty() {
                MessageContent::Text(a)
            } else {
                MessageContent::Text(format!("{a}\n\n{b}"))
            }
        }
        (a, b) => {
            let mut blocks = into_blocks(a);
            blocks.extend(into_blocks(b));
            MessageContent::Blocks(blocks)
        }
    }
}

/// Rewrites a message list into a shape every strict Chat Completions host accepts:
/// - internal-only fields (`model_id`) never go out, and invalid `name` values are dropped;
/// - only a leading system message stays `system`; later ones become `user` notes;
/// - assistant turns with neither text nor tool calls are dropped;
/// - empty user and tool text gets a short placeholder;
/// - back-to-back user turns, and a text-only assistant followed by another assistant, are merged;
/// - `reasoning_content` is only sent for the current turn (after the last user message).
pub fn normalize_chat_wire_messages(messages: &[ChatRequestMessage]) -> Vec<ChatRequestMessage> {
    let last_user = messages.iter().rposition(|m| m.role == Role::User);
    let mut out: Vec<ChatRequestMessage> = Vec::with_capacity(messages.len());
    for (i, msg) in messages.iter().enumerate() {
        let mut msg = msg.clone();
        msg.model_id = None;
        if msg.name.as_deref().is_some_and(|n| !valid_name(n)) {
            msg.name = None;
        }
        if msg.role == Role::System && i > 0 {
            msg.role = Role::User;
        }
        if msg.role != Role::Assistant || last_user.is_some_and(|u| i < u) {
            msg.reasoning_content = None;
        }
        if msg.reasoning_content.as_deref().is_some_and(|r| r.trim().is_empty()) {
            msg.reasoning_content = None;
        }
        if msg.role != Role::Assistant {
            msg.tool_calls.clear();
        }
        if msg.role != Role::Tool {
            msg.tool_call_id = None;
        }
        match msg.role {
            Role::Assistant if msg.tool_calls.is_empty() && !has_text(&msg.content) => continue,
            Role::User if !has_text(&msg.content) => {
                msg.content = merge_content(msg.content, MessageContent::Text("(empty message)".into()));
            }
            Role::Tool if !has_text(&msg.content) => {
                msg.content = merge_content(msg.content, MessageContent::Text("(no output)".into()));
            }
            _ => {}
        }
        if let Some(prev) = out.last_mut() {
            let mergeable = prev.role == msg.role
                && match msg.role {
                    Role::User => true,
                    Role::Assistant => prev.tool_calls.is_empty(),
                    _ => false,
                };
            if mergeable {
                let prev_content = std::mem::replace(&mut prev.content, MessageContent::Text(String::new()));
                prev.content = merge_content(prev_content, msg.content);
                prev.tool_calls = msg.tool_calls;
                prev.reasoning_content = match (prev.reasoning_content.take(), msg.reasoning_content) {
                    (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
                    (a, b) => a.or(b),
                };
                if prev.name != msg.name {
                    prev.name = None;
                }
                continue;
            }
        }
        out.push(msg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, ToolCallRequest};

    fn wire(messages: Vec<ChatRequestMessage>) -> serde_json::Value {
        let req = ChatCompletionRequest::new("m", messages);
        serde_json::to_value(&req).unwrap()["messages"].clone()
    }

    #[test]
    fn mid_history_system_becomes_user_and_merges() {
        let out = wire(vec![
            ChatRequestMessage::system("sys"),
            ChatRequestMessage::user("hi"),
            ChatRequestMessage::system("reminder"),
        ]);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[1]["role"], "user");
        assert_eq!(arr[1]["content"], "hi\n\nreminder");
    }

    #[test]
    fn empty_assistant_dropped_and_users_merged() {
        let out = wire(vec![
            ChatRequestMessage::user("a"),
            ChatRequestMessage::assistant("", "m", None),
            ChatRequestMessage::user(""),
        ]);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["content"], "a\n\n(empty message)");
    }

    #[test]
    fn internal_fields_and_old_reasoning_stay_off_wire() {
        let mut named = ChatRequestMessage::user("q");
        named.name = Some("bad name!".into());
        let mut call = ChatRequestMessage::assistant_tool_call(
            ToolCallRequest::function("read", "{}").with_id("c1"),
        );
        call.reasoning_content = Some("now".into());
        let out = wire(vec![
            named,
            ChatRequestMessage::assistant("x", "m", Some("old".into())),
            ChatRequestMessage::user("again"),
            call,
            ChatRequestMessage::tool("c1", ""),
        ]);
        let arr = out.as_array().unwrap();
        assert!(arr[0].get("name").is_none());
        assert!(arr[1].get("model_id").is_none());
        assert!(arr[1].get("reasoning_content").is_none());
        assert_eq!(arr[3]["reasoning_content"], "now");
        assert_eq!(arr[4]["tool_call_id"], "c1");
        assert_eq!(arr[4]["content"], "(no output)");
    }

    #[test]
    fn text_assistant_folds_into_following_tool_call() {
        let out = wire(vec![
            ChatRequestMessage::user("q"),
            ChatRequestMessage::assistant("searching", "m", None),
            ChatRequestMessage::assistant_tool_call(ToolCallRequest::function("f", "{}").with_id("c")),
            ChatRequestMessage::tool("c", "ok"),
        ]);
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[1]["content"], "searching");
        assert_eq!(arr[1]["tool_calls"][0]["id"], "c");
    }
}
