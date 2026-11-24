//! Content hashes: the cache key of a node result.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A 256-bit BLAKE3 content hash, shown as lowercase hex.
///
/// A node's cache key is the hash of (its kind, resolved params, fingerprint and the
/// hashes of the outputs bound to its inputs). An output's hash is derived from its
/// node's key and the port name, so the whole graph can be keyed without evaluating it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// Lowercase hex, 64 characters. Used as the file name in a run directory.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Parse 64 hex characters.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).ok_or_else(|| serde::de::Error::custom("expected 64 hex characters"))
    }
}

/// Length-prefixed hashing so that ("ab", "c") and ("a", "bc") never collide.
#[derive(Debug)]
pub(crate) struct KeyHasher(blake3::Hasher);

impl KeyHasher {
    pub(crate) fn new(domain: &str) -> Self {
        let mut h = Self(blake3::Hasher::new());
        h.bytes(domain.as_bytes());
        h
    }

    pub(crate) fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.update(&(b.len() as u64).to_le_bytes());
        self.0.update(b);
        self
    }

    pub(crate) fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    pub(crate) fn tag(&mut self, t: u8) -> &mut Self {
        self.0.update(&[t]);
        self
    }

    pub(crate) fn finish(&self) -> ContentHash {
        ContentHash(*self.0.finalize().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_length_prefixing() {
        let a = KeyHasher::new("t").str("ab").str("c").finish();
        let b = KeyHasher::new("t").str("a").str("bc").finish();
        assert_ne!(a, b);
        assert_eq!(ContentHash::from_hex(&a.to_hex()), Some(a));
    }
}
