//! Linear disassembly. x86/x86-64 is decoded in-process; other architectures
//! use `objdump` when it is on PATH and otherwise say so clearly.

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};
use object::{Architecture, Object, ObjectSection, ObjectSymbol};
use serde::Serialize;

use super::binary::fat_slice;
use super::bounds::MAX_BINARY_BYTES;
use super::bounds::MAX_INSTRUCTIONS;
use super::evidence::{EvidenceError, EvidenceReply};
use super::read_bytes::read_with_cap;

#[derive(Debug, Serialize)]
pub struct DisasmResult {
    pub architecture: String,
    pub start: String,
    pub section: String,
    pub instructions: Vec<String>,
}

pub fn parse_addr(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

/// Decode x86 bytes at `ip`, at most `count` instructions.
pub fn decode_x86(bitness: u32, bytes: &[u8], ip: u64, count: usize) -> Vec<String> {
    let mut decoder = Decoder::with_ip(bitness, bytes, ip, DecoderOptions::NONE);
    let mut fmt = IntelFormatter::new();
    let mut ins = Instruction::default();
    let mut text = String::new();
    let mut out = Vec::new();
    while decoder.can_decode() && out.len() < count {
        decoder.decode_out(&mut ins);
        text.clear();
        fmt.format(&ins, &mut text);
        let start = ins.ip().wrapping_sub(ip) as usize;
        let raw: String = bytes
            .get(start..start + ins.len())
            .unwrap_or(&[])
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        out.push(format!("{:016x}  {raw:<24} {text}", ins.ip()));
    }
    out
}

pub fn disassemble_bytes(
    bytes: &[u8],
    address: Option<u64>,
    symbol: Option<&str>,
    count: usize,
) -> Result<(DisasmResult, Vec<String>), String> {
    let data = fat_slice(bytes, None)
        .and_then(|(_, r)| bytes.get(r))
        .unwrap_or(bytes);
    let file = object::File::parse(data).map_err(|e| format!("not a parseable executable: {e}"))?;
    let bitness = match file.architecture() {
        Architecture::X86_64 | Architecture::X86_64_X32 => 64,
        Architecture::I386 => 32,
        other => {
            return Err(format!(
                "in-process disassembly covers x86 and x86-64 only (this file is {other:?}); use rea_external_tools to check for objdump"
            ));
        }
    };
    let start = match (address, symbol) {
        (Some(a), _) => a,
        (None, Some(name)) => file
            .symbols()
            .chain(file.dynamic_symbols())
            .find(|s| {
                s.name()
                    .is_ok_and(|n| n == name || n.trim_start_matches('_') == name)
            })
            .map(|s| s.address())
            .ok_or_else(|| format!("symbol {name} not found"))?,
        (None, None) => file.entry(),
    };
    let section = file
        .sections()
        .find(|s| s.address() <= start && start < s.address().saturating_add(s.size()))
        .ok_or_else(|| format!("address 0x{start:x} is not inside any section"))?;
    let sec_data = section
        .data()
        .map_err(|e| format!("section data unreadable: {e}"))?;
    let off = (start - section.address()) as usize;
    let slice = sec_data
        .get(off..)
        .ok_or("address is past the section's file data")?;
    let mut limitations = vec!["linear sweep: data inside code can decode as junk".to_owned()];
    if count >= MAX_INSTRUCTIONS {
        limitations.push(format!("capped at {MAX_INSTRUCTIONS} instructions"));
    }
    Ok((
        DisasmResult {
            architecture: format!("{:?}", file.architecture()),
            start: format!("0x{start:x}"),
            section: section.name().unwrap_or("").to_owned(),
            instructions: decode_x86(bitness, slice, start, count),
        },
        limitations,
    ))
}

/// `rea_disassemble`.
pub fn disassemble_path(
    path: &str,
    address: Option<&str>,
    symbol: Option<&str>,
    count: Option<usize>,
) -> String {
    const OP: &str = "rea_disassemble";
    let addr = match address.map(|a| parse_addr(a).ok_or(a)) {
        Some(Err(bad)) => {
            return EvidenceError::new(OP, Some(path.into()), format!("bad address {bad}"))
                .to_json_text();
        }
        Some(Ok(a)) => Some(a),
        None => None,
    };
    let count = count.unwrap_or(100).clamp(1, MAX_INSTRUCTIONS);
    let inspected = match read_with_cap(std::path::Path::new(path), MAX_BINARY_BYTES) {
        Ok(v) => v,
        Err(e) => return EvidenceError::new(OP, Some(path.into()), e.to_string()).to_json_text(),
    };
    match disassemble_bytes(&inspected.bytes, addr, symbol, count) {
        Ok((res, lims)) => {
            let mut reply = EvidenceReply::ok(OP, path, res);
            if inspected.truncated {
                reply =
                    reply.with_limitation("file larger than the parse cap; parsed only the prefix");
            }
            for l in lims {
                reply = reply.with_limitation(l);
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
    fn decodes_known_bytes() {
        let lines = decode_x86(64, &[0x55, 0x48, 0x89, 0xe5, 0xc3], 0x1000, 10);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("push rbp"), "{lines:?}");
        assert!(lines[2].ends_with("ret"));
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(parse_addr("0x10"), Some(16));
        assert_eq!(parse_addr("16"), Some(16));
        assert_eq!(parse_addr("zz"), None);
    }

    #[test]
    fn entry_of_own_binary_or_clear_error() {
        let exe = std::env::current_exe().expect("exe").display().to_string();
        let out = disassemble_path(&exe, None, None, Some(5));
        assert!(
            out.contains("instructions") || out.contains("x86-64 only"),
            "{out}"
        );
    }

    #[test]
    fn junk_bytes_never_panic() {
        let mut seed = 7u32;
        let bytes: Vec<u8> = (0..4096)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (seed >> 16) as u8
            })
            .collect();
        let _ = decode_x86(64, &bytes, 0, MAX_INSTRUCTIONS);
        let _ = decode_x86(32, &bytes[..3], u64::MAX - 1, 10);
        let _ = disassemble_bytes(&bytes, Some(0), None, 10);
    }
}
