//! Hard caps so large or hostile inputs cannot exhaust memory or flood the model.

/// Bytes admitted for whole-file inspect (identify / strings). Larger files are
/// sampled from the head only and that fact is recorded in `limitations`.
pub const MAX_INSPECT_BYTES: u64 = 64 * 1024 * 1024;

/// Chunk size for streaming hash and string scans.
pub const READ_CHUNK: usize = 64 * 1024;

/// Maximum strings returned from one call.
pub const MAX_STRINGS: usize = 8_192;

/// Minimum printable run length to count as a string.
pub const MIN_STRING_LEN: usize = 4;

/// Cap on a single string's retained characters (bytes of the UTF-8 view).
pub const MAX_STRING_CHARS: usize = 512;

/// Cap on serialized JSON text returned to the model (~1 MiB).
pub const MAX_OUTPUT_CHARS: usize = 1_048_576;

/// Cap on how many archive/zip member names we list.
pub const MAX_ARCHIVE_ENTRIES: usize = 4_096;

/// Largest ASAR JSON header we parse.
pub const MAX_ASAR_HEADER: u64 = 32 * 1024 * 1024;

/// Bytes returned when reading one archive entry as text.
pub const MAX_ENTRY_READ: u64 = 256 * 1024;

/// Total bytes one extract call may write.
pub const MAX_EXTRACT_TOTAL: u64 = 512 * 1024 * 1024;

/// Cap on sections, imports, exports or symbols listed per binary.
pub const MAX_BINARY_ITEMS: usize = 2_000;

/// Cap on decoded instructions per disassembly call.
pub const MAX_INSTRUCTIONS: usize = 2_000;

/// Cap on rendered manifest lines.
pub const MAX_MANIFEST_LINES: usize = 4_000;

/// Bytes admitted for structural binary parsing. Section and symbol tables often
/// sit at the end of the file, so this is larger than the triage cap.
pub const MAX_BINARY_BYTES: u64 = 768 * 1024 * 1024;
