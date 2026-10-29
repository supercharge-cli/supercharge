//! Materialize an agent definition's `mcpServers` and overlay them onto the disk/client merge.
//!
//! Servers named in the active agent's agent.md win over the disk/client merge by name, so
//! their URLs and headers override config.toml. The overlay is reapplied whenever the merge is
//! rebuilt (config reload, plugin reload, agent switch) so it survives those events.

use std::collections::HashSet;
use std::path::Path;

use agent_client_protocol as acp;
use serde::Deserialize;
use supercharge_agent::config::{AgentDefinition, McpServerRef};
use supercharge_tools::types::compat::CompatConfig;

use crate::session::mcp_servers::mcp_server_name;

/// Inputs for rebuilding the disk/client merge plus the agent overlay.
pub(crate) struct RematerializeParams<'a> {
    pub initial_client_mcp_servers: Vec<acp::McpServer>,
    pub cwd: &'a Path,
    pub plugin_registry: Option<&'a supercharge_agent::plugins::PluginRegistry>,
    pub compat: &'a CompatConfig,
    pub definition: &'a AgentDefinition,
}

/// Plugin agents and untrusted project agents contribute nothing; managed policy and the
/// `enabled = false` / `disabled_mcp_servers` kill switch still apply to what remains.
///
/// `named_lookup` resolves `Named` refs (the pre-overlay merge, or the parent's servers for a child).
pub(crate) fn materialize_agent_mcp_servers(
    definition: &AgentDefinition,
    named_lookup: &[acp::McpServer],
    cwd: &Path,
) -> Vec<acp::McpServer> {
    let servers = if definition.mcp_servers.is_empty() {
        Vec::new()
    } else if definition.plugin_name.is_some() {
        tracing::warn!(
            agent = %definition.name,
            plugin = ?definition.plugin_name,
            "ignoring mcpServers on plugin agent (not supported for security)"
        );
        Vec::new()
    } else if !crate::agent::folder_trust::agent_inline_hooks_allowed(definition.scope, || {
        crate::agent::folder_trust::project_scope_allowed(cwd)
    }) {
        tracing::warn!(
            agent = %definition.name,
            "ignoring mcpServers on untrusted project agent (folder not trusted; re-run with --trust)"
        );
        Vec::new()
    } else {
        parse_agent_mcp_server_refs(definition, named_lookup)
    };
    crate::session::managed_mcp::filter_policy_blocked_agent_mcp(servers, cwd)
}

/// Insert `overlay` servers by name; a same-named server in `merged` is replaced in place.
pub(crate) fn overlay_agent_mcp_servers(
    merged: &mut Vec<acp::McpServer>,
    overlay: Vec<acp::McpServer>,
) {
    for server in overlay {
        let name = mcp_server_name(&server).to_owned();
        if let Some(slot) = merged
            .iter_mut()
            .find(|existing| mcp_server_name(existing) == name)
        {
            *slot = server;
        } else {
            merged.push(server);
        }
    }
}

/// Materialize against the current merge, then overlay by name.
pub(crate) fn apply_agent_mcp_overlay(
    merged: &mut Vec<acp::McpServer>,
    definition: &AgentDefinition,
    cwd: &Path,
) {
    let overlay = materialize_agent_mcp_servers(definition, merged, cwd);
    overlay_agent_mcp_servers(merged, overlay);
}

/// Rebuild the disk/client merge for `cwd`, then apply the agent overlay.
pub(crate) fn rematerialize_with_agent_overlay(
    params: RematerializeParams<'_>,
) -> Vec<acp::McpServer> {
    let mut merged = crate::session::managed_mcp::merge_managed_mcp_servers(
        params.initial_client_mcp_servers,
        params.cwd,
        params.plugin_registry,
        params.compat,
    );
    apply_agent_mcp_overlay(&mut merged, params.definition, params.cwd);
    merged
}

/// Agent switch: rebuild merge plus the new agent's overlay, then keep live servers that
/// neither the new merge has nor the previous agent's overlay contributed (e.g. servers
/// added at runtime). Servers that only the previous agent added are dropped.
pub(crate) fn rematerialize_live_for_agent_seat(
    live: &[acp::McpServer],
    previous_mcp_servers: &[McpServerRef],
    params: RematerializeParams<'_>,
) -> Vec<acp::McpServer> {
    let desired = rematerialize_with_agent_overlay(params);
    keep_live_outside_previous_overlay(desired, live, previous_mcp_servers)
}

