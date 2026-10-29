use toml::Value as TomlValue;
use toml::map::Map as TomlMap;

use supercharge_config_types::McpServerConfig;

use super::error::ImportError;

/// Add servers to `[mcp_servers]`; an existing server with the same name is kept.
pub(super) fn merge_mcp_servers(
    table: &mut TomlMap<String, TomlValue>,
    servers: &[(&str, &McpServerConfig)],
) -> Result<usize, ImportError> {
    let mcp = table
        .entry("mcp_servers")
        .or_insert_with(|| TomlValue::Table(TomlMap::new()));
    let mcp_table = mcp
        .as_table_mut()
        .ok_or_else(|| ImportError::not_a_table("[mcp_servers]"))?;

    let mut count = 0;
    for (name, config) in servers {
        if mcp_table.contains_key(*name) {
            continue;
        }
        let serialized =
            TomlValue::try_from(*config).map_err(|source| ImportError::McpSerialize {
                name: (*name).to_string(),
                source,
            })?;
        mcp_table.insert((*name).to_string(), serialized);
        count += 1;
    }
    Ok(count)
}
