//! SHA256 hashing utilities.
//!
//! Provides both one-shot hashing ([`Sha256`]) and incremental/streaming
//! hashing ([`Sha256Streaming`]) for verifying downloaded file integrity.

use sha2::{Digest, Sha256 as Sha256Hasher};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sha256 {
    pub bytes: [u8; 32],
}

impl Sha256 {
    pub fn parse_hex(hex_str: &str) -> Option<Sha256> {
        let decoded = hex::decode(hex_str).ok()?;
        let bytes: [u8; 32] = decoded.try_into().ok()?;
        Some(Sha256 { bytes })
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn to_hex(&self) -> String {
        hex::encode(self.bytes)
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({})", self)
    }
}

pub struct Sha256Streaming {
    hasher: Sha256Hasher,
}

impl Sha256Streaming {
    pub fn new() -> Self {
        Self {
            hasher: Sha256Hasher::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    pub fn finalize(self) -> Sha256 {
        let result = self.hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Sha256 { bytes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZEROS_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn parse_hex_valid() {
        let sha = Sha256::parse_hex(ZEROS_HEX).unwrap();
        assert_eq!(sha.bytes, [0u8; 32]);
    }

    #[test]
    fn parse_hex_roundtrip() {
        let sha = Sha256::parse_hex(HELLO_SHA256).unwrap();
        assert_eq!(sha.to_hex(), HELLO_SHA256);
    }

    #[test]
    fn parse_hex_rejects_short() {
        assert!(Sha256::parse_hex("abcd").is_none());
    }

    #[test]
    fn parse_hex_rejects_invalid_chars() {
        let bad = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert!(Sha256::parse_hex(bad).is_none());
    }

    #[test]
    fn parse_hex_rejects_empty() {
        assert!(Sha256::parse_hex("").is_none());
    }

    #[test]
    fn display_matches_to_hex() {
        let sha = Sha256::parse_hex(HELLO_SHA256).unwrap();
        assert_eq!(format!("{}", sha), HELLO_SHA256);
    }

    #[test]
    fn debug_format() {
        let sha = Sha256::parse_hex(ZEROS_HEX).unwrap();
        let dbg = format!("{:?}", sha);
        assert!(dbg.starts_with("Sha256("));
        assert!(dbg.contains(ZEROS_HEX));
    }

    #[test]
    fn equality() {
        let a = Sha256::parse_hex(HELLO_SHA256).unwrap();
        let b = Sha256::parse_hex(HELLO_SHA256).unwrap();
        let c = Sha256::parse_hex(ZEROS_HEX).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn streaming_hash_of_hello() {
        let mut hasher = Sha256Streaming::new();
        hasher.update(b"hello");
        let result = hasher.finalize();
        assert_eq!(result.to_hex(), HELLO_SHA256);
    }

    #[test]
    fn streaming_hash_incremental() {
        let mut hasher = Sha256Streaming::new();
        hasher.update(b"hel");
        hasher.update(b"lo");
        let result = hasher.finalize();
        assert_eq!(result.to_hex(), HELLO_SHA256);
    }

    #[test]
    fn streaming_hash_empty() {
        let hasher = Sha256Streaming::new();
        let result = hasher.finalize();
        // SHA-256 of empty input
        assert_eq!(
            result.to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
