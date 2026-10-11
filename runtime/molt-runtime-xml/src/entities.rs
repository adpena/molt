//! XML character and entity references shared by the SAX and ElementTree parsers.

/// Decode XML references in one pass: the five predefined entities and
/// decimal (`&#38;`) or hexadecimal (`&#x26;`) character references, as
/// CPython's expat does. A decoded `&` never starts a second reference
/// (`&amp;lt;` is `&lt;`), and a reference that is not well formed or names
/// another entity stays as written.
pub(crate) fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        match decode_reference(tail) {
            Some((ch, consumed)) => {
                out.push(ch);
                rest = &tail[consumed..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character a reference at the start of `tail` names, and its length.
fn decode_reference(tail: &str) -> Option<(char, usize)> {
    let semi = tail.find(';')?;
    let name = &tail[1..semi];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "apos" => '\'',
        "quot" => '"',
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix('x') {
                Some(hex) if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                    u32::from_str_radix(hex, 16).ok()?
                }
                Some(_) => return None,
                None if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                    digits.parse::<u32>().ok()?
                }
                None => return None,
            };
            char::from_u32(code)?
        }
    };
    Some((ch, semi + 1))
}

#[cfg(test)]
mod tests {
    use super::unescape_xml;

    // Expected values are CPython's xml.etree.ElementTree.fromstring(...).text.
    #[test]
    fn decodes_predefined_entities_once() {
        assert_eq!(
            unescape_xml("a &lt; b &amp;&amp; c &gt; d"),
            "a < b && c > d"
        );
        assert_eq!(unescape_xml("&amp;lt;"), "&lt;");
        assert_eq!(unescape_xml("&quot;&apos;"), "\"'");
    }

    #[test]
    fn decodes_character_references() {
        assert_eq!(unescape_xml("&#65;&#x42;&#x1F600;"), "AB\u{1F600}");
        assert_eq!(unescape_xml("&#38;lt;"), "&lt;");
    }

    #[test]
    fn leaves_malformed_and_unknown_references() {
        assert_eq!(unescape_xml("a & b"), "a & b");
        assert_eq!(unescape_xml("&unknown;"), "&unknown;");
        assert_eq!(
            unescape_xml("&#;&#x;&#+5;&#X41;&#xD800;"),
            "&#;&#x;&#+5;&#X41;&#xD800;"
        );
        assert_eq!(unescape_xml("no references"), "no references");
    }
}