/// Append live servers that `desired` lacks unless the previous agent's overlay contributed them.
fn keep_live_outside_previous_overlay(
    mut desired: Vec<acp::McpServer>,
    live: &[acp::McpServer],
    previous_mcp_servers: &[McpServerRef],
) -> Vec<acp::McpServer> {
    let previous_names: HashSet<&str> = previous_mcp_servers
        .iter()
        .map(mcp_server_ref_name)
        .collect();
    let mut desired_names: HashSet<String> = desired
        .iter()
        .map(|s| mcp_server_name(s).to_owned())
        .collect();
    for server in live {
        let name = mcp_server_name(server);
        if desired_names.contains(name) || previous_names.contains(name) {
            continue;
        }
        desired_names.insert(name.to_owned());
        desired.push(server.clone());
    }
    desired
}

fn mcp_server_ref_name(entry: &McpServerRef) -> &str {
    match entry {
        McpServerRef::Named(name) | McpServerRef::Inline { name, .. } => name,
    }
}

fn parse_agent_mcp_server_refs(
    definition: &AgentDefinition,
    named_lookup: &[acp::McpServer],
) -> Vec<acp::McpServer> {
    definition
        .mcp_servers
        .iter()
        .filter_map(|entry| match entry {
            McpServerRef::Named(name) => named_lookup
                .iter()
                .find(|s| mcp_server_name(s) == name)
                .cloned()
                .or_else(|| {
                    tracing::warn!(
                        agent = %definition.name,
                        server = name,
                        "mcpServers: named ref not found"
                    );
                    None
                }),
            McpServerRef::Inline { name, config } => {
                parse_inline_mcp_server(definition, name, config)
            }
        })
        .collect()
}

/// Accepts the ACP wire shape (`type`, `headers: [{name, value}]`) and the config.toml shape
/// (`url`, `headers = { K = V }`, `command`, `env = { K = V }`).
fn parse_inline_mcp_server(
    definition: &AgentDefinition,
    name: &str,
    config: &serde_json::Value,
) -> Option<acp::McpServer> {
    let Some(obj) = config.as_object() else {
        tracing::warn!(
            agent = %definition.name,
            server = name,
            "mcpServers: inline config is not an object"
        );
        return None;
    };
    match parse_acp_mcp_object(name, obj) {
        Ok(server) => return Some(server),
        Err(error) => tracing::debug!(
            server = name,
            %error,
            "ACP wire format parse failed, trying config format"
        ),
    }
    match supercharge_config_types::McpServerConfig::deserialize(config) {
        Ok(cfg) => acp_server_from_config(definition, name, &cfg),
        Err(error) => {
            tracing::warn!(
                agent = %definition.name,
                server = name,
                %error,
                "mcpServers: inline config could not be parsed"
            );
            None
        }
    }
}

fn acp_server_from_config(
    definition: &AgentDefinition,
    name: &str,
    cfg: &supercharge_config_types::McpServerConfig,
) -> Option<acp::McpServer> {
    let Some(mut server) = cfg.to_acp_mcp_server(name) else {
        if cfg.enabled {
            tracing::warn!(
                agent = %definition.name,
                server = name,
                "mcpServers: inline config dropped (setup block or empty url)"
            );
        } else {
            tracing::debug!(
                agent = %definition.name,
                server = name,
                "mcpServers: skipping disabled inline server"
            );
        }
        return None;
    };
    canonicalize_mcp_maps(&mut server);
    Some(server)
}

fn parse_acp_mcp_object(
    name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<acp::McpServer, serde_json::Error> {
    let mut flat = obj.clone();
    flat.insert(
        "name".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    serde_json::from_value(serde_json::Value::Object(flat))
}

/// Map-keyed configs come out of a `HashMap` in arbitrary order; sort so reloads diff as unchanged.
fn canonicalize_mcp_maps(server: &mut acp::McpServer) {
    match server {
        acp::McpServer::Stdio(stdio) => stdio
            .env
            .sort_by(|a, b| (&a.name, &a.value).cmp(&(&b.name, &b.value))),
        acp::McpServer::Http(acp::McpServerHttp { headers, .. })
        | acp::McpServer::Sse(acp::McpServerSse { headers, .. }) => {
            headers.sort_by(|a, b| (&a.name, &a.value).cmp(&(&b.name, &b.value)))
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "agent_mcp_tests.rs"]
mod tests;
