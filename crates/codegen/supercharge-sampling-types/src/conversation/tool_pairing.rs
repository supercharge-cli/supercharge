//! Tool call / tool result pairing repair shared by every wire builder.
//!
//! Strict providers reject a request when a tool result has an empty id, when a tool result has no matching call,
//! or when a call has no result. History can carry any of those: providers that stream tool calls without ids,
//! sessions saved by older builds, and compaction or cancellation that keeps only one half of a pair.
//! [`repair_tool_pairing`] rewrites the item list so every call has a non-empty id and exactly one result directly after
//! its assistant item. Already-valid histories come out unchanged.

use super::*;
use std::collections::HashSet;

/// Text used for a synthetic result when a call's real result is missing from history.
pub const MISSING_TOOL_RESULT: &str = "[tool result unavailable]";

fn synthetic_call_id(item_idx: usize, call_idx: usize) -> String {
    format!("call_auto_{item_idx}_{call_idx}")
}

/// Returns `true` when `items` already satisfies the pairing invariant, so callers can skip the copy.
fn is_well_paired(items: &[ConversationItem]) -> bool {
    let mut pending: Vec<&str> = Vec::new();
    for item in items {
        match item {
            ConversationItem::ToolResult(t) => {
                let Some(pos) = pending.iter().position(|id| *id == t.tool_call_id) else {
                    return false;
                };
                if t.tool_call_id.is_empty() {
                    return false;
                }
                pending.remove(pos);
            }
            other => {
                if !pending.is_empty() {
                    return false;
                }
                if let ConversationItem::Assistant(a) = other {
                    let mut seen = HashSet::new();
                    for tc in &a.tool_calls {
                        if tc.id.is_empty() || !seen.insert(tc.id.as_ref()) {
                            return false;
                        }
                        pending.push(tc.id.as_ref());
                    }
                }
            }
        }
    }
    pending.is_empty()
}

/// Borrowing variant of [`repair_tool_pairing`]: no copy when history is already valid.
pub fn repair_tool_pairing_ref(
    items: &[ConversationItem],
) -> std::borrow::Cow<'_, [ConversationItem]> {
    if is_well_paired(items) {
        std::borrow::Cow::Borrowed(items)
    } else {
        std::borrow::Cow::Owned(repair_tool_pairing(items.to_vec()))
    }
}

