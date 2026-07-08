//! Pure RFC 3492 Punycode codec algorithms for `encodings.punycode`.
//!
//! Runtime crates keep object marshaling and ABI exports; this module owns the
//! reusable text/bytes encode-decode behavior.

const BASE: u32 = 36;
const TMIN: u32 = 1;
const TMAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 0x80;

const DIGITS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

fn adapt(mut delta: u32, num_points: u32, first_time: bool) -> u32 {
    delta = if first_time { delta / DAMP } else { delta / 2 };
    delta += delta / num_points;
    let mut k = 0u32;
    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }
    k + (BASE * delta) / (delta + SKEW)
}

#[inline]
fn threshold(k: u32, bias: u32) -> u32 {
    if k <= bias + TMIN {
        TMIN
    } else if k >= bias + TMAX {
        TMAX
    } else {
        k - bias
    }
}

fn encode_vli(mut n: u32, bias: u32, out: &mut Vec<u8>) {
    let mut k = BASE;
    loop {
        let t = threshold(k, bias);
        if n < t {
            out.push(DIGITS[n as usize]);
            return;
        }
        let digit = t + ((n - t) % (BASE - t));
        out.push(DIGITS[digit as usize]);
        n = (n - t) / (BASE - t);
        k += BASE;
    }
}

pub fn punycode_encode_impl(input: &str) -> Vec<u8> {
    let chars: Vec<char> = input.chars().collect();
    let mut base = Vec::new();
    let mut extended_set: Vec<u32> = Vec::new();

    for &ch in &chars {
        let cp = ch as u32;
        if cp < 128 {
            base.push(cp as u8);
        } else if !extended_set.contains(&cp) {
            extended_set.push(cp);
        }
    }
    extended_set.sort_unstable();

    let mut result = base.clone();

    let b = base.len() as u32;
    let mut h = b;
    let mut n = INITIAL_N;
    let mut delta = 0u32;
    let mut bias = INITIAL_BIAS;
    let total = chars.len() as u32;

    if b > 0 {
        result.push(b'-');
    }

    while h < total {
        let mut m = u32::MAX;
        for &ch in &chars {
            let cp = ch as u32;
            if cp >= n && cp < m {
                m = cp;
            }
        }

        delta = delta.saturating_add((m - n).saturating_mul(h + 1));
        n = m;

        for &ch in &chars {
            let cp = ch as u32;
            if cp < n {
                delta = delta.saturating_add(1);
            } else if cp == n {
                encode_vli(delta, bias, &mut result);
                bias = adapt(delta, h + 1, h == b);
                delta = 0;
                h += 1;
            }
        }

        delta += 1;
        n += 1;
    }

    result
}

fn decode_digit(ch: u8) -> Option<u32> {
    match ch {
        b'A'..=b'Z' => Some(u32::from(ch - b'A')),
        b'a'..=b'z' => Some(u32::from(ch - b'a')),
        b'0'..=b'9' => Some(u32::from(ch - b'0') + 26),
        _ => None,
    }
}

pub fn punycode_decode_impl(input: &[u8], errors: &str) -> Result<String, String> {
    let (base_part, extended_part) = match input.iter().rposition(|&b| b == b'-') {
        Some(pos) => (&input[..pos], &input[pos + 1..]),
        None => (&input[..0], input),
    };

    let mut output: Vec<char> = Vec::with_capacity(base_part.len() + extended_part.len());
    for &b in base_part {
        if b > 127 {
            if errors == "strict" {
                return Err(format!("Invalid character U+{:x}", b));
            }
            output.push('?');
        } else {
            output.push(b as char);
        }
    }

    let ext_upper: Vec<u8> = extended_part
        .iter()
        .map(|b| b.to_ascii_uppercase())
        .collect();

    let mut n = INITIAL_N;
    let mut i = 0u32;
    let mut bias = INITIAL_BIAS;
    let mut extpos = 0usize;

    while extpos < ext_upper.len() {
        let oldi = i;
        let mut w = 1u32;
        let mut k = BASE;

        loop {
            if extpos >= ext_upper.len() {
                if errors == "strict" {
                    return Err("incomplete punicode string".to_string());
                }
                return Ok(output.into_iter().collect());
            }

            let digit = match decode_digit(ext_upper[extpos]) {
                Some(d) => d,
                None => {
                    if errors == "strict" {
                        return Err(format!(
                            "Invalid extended code point '{}'",
                            ext_upper[extpos] as char
                        ));
                    }
                    return Ok(output.into_iter().collect());
                }
            };
            extpos += 1;

            i = i.saturating_add(digit.saturating_mul(w));

            let t = threshold(k, bias);
            if digit < t {
                break;
            }
            w = w.saturating_mul(BASE - t);
            k += BASE;
        }

        let out_len = (output.len() + 1) as u32;
        bias = adapt(i - oldi, out_len, oldi == 0);
        n = n.saturating_add(i / out_len);
        i %= out_len;

        if n > 0x10FFFF {
            if errors == "strict" {
                return Err(format!("Invalid character U+{n:x}"));
            }
            output.insert(i as usize, '?');
        } else {
            match char::from_u32(n) {
                Some(ch) => output.insert(i as usize, ch),
                None => {
                    if errors == "strict" {
                        return Err(format!("Invalid character U+{n:x}"));
                    }
                    output.insert(i as usize, '?');
                }
            }
        }

        i += 1;
    }

    Ok(output.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_ascii_only() {
        assert_eq!(punycode_encode_impl("abc"), b"abc-");
    }

    #[test]
    fn encode_empty() {
        assert_eq!(punycode_encode_impl(""), b"");
    }

    #[test]
    fn encode_non_ascii_only() {
        let result = punycode_encode_impl("\u{00fc}");
        assert!(!result.is_empty());
        assert!(!result.contains(&b'-'));
    }

    #[test]
    fn encode_mixed() {
        let result = punycode_encode_impl("M\u{00fc}nchen");
        assert_eq!(result, b"Mnchen-3ya");
    }

    #[test]
    fn decode_ascii_only() {
        let result = punycode_decode_impl(b"abc-", "strict").unwrap();
        assert_eq!(result, "abc");
    }

    #[test]
    fn decode_empty() {
        let result = punycode_decode_impl(b"", "strict").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn roundtrip_mixed() {
        let original = "M\u{00fc}nchen";
        let encoded = punycode_encode_impl(original);
        let decoded = punycode_decode_impl(&encoded, "strict").unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn roundtrip_cjk() {
        let original = "\u{65e5}\u{672c}\u{8a9e}";
        let encoded = punycode_encode_impl(original);
        let decoded = punycode_decode_impl(&encoded, "strict").unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn roundtrip_emoji() {
        let original = "hello\u{1f30d}";
        let encoded = punycode_encode_impl(original);
        let decoded = punycode_decode_impl(&encoded, "strict").unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_invalid_strict() {
        let result = punycode_decode_impl(b"abc-!!!!", "strict");
        assert!(result.is_err());
    }

    #[test]
    fn decode_invalid_replace() {
        let result = punycode_decode_impl(b"abc-!!!!", "replace");
        assert!(result.is_ok());
    }
}
