//! Shared code-point views and encoding helpers for Molt string storage.

pub use wtf8::{CodePoint, Wtf8};

/// View bytes already validated by Molt's string-storage constructors.
///
/// This preserves each stored code point independently, including adjacent
/// surrogate code points. Do not use `Wtf8Buf`, whose append operation may
/// fuse an adjacent surrogate pair into a scalar.
///
/// # Safety
/// `bytes` must contain only valid UTF-8 encodings and the three-byte WTF-8
/// encodings emitted by Molt for surrogate code points. The bytes must remain
/// alive and immutable for the returned view's lifetime.
#[inline]
pub unsafe fn from_validated_bytes(bytes: &[u8]) -> &Wtf8 {
    // SAFETY: upheld by the caller's Molt string-storage validation invariant.
    unsafe { &*(bytes as *const [u8] as *const Wtf8) }
}

pub fn push_wtf8_codepoint(out: &mut Vec<u8>, code: u32) {
    if code <= 0x7F {
        out.push(code as u8);
    } else if code <= 0x7FF {
        out.push(0xC0 | ((code >> 6) as u8));
        out.push(0x80 | (code as u8 & 0x3F));
    } else if code <= 0xFFFF {
        out.push(0xE0 | ((code >> 12) as u8));
        out.push(0x80 | (((code >> 6) as u8) & 0x3F));
        out.push(0x80 | (code as u8 & 0x3F));
    } else {
        out.push(0xF0 | ((code >> 18) as u8));
        out.push(0x80 | (((code >> 12) as u8) & 0x3F));
        out.push(0x80 | (((code >> 6) as u8) & 0x3F));
        out.push(0x80 | (code as u8 & 0x3F));
    }
}

pub fn push_backslash_bytes_vec(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in bytes {
        out.push(b'\\');
        out.push(b'x');
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0x0f) as usize]);
    }
}

#[cfg(test)]
mod tests {
    use super::{from_validated_bytes, push_wtf8_codepoint};

    #[test]
    fn validated_view_preserves_adjacent_surrogate_codepoints() {
        let points = [0xD83D, 0xDE00, 0x1F600, 0xD800, 0xDFFF];
        let expected_bytes = [
            0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80, 0xF0, 0x9F, 0x98, 0x80, 0xED, 0xA0, 0x80, 0xED,
            0xBF, 0xBF,
        ];
        let mut bytes = Vec::new();
        for point in points {
            push_wtf8_codepoint(&mut bytes, point);
        }
        assert_eq!(bytes, expected_bytes);

        // SAFETY: the fixed oracle vector is composed of valid UTF-8 scalar
        // encodings and Molt's three-byte surrogate encodings.
        let decoded: Vec<u32> = unsafe { from_validated_bytes(&expected_bytes) }
            .code_points()
            .map(|point| point.to_u32())
            .collect();
        assert_eq!(decoded, points);
    }
}
