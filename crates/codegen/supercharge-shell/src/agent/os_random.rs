//! Cross-platform secure randomness for sign-in secrets (PKCE verifiers, OAuth state).
//!
//! Bytes come from the operating system CSPRNG on every platform: the system RNG
//! APIs on Windows, `getentropy`/`getrandom` on macOS and Linux. Nothing here reads a
//! device path, so these helpers work the same on Windows, macOS, and Linux.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use rand::rngs::OsRng;

/// Fill `buf` from the OS CSPRNG.
pub fn fill_secure(buf: &mut [u8]) -> std::io::Result<()> {
    OsRng.try_fill_bytes(buf).map_err(std::io::Error::other)
}

/// `N` bytes from the OS CSPRNG, encoded as unpadded base64url.
pub fn secure_token_b64url<const N: usize>() -> std::io::Result<String> {
    let mut bytes = [0u8; N];
    fill_secure(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_secure_fills_and_differs() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        fill_secure(&mut a).unwrap();
        fill_secure(&mut b).unwrap();
        assert_ne!(a, [0u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn token_is_unpadded_base64url_of_expected_length() {
        let t = secure_token_b64url::<48>().unwrap();
        assert_eq!(t.len(), 64);
        assert!(
            t.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_eq!(URL_SAFE_NO_PAD.decode(&t).unwrap().len(), 48);
        assert_ne!(t, secure_token_b64url::<48>().unwrap());
    }

    #[test]
    fn no_device_path_in_this_module() {
        let src = include_str!("os_random.rs");
        let needle = ["/dev/", "urandom"].concat();
        assert!(!src.contains(&needle));
    }
}
