//! File triage: magic bytes, extension hints, archive / Electron / Bun markers.
//! Pure-Rust; never crashes on malformed input.

use serde::Serialize;

use super::bounds::MAX_ARCHIVE_ENTRIES;
use super::evidence::{EvidenceError, EvidenceReply};
use super::read_bytes::read_for_inspect;

#[derive(Debug, Clone, Serialize)]
pub struct IdentifyResult {
    pub kind: String,
    pub mime: Option<String>,
    pub extension_guess: Option<String>,
    pub file_size: u64,
    pub magic_hex: String,
    pub markers: Vec<String>,
    pub archive: Option<ArchiveSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchiveSummary {
    pub format: String,
    pub entry_count_listed: usize,
    pub entry_count_total_hint: Option<usize>,
    pub entries: Vec<String>,
    pub truncated: bool,
}

pub fn identify_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return EvidenceError::new("rea_identify", Some(path.to_owned()), "path does not exist")
            .to_json_text();
    }
    if p.is_dir() {
        return EvidenceReply::ok(
            "rea_identify",
            path,
            IdentifyResult {
                kind: "directory".into(),
                mime: None,
                extension_guess: None,
                file_size: 0,
                magic_hex: String::new(),
                markers: vec!["directory".into()],
                archive: None,
            },
        )
        .with_limitation("Directories are not byte-scanned; list entries with a filesystem tool.")
        .to_json_text();
    }

    let inspected = match read_for_inspect(p) {
        Ok(v) => v,
        Err(e) => {
            return EvidenceError::new(
                "rea_identify",
                Some(path.to_owned()),
                format!("read failed: {e}"),
            )
            .to_json_text();
        }
    };

    let mut limitations = Vec::new();
    if inspected.truncated {
        limitations.push(format!(
            "Only the first {} bytes were inspected (file is {} bytes).",
            inspected.bytes.len(),
            inspected.file_size
        ));
    }

    let magic_hex = inspected
        .bytes
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");

    let mut markers = Vec::new();
    let mut kind = "unknown".to_string();
    let mut mime = None;
    let mut extension_guess = None;
    let mut archive = None;

    // infer crate (magic DB) — never panics
    if let Some(t) = infer::get(&inspected.bytes) {
        mime = Some(t.mime_type().to_string());
        extension_guess = Some(t.extension().to_string());
        kind = t.mime_type().to_string();
        markers.push(format!("infer:{}", t.mime_type()));
    }

    classify_magic(&inspected.bytes, &mut kind, &mut markers);

    // ZIP / APK / JAR / IPA family
    if inspected.bytes.starts_with(b"PK\x03\x04") || inspected.bytes.starts_with(b"PK\x05\x06") {
        markers.push("zip_local_or_empty".into());
        if kind == "unknown" || kind.starts_with("application/") {
            kind = "zip_archive".into();
        }
        match summarize_zip(p) {
            Ok(sum) => {
                if path_looks_like_apk(path, &sum) {
                    kind = "android_apk".into();
                    markers.push("android_apk_candidate".into());
                }
                archive = Some(sum);
            }
            Err(e) => limitations.push(format!("zip listing failed: {e}")),
        }
    }

    // Electron ASAR: uint32 LE header size, then JSON
    if let Some(asar) = detect_asar(&inspected.bytes) {
        kind = "electron_asar".into();
        markers.push("electron_asar".into());
        archive = Some(asar);
    }

    // Bun standalone: look for common trailer / embedded markers in the prefix+we already have
    if looks_like_bun(&inspected.bytes) {
        markers.push("bun_standalone_candidate".into());
        if kind == "unknown"
            || kind.contains("elf")
            || kind.contains("executable")
            || kind == "application/x-mach-binary"
            || kind == "application/vnd.microsoft.portable-executable"
        {
            // keep binary kind; note bun as marker
        }
    }

    // Node SEA / pkg markers (bounded search)
    if contains_ascii(&inspected.bytes, b"NODE_SEA_BLOB")
        || contains_ascii(&inspected.bytes, b"pkg/prelude")
    {
        markers.push("node_standalone_candidate".into());
    }
    if contains_ascii(&inspected.bytes, b"d3n0l4nd") || contains_ascii(&inspected.bytes, b"denort") {
        markers.push("deno_standalone_candidate".into());
    }

    let mut reply = EvidenceReply::ok(
        "rea_identify",
        path,
        IdentifyResult {
            kind,
            mime,
            extension_guess,
            file_size: inspected.file_size,
            magic_hex,
            markers,
            archive,
        },
    );
    for lim in limitations {
        reply = reply.with_limitation(lim);
    }
    reply = reply.with_limitation(
        "Static triage only: no code execution, no deep decompilation. Use rea_hash, rea_strings, rea_binary, rea_asar or rea_apk next.",
    );
    reply.to_json_text()
}

