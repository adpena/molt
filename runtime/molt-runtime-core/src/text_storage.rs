//! Python code points in Molt's UTF-8 surrogatepass string storage.
//!
//! Adjacent surrogate code points remain distinct. This is broader than
//! well-formed WTF-8 and cannot borrow the external `wtf8::Wtf8` type.

/// Iterate storage already admitted by a string producer, without allocating
/// or validating it a second time. Arbitrary external bytes belong to the
/// encoding-specific decoder before entering string storage.
///
/// Parsing uses ordinary bounds-checked reads and never casts bytes to a Rust
/// string or foreign unsized type. Invalid storage is an internal producer
/// defect; truncated storage panics, and debug builds check encoding validity.
#[inline]
pub fn code_points(bytes: &[u8]) -> CodePoints<'_> {
    CodePoints {
        bytes: bytes.iter(),
    }
}

#[derive(Clone)]
pub struct CodePoints<'a> {
    bytes: std::slice::Iter<'a, u8>,
}

impl CodePoints<'_> {
    #[inline]
    fn continuation(&mut self) -> u32 {
        let byte = *self.bytes.next().expect("truncated Python string storage");
        debug_assert_eq!(byte & 0xc0, 0x80, "invalid Python string continuation");
        u32::from(byte & 0x3f)
    }
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        let first = *self.bytes.next()?;
        if first < 0x80 {
            return Some(u32::from(first));
        }
        debug_assert!((0xc2..=0xf4).contains(&first), "invalid Python string lead");
        let second = self.continuation();
        let (code, minimum) = if first < 0xe0 {
            ((u32::from(first & 0x1f) << 6) | second, 0x80)
        } else {
            let third = self.continuation();
            if first < 0xf0 {
                (
                    (u32::from(first & 0x0f) << 12) | (second << 6) | third,
                    0x800,
                )
            } else {
                let fourth = self.continuation();
                (
                    (u32::from(first & 0x07) << 18) | (second << 12) | (third << 6) | fourth,
                    0x10000,
                )
            }
        };
        debug_assert!(
            (minimum..=0x10ffff).contains(&code),
            "invalid Python code point"
        );
        Some(code)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.bytes.len();
        (remaining.div_ceil(4), Some(remaining))
    }
}

impl std::iter::FusedIterator for CodePoints<'_> {}

/// Append one Python Unicode code point, including a surrogate. Invalid
/// ordinals fail before changing the output. Keep the direct Vec append path:
/// a measured stack-buffer/extend path costs substantially more per point.
#[inline(always)]
pub fn push_code_point(out: &mut Vec<u8>, code: u32) {
    if code <= 0x7f {
        out.push(code as u8);
    } else if code <= 0x7ff {
        out.extend_from_slice(&[0xc0 | ((code >> 6) as u8), 0x80 | (code as u8 & 0x3f)]);
    } else if code <= 0xffff {
        out.extend_from_slice(&[
            0xe0 | ((code >> 12) as u8),
            0x80 | (((code >> 6) as u8) & 0x3f),
            0x80 | (code as u8 & 0x3f),
        ]);
    } else {
        assert!(code <= 0x10ffff, "Python code point exceeds U+10FFFF");
        out.extend_from_slice(&[
            0xf0 | ((code >> 18) as u8),
            0x80 | (((code >> 12) as u8) & 0x3f),
            0x80 | (((code >> 6) as u8) & 0x3f),
            0x80 | (code as u8 & 0x3f),
        ]);
    }
}

#[inline]
pub fn push_backslash_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in bytes {
        out.push(b'\\');
        out.push(b'x');
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0x0f) as usize]);
    }
}

#[cfg(test)]
#[path = "text_storage_tests.rs"]
mod tests;
