//! MCP server types — deferred.
//!
//! `../E2B/spec/mcp-server.json` is a catalog of available MCP servers, not
//! the user-facing `McpServer` config type (a small union in the JS SDK).
//! typify produced no useful named types from its bare object-of-servers
//! schema (no `$defs`). The `McpServer` config type will be hand-written when
//! MCP is wired into sandbox options / templates in a later milestone.
