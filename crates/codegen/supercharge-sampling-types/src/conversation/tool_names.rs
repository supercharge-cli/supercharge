//! Wire-safe tool names.
//!
//! Strict providers only accept function names matching `^[A-Za-z0-9_-]{1,64}$`. Connector (MCP) servers are free to
//! publish names with dots, slashes, spaces or non-ASCII text, and long server prefixes can push a name past 64
//! characters. [`WireToolNames::apply`] rewrites every such name in a request (tool definitions, a forced tool choice
//! and earlier assistant tool calls) to a safe alias, and keeps the reverse map so the model's calls are restored to
//! the real tool name before anything else sees them. Requests whose names are already safe pass through untouched.
//! The mapping is a pure function of the request's tool list and history, so the same session always gets the
//! same aliases across turns, restarts and retries.

use super::*;
use std::collections::{HashMap, HashSet};

/// Longest function name every strict provider accepts.
pub const MAX_WIRE_TOOL_NAME_LEN: usize = 64;

/// Returns `true` when `name` is accepted as-is by strict providers.
pub fn is_wire_safe_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_WIRE_TOOL_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// FNV-1a; stable across builds and platforms, unlike `DefaultHasher`.
fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The safe alias for `name`, before collision handling.
fn base_alias(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out.push_str("tool");
    }
    // A changed name always carries a hash so two distinct real names that collapse to the same text stay distinct.
    let needs_hash = out != name || out.len() > MAX_WIRE_TOOL_NAME_LEN;
    if needs_hash {
        let suffix = format!("_{:08x}", stable_hash(name) as u32);
        let keep = MAX_WIRE_TOOL_NAME_LEN - suffix.len();
        out.truncate(keep.min(out.len()));
        out.push_str(&suffix);
    }
    out
}

/// Reverse map from the aliases sent on the wire to the real tool names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireToolNames {
    to_real: HashMap<String, String>,
    to_wire: HashMap<String, String>,
}

impl WireToolNames {
    /// `true` when no name was rewritten, so restoring is a no-op.
    pub fn is_empty(&self) -> bool {
        self.to_real.is_empty()
    }

    /// The alias sent on the wire for `real`, or `real` itself when it was already safe.
    pub fn wire_name<'a>(&'a self, real: &'a str) -> &'a str {
        self.to_wire.get(real).map(String::as_str).unwrap_or(real)
    }

    /// The real tool name for a name the model sent back. Unknown names come back unchanged.
    pub fn real_name<'a>(&'a self, wire: &'a str) -> &'a str {
        self.to_real.get(wire).map(String::as_str).unwrap_or(wire)
    }

    fn alias(&mut self, real: &str, taken: &mut HashSet<String>) -> String {
        if let Some(w) = self.to_wire.get(real) {
            return w.clone();
        }
        let base = base_alias(real);
        let mut candidate = base.clone();
        let mut n = 2u32;
        while taken.contains(&candidate) {
            let suffix = format!("_{n}");
            let mut c = base.clone();
            c.truncate(MAX_WIRE_TOOL_NAME_LEN - suffix.len());
            c.push_str(&suffix);
            candidate = c;
            n += 1;
        }
        taken.insert(candidate.clone());
        self.to_real.insert(candidate.clone(), real.to_string());
        self.to_wire.insert(real.to_string(), candidate.clone());
        candidate
    }

    /// Rewrite every unsafe tool name in `request` and return the reverse map.
    pub fn apply(request: &mut ConversationRequest) -> Self {
        let mut map = Self::default();
        let tools_ok = request.tools.iter().all(|t| is_wire_safe_tool_name(&t.name));
        let choice_ok = match &request.tool_choice {
            Some(ConversationToolChoice::Function(n)) => is_wire_safe_tool_name(n),
            _ => true,
        };
        let history_ok = request.items.iter().all(|item| match item {
            ConversationItem::Assistant(a) => {
                a.tool_calls.iter().all(|c| is_wire_safe_tool_name(&c.name))
            }
            _ => true,
        });
        if tools_ok && choice_ok && history_ok {
            return map;
        }
        // Safe names are reserved first so an alias can never shadow a real tool.
        let mut taken: HashSet<String> = request
            .tools
            .iter()
            .map(|t| t.name.clone())
            .filter(|n| is_wire_safe_tool_name(n))
            .collect();
        for item in &request.items {
            if let ConversationItem::Assistant(a) = item {
                for c in &a.tool_calls {
                    if is_wire_safe_tool_name(&c.name) {
                        taken.insert(c.name.clone());
                    }
                }
            }
        }
        for tool in &mut request.tools {
            if !is_wire_safe_tool_name(&tool.name) {
                tool.name = map.alias(&tool.name, &mut taken);
            }
        }
        if let Some(ConversationToolChoice::Function(n)) = &mut request.tool_choice
            && !is_wire_safe_tool_name(n)
        {
            *n = map.alias(n, &mut taken);
        }
        for item in &mut request.items {
            if let ConversationItem::Assistant(a) = item {
                for c in &mut a.tool_calls {
                    if !is_wire_safe_tool_name(&c.name) {
                        c.name = map.alias(&c.name, &mut taken);
                    }
                }
            }
        }
        map
    }

