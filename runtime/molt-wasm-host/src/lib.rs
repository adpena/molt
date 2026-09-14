//! Content identity shared by the production host and its artifact consumers.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// Canonical lowercase SHA-256, independent of the digest crate's output wrapper.
///
/// ```
/// use molt_wasm_host::sha256_hex;
///
/// assert_eq!(
///     sha256_hex(b"abc"),
///     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
/// );
/// ```
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256_hex_parts([bytes])
}

/// Hash a segmented artifact without materializing a second payload buffer.
pub fn sha256_hex_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part);
    }
    sha256_digest_hex(&digest.finalize().into())
}

/// Format an already-computed SHA-256 digest using the canonical lowercase encoding.
pub fn sha256_digest_hex(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("formatting into String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{sha256_digest_hex, sha256_hex, sha256_hex_parts};
    use sha2::{Digest, Sha256};

    #[test]
    fn sha256_identity_matches_standard_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn segmented_and_cached_digest_identity_match() {
        let digest: [u8; 32] = Sha256::digest(b"abc").into();
        let expected = sha256_hex(b"abc");
        assert_eq!(sha256_digest_hex(&digest), expected);
        assert_eq!(sha256_hex_parts([b"a".as_slice(), b"", b"bc"]), expected);
        assert_eq!(sha256_hex_parts(std::iter::empty()), sha256_hex(b""));
    }

    #[test]
    fn digest_encoding_preserves_zero_padding_and_lowercase() {
        let mut digest = [0_u8; 32];
        digest[..4].copy_from_slice(&[0x00, 0x0f, 0x10, 0xff]);
        assert_eq!(
            sha256_digest_hex(&digest),
            format!("000f10ff{}", "00".repeat(28))
        );
    }
}