fn classify_magic(bytes: &[u8], kind: &mut String, markers: &mut Vec<String>) {
    if bytes.len() >= 4 && bytes.starts_with(&[0x7f, b'E', b'L', b'F']) {
        *kind = "elf".into();
        markers.push("elf".into());
        if let Some(cls) = bytes.get(4) {
            markers.push(match cls {
                1 => "elf32".into(),
                2 => "elf64".into(),
                _ => format!("elf_class_{cls}"),
            });
        }
    } else if bytes.len() >= 2 && (bytes.starts_with(b"MZ") || bytes.starts_with(b"ZM")) {
        *kind = "pe".into();
        markers.push("pe_mz".into());
    } else if bytes.len() >= 4
        && (bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xce])
            || bytes.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
            || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
            || bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
            || bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
            || bytes.starts_with(&[0xbe, 0xba, 0xfe, 0xca]))
    {
        *kind = "macho".into();
        markers.push("macho".into());
    } else if bytes.starts_with(b"\0asm") {
        *kind = "wasm".into();
        markers.push("wasm".into());
    } else if bytes.starts_with(b"#!") {
        markers.push("shebang".into());
        if kind == "unknown" {
            *kind = "script".into();
        }
    }
}

fn path_looks_like_apk(path: &str, sum: &ArchiveSummary) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".apk") || lower.ends_with(".aab") {
        return true;
    }
    sum.entries
        .iter()
        .any(|e| e == "AndroidManifest.xml" || e.starts_with("classes") && e.ends_with(".dex"))
}

fn summarize_zip(path: &std::path::Path) -> Result<ArchiveSummary, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let total = archive.len();
    let mut entries = Vec::new();
    let limit = total.min(MAX_ARCHIVE_ENTRIES);
    for i in 0..limit {
        match archive.by_index(i) {
            Ok(f) => entries.push(f.name().to_owned()),
            Err(e) => entries.push(format!("<unreadable index {i}: {e}>")),
        }
    }
    Ok(ArchiveSummary {
        format: "zip".into(),
        entry_count_listed: entries.len(),
        entry_count_total_hint: Some(total),
        entries,
        truncated: total > MAX_ARCHIVE_ENTRIES,
    })
}

fn detect_asar(bytes: &[u8]) -> Option<ArchiveSummary> {
    let (header, _) = super::asar::parse_header(bytes).ok()?;
    let (entries, total) = super::asar::flatten(&header);
    let entries: Vec<String> = entries.into_iter().map(|e| e.path).collect();
    Some(ArchiveSummary {
        format: "asar".into(),
        entry_count_listed: entries.len(),
        entry_count_total_hint: Some(total),
        truncated: total > entries.len(),
        entries,
    })
}

fn looks_like_bun(bytes: &[u8]) -> bool {
    // Bun compile embeds version / trailer strings; search a bounded window only.
    contains_ascii(bytes, b"Bun/")
        || contains_ascii(bytes, b"bun-linux")
        || contains_ascii(bytes, b"bun-darwin")
        || contains_ascii(bytes, b"\0bun\0")
}

fn contains_ascii(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn identifies_elf_magic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.elf");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0]).unwrap();
        f.write_all(&[0u8; 64]).unwrap();
        let out = identify_path(path.to_str().unwrap());
        assert!(out.contains("elf"), "{out}");
        assert!(out.contains("limitations"), "{out}");
    }

    #[test]
    fn identifies_empty_and_missing_without_panic() {
        let out = identify_path("/no/such/rea/target/zzz");
        assert!(
            out.contains("does not exist") || out.contains("error"),
            "{out}"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        std::fs::write(&path, b"").unwrap();
        let out = identify_path(path.to_str().unwrap());
        assert!(out.contains("rea_identify"), "{out}");
    }

    #[test]
    fn fuzz_random_bytes_never_panic() {
        let dir = tempfile::tempdir().unwrap();
        for seed in 0u64..32 {
            let mut data = vec![0u8; 4096];
            for (i, b) in data.iter_mut().enumerate() {
                *b = ((seed.wrapping_mul(1103515245).wrapping_add(i as u64 * 12345)) % 256) as u8;
            }
            // sprinkle zip / elf / asar-ish headers
            if seed % 3 == 0 {
                data[..4].copy_from_slice(b"PK\x03\x04");
            }
            if seed % 5 == 0 {
                data[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
            }
            let path = dir.path().join(format!("fuzz-{seed}"));
            std::fs::write(&path, &data).unwrap();
            let _ = identify_path(path.to_str().unwrap());
        }
    }

    #[test]
    fn detects_asar_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.asar");
        let bytes = super::super::asar::build_asar(&[("main.js", b"x"), ("package.json", b"{}")]);
        std::fs::write(&path, &bytes).unwrap();
        let out = identify_path(path.to_str().unwrap());
        assert!(out.contains("electron_asar"), "{out}");
        assert!(out.contains("main.js"), "{out}");
    }
}
