//! Bounded file reads. Never maps the whole file for unbounded work.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::bounds::{MAX_INSPECT_BYTES, READ_CHUNK};

#[derive(Debug)]
pub struct InspectedBytes {
    pub bytes: Vec<u8>,
    pub file_size: u64,
    /// True when `bytes` is only a prefix of the file.
    pub truncated: bool,
}

/// Read up to `MAX_INSPECT_BYTES` from `path`. Empty/missing files return Err.
pub fn read_for_inspect(path: &Path) -> std::io::Result<InspectedBytes> {
    read_with_cap(path, MAX_INSPECT_BYTES)
}

/// Read up to `cap` bytes from `path`.
pub fn read_with_cap(path: &Path, cap: u64) -> std::io::Result<InspectedBytes> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let take = file_size.min(cap) as usize;
    let mut bytes = Vec::with_capacity(take.min(READ_CHUNK));
    let mut buf = vec![0u8; READ_CHUNK];
    let mut remaining = take;
    while remaining > 0 {
        let n = file.read(&mut buf[..remaining.min(READ_CHUNK)])?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        remaining = remaining.saturating_sub(n);
    }
    Ok(InspectedBytes {
        truncated: file_size > cap,
        file_size,
        bytes,
    })
}

/// File size without reading contents. Returns 0 for non-files.
#[allow(dead_code)]
pub fn file_size(path: &Path) -> std::io::Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

/// Rewind helper kept for callers that stream after a peek.
#[allow(dead_code)]
pub fn rewind(file: &mut File) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}
