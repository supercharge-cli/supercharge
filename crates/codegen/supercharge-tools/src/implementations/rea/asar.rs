//! Electron ASAR archives: bounded header parse, listing, entry read and extract.
//!
//! Layout: u32 4, u32 header_pickle_size, then a pickle holding
//! u32 payload_size, u32 json_len, json. File data starts at 8 + header_pickle_size.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

use serde::Serialize;

use super::bounds::{MAX_ARCHIVE_ENTRIES, MAX_ASAR_HEADER, MAX_ENTRY_READ};
use super::evidence::{EvidenceError, EvidenceReply};
use super::js::{JsMarkers, js_markers};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AsarEntry {
    pub path: String,
    pub size: u64,
    /// Offset relative to the data section; None for unpacked or directories.
    pub offset: Option<u64>,
    pub unpacked: bool,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsarListing {
    pub data_offset: u64,
    pub entry_count_total: usize,
    pub entries: Vec<AsarEntry>,
    pub truncated: bool,
    pub package_main: Option<String>,
    pub electron_hints: Vec<String>,
}

/// Parse the JSON header from a byte prefix. Returns (header, data_offset).
pub fn parse_header(bytes: &[u8]) -> Result<(serde_json::Value, u64), String> {
    let u32_at = |i: usize| -> Option<u32> {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if u32_at(0) != Some(4) {
        return Err("not an ASAR archive (bad size prefix)".into());
    }
    let header_size = u32_at(4).ok_or("truncated ASAR prefix")? as u64;
    let json_len = u32_at(12).ok_or("truncated ASAR header")? as usize;
    if json_len == 0 || json_len as u64 > MAX_ASAR_HEADER || json_len as u64 + 8 > header_size {
        return Err(format!("implausible ASAR header length {json_len}"));
    }
    let json = bytes
        .get(16..16 + json_len)
        .ok_or("ASAR header extends past the inspected bytes")?;
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("ASAR header is not JSON: {e}"))?;
    if !value.get("files").is_some_and(|f| f.is_object()) {
        return Err("ASAR header has no files table".into());
    }
    Ok((value, 8 + header_size))
}

/// Read and parse the header straight from the file (only the header bytes).
pub fn read_header(path: &Path) -> Result<(serde_json::Value, u64), String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut prefix = [0u8; 16];
    file.read_exact(&mut prefix).map_err(|e| format!("too small for ASAR: {e}"))?;
    let json_len = u32::from_le_bytes([prefix[12], prefix[13], prefix[14], prefix[15]]) as u64;
    if json_len > MAX_ASAR_HEADER {
        return Err(format!("ASAR header of {json_len} bytes exceeds the limit"));
    }
    let mut buf = prefix.to_vec();
    buf.resize(16 + json_len as usize, 0);
    file.read_exact(&mut buf[16..]).map_err(|e| format!("truncated ASAR header: {e}"))?;
    parse_header(&buf)
}