/// Repair tool call / result pairing. See the module docs.
pub fn repair_tool_pairing(items: Vec<ConversationItem>) -> Vec<ConversationItem> {
    if is_well_paired(&items) {
        return items;
    }

    // Pass 1: give every call a non-empty, unique id, and bind id-less results to the oldest unanswered call before them.
    let mut items = items;
    let mut unanswered: Vec<String> = Vec::new();
    for (item_idx, item) in items.iter_mut().enumerate() {
        match item {
            ConversationItem::Assistant(a) => {
                unanswered.clear();
                let mut seen_here: HashSet<String> = HashSet::new();
                for (call_idx, tc) in a.tool_calls.iter_mut().enumerate() {
                    if tc.id.is_empty() || seen_here.contains(tc.id.as_ref()) {
                        tc.id = Arc::<str>::from(synthetic_call_id(item_idx, call_idx));
                    }
                    seen_here.insert(tc.id.to_string());
                    unanswered.push(tc.id.to_string());
                }
            }
            ConversationItem::ToolResult(t) => {
                if t.tool_call_id.is_empty() {
                    if !unanswered.is_empty() {
                        t.tool_call_id = unanswered.remove(0);
                    }
                } else if let Some(pos) = unanswered.iter().position(|id| *id == t.tool_call_id) {
                    unanswered.remove(pos);
                }
            }
            _ => {}
        }
    }

    // Pass 2: emit each assistant followed by its results (pulled forward if something got between them), synthesize
    // missing ones, and turn results no call claims into plain text notes.
    let mut consumed = vec![false; items.len()];
    let mut out: Vec<ConversationItem> = Vec::with_capacity(items.len());
    for idx in 0..items.len() {
        if consumed[idx] {
            continue;
        }
        consumed[idx] = true;
        match &items[idx] {
            ConversationItem::Assistant(a) if !a.tool_calls.is_empty() => {
                out.push(items[idx].clone());
                for tc in &a.tool_calls {
                    let found = (idx + 1..items.len()).find(|&j| {
                        !consumed[j]
                            && matches!(&items[j], ConversationItem::ToolResult(t) if t.tool_call_id == tc.id.as_ref())
                    });
                    match found {
                        Some(j) => {
                            consumed[j] = true;
                            out.push(items[j].clone());
                        }
                        None => out.push(ConversationItem::tool_result(
                            tc.id.as_ref(),
                            MISSING_TOOL_RESULT,
                        )),
                    }
                }
            }
            ConversationItem::ToolResult(t) => {
                let label = if t.tool_call_id.is_empty() {
                    "[earlier tool result]".to_owned()
                } else {
                    format!("[earlier tool result {}]", t.tool_call_id)
                };
                out.push(ConversationItem::user_meta(format!(
                    "{label}\n{}",
                    t.content
                )));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        }
    }

    /// Compact shape: "U", "A", "A[id,id]", "R(id)".
    fn shape(items: &[ConversationItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                ConversationItem::Assistant(a) if !a.tool_calls.is_empty() => format!(
                    "A[{}]",
                    a.tool_calls
                        .iter()
                        .map(|t| t.id.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                ConversationItem::Assistant(_) => "A".into(),
                ConversationItem::ToolResult(t) => format!("R({})", t.tool_call_id),
                ConversationItem::User(_) => "U".into(),
                ConversationItem::System(_) => "S".into(),
                _ => "?".into(),
            })
            .collect()
    }

    fn assert_valid(items: &[ConversationItem]) {
        assert!(is_well_paired(items), "{:?}", shape(items));
    }

    #[test]
    fn table() {
        let cases: Vec<(&str, Vec<ConversationItem>, Vec<&str>)> = vec![
            (
                "valid untouched",
                vec![
                    ConversationItem::user("hi"),
                    ConversationItem::assistant_tool_calls(vec![call("a")]),
                    ConversationItem::tool_result("a", "ok"),
                ],
                vec!["U", "A[a]", "R(a)"],
            ),
            (
                "empty call id and empty result id",
                vec![
                    ConversationItem::assistant_tool_calls(vec![call("")]),
                    ConversationItem::tool_result("", "ok"),
                ],
                vec!["A[call_auto_0_0]", "R(call_auto_0_0)"],
            ),
            (
                "missing result id binds in order",
                vec![
                    ConversationItem::assistant_tool_calls(vec![call("a"), call("b")]),
                    ConversationItem::tool_result("a", "1"),
                    ConversationItem::tool_result("", "2"),
                ],
                vec!["A[a,b]", "R(a)", "R(b)"],
            ),
            (
                "orphan result becomes note",
                vec![
                    ConversationItem::user("hi"),
                    ConversationItem::tool_result("ghost", "old"),
                    ConversationItem::assistant("done"),
                ],
                vec!["U", "U", "A"],
            ),
            (
                "missing result synthesized",
                vec![
                    ConversationItem::assistant_tool_calls(vec![call("a")]),
                    ConversationItem::user("next"),
                ],
                vec!["A[a]", "R(a)", "U"],
            ),
            (
                "compaction summary between call and result",
                vec![
                    ConversationItem::assistant_tool_calls(vec![call("a")]),
                    ConversationItem::user_meta("<summary/>"),
                    ConversationItem::tool_result("a", "ok"),
                ],
                vec!["A[a]", "R(a)", "U"],
            ),
            (
                "compaction dropped the call",
                vec![
                    ConversationItem::user_meta("<summary/>"),
                    ConversationItem::tool_result("a", "ok"),
                    ConversationItem::user("go on"),
                ],
                vec!["U", "U", "U"],
            ),
            (
                "duplicate call id in one turn renamed",
                vec![
                    ConversationItem::assistant_tool_calls(vec![call("x"), call("x")]),
                    ConversationItem::tool_result("x", "1"),
                    ConversationItem::tool_result("", "2"),
                ],
                vec!["A[x,call_auto_0_1]", "R(x)", "R(call_auto_0_1)"],
            ),
        ];
        for (name, input, want) in cases {
            let out = repair_tool_pairing(input);
            assert_eq!(shape(&out), want, "{name}");
            assert_valid(&out);
        }
    }

    #[test]
    fn missing_result_text() {
        let out = repair_tool_pairing(vec![ConversationItem::assistant_tool_calls(vec![call(
            "a",
        )])]);
        assert_matches::assert_matches!(&out[1], ConversationItem::ToolResult(t) if t.content.as_ref() == MISSING_TOOL_RESULT);
    }
}
