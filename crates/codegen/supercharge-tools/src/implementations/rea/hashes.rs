//! Streaming digests: MD5, SHA-256, BLAKE3. Bound memory; never load whole file.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use md5::Context as Md5Context;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::bounds::READ_CHUNK;
use super::evidence::{EvidenceError, EvidenceReply};

#[derive(Debug, Clone, Serialize)]
pub struct HashResult {
    pub file_size: u64,
    pub bytes_hashed: u64,
    pub truncated: bool,
    pub md5: String,
    pub sha256: String,
    pub blake3: String,
}

/// Hash the whole file, or up to `max_bytes` when set.
pub fn hash_path(path: &str, max_bytes: Option<u64>) -> String {
    let p = Path::new(path);
    match hash_path_inner(p, max_bytes) {
        Ok((result, mut limitations)) => {
            let mut reply = EvidenceReply::ok("rea_hash", path, result);
            limitations.push(
                "Digests cover the bytes that were read. Truncation is recorded when a max_bytes cap applied."
                    .into(),
            );
            for lim in limitations {
                reply = reply.with_limitation(lim);
            }
            reply.to_json_text()
        }
        Err(e) => EvidenceError::new("rea_hash", Some(path.to_owned()), e).to_json_text(),
    }
}

fn hash_path_inner(
    path: &Path,
    max_bytes: Option<u64>,
) -> Result<(HashResult, Vec<String>), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    if meta.is_dir() {
        return Err("path is a directory".into());
    }
    let file_size = meta.len();
    let mut file = File::open(path).map_err(|e| format!("open failed: {e}"))?;

    let mut md5 = Md5Context::new();
    let mut sha = Sha256::new();
    let mut blake = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut hashed: u64 = 0;
    let limit = max_bytes.unwrap_or(u64::MAX);
    let mut limitations = Vec::new();

    loop {
        if hashed >= limit {
            break;
        }
        let want = ((limit - hashed) as usize).min(READ_CHUNK);
        let n = file
            .read(buf.get_mut(..want).unwrap_or_default())
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        md5.consume(chunk);
        sha.update(chunk);
        blake.update(chunk);
        hashed += n as u64;
    }

    let truncated = hashed < file_size;
    if truncated {
        limitations.push(format!(
            "Hashed {hashed} of {file_size} bytes (cap {:?}).",
            max_bytes
        ));
    }

    Ok((
        HashResult {
            file_size,
            bytes_hashed: hashed,
            truncated,
            md5: format!("{:x}", md5.finalize()),
            sha256: format!("{:x}", sha.finalize()),
            blake3: blake.finalize().to_hex().to_string(),
        },
        limitations,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v");
        std::fs::write(&path, b"abc").unwrap();
        let out = hash_path(path.to_str().unwrap(), None);
        // SHA-256("abc")
        assert!(
            out.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            "{out}"
        );
        assert!(out.contains("md5"), "{out}");
        assert!(out.contains("blake3"), "{out}");
    }

    #[test]
    fn respects_max_bytes_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        std::fs::write(&path, vec![b'x'; 1000]).unwrap();
        let out = hash_path(path.to_str().unwrap(), Some(10));
        assert!(
            out.contains("\"truncated\": true") || out.contains("\"truncated\":true"),
            "{out}"
        );
        assert!(out.contains("bytes_hashed"), "{out}");
    }

    #[test]
    fn missing_file_is_error_json() {
        let out = hash_path("/no/such/rea/hash", None);
        assert!(out.contains("error"), "{out}");
    }
}
