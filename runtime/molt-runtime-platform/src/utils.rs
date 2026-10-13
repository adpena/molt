#[inline(always)]
pub fn usize_from_bits(bits: u64) -> Option<usize> {
    usize::try_from(bits).ok()
}

/// Unicode lowercase over Python's WTF-8 storage, preserving lone surrogates.
/// Surrogates are neither cased nor case-ignorable, so each valid UTF-8 span is
/// also an independent boundary for Unicode's contextual final-sigma rule.
pub fn python_lowercase(mut text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    while !text.is_empty() {
        match std::str::from_utf8(text) {
            Ok(valid) => {
                out.extend_from_slice(valid.to_lowercase().as_bytes());
                break;
            }
            Err(error) => {
                let valid_end = error.valid_up_to();
                let valid = std::str::from_utf8(&text[..valid_end])
                    .expect("UTF-8 validator established this prefix");
                out.extend_from_slice(valid.to_lowercase().as_bytes());
                let end = valid_end + error.error_len().unwrap_or(text.len() - valid_end);
                out.extend_from_slice(&text[valid_end..end]);
                text = &text[end..];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{python_lowercase, usize_from_bits};

    #[test]
    fn lowercase_preserves_python_surrogates_and_context() {
        for (input, expected) in [
            (b"A\xce\xa3".as_slice(), b"a\xcf\x82".as_slice()),
            (
                b"A\xce\xa3\xed\xa0\x80A\xce\xa3".as_slice(),
                b"a\xcf\x82\xed\xa0\x80a\xcf\x82".as_slice(),
            ),
            (
                b"A\xed\xa0\x80\xce\xa3A".as_slice(),
                b"a\xed\xa0\x80\xcf\x83a".as_slice(),
            ),
            (
                b"\xed\xa0\x80\xc3\x89\xc4\xb0\xce\xa3\xed\xbf\xbf".as_slice(),
                b"\xed\xa0\x80\xc3\xa9i\xcc\x87\xcf\x82\xed\xbf\xbf".as_slice(),
            ),
        ] {
            assert_eq!(python_lowercase(input), expected);
        }
    }

    #[test]
    fn usize_bits_conversion_preserves_the_target_width() {
        let value = usize::MAX;
        assert_eq!(usize_from_bits(value as u64), Some(value));
    }

    #[test]
    #[cfg(target_pointer_width = "32")]
    fn usize_bits_conversion_rejects_truncation() {
        assert_eq!(usize_from_bits(u64::from(u32::MAX) + 1), None);
    }
}
