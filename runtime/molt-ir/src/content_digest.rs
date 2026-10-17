//! Shared framing and wire encoding for compiler content digests.

use std::io::Write;

use sha2::{Digest, Sha256};

pub fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Durable, ambiguity-resistant SHA-256 identity for persistent compiler cache
/// artifacts. Every field is tagged and length-delimited; large semantic
/// contracts can be streamed through a nested digest without an O(IR) buffer.
pub struct CompilationCacheKey {
    digest: Sha256,
}

struct Sha256Writer<'a>(&'a mut Sha256);

impl Write for Sha256Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CompilationCacheKey {
    pub fn new(domain: &[u8]) -> Self {
        let mut key = Self {
            digest: Sha256::new(),
        };
        key.field(b"cache-key-domain", domain);
        key
    }

    fn length_delimited(digest: &mut Sha256, bytes: &[u8]) {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }

    pub fn field(&mut self, label: &[u8], value: &[u8]) {
        self.digest.update([0]);
        Self::length_delimited(&mut self.digest, label);
        Self::length_delimited(&mut self.digest, value);
    }

    pub fn digest_field(
        &mut self,
        label: &[u8],
        write_value: impl FnOnce(&mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut field_digest = Sha256::new();
        write_value(&mut Sha256Writer(&mut field_digest))?;
        self.digest.update([1]);
        Self::length_delimited(&mut self.digest, label);
        Self::length_delimited(&mut self.digest, &field_digest.finalize());
        Ok(())
    }

    pub fn finish_bytes(self) -> [u8; 32] {
        self.digest.finalize().into()
    }

    pub fn finish_hex(self) -> String {
        bytes_to_lower_hex(&self.finish_bytes())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_framing_preserves_independent_sha256_vectors() {
        // Python hashlib over literal tag/u64-BE-length/bytes fields, including
        // the domain field. These pin the persistent format across relocation.
        let mut raw = CompilationCacheKey::new(b"typed-field-test-v1");
        raw.field(b"value", &[7; 32]);
        assert_eq!(
            raw.finish_hex(),
            "c7eb9e5c75f5e1cd98f5ca03773354c5654760eda837cd20255702fa2689c745"
        );
        let mut streamed = CompilationCacheKey::new(b"typed-field-test-v1");
        streamed
            .digest_field(b"value", |writer| {
                writer
                    .write_all(&[7; 32])
                    .map_err(|error| error.to_string())
            })
            .unwrap();
        assert_eq!(
            streamed.finish_hex(),
            "cc63090ca661f7363b8dc60dcb3a0d65b330eb25e33f654bbbb591fbf6b874be"
        );
    }
}
