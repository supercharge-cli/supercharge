//! APK / ZIP listing plus a bounded decoder for Android binary XML manifests.

use std::io::Read;

use serde::Serialize;

use super::bounds::{MAX_ARCHIVE_ENTRIES, MAX_ENTRY_READ, MAX_MANIFEST_LINES};
use super::evidence::{EvidenceError, EvidenceReply};

#[derive(Debug, Serialize)]
pub struct ZipMember {
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Serialize, Default)]
pub struct ApkInfo {
    pub entry_count_total: usize,
    pub entries: Vec<ZipMember>,
    pub dex_files: Vec<String>,
    pub native_libs: Vec<String>,
    pub signing_files: Vec<String>,
    pub package: Option<String>,
    pub permissions: Vec<String>,
    pub manifest: Option<Vec<String>>,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn string_pool(chunk: &[u8]) -> Vec<String> {
    let (Some(count), Some(flags), Some(start)) = (u32_at(chunk, 8), u32_at(chunk, 16), u32_at(chunk, 20)) else {
        return Vec::new();
    };
    let utf8 = flags & 0x100 != 0;
    let count = (count as usize).min(chunk.len() / 4);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let s = u32_at(chunk, 28 + i * 4)
            .and_then(|off| (start as usize).checked_add(off as usize))
            .and_then(|at| if utf8 { utf8_str(chunk, at) } else { utf16_str(chunk, at) })
            .unwrap_or_default();
        out.push(s);
    }
    out
}

fn len8(b: &[u8], at: usize) -> Option<(usize, usize)> {
    let first = *b.get(at)? as usize;
    if first & 0x80 != 0 {
        Some((((first & 0x7f) << 8) | *b.get(at + 1)? as usize, at + 2))
    } else {
        Some((first, at + 1))
    }
}

fn utf8_str(b: &[u8], at: usize) -> Option<String> {
    let (_, at) = len8(b, at)?;
    let (n, at) = len8(b, at)?;
    b.get(at..at.checked_add(n)?).map(|s| String::from_utf8_lossy(s).into_owned())
}

fn utf16_str(b: &[u8], at: usize) -> Option<String> {
    let mut len = u16_at(b, at)? as usize;
    let mut at = at + 2;
    if len & 0x8000 != 0 {
        len = ((len & 0x7fff) << 16) | u16_at(b, at)? as usize;
        at += 2;
    }
    let units: Vec<u16> = (0..len.min(65_536)).map_while(|i| u16_at(b, at + i * 2)).collect();
    Some(String::from_utf16_lossy(&units))
}

fn typed_value(pool: &[String], raw: u32, data_type: u8, data: u32) -> String {
    if raw != u32::MAX {
        return pool.get(raw as usize).cloned().unwrap_or_default();
    }
    match data_type {
        0x03 => pool.get(data as usize).cloned().unwrap_or_default(),
        0x10 => (data as i32).to_string(),
        0x11 => format!("0x{data:x}"),
        0x12 => (data != 0).to_string(),
        0x01 => format!("@0x{data:08x}"),
        _ => format!("0x{data:x}"),
    }
}

/// Decode Android binary XML into indented pseudo-XML lines (bounded).
pub fn decode_axml(b: &[u8]) -> Result<Vec<String>, String> {
    if u16_at(b, 0) != Some(0x0003) {
        return Err("not Android binary XML".into());
    }
    let mut pool = Vec::new();
    let mut lines = Vec::new();
    let mut depth = 0usize;
    let mut at = u16_at(b, 2).ok_or("truncated")? as usize;
    while at + 8 <= b.len() && lines.len() < MAX_MANIFEST_LINES {
        let (Some(ty), Some(size)) = (u16_at(b, at), u32_at(b, at + 4)) else { break };
        let size = size as usize;
        if size < 8 {
            break;
        }
        let chunk = &b[at..b.len().min(at.saturating_add(size))];
        let s = |i: u32| pool.get(i as usize).cloned().unwrap_or_default();
        match ty {
            0x0001 => pool = string_pool(chunk),
            0x0102 => {
                let ext = 16;
                let name = u32_at(chunk, ext + 4).map(s).unwrap_or_default();
                let attr_start = u16_at(chunk, ext + 8).unwrap_or(20) as usize;
                let attr_size = (u16_at(chunk, ext + 10).unwrap_or(20) as usize).max(20);
                let attr_count = u16_at(chunk, ext + 12).unwrap_or(0) as usize;
                let mut line = format!("{}<{name}", "  ".repeat(depth.min(32)));
                for i in 0..attr_count.min(256) {
                    let a = ext + attr_start + i * attr_size;
                    let (Some(an), Some(raw), Some(dt), Some(data)) =
                        (u32_at(chunk, a + 4), u32_at(chunk, a + 8), chunk.get(a + 15), u32_at(chunk, a + 16))
                    else {
                        break;
                    };
                    line.push_str(&format!(" {}=\"{}\"", s(an), typed_value(&pool, raw, *dt, data)));
                }
                line.push('>');
                lines.push(line);
                depth += 1;
            }
            0x0103 => depth = depth.saturating_sub(1),
            _ => {}
        }
        at = at.saturating_add(size);
    }
    Ok(lines)
}

fn attr<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!(" {key}=\"");
    let i = line.find(&pat)? + pat.len();
    line[i..].split('"').next()
}