    /// Restore real names on every assistant tool call in `items`.
    pub fn restore_items(&self, items: &mut [ConversationItem]) {
        if self.is_empty() {
            return;
        }
        for item in items {
            if let ConversationItem::Assistant(a) = item {
                for c in &mut a.tool_calls {
                    if let Some(real) = self.to_real.get(&c.name) {
                        c.name = real.clone();
                    }
                }
            }
        }
    }

    /// Restore real names on a finished response.
    pub fn restore_response(&self, response: &mut ConversationResponse) {
        self.restore_items(&mut response.items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: None,
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: "{}".into(),
        }
    }

    fn assistant_with(names: &[&str]) -> ConversationItem {
        let mut item = ConversationItem::assistant("");
        if let ConversationItem::Assistant(a) = &mut item {
            a.tool_calls = names.iter().map(|n| call(n)).collect();
        }
        item
    }

    fn request(tools: &[&str], history: &[&str]) -> ConversationRequest {
        let mut req = ConversationRequest {
            tools: tools.iter().map(|n| spec(n)).collect(),
            ..Default::default()
        };
        if !history.is_empty() {
            req.items.push(assistant_with(history));
        }
        req
    }

    #[test]
    fn safe_names_pass_through_untouched() {
        let mut req = request(&["read_file", "linear__save-issue"], &["read_file"]);
        let map = WireToolNames::apply(&mut req);
        assert!(map.is_empty());
        assert_eq!(req.tools[0].name, "read_file");
        assert_eq!(req.tools[1].name, "linear__save-issue");
    }

    #[test]
    fn dotted_names_are_rewritten_and_restored() {
        let mut req = request(&["docs__files.read", "bash"], &["docs__files.read"]);
        let map = WireToolNames::apply(&mut req);
        let wire = req.tools[0].name.clone();
        assert!(is_wire_safe_tool_name(&wire), "{wire}");
        assert_eq!(req.tools[1].name, "bash");
        let ConversationItem::Assistant(a) = &req.items[0] else {
            panic!()
        };
        assert_eq!(a.tool_calls[0].name, wire, "history must use the same alias");
        assert_eq!(map.real_name(&wire), "docs__files.read");

        let mut items = vec![assistant_with(&[&wire])];
        map.restore_items(&mut items);
        let ConversationItem::Assistant(a) = &items[0] else {
            panic!()
        };
        assert_eq!(a.tool_calls[0].name, "docs__files.read");
    }

    #[test]
    fn colliding_names_get_distinct_aliases() {
        let mut req = request(&["a.b", "a/b", "a b", "a_b"], &[]);
        let map = WireToolNames::apply(&mut req);
        let names: HashSet<_> = req.tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names.len(), 4);
        assert!(names.contains("a_b"), "safe real name keeps its name");
        for t in &req.tools {
            assert!(is_wire_safe_tool_name(&t.name));
        }
        assert_eq!(map.real_name(&req.tools[0].name), "a.b");
        assert_eq!(map.real_name(&req.tools[1].name), "a/b");
        assert_eq!(map.real_name(&req.tools[2].name), "a b");
    }

    #[test]
    fn long_and_empty_and_unicode_names_are_safe() {
        let long = "x".repeat(200);
        let mut req = request(&[&long, "", "工具.搜索"], &[]);
        let map = WireToolNames::apply(&mut req);
        for t in &req.tools {
            assert!(is_wire_safe_tool_name(&t.name), "{:?}", t.name);
        }
        assert_eq!(map.real_name(&req.tools[0].name), long);
        assert_eq!(map.real_name(&req.tools[1].name), "");
    }

    #[test]
    fn aliases_are_stable_across_requests() {
        let mut a = request(&["s.one", "s.two"], &[]);
        let mut b = request(&["s.one", "s.two"], &["s.one"]);
        WireToolNames::apply(&mut a);
        WireToolNames::apply(&mut b);
        assert_eq!(a.tools[0].name, b.tools[0].name);
        assert_eq!(a.tools[1].name, b.tools[1].name);
    }

    #[test]
    fn forced_tool_choice_is_rewritten() {
        let mut req = request(&["s.one"], &[]);
        req.tool_choice = Some(ConversationToolChoice::Function("s.one".into()));
        WireToolNames::apply(&mut req);
        let Some(ConversationToolChoice::Function(n)) = &req.tool_choice else {
            panic!()
        };
        assert_eq!(n, &req.tools[0].name);
    }

    #[test]
    fn unknown_names_restore_unchanged() {
        let map = WireToolNames::default();
        assert_eq!(map.real_name("whatever"), "whatever");
    }
}