/// Flatten the nested files table, depth-first, capped.
pub fn flatten(header: &serde_json::Value) -> (Vec<AsarEntry>, usize) {
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut stack: Vec<(String, &serde_json::Value, usize)> = Vec::new();
    if let Some(files) = header.get("files") {
        stack.push((String::new(), files, 0));
    }
    while let Some((prefix, files, depth)) = stack.pop() {
        let Some(map) = files.as_object() else { continue };
        let mut names: Vec<&String> = map.keys().collect();
        names.sort();
        for name in names.into_iter().rev() {
            let node = &map[name];
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if let Some(sub) = node.get("files") {
                if depth < 64 {
                    stack.push((path, sub, depth + 1));
                }
                continue;
            }
            total += 1;
            if out.len() < MAX_ARCHIVE_ENTRIES {
                out.push(AsarEntry {
                    size: node.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    offset: node
                        .get("offset")
                        .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or(v.as_u64())),
                    unpacked: node.get("unpacked").and_then(|v| v.as_bool()).unwrap_or(false),
                    executable: node.get("executable").and_then(|v| v.as_bool()).unwrap_or(false),
                    path,
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    (out, total)
}

fn find<'a>(entries: &'a [AsarEntry], entry: &str) -> Option<&'a AsarEntry> {
    let want = entry.trim_start_matches("./").trim_start_matches('/');
    entries.iter().find(|e| e.path == want)
}

fn read_entry_bytes(path: &Path, data_offset: u64, e: &AsarEntry, cap: u64) -> Result<Vec<u8>, String> {
    if e.unpacked {
        return Err(format!("{} is stored unpacked next to the archive (<archive>.unpacked/)", e.path));
    }
    let off = e.offset.ok_or("entry has no offset")?;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let start = data_offset.checked_add(off).ok_or("offset overflow")?;
    if start.checked_add(e.size).is_none_or(|end| end > len) {
        return Err("entry lies outside the archive".into());
    }
    file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.take(e.size.min(cap)).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn electron_hints(entries: &[AsarEntry]) -> Vec<String> {
    let mut hints = Vec::new();
    for e in entries {
        let lower = e.path.to_ascii_lowercase();
        if lower.contains("preload") && lower.ends_with(".js") {
            hints.push(format!("preload script: {}", e.path));
        }
        if lower.ends_with(".node") {
            hints.push(format!("native addon: {}", e.path));
        }
        if hints.len() >= 64 {
            break;
        }
    }
    hints
}

#[derive(Debug, Serialize)]
struct EntryRead {
    entry: AsarEntry,
    bytes_returned: usize,
    text: String,
    js_markers: Option<JsMarkers>,
}

/// `rea_asar`: list the archive, or read one entry when `entry` is set.
pub fn asar_path(path: &str, entry: Option<&str>, max_bytes: Option<u64>) -> String {
    const OP: &str = "rea_asar";
    let p = Path::new(path);
    let (header, data_offset) = match read_header(p) {
        Ok(v) => v,
        Err(e) => return EvidenceError::new(OP, Some(path.into()), e).to_json_text(),
    };
    let (entries, total) = flatten(&header);
    let Some(entry) = entry else {
        let package_main = find(&entries, "package.json")
            .and_then(|e| read_entry_bytes(p, data_offset, e, 256 * 1024).ok())
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("main").and_then(|m| m.as_str()).map(str::to_owned));
        let mut reply = EvidenceReply::ok(
            OP,
            path,
            AsarListing {
                data_offset,
                entry_count_total: total,
                truncated: total > entries.len(),
                electron_hints: electron_hints(&entries),
                entries,
                package_main,
            },
        );
        if total > MAX_ARCHIVE_ENTRIES {
            reply = reply.with_limitation(format!("listing capped at {MAX_ARCHIVE_ENTRIES} entries"));
        }
        return reply.to_json_text();
    };
    let Some(found) = find(&entries, entry).cloned() else {
        return EvidenceError::new(OP, Some(path.into()), format!("no entry named {entry}")).to_json_text();
    };
    let cap = max_bytes.unwrap_or(MAX_ENTRY_READ).min(MAX_ENTRY_READ);
    match read_entry_bytes(p, data_offset, &found, cap) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let is_js = [".js", ".mjs", ".cjs", ".ts"].iter().any(|s| found.path.ends_with(s));
            let mut reply = EvidenceReply::ok(
                OP,
                path,
                EntryRead {
                    bytes_returned: bytes.len(),
                    js_markers: is_js.then(|| js_markers(&text)),
                    text,
                    entry: found.clone(),
                },
            );
            if (bytes.len() as u64) < found.size {
                reply = reply.with_limitation(format!("entry truncated to {} of {} bytes", bytes.len(), found.size));
            }
            reply.to_json_text()
        }
        Err(e) => EvidenceError::new(OP, Some(path.into()), e).to_json_text(),
    }
}

/// Reject absolute paths and `..` so extraction stays inside `dest_dir`.
fn safe_relative(entry: &str) -> Option<&Path> {
    let p = Path::new(entry.trim_start_matches("./"));
    p.components().all(|c| matches!(c, Component::Normal(_))).then_some(p)
}

#[derive(Debug, Serialize)]
struct Extracted {
    written: Vec<String>,
    skipped: Vec<String>,
}

