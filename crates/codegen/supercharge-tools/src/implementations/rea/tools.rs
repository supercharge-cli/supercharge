//! Agent tool wrappers for native REA operations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::ToolOutput;
use crate::types::tool::{ToolKind, ToolNamespace};

pub const REA_IDENTIFY: &str = "rea_identify";
pub const REA_HASH: &str = "rea_hash";
pub const REA_STRINGS: &str = "rea_strings";
pub const REA_EXTERNAL_TOOLS: &str = "rea_external_tools";
pub const REA_BINARY: &str = "rea_binary";
pub const REA_DISASSEMBLE: &str = "rea_disassemble";
pub const REA_ASAR: &str = "rea_asar";
pub const REA_ASAR_EXTRACT: &str = "rea_asar_extract";
pub const REA_APK: &str = "rea_apk";
pub const REA_PCAP: &str = "rea_pcap";

/// All native REA tool names registered when `[rea] enabled` is on.
pub const REA_TOOL_NAMES: &[&str] = &[
    REA_IDENTIFY,
    REA_HASH,
    REA_STRINGS,
    REA_BINARY,
    REA_DISASSEMBLE,
    REA_ASAR,
    REA_ASAR_EXTRACT,
    REA_APK,
    REA_PCAP,
    REA_EXTERNAL_TOOLS,
];

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PathInput {
    /// Absolute or workspace-relative path to the file or archive to inspect.
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HashInput {
    /// Path to hash.
    pub path: String,
    /// Optional cap on bytes to hash. Omit to hash the whole file (streaming).
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct StringsInput {
    /// Path to scan for printable strings.
    pub path: String,
    /// Minimum string length (default 4).
    #[serde(default)]
    pub min_length: Option<usize>,
    /// Optional substring filter; only matching strings are returned.
    #[serde(default)]
    pub needle: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct EmptyInput {}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BinaryInput {
    /// Path to an ELF, PE or Mach-O file.
    pub path: String,
    /// For fat Mach-O files, the slice architecture to parse (for example X86_64 or Aarch64).
    #[serde(default)]
    pub arch: Option<String>,
    /// Optional case-insensitive substring filter for imports, exports and symbols.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DisasmInput {
    /// Path to an executable or object file.
    pub path: String,
    /// Start address (hex like 0x401000 or decimal). Defaults to the entry point.
    #[serde(default)]
    pub address: Option<String>,
    /// Start at this symbol instead of an address.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Instructions to decode (default 100, max 2000).
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AsarInput {
    /// Path to an Electron .asar archive.
    pub path: String,
    /// Entry to read (for example main.js). Omit to list the archive.
    #[serde(default)]
    pub entry: Option<String>,
    /// Cap on bytes returned for the entry (default and max 256 KiB).
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AsarExtractInput {
    /// Path to an Electron .asar archive.
    pub path: String,
    /// Directory to write into. Entries never escape it.
    pub dest_dir: String,
    /// One entry to extract. Omit to extract every packed entry.
    #[serde(default)]
    pub entry: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ApkInput {
    /// Path to an APK, JAR, IPA or other zip archive.
    pub path: String,
    /// Decode AndroidManifest.xml when present (default true).
    #[serde(default)]
    pub include_manifest: Option<bool>,
}

macro_rules! rea_tool {
    ($ty:ident, $name:expr, $args:ty, $read_only:expr, $desc:expr, |$input:ident| $body:expr) => {
        #[derive(Debug, Default)]
        pub struct $ty;

        impl crate::types::tool_metadata::ToolMetadata for $ty {
            fn kind(&self) -> ToolKind {
                ToolKind::Other
            }
            fn tool_namespace(&self) -> ToolNamespace {
                ToolNamespace::GrokBuild
            }
            fn description_template(&self) -> &str {
                $desc
            }
        }

        impl supercharge_tool_runtime::Tool for $ty {
            type Args = $args;
            type Output = ToolOutput;

            fn id(&self) -> supercharge_tool_protocol::ToolId {
                supercharge_tool_protocol::ToolId::new($name).expect("valid tool id")
            }

            fn description(
                &self,
                _ctx: &::supercharge_tool_runtime::ListToolsContext,
            ) -> supercharge_tool_types::ToolDescription {
                supercharge_tool_types::ToolDescription::new(
                    $name,
                    crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
                )
            }

            fn capabilities(&self) -> supercharge_tool_protocol::ToolCapabilities {
                supercharge_tool_protocol::ToolCapabilities {
                    is_read_only: $read_only,
                    tool_scope: Some(if $read_only {
                        supercharge_tool_protocol::ToolScope::Read
                    } else {
                        supercharge_tool_protocol::ToolScope::Write
                    }),
                    ..Default::default()
                }
            }

            async fn run(
                &self,
                _ctx: supercharge_tool_runtime::ToolCallContext,
                $input: $args,
            ) -> Result<ToolOutput, supercharge_tool_runtime::ToolError> {
                Ok(ToolOutput::Text(($body).into()))
            }
        }
    };
}

/// Run blocking file work off the async executor; a panic becomes an error reply.
async fn blocking(op: &'static str, f: impl FnOnce() -> String + Send + 'static) -> String {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| {
        super::evidence::EvidenceError::new(op, None, format!("analysis aborted: {e}"))
            .to_json_text()
    })
}

rea_tool!(
    ReaIdentifyImpl,
    REA_IDENTIFY,
    PathInput,
    true,
    "Identify a local file or archive (ELF/PE/Mach-O, ZIP/APK, ASAR, Bun/Node/Deno standalone markers). \
Returns evidence JSON with kind, magic, markers, optional archive listing, and limitations. \
Static triage only, no code execution. Part of built-in REA mode.",
    |input| blocking(REA_IDENTIFY, move || super::identify::identify_path(
        &input.path
    ))
    .await
);

rea_tool!(
    ReaHashImpl,
    REA_HASH,
    HashInput,
    true,
    "Compute MD5, SHA-256, and BLAKE3 digests of a local file by streaming. \
Optional max_bytes caps how much is hashed. Returns evidence JSON with sizes and limitations.",
    |input| blocking(REA_HASH, move || super::hashes::hash_path(
        &input.path,
        input.max_bytes
    ))
    .await
);

rea_tool!(
    ReaStringsImpl,
    REA_STRINGS,
    StringsInput,
    true,
    "Extract printable ASCII strings with file offsets from a local file (bounded). \
Optional min_length and needle filter. Returns evidence JSON; string values are untrusted target content.",
    |input| blocking(REA_STRINGS, move || super::strings::strings_path(
        &input.path,
        input.min_length,
        input.needle.as_deref()
    ))
    .await
);

rea_tool!(
    ReaBinaryImpl,
    REA_BINARY,
    BinaryInput,
    true,
    "Parse an ELF, PE or Mach-O file (fat Mach-O slices too): format, architecture, entry point, \
segments, sections, imports, exports and symbols, each capped. Optional filter narrows names.",
    |input| blocking(REA_BINARY, move || super::binary::binary_path(
        &input.path,
        input.arch.as_deref(),
        input.filter.as_deref()
    ))
    .await
);

rea_tool!(
    ReaDisassembleImpl,
    REA_DISASSEMBLE,
    DisasmInput,
    true,
    "Disassemble x86 or x86-64 code (Intel syntax) from an executable, starting at an address, \
a symbol, or the entry point. Linear sweep, bounded instruction count.",
    |input| blocking(REA_DISASSEMBLE, move || super::disasm::disassemble_path(
        &input.path,
        input.address.as_deref(),
        input.symbol.as_deref(),
        input.count
    ))
    .await
);

rea_tool!(
    ReaAsarImpl,
    REA_ASAR,
    AsarInput,
    true,
    "List an Electron .asar archive (entries, package.json main, preload scripts, native addons), \
or read one entry as text with JavaScript/Electron security markers (IPC, Node integration, eval, child processes).",
    |input| blocking(REA_ASAR, move || super::asar::asar_path(
        &input.path,
        input.entry.as_deref(),
        input.max_bytes
    ))
    .await
);

rea_tool!(
    ReaAsarExtractImpl,
    REA_ASAR_EXTRACT,
    AsarExtractInput,
    false,
    "Extract one entry or every packed entry of an Electron .asar archive into dest_dir. \
Paths that would escape dest_dir are skipped; total output is capped.",
    |input| blocking(REA_ASAR_EXTRACT, move || super::asar::extract(
        &input.path,
        input.entry.as_deref(),
        &input.dest_dir
    ))
    .await
);

rea_tool!(
    ReaApkImpl,
    REA_APK,
    ApkInput,
    true,
    "List an APK, JAR, IPA or zip archive (sizes, DEX files, native libraries, signing files) \
and decode the binary AndroidManifest.xml into readable XML with package and permissions.",
    |input| blocking(REA_APK, move || super::apk::apk_path(
        &input.path,
        input.include_manifest.unwrap_or(true)
    ))
    .await
);

rea_tool!(
    ReaPcapImpl,
    REA_PCAP,
    PathInput,
    true,
    "Summarize a packet capture (protocol hierarchy and IP conversations) with tshark when it is installed; \
otherwise returns a clear message saying tshark is missing.",
    |input| super::external::pcap_summary(&input.path).await
);

rea_tool!(
    ReaExternalToolsImpl,
    REA_EXTERNAL_TOOLS,
    EmptyInput,
    true,
    "Report whether optional heavy engines (Ghidra, tshark, jadx, objdump) are on PATH. \
Built-in REA tools work without them.",
    |_input| super::external::status_json()
);

/// Register every native REA tool that is not already on the bridge. Idempotent,
/// so it is safe to call on every turn while REA mode is on.
pub fn register_all(
    bridge: &crate::bridge::ToolBridge,
) -> Result<(), supercharge_tool_runtime::ToolError> {
    let toolset = bridge.toolset();
    let missing = |name: &str| toolset.get_tool_metadata(name).is_none();
    if missing(REA_IDENTIFY) {
        bridge.register_mcp_tools(REA_IDENTIFY.to_owned(), ReaIdentifyImpl, None)?;
    }
    if missing(REA_HASH) {
        bridge.register_mcp_tools(REA_HASH.to_owned(), ReaHashImpl, None)?;
    }
    if missing(REA_STRINGS) {
        bridge.register_mcp_tools(REA_STRINGS.to_owned(), ReaStringsImpl, None)?;
    }
    if missing(REA_BINARY) {
        bridge.register_mcp_tools(REA_BINARY.to_owned(), ReaBinaryImpl, None)?;
    }
    if missing(REA_DISASSEMBLE) {
        bridge.register_mcp_tools(REA_DISASSEMBLE.to_owned(), ReaDisassembleImpl, None)?;
    }
    if missing(REA_ASAR) {
        bridge.register_mcp_tools(REA_ASAR.to_owned(), ReaAsarImpl, None)?;
    }
    if missing(REA_ASAR_EXTRACT) {
        bridge.register_mcp_tools(REA_ASAR_EXTRACT.to_owned(), ReaAsarExtractImpl, None)?;
    }
    if missing(REA_APK) {
        bridge.register_mcp_tools(REA_APK.to_owned(), ReaApkImpl, None)?;
    }
    if missing(REA_PCAP) {
        bridge.register_mcp_tools(REA_PCAP.to_owned(), ReaPcapImpl, None)?;
    }
    if missing(REA_EXTERNAL_TOOLS) {
        bridge.register_mcp_tools(REA_EXTERNAL_TOOLS.to_owned(), ReaExternalToolsImpl, None)?;
    }
    Ok(())
}

/// Remove native REA tools by exact name.
pub fn unregister_all(bridge: &crate::bridge::ToolBridge) -> usize {
    let mut n = 0;
    for name in REA_TOOL_NAMES {
        if bridge.unregister_tool_by_name(name) {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_ids_match_constants() {
        use supercharge_tool_runtime::Tool;
        let ids = [
            ReaIdentifyImpl.id().to_string(),
            ReaHashImpl.id().to_string(),
            ReaStringsImpl.id().to_string(),
            ReaBinaryImpl.id().to_string(),
            ReaDisassembleImpl.id().to_string(),
            ReaAsarImpl.id().to_string(),
            ReaAsarExtractImpl.id().to_string(),
            ReaApkImpl.id().to_string(),
            ReaPcapImpl.id().to_string(),
            ReaExternalToolsImpl.id().to_string(),
        ];
        assert_eq!(
            ids.to_vec(),
            REA_TOOL_NAMES
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn register_is_idempotent_and_unregister_removes_all() {
        let bridge = crate::bridge::ToolBridge::for_test();
        register_all(&bridge).expect("first register");
        register_all(&bridge).expect("second register is a no-op");
        for name in REA_TOOL_NAMES {
            assert!(bridge.toolset().get_tool_metadata(name).is_some(), "{name}");
        }
        assert_eq!(unregister_all(&bridge), REA_TOOL_NAMES.len());
        assert_eq!(unregister_all(&bridge), 0);
    }
}
