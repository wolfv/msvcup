use sha2::{Digest, Sha256 as Sha256Hasher};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct Sha256 {
    pub bytes: [u8; 32],
}

impl Sha256 {
    pub fn parse_hex(hex: &str) -> Option<Sha256> {
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            let high = nibble_from_hex(hex.as_bytes()[i * 2])?;
            let low = nibble_from_hex(hex.as_bytes()[i * 2 + 1])?;
            *byte = (high << 4) | low;
        }
        Some(Sha256 { bytes })
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.bytes)
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.bytes {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({})", self)
    }
}

fn nibble_from_hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub fn lower_sha_hex(hex: &str) -> String {
    hex.to_ascii_lowercase()
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
