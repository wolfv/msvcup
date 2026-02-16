use sha2::{Digest, Sha256 as Sha256Hasher};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct Sha256 {
    pub bytes: [u8; 32],
}

impl Sha256 {
    pub fn parse_hex(hex_str: &str) -> Option<Sha256> {
        let decoded = hex::decode(hex_str).ok()?;
        let bytes: [u8; 32] = decoded.try_into().ok()?;
        Some(Sha256 { bytes })
    }

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
