//! ASCII indentation removal shared by Python traceback source rendering.

/// Dedent only examines ASCII indentation; every other byte, including Python
/// lone-surrogate WTF-8, passes through the same implementation unchanged.
pub fn textwrap_dedent_bytes(text: &[u8]) -> Vec<u8> {
    let mut margin: Option<&[u8]> = None;
    let lines: Vec<&[u8]> = text.split(|byte| *byte == b'\n').collect();
    for &line in &lines {
        let indent_len = line
            .iter()
            .take_while(|byte| **byte == b' ' || **byte == b'\t')
            .count();
        if indent_len == line.len() {
            continue;
        }
        let indent = &line[..indent_len];
        margin = Some(match margin {
            Some(current) => {
                let common = current
                    .iter()
                    .zip(indent)
                    .take_while(|(left, right)| left == right)
                    .count();
                &current[..common]
            }
            None => indent,
        });
    }
    let margin = margin.unwrap_or(b"");
    let mut result = Vec::with_capacity(text.len());
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            result.push(b'\n');
        }
        if line.iter().all(|byte| *byte == b' ' || *byte == b'\t') {
            continue;
        }
        result.extend_from_slice(line.strip_prefix(margin).unwrap_or(line));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::textwrap_dedent_bytes;
    #[test]
    fn dedent_preserves_python_source_bytes() {
        for (input, expected) in [
            ("    alpha\n      beta\n", "alpha\n  beta\n"),
            ("a\n   \nb", "a\n\nb"),
            ("\t \n", "\n"),
            ("\u{a0}a\n\u{a0}b", "\u{a0}a\n\u{a0}b"),
        ] {
            assert_eq!(textwrap_dedent_bytes(input.as_bytes()), expected.as_bytes());
        }
        assert_eq!(
            textwrap_dedent_bytes(b"  \xed\xa0\x80\n"),
            b"\xed\xa0\x80\n"
        );
    }
}