/// `rea_apk`: list a zip/APK/JAR/IPA and decode its manifest when present.
pub fn apk_path(path: &str, include_manifest: bool) -> String {
    const OP: &str = "rea_apk";
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return EvidenceError::new(OP, Some(path.into()), e.to_string()).to_json_text(),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return EvidenceError::new(OP, Some(path.into()), format!("not a zip archive: {e}")).to_json_text(),
    };
    let mut info = ApkInfo { entry_count_total: archive.len(), ..Default::default() };
    let mut limitations = Vec::new();
    for i in 0..archive.len().min(MAX_ARCHIVE_ENTRIES) {
        let Ok(f) = archive.by_index_raw(i) else { continue };
        let name = f.name().to_owned();
        if name.ends_with(".dex") {
            info.dex_files.push(name.clone());
        } else if name.starts_with("lib/") && name.ends_with(".so") {
            info.native_libs.push(name.clone());
        } else if name.starts_with("META-INF/") && [".RSA", ".DSA", ".EC", ".SF"].iter().any(|s| name.ends_with(s)) {
            info.signing_files.push(name.clone());
        }
        info.entries.push(ZipMember { size: f.size(), compressed_size: f.compressed_size(), name });
    }
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        limitations.push(format!("listing capped at {MAX_ARCHIVE_ENTRIES} entries"));
    }
    if include_manifest {
        let read = archive.by_name("AndroidManifest.xml").ok().map(|f| {
            let mut buf = Vec::new();
            f.take(MAX_ENTRY_READ * 16).read_to_end(&mut buf).map(|_| buf)
        });
        match read {
            None => limitations.push("no AndroidManifest.xml (not an APK, or an app bundle)".into()),
            Some(Err(e)) => limitations.push(format!("manifest unreadable: {e}")),
            Some(Ok(buf)) => match decode_axml(&buf) {
                Ok(lines) => {
                    info.package = lines.first().and_then(|l| attr(l, "package")).map(str::to_owned);
                    info.permissions = lines
                        .iter()
                        .filter(|l| l.trim_start().starts_with("<uses-permission"))
                        .filter_map(|l| attr(l, "name").map(str::to_owned))
                        .collect();
                    info.manifest = Some(lines);
                }
                Err(e) => limitations.push(format!("manifest not decoded: {e}")),
            },
        }
    }
    limitations.push("DEX bytecode is listed, not decompiled; use jadx if rea_external_tools reports it".into());
    let mut reply = EvidenceReply::ok(OP, path, info);
    for l in limitations {
        reply = reply.with_limitation(l);
    }
    reply.to_json_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal AXML: UTF-16 pool ["manifest","package","com.example"], one element.
    fn sample_axml() -> Vec<u8> {
        let strings = ["manifest", "package", "com.example"];
        let mut data = Vec::new();
        let mut offs = Vec::new();
        for s in strings {
            offs.push(data.len() as u32);
            data.extend_from_slice(&(s.len() as u16).to_le_bytes());
            for u in s.encode_utf16() {
                data.extend_from_slice(&u.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
        }
        let start = 28 + offs.len() * 4;
        let mut pool = Vec::new();
        pool.extend_from_slice(&1u16.to_le_bytes());
        pool.extend_from_slice(&28u16.to_le_bytes());
        pool.extend_from_slice(&((start + data.len()) as u32).to_le_bytes());
        pool.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        pool.extend_from_slice(&(start as u32).to_le_bytes());
        pool.extend_from_slice(&0u32.to_le_bytes());
        for o in offs {
            pool.extend_from_slice(&o.to_le_bytes());
        }
        pool.extend_from_slice(&data);
        let mut el = Vec::new();
        el.extend_from_slice(&0x0102u16.to_le_bytes());
        el.extend_from_slice(&16u16.to_le_bytes());
        el.extend_from_slice(&56u32.to_le_bytes());
        el.extend_from_slice(&[0u8; 8]);
        el.extend_from_slice(&u32::MAX.to_le_bytes());
        el.extend_from_slice(&0u32.to_le_bytes());
        el.extend_from_slice(&20u16.to_le_bytes());
        el.extend_from_slice(&20u16.to_le_bytes());
        el.extend_from_slice(&1u16.to_le_bytes());
        el.extend_from_slice(&[0u8; 6]);
        el.extend_from_slice(&u32::MAX.to_le_bytes());
        el.extend_from_slice(&1u32.to_le_bytes());
        el.extend_from_slice(&2u32.to_le_bytes());
        el.extend_from_slice(&8u16.to_le_bytes());
        el.push(0);
        el.push(0x03);
        el.extend_from_slice(&2u32.to_le_bytes());
        let mut out = Vec::new();
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&((8 + pool.len() + el.len()) as u32).to_le_bytes());
        out.extend_from_slice(&pool);
        out.extend_from_slice(&el);
        out
    }

    #[test]
    fn decodes_minimal_manifest() {
        let lines = decode_axml(&sample_axml()).expect("decode");
        assert_eq!(lines, vec!["<manifest package=\"com.example\">".to_owned()]);
    }

    #[test]
    fn lists_apk_and_reads_package() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("a.apk");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).expect("create"));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("AndroidManifest.xml", opts).expect("start");
        w.write_all(&sample_axml()).expect("write");
        w.start_file("classes.dex", opts).expect("start");
        w.write_all(b"dex\n035\0").expect("write");
        w.finish().expect("finish");
        let out = apk_path(&path.display().to_string(), true);
        assert!(out.contains("\"package\": \"com.example\""), "{out}");
        assert!(out.contains("classes.dex"));
    }

    #[test]
    fn malformed_axml_never_panics() {
        let good = sample_axml();
        let mut seed = 99u32;
        for n in 0..good.len() {
            let _ = decode_axml(&good[..n]);
        }
        for _ in 0..2_000 {
            let mut b = good.clone();
            for _ in 0..3 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let i = seed as usize % b.len();
                b[i] = (seed >> 16) as u8;
            }
            let _ = decode_axml(&b);
        }
    }
}