/// `rea_asar_extract`: copy one entry, or every packed entry, under `dest_dir`.
pub fn extract(path: &str, entry: Option<&str>, dest_dir: &str) -> String {
    const OP: &str = "rea_asar_extract";
    let p = Path::new(path);
    let (header, data_offset) = match read_header(p) {
        Ok(v) => v,
        Err(e) => return EvidenceError::new(OP, Some(path.into()), e).to_json_text(),
    };
    let (entries, total) = flatten(&header);
    let selected: Vec<&AsarEntry> = match entry {
        Some(name) => match find(&entries, name) {
            Some(e) => vec![e],
            None => return EvidenceError::new(OP, Some(path.into()), format!("no entry named {name}")).to_json_text(),
        },
        None => entries.iter().collect(),
    };
    let mut out = Extracted { written: Vec::new(), skipped: Vec::new() };
    let mut budget: u64 = super::bounds::MAX_EXTRACT_TOTAL;
    for e in selected {
        let Some(rel) = safe_relative(&e.path) else {
            out.skipped.push(format!("{}: unsafe path", e.path));
            continue;
        };
        if e.size > budget {
            out.skipped.push(format!("{}: extraction size budget reached", e.path));
            continue;
        }
        let target = Path::new(dest_dir).join(rel);
        let res = read_entry_bytes(p, data_offset, e, e.size).and_then(|bytes| {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
            Ok(bytes.len() as u64)
        });
        match res {
            Ok(n) => {
                budget -= n;
                out.written.push(target.display().to_string());
            }
            Err(err) => out.skipped.push(format!("{}: {err}", e.path)),
        }
    }
    let mut reply = EvidenceReply::ok(OP, path, out);
    if total > entries.len() {
        reply = reply.with_limitation(format!("only the first {MAX_ARCHIVE_ENTRIES} entries were considered"));
    }
    reply.to_json_text()
}

#[cfg(test)]
pub(crate) fn build_asar(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut map = serde_json::Map::new();
    let mut data = Vec::new();
    for (name, bytes) in files {
        let mut node = serde_json::Map::new();
        node.insert("size".into(), (bytes.len() as u64).into());
        node.insert("offset".into(), data.len().to_string().into());
        data.extend_from_slice(bytes);
        let parts: Vec<&str> = name.split('/').collect();
        let mut cur = &mut map;
        for dir in &parts[..parts.len() - 1] {
            let d = cur
                .entry(dir.to_string())
                .or_insert_with(|| serde_json::json!({"files": {}}));
            cur = d.get_mut("files").and_then(|f| f.as_object_mut()).expect("dir");
        }
        cur.insert(parts[parts.len() - 1].to_string(), serde_json::Value::Object(node));
    }
    let json = serde_json::to_vec(&serde_json::json!({ "files": map })).expect("json");
    let padded = (json.len() + 3) / 4 * 4;
    let header_pickle = 8 + padded;
    let mut out = Vec::new();
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&(header_pickle as u32).to_le_bytes());
    out.extend_from_slice(&((padded + 4) as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.resize(8 + header_pickle, 0);
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tmp");
        let bytes = build_asar(&[
            ("package.json", br#"{"main":"main.js"}"#),
            ("main.js", b"const { app } = require('electron'); require('child_process');"),
            ("lib/preload.js", b"contextBridge.exposeInMainWorld('x', {})"),
        ]);
        let path = dir.path().join("app.asar");
        std::fs::write(&path, bytes).expect("write");
        let s = path.display().to_string();
        (dir, s)
    }

    #[test]
    fn lists_nested_entries_and_main() {
        let (_d, path) = sample();
        let out = asar_path(&path, None, None);
        assert!(out.contains("lib/preload.js"), "{out}");
        assert!(out.contains("\"package_main\": \"main.js\""), "{out}");
        assert!(out.contains("preload script"), "{out}");
    }

    #[test]
    fn reads_entry_with_js_markers() {
        let (_d, path) = sample();
        let out = asar_path(&path, Some("main.js"), None);
        assert!(out.contains("child_process"), "{out}");
        assert!(out.contains("js_markers"), "{out}");
    }

    #[test]
    fn extracts_inside_dest_only() {
        let (d, path) = sample();
        let dest = d.path().join("out");
        let out = extract(&path, None, &dest.display().to_string());
        assert!(dest.join("lib/preload.js").is_file(), "{out}");
        assert!(safe_relative("../etc/passwd").is_none());
        assert!(safe_relative("/abs").is_none());
    }

    #[test]
    fn malformed_headers_never_panic() {
        for len in 0..64usize {
            let mut bytes = build_asar(&[("a.js", b"x")]);
            bytes.truncate(len);
            let _ = parse_header(&bytes);
        }
        let mut seed = 0x1234_5678u32;
        for _ in 0..500 {
            let mut bytes = build_asar(&[("a.js", b"x")]);
            for _ in 0..4 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let i = seed as usize % bytes.len();
                bytes[i] = (seed >> 16) as u8;
            }
            if let Ok((h, _)) = parse_header(&bytes) {
                let _ = flatten(&h);
            }
        }
    }
}
