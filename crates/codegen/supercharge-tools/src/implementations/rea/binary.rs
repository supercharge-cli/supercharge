//! ELF / PE / Mach-O structure: headers, sections, imports, exports, symbols.

use object::read::macho::{FatArch, MachOFatFile32, MachOFatFile64};
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
use serde::Serialize;

use super::bounds::MAX_BINARY_ITEMS;
use super::evidence::{EvidenceError, EvidenceReply};
use super::bounds::MAX_BINARY_BYTES;
use super::read_bytes::read_with_cap;

#[derive(Debug, Serialize)]
pub struct SectionInfo {
    pub name: String,
    pub address: String,
    pub size: u64,
    pub file_offset: Option<u64>,
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct ImportInfo {
    pub library: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct SymbolInfo {
    pub name: String,
    pub address: String,
    pub size: u64,
    pub kind: String,
    pub global: bool,
    pub undefined: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct BinaryInfo {
    pub format: String,
    pub architecture: String,
    pub endianness: String,
    pub is_64: bool,
    pub kind: String,
    pub entry: String,
    pub fat_slices: Vec<String>,
    pub segments: Vec<SectionInfo>,
    pub sections: Vec<SectionInfo>,
    pub imports: Vec<ImportInfo>,
    pub exports: Vec<SymbolInfo>,
    pub symbols: Vec<SymbolInfo>,
    pub symbol_count_total: usize,
}

fn hex(v: u64) -> String {
    format!("0x{v:x}")
}

/// For a fat Mach-O, return the slice list and the byte range of the chosen slice.
pub fn fat_slice(bytes: &[u8], arch: Option<&str>) -> Option<(Vec<String>, std::ops::Range<usize>)> {
    fn pick<A: FatArch>(arches: &[A], arch: Option<&str>) -> (Vec<String>, Option<(u64, u64)>) {
        let names: Vec<String> = arches.iter().map(|a| format!("{:?}", a.architecture())).collect();
        let idx = arch
            .and_then(|want| names.iter().position(|n| n.eq_ignore_ascii_case(want)))
            .unwrap_or(0);
        (names, arches.get(idx).map(|a| a.file_range()))
    }
    let (names, range) = match object::FileKind::parse(bytes).ok()? {
        object::FileKind::MachOFat32 => pick(MachOFatFile32::parse(bytes).ok()?.arches(), arch),
        object::FileKind::MachOFat64 => pick(MachOFatFile64::parse(bytes).ok()?.arches(), arch),
        _ => return None,
    };
    let (off, size) = range?;
    let start = usize::try_from(off).ok()?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    (end <= bytes.len()).then_some((names, start..end))
}

pub fn parse_info(bytes: &[u8], arch: Option<&str>, filter: Option<&str>) -> Result<(BinaryInfo, Vec<String>), String> {
    let mut limitations = Vec::new();
    let mut info = BinaryInfo::default();
    let data = match fat_slice(bytes, arch) {
        Some((names, range)) => {
            limitations.push(format!("fat Mach-O: showing slice {} of {}", range.start, names.join(", ")));
            info.fat_slices = names;
            &bytes[range]
        }
        None => bytes,
    };
    let file = object::File::parse(data).map_err(|e| format!("not a parseable ELF/PE/Mach-O/COFF/Wasm file: {e}"))?;
    let keep = |name: &str| filter.is_none_or(|f| name.to_ascii_lowercase().contains(&f.to_ascii_lowercase()));
    info.format = format!("{:?}", file.format());
    info.architecture = format!("{:?}", file.architecture());
    info.endianness = format!("{:?}", file.endianness());
    info.is_64 = file.is_64();
    info.kind = format!("{:?}", file.kind());
    info.entry = hex(file.entry());
    for seg in file.segments().take(MAX_BINARY_ITEMS) {
        let (off, _) = seg.file_range();
        info.segments.push(SectionInfo {
            name: seg.name().ok().flatten().unwrap_or("").to_owned(),
            address: hex(seg.address()),
            size: seg.size(),
            file_offset: Some(off),
            kind: String::new(),
        });
    }
    for sec in file.sections() {
        if info.sections.len() >= MAX_BINARY_ITEMS {
            limitations.push(format!("sections capped at {MAX_BINARY_ITEMS}"));
            break;
        }
        info.sections.push(SectionInfo {
            name: sec.name().unwrap_or("<bad name>").to_owned(),
            address: hex(sec.address()),
            size: sec.size(),
            file_offset: sec.file_range().map(|(o, _)| o),
            kind: format!("{:?}", sec.kind()),
        });
    }
    match file.imports() {
        Ok(imports) => {
            for imp in imports {
                let name = String::from_utf8_lossy(imp.name()).into_owned();
                if !keep(&name) {
                    continue;
                }
                if info.imports.len() >= MAX_BINARY_ITEMS {
                    limitations.push(format!("imports capped at {MAX_BINARY_ITEMS}"));
                    break;
                }
                info.imports.push(ImportInfo { library: String::from_utf8_lossy(imp.library()).into_owned(), name });
            }
        }
        Err(e) => limitations.push(format!("imports unreadable: {e}")),
    }
    match file.exports() {
        Ok(exports) => {
            for exp in exports {
                let name = String::from_utf8_lossy(exp.name()).into_owned();
                if !keep(&name) {
                    continue;
                }
                if info.exports.len() >= MAX_BINARY_ITEMS {
                    limitations.push(format!("exports capped at {MAX_BINARY_ITEMS}"));
                    break;
                }
                info.exports.push(SymbolInfo {
                    name,
                    address: hex(exp.address()),
                    size: 0,
                    kind: "export".into(),
                    global: true,
                    undefined: false,
                });
            }
        }
        Err(e) => limitations.push(format!("exports unreadable: {e}")),
    }
    let mut total = 0usize;
    for sym in file.symbols().chain(file.dynamic_symbols()) {
        let name = sym.name().unwrap_or("");
        if name.is_empty() || !keep(name) {
            continue;
        }
        total += 1;
        if info.symbols.len() < MAX_BINARY_ITEMS {
            info.symbols.push(SymbolInfo {
                name: name.to_owned(),
                address: hex(sym.address()),
                size: sym.size(),
                kind: format!("{:?}", sym.kind()),
                global: sym.is_global(),
                undefined: sym.is_undefined(),
            });
        }
    }
    info.symbol_count_total = total;
    if total > MAX_BINARY_ITEMS {
        limitations.push(format!("symbols capped at {MAX_BINARY_ITEMS} of {total}; pass filter to narrow"));
    }
    if total == 0 {
        limitations.push("no symbols found (stripped or filtered)".into());
    }
    Ok((info, limitations))
}

/// `rea_binary`.
pub fn binary_path(path: &str, arch: Option<&str>, filter: Option<&str>) -> String {
    const OP: &str = "rea_binary";
    let inspected = match read_with_cap(std::path::Path::new(path), MAX_BINARY_BYTES) {
        Ok(v) => v,
        Err(e) => return EvidenceError::new(OP, Some(path.into()), e.to_string()).to_json_text(),
    };
    match parse_info(&inspected.bytes, arch, filter) {
        Ok((info, lims)) => {
            let mut reply = EvidenceReply::ok(OP, path, info);
            for l in lims {
                reply = reply.with_limitation(l);
            }
            if inspected.truncated {
                reply = reply.with_limitation("file larger than the inspect cap; parsed only the prefix");
            }
            reply.to_json_text()
        }
        Err(e) => EvidenceError::new(OP, Some(path.into()), e).to_json_text(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_own_test_binary() {
        let exe = std::env::current_exe().expect("exe");
        let out = binary_path(&exe.display().to_string(), None, Some("main"));
        assert!(out.contains("\"format\""), "{}", &out[..out.len().min(400)]);
        assert!(!out.contains("\"error\""));
    }

    #[test]
    fn malformed_input_never_panics() {
        let exe = std::fs::read(std::env::current_exe().expect("exe")).expect("read");
        let head = &exe[..exe.len().min(64 * 1024)];
        let mut seed = 0xdead_beefu32;
        for round in 0..300 {
            let mut bytes = head.to_vec();
            bytes.truncate(16 + round * 97 % head.len().max(17));
            for _ in 0..8 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                if !bytes.is_empty() {
                    let i = seed as usize % bytes.len();
                    bytes[i] = (seed >> 16) as u8;
                }
            }
            let _ = parse_info(&bytes, None, None);
        }
        for n in 0..32 {
            let _ = parse_info(&vec![0xCAu8; n], None, None);
        }
        let _ = parse_info(b"\xCA\xFE\xBA\xBE\x00\x00\x00\x02garbage", None, None);
    }
}
