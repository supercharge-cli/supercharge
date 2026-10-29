use agent_client_protocol as acp;
use supercharge_agent::config::{AgentDefinition, AgentScope, McpServerRef};

use super::{
    apply_agent_mcp_overlay, keep_live_outside_previous_overlay, materialize_agent_mcp_servers,
    overlay_agent_mcp_servers,
};
use crate::session::mcp_servers::mcp_server_name;

fn http(name: &str, url: &str, header: &str, value: &str) -> acp::McpServer {
    acp::McpServer::Http(
        acp::McpServerHttp::new(name, url).headers(vec![acp::HttpHeader::new(header, value)]),
    )
}

fn names(servers: &[acp::McpServer]) -> Vec<&str> {
    servers.iter().map(mcp_server_name).collect()
}

fn find<'a>(servers: &'a [acp::McpServer], name: &str) -> &'a acp::McpServer {
    servers
        .iter()
        .find(|s| mcp_server_name(s) == name)
        .unwrap_or_else(|| panic!("missing server {name}"))
}

fn http_parts(server: &acp::McpServer) -> (String, Vec<(String, String)>) {
    match server {
        acp::McpServer::Http(s) => (
            s.url.clone(),
            s.headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect(),
        ),
        other => panic!("expected http server, got {other:?}"),
    }
}

fn definition_with(mcp_servers: Vec<McpServerRef>) -> AgentDefinition {
    let mut definition = AgentDefinition::default_grok_build();
    definition.name = "seat".to_owned();
    definition.scope = AgentScope::User;
    definition.mcp_servers = mcp_servers;
    definition
}

fn inline(name: &str, config: serde_json::Value) -> McpServerRef {
    McpServerRef::Inline {
        name: name.to_owned(),
        config,
    }
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn overlay_replaces_same_name_in_place_and_appends_new() {
    let mut merged = vec![
        http("a", "https://toml/a", "X-Key", "toml"),
        http("b", "https://toml/b", "X-Key", "toml"),
    ];
    overlay_agent_mcp_servers(
        &mut merged,
        vec![
            http("a", "https://agent/a", "X-Key", "agent"),
            http("c", "https://agent/c", "X-Key", "agent"),
        ],
    );
    assert_eq!(names(&merged), vec!["a", "b", "c"]);
    assert_eq!(
        http_parts(find(&merged, "a")),
        (
            "https://agent/a".to_owned(),
            vec![("X-Key".to_owned(), "agent".to_owned())]
        )
    );
    assert_eq!(http_parts(find(&merged, "b")).0, "https://toml/b");
}

#[test]
fn overlay_is_idempotent_and_empty_is_noop() {
    let dir = tmp();
    let definition = definition_with(vec![inline(
        "a",
        serde_json::json!({"type": "http", "url": "https://agent/a", "headers": [{"name": "X-Key", "value": "agent"}]}),
    )]);
    let mut merged = vec![http("a", "https://toml/a", "X-Key", "toml")];
    apply_agent_mcp_overlay(&mut merged, &definition, dir.path());
    let once = merged.clone();
    apply_agent_mcp_overlay(&mut merged, &definition, dir.path());
    assert_eq!(merged, once);
    assert_eq!(http_parts(&merged[0]).0, "https://agent/a");

    let mut untouched = once.clone();
    apply_agent_mcp_overlay(&mut untouched, &definition_with(vec![]), dir.path());
    assert_eq!(untouched, once);
}

#[test]
fn config_style_inline_headers_override_toml() {
    let dir = tmp();
    let definition = definition_with(vec![inline(
        "a",
        serde_json::json!({"url": "https://agent/a", "headers": {"X-Key": "agent", "A-First": "1"}}),
    )]);
    let mut merged = vec![http("a", "https://toml/a", "X-Key", "toml")];
    apply_agent_mcp_overlay(&mut merged, &definition, dir.path());
    let (url, headers) = http_parts(&merged[0]);
    assert_eq!(url, "https://agent/a");
    // Map-style headers come out sorted so reloads diff as unchanged.
    assert_eq!(
        headers,
        vec![
            ("A-First".to_owned(), "1".to_owned()),
            ("X-Key".to_owned(), "agent".to_owned())
        ]
    );
}

#[test]
fn named_ref_resolves_against_lookup_and_missing_is_dropped() {
    let dir = tmp();
    let lookup = vec![http("a", "https://toml/a", "X-Key", "toml")];
    let definition = definition_with(vec![
        McpServerRef::Named("a".to_owned()),
        McpServerRef::Named("missing".to_owned()),
    ]);
    let servers = materialize_agent_mcp_servers(&definition, &lookup, dir.path());
    assert_eq!(servers, lookup);
}

#[test]
fn plugin_agent_contributes_nothing() {
    let dir = tmp();
    let mut definition = definition_with(vec![inline(
        "a",
        serde_json::json!({"url": "https://agent/a"}),
    )]);
    definition.plugin_name = Some("some-plugin".to_owned());
    assert!(materialize_agent_mcp_servers(&definition, &[], dir.path()).is_empty());
}

#[test]
fn garbage_inline_entries_are_dropped_and_siblings_kept() {
    let dir = tmp();
    let definition = definition_with(vec![
        inline("not-object", serde_json::json!("https://x")),
        inline("bad-shape", serde_json::json!({"type": 7, "url": []})),
        inline("no-transport", serde_json::json!({})),
        inline(
            "disabled",
            serde_json::json!({"url": "https://d", "enabled": false}),
        ),
        inline("good", serde_json::json!({"url": "https://good"})),
    ]);
    let servers = materialize_agent_mcp_servers(&definition, &[], dir.path());
    assert_eq!(names(&servers), vec!["good"]);
}

#[test]
fn inline_stdio_wire_format_parses() {
    let dir = tmp();
    let definition = definition_with(vec![inline(
        "local",
        serde_json::json!({"command": "/bin/echo", "args": ["hi"], "env": {"B": "2", "A": "1"}}),
    )]);
    let servers = materialize_agent_mcp_servers(&definition, &[], dir.path());
    assert_eq!(servers.len(), 1);
    match &servers[0] {
        acp::McpServer::Stdio(stdio) => {
            assert_eq!(stdio.command.to_string_lossy(), "/bin/echo");
            let env: Vec<_> = stdio.env.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(env, vec!["A", "B"]);
        }
        other => panic!("expected stdio, got {other:?}"),
    }
}

#[test]
fn agent_switch_drops_previous_overlay_and_keeps_runtime_servers() {
    let desired = vec![
        http("toml", "https://toml", "X", "1"),
        http("new-seat", "https://new", "X", "1"),
    ];
    let live = vec![
        http("toml", "https://toml-old", "X", "0"),
        http("old-seat", "https://old", "X", "1"),
        http("runtime", "https://runtime", "X", "1"),
    ];
    let previous = vec![inline("old-seat", serde_json::json!({}))];
    let result = keep_live_outside_previous_overlay(desired, &live, &previous);
    assert_eq!(names(&result), vec!["toml", "new-seat", "runtime"]);
    assert_eq!(http_parts(find(&result, "toml")).0, "https://toml");
}
