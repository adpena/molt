//! String encoding and decoding — extracted from ops.rs for maintainability.

use super::ops_string::{push_wtf8_codepoint, wtf8_from_bytes, wtf8_has_surrogates};
use molt_stdlib_text::charmap_codecs_generated::{
    decode_single_byte_charmap_with_errors, encode_single_byte_charmap_byte, single_byte_charmap,
};
pub(crate) use molt_stdlib_text::codec_errors::DecodeFailure;
pub(crate) use molt_stdlib_text::codec_registry::{
    CodecRuntimeClass, EncodingKind, normalize_encoding,
};
use molt_stdlib_text::wtf8::push_backslash_bytes_vec;

pub(crate) fn encoding_kind_name(kind: EncodingKind) -> &'static str {
    kind.name()
}

pub(crate) enum EncodeError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    /// One strict failure covers CPython's whole range: `start` is the first
    /// code point the handler could not consume and `end` closes the run of
    /// consecutive code points the codec cannot encode (UTF-16/32 report one).
    InvalidChar {
        encoding: &'static str,
        code: u32,
        start: usize,
        end: usize,
        limit: u32,
    },
}

/// End of the run of consecutive unencodable code points that begins at the
/// unencodable code point `start`, as CPython's encoders collect it.
fn unencodable_run_end(bytes: &[u8], start: usize, unencodable: impl Fn(u32) -> bool) -> usize {
    start
        + 1
        + wtf8_from_bytes(bytes)
            .code_points()
            .skip(start + 1)
            .take_while(|cp| unencodable(cp.to_u32()))
            .count()
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

fn native_endian() -> Endian {
    if cfg!(target_endian = "big") {
        Endian::Big
    } else {
        Endian::Little
    }
}

fn push_u16(out: &mut Vec<u8>, val: u16, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&val.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&val.to_be_bytes()),
    }
}

fn push_u32(out: &mut Vec<u8>, val: u32, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&val.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&val.to_be_bytes()),
    }
}

#[allow(dead_code)]
fn encode_utf16(text: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(2) + if with_bom { 2 } else { 0 });
    if with_bom {
        push_u16(&mut out, 0xFEFF, endian);
    }
    for code in text.encode_utf16() {
        push_u16(&mut out, code, endian);
    }
    out
}

#[allow(dead_code)]
fn encode_utf32(text: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(4) + if with_bom { 4 } else { 0 });
    if with_bom {
        push_u32(&mut out, 0x0000_FEFF, endian);
    }
    for ch in text.chars() {
        push_u32(&mut out, ch as u32, endian);
    }
    out
}

pub(crate) fn is_surrogate(code: u32) -> bool {
    (0xD800..=0xDFFF).contains(&code)
}

pub(crate) fn unicode_escape_codepoint(code: u32) -> String {
    if code <= 0xFF {
        format!("\\x{code:02x}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

pub(crate) fn unicode_name_escape(code: u32) -> String {
    #[cfg(feature = "stdlib_unicode_names")]
    if let Some(ch) = char::from_u32(code)
        && let Some(name) = unicode_names2::name(ch)
    {
        return format!("\\N{{{name}}}");
    }
    unicode_escape_codepoint(code)
}

pub(crate) fn unicode_escape(ch: char) -> String {
    unicode_escape_codepoint(ch as u32)
}

pub(crate) fn encode_error_reason(encoding: &str, code: u32, limit: u32) -> String {
    if encoding == "charmap" {
        return "character maps to <undefined>".to_string();
    }
    if is_surrogate(code) && encoding.starts_with("utf-") {
        return "surrogates not allowed".to_string();
    }
    format!("ordinal not in range({limit})")
}

#[allow(dead_code)]
fn push_backslash_bytes(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in bytes {
        out.push('\\');
        out.push('x');
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn push_hex_escape(out: &mut Vec<u8>, prefix: u8, code: u32, width: usize) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'\\');
    out.push(prefix);
    for shift in (0..width).rev() {
        let nibble = ((code >> (shift * 4)) & 0x0f) as usize;
        out.push(HEX[nibble]);
    }
}

fn xmlcharref_bytes(code: u32, buf: &mut [u8; 16]) -> &[u8] {
    buf[0] = b'&';
    buf[1] = b'#';
    let mut digits = [0u8; 10];
    let mut idx = digits.len();
    let mut value = code;
    loop {
        idx = idx.saturating_sub(1);
        digits[idx] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let digits_len = digits.len() - idx;
    buf[2..2 + digits_len].copy_from_slice(&digits[idx..]);
    buf[2 + digits_len] = b';';
    &buf[..2 + digits_len + 1]
}

pub(crate) fn push_xmlcharref_ascii(out: &mut Vec<u8>, code: u32) {
    let mut buf = [0u8; 16];
    let bytes = xmlcharref_bytes(code, &mut buf);
    out.extend_from_slice(bytes);
}

fn push_xmlcharref_utf16(out: &mut Vec<u8>, code: u32, endian: Endian) {
    let mut buf = [0u8; 16];
    let bytes = xmlcharref_bytes(code, &mut buf);
    for &byte in bytes {
        push_u16(out, byte as u16, endian);
    }
}

fn push_xmlcharref_utf32(out: &mut Vec<u8>, code: u32, endian: Endian) {
    let mut buf = [0u8; 16];
    let bytes = xmlcharref_bytes(code, &mut buf);
    for &byte in bytes {
        push_u32(out, byte as u32, endian);
    }
}

pub(crate) fn encode_string_with_errors(
    bytes: &[u8],
    encoding: &str,
    errors: Option<&str>,
) -> Result<Vec<u8>, EncodeError> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(EncodeError::UnknownEncoding(encoding.to_string()));
    };
    let handler = errors.unwrap_or("strict");
    let mut unknown_handler: Option<String> = None;
    let handler = match handler {
        "surrogatepass" | "strict" | "surrogateescape" | "ignore" | "replace"
        | "backslashreplace" | "namereplace" | "xmlcharrefreplace" => handler,
        other => {
            unknown_handler = Some(other.to_string());
            "strict"
        }
    };
    let error_encoding = kind.encode_error_label();
    let invalid_char_err =
        |encoding: &'static str, code: u32, start: usize, end: usize, limit: u32| -> EncodeError {
            if let Some(name) = unknown_handler.as_ref() {
                EncodeError::UnknownErrorHandler(name.clone())
            } else {
                EncodeError::InvalidChar {
                    encoding,
                    code,
                    start,
                    end,
                    limit,
                }
            }
        };
    let encode_charmap = |kind: EncodingKind| -> Result<Vec<u8>, EncodeError> {
        let Some(table) = single_byte_charmap(kind) else {
            unreachable!("single-byte charmap encoder called for non-charmap encoding");
        };
        let unencodable =
            |code: u32| code > 0x7F && encode_single_byte_charmap_byte(table, code).is_none();
        let mut out = Vec::new();
        // CPython hands the whole unencodable run to the handler, so a run
        // that surrogateescape cannot consume fails from its first position.
        let mut run_end = 0;
        for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
            let code = cp.to_u32();
            if code <= 0x7F {
                out.push(code as u8);
                continue;
            }
            if let Some(byte) = encode_single_byte_charmap_byte(table, code) {
                out.push(byte);
                continue;
            }
            if idx >= run_end {
                run_end = unencodable_run_end(bytes, idx, unencodable);
                let escapable = handler == "surrogateescape"
                    && wtf8_from_bytes(bytes)
                        .code_points()
                        .skip(idx)
                        .take(run_end - idx)
                        .all(|cp| (0xDC80..=0xDCFF).contains(&cp.to_u32()));
                if matches!(handler, "surrogateescape" | "surrogatepass" | "strict") && !escapable {
                    return Err(invalid_char_err(error_encoding, code, idx, run_end, 0));
                }
            }
            match handler {
                "ignore" => {}
                "replace" => out.push(b'?'),
                "backslashreplace" => {
                    out.extend_from_slice(unicode_escape_codepoint(code).as_bytes());
                }
                "namereplace" => {
                    out.extend_from_slice(unicode_name_escape(code).as_bytes());
                }
                "xmlcharrefreplace" => {
                    push_xmlcharref_ascii(&mut out, code);
                }
                "surrogateescape" => out.push((code - 0xDC00) as u8),
                "surrogatepass" | "strict" => {
                    unreachable!("strict charmap failures raise at their run start")
                }
                other => {
                    return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                }
            }
        }
        Ok(out)
    };
    let mut out = Vec::new();
    let encode_utf8 =
        |handler: &str, bytes: &[u8], out: &mut Vec<u8>| -> Result<Vec<u8>, EncodeError> {
            match handler {
                "surrogatepass" => Ok(bytes.to_vec()),
                "strict" => {
                    if !wtf8_has_surrogates(bytes) {
                        return Ok(bytes.to_vec());
                    }
                    for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                        let code = cp.to_u32();
                        if is_surrogate(code) {
                            let end = unencodable_run_end(bytes, idx, is_surrogate);
                            return Err(invalid_char_err(error_encoding, code, idx, end, 0x110000));
                        }
                    }
                    Ok(bytes.to_vec())
                }
                "surrogateescape" => {
                    for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                        let code = cp.to_u32();
                        if (0xDC80..=0xDCFF).contains(&code) {
                            out.push((code - 0xDC00) as u8);
                        } else if is_surrogate(code) {
                            let end = unencodable_run_end(bytes, idx, is_surrogate);
                            return Err(invalid_char_err(error_encoding, code, idx, end, 0x110000));
                        } else {
                            push_wtf8_codepoint(out, code);
                        }
                    }
                    Ok(std::mem::take(out))
                }
                "ignore" | "replace" | "backslashreplace" | "namereplace" | "xmlcharrefreplace" => {
                    for cp in wtf8_from_bytes(bytes).code_points() {
                        let code = cp.to_u32();
                        if is_surrogate(code) {
                            match handler {
                                "ignore" => {}
                                "replace" => out.push(b'?'),
                                "backslashreplace" => {
                                    out.extend_from_slice(unicode_escape_codepoint(code).as_bytes())
                                }
                                "namereplace" => {
                                    out.extend_from_slice(unicode_name_escape(code).as_bytes())
                                }
                                "xmlcharrefreplace" => {
                                    push_xmlcharref_ascii(out, code);
                                }
                                _ => {}
                            }
                            continue;
                        }
                        push_wtf8_codepoint(out, code);
                    }
                    Ok(std::mem::take(out))
                }
                other => Err(EncodeError::UnknownErrorHandler(other.to_string())),
            }
        };
    match kind.runtime_class() {
        CodecRuntimeClass::Utf8 => encode_utf8(handler, bytes, &mut out),
        CodecRuntimeClass::Utf8Sig => {
            let encoded = encode_utf8(handler, bytes, &mut out)?;
            let mut with_bom = Vec::with_capacity(encoded.len() + 3);
            with_bom.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            with_bom.extend_from_slice(&encoded);
            Ok(with_bom)
        }
        CodecRuntimeClass::Charmap => encode_charmap(kind),
        CodecRuntimeClass::Latin1 | CodecRuntimeClass::Ascii => {
            let limit = kind.ordinal_limit();
            for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                let code = cp.to_u32();
                if code < limit {
                    out.push(code as u8);
                    continue;
                }
                match handler {
                    "ignore" => {}
                    "replace" => out.push(b'?'),
                    "backslashreplace" => {
                        out.extend_from_slice(unicode_escape_codepoint(code).as_bytes());
                    }
                    "namereplace" => {
                        out.extend_from_slice(unicode_name_escape(code).as_bytes());
                    }
                    "xmlcharrefreplace" => {
                        push_xmlcharref_ascii(&mut out, code);
                    }
                    "surrogateescape" => {
                        if (0xDC80..=0xDCFF).contains(&code) {
                            out.push((code - 0xDC00) as u8);
                        } else {
                            let end = unencodable_run_end(bytes, idx, |code| code >= limit);
                            return Err(invalid_char_err(error_encoding, code, idx, end, limit));
                        }
                    }
                    "surrogatepass" | "strict" => {
                        let end = unencodable_run_end(bytes, idx, |code| code >= limit);
                        return Err(invalid_char_err(error_encoding, code, idx, end, limit));
                    }
                    other => {
                        return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                    }
                }
            }
            Ok(out)
        }
        CodecRuntimeClass::UnicodeEscape => {
            for cp in wtf8_from_bytes(bytes).code_points() {
                let code = cp.to_u32();
                match code {
                    0x5C => out.extend_from_slice(b"\\\\"),
                    0x09 => out.extend_from_slice(b"\\t"),
                    0x0A => out.extend_from_slice(b"\\n"),
                    0x0D => out.extend_from_slice(b"\\r"),
                    0x20..=0x7E => out.push(code as u8),
                    _ if code <= 0xFF => push_hex_escape(&mut out, b'x', code, 2),
                    _ if code <= 0xFFFF => push_hex_escape(&mut out, b'u', code, 4),
                    _ => push_hex_escape(&mut out, b'U', code, 8),
                }
            }
            Ok(out)
        }
        CodecRuntimeClass::Utf16 | CodecRuntimeClass::Utf16LE | CodecRuntimeClass::Utf16BE => {
            let (endian, with_bom) = match kind {
                EncodingKind::Utf16 => (native_endian(), true),
                EncodingKind::Utf16LE => (Endian::Little, false),
                EncodingKind::Utf16BE => (Endian::Big, false),
                _ => (native_endian(), false),
            };
            if with_bom {
                push_u16(&mut out, 0xFEFF, endian);
            }
            for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                let code = cp.to_u32();
                if is_surrogate(code) {
                    match handler {
                        "surrogatepass" | "surrogateescape" => {
                            push_u16(&mut out, code as u16, endian);
                            continue;
                        }
                        "ignore" => continue,
                        "replace" => {
                            push_u16(&mut out, 0xFFFD, endian);
                            continue;
                        }
                        "backslashreplace" => {
                            for ch in unicode_escape_codepoint(code).chars() {
                                push_u16(&mut out, ch as u16, endian);
                            }
                            continue;
                        }
                        "namereplace" => {
                            for ch in unicode_name_escape(code).chars() {
                                push_u16(&mut out, ch as u16, endian);
                            }
                            continue;
                        }
                        "xmlcharrefreplace" => {
                            push_xmlcharref_utf16(&mut out, code, endian);
                            continue;
                        }
                        "strict" => {
                            return Err(invalid_char_err(
                                error_encoding,
                                code,
                                idx,
                                idx + 1,
                                0x110000,
                            ));
                        }
                        other => {
                            return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                        }
                    }
                }
                if code <= 0xFFFF {
                    push_u16(&mut out, code as u16, endian);
                } else {
                    let val = code - 0x10000;
                    let high = 0xD800 | ((val >> 10) as u16);
                    let low = 0xDC00 | ((val & 0x3FF) as u16);
                    push_u16(&mut out, high, endian);
                    push_u16(&mut out, low, endian);
                }
            }
            Ok(out)
        }
        CodecRuntimeClass::Utf32 | CodecRuntimeClass::Utf32LE | CodecRuntimeClass::Utf32BE => {
            let (endian, with_bom) = match kind {
                EncodingKind::Utf32 => (native_endian(), true),
                EncodingKind::Utf32LE => (Endian::Little, false),
                EncodingKind::Utf32BE => (Endian::Big, false),
                _ => (native_endian(), false),
            };
            if with_bom {
                push_u32(&mut out, 0x0000_FEFF, endian);
            }
            for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                let code = cp.to_u32();
                if is_surrogate(code) {
                    match handler {
                        "surrogatepass" | "surrogateescape" => {
                            push_u32(&mut out, code, endian);
                            continue;
                        }
                        "ignore" => continue,
                        "replace" => {
                            push_u32(&mut out, 0xFFFD, endian);
                            continue;
                        }
                        "backslashreplace" => {
                            for ch in unicode_escape_codepoint(code).chars() {
                                push_u32(&mut out, ch as u32, endian);
                            }
                            continue;
                        }
                        "namereplace" => {
                            for ch in unicode_name_escape(code).chars() {
                                push_u32(&mut out, ch as u32, endian);
                            }
                            continue;
                        }
                        "xmlcharrefreplace" => {
                            push_xmlcharref_utf32(&mut out, code, endian);
                            continue;
                        }
                        "strict" => {
                            return Err(invalid_char_err(
                                kind.name(),
                                code,
                                idx,
                                idx + 1,
                                0x110000,
                            ));
                        }
                        other => {
                            return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                        }
                    }
                }
                push_u32(&mut out, code, endian);
            }
            Ok(out)
        }
    }
}

pub(crate) fn decode_error_byte(label: &str, byte: u8, pos: usize, message: &str) -> String {
    format!("'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}")
}

pub(crate) fn decode_error_range(label: &str, start: usize, end: usize, message: &str) -> String {
    format!("'{label}' codec can't decode bytes in position {start}-{end}: {message}")
}

fn read_u16(bytes: &[u8], idx: usize, endian: Endian) -> u16 {
    match endian {
        Endian::Little => u16::from_le_bytes([bytes[idx], bytes[idx + 1]]),
        Endian::Big => u16::from_be_bytes([bytes[idx], bytes[idx + 1]]),
    }
}

fn read_u32(bytes: &[u8], idx: usize, endian: Endian) -> u32 {
    match endian {
        Endian::Little => {
            u32::from_le_bytes([bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]])
        }
        Endian::Big => {
            u32::from_be_bytes([bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]])
        }
    }
}

fn decode_ascii_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::with_capacity(bytes.len());
    for (idx, &byte) in bytes.iter().enumerate() {
        if byte <= 0x7f {
            out.push(byte);
            continue;
        }
        match errors {
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => push_backslash_bytes_vec(&mut out, &[byte]),
            "surrogateescape" => push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32),
            "strict" | "surrogatepass" => {
                return Err(DecodeFailure::Byte {
                    pos: idx,
                    byte,
                    message: "ordinal not in range(128)",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn decode_utf8_bytes_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    let allow_surrogates = errors == "surrogatepass";
    while idx < bytes.len() {
        let first = bytes[idx];
        if first < 0x80 {
            out.push(first);
            idx += 1;
            continue;
        }
        if first < 0xC0 {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        let (needed, min_code) = if first < 0xE0 {
            (1usize, 0x80u32)
        } else if first < 0xF0 {
            (2usize, 0x800u32)
        } else if first < 0xF8 {
            (3usize, 0x10000u32)
        } else {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        };
        if idx + needed >= bytes.len() {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        let mut code: u32 = (first & (0x7F >> needed)) as u32;
        let mut ok = true;
        for off in 1..=needed {
            let byte = bytes[idx + off];
            if (byte & 0xC0) != 0x80 {
                ok = false;
                break;
            }
            code = (code << 6) | (byte & 0x3F) as u32;
        }
        if !ok || code < min_code || code > 0x10FFFF {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        if is_surrogate(code) && !allow_surrogates {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        push_wtf8_codepoint(&mut out, code);
        idx += needed + 1;
    }
    Ok(out)
}

fn decode_utf8_invalid_byte(
    errors: &str,
    out: &mut Vec<u8>,
    pos: usize,
    byte: u8,
) -> Result<(), DecodeFailure> {
    match errors {
        "ignore" => Ok(()),
        "replace" => {
            push_wtf8_codepoint(out, 0xFFFD);
            Ok(())
        }
        "backslashreplace" => {
            push_backslash_bytes_vec(out, &[byte]);
            Ok(())
        }
        "surrogateescape" => {
            push_wtf8_codepoint(out, 0xDC00 + byte as u32);
            Ok(())
        }
        "strict" | "surrogatepass" => Err(DecodeFailure::Byte {
            pos,
            byte,
            message: "invalid start byte",
        }),
        other => Err(DecodeFailure::UnknownErrorHandler(other.to_string())),
    }
}

fn hex_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

fn parse_hex_prefix(bytes: &[u8], max: usize) -> (u32, usize) {
    let mut value = 0u32;
    let mut count = 0usize;
    for &byte in bytes.iter().take(max) {
        let Some(digit) = hex_value(byte) else {
            break;
        };
        value = (value << 4) | digit;
        count += 1;
    }
    (value, count)
}

fn parse_octal_prefix(bytes: &[u8], max: usize) -> (u32, usize) {
    let mut value = 0u32;
    let mut count = 0usize;
    for &byte in bytes.iter().take(max) {
        if !(b'0'..=b'7').contains(&byte) {
            break;
        }
        value = (value << 3) | (byte - b'0') as u32;
        count += 1;
    }
    (value, count)
}

fn handle_unicode_escape_failure(
    errors: &str,
    out: &mut Vec<u8>,
    bytes: &[u8],
    start: usize,
    end: usize,
    failure: DecodeFailure,
) -> Result<usize, DecodeFailure> {
    match errors {
        "ignore" => Ok(end + 1),
        "replace" => {
            push_wtf8_codepoint(out, 0xFFFD);
            Ok(end + 1)
        }
        "backslashreplace" => {
            if start <= end && end < bytes.len() {
                push_backslash_bytes_vec(out, &bytes[start..=end]);
            }
            Ok(end + 1)
        }
        "strict" | "surrogatepass" | "surrogateescape" => Err(failure),
        other => Err(DecodeFailure::UnknownErrorHandler(other.to_string())),
    }
}

fn decode_unicode_escape_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    const TRUNC_X: &str = "truncated \\xXX escape";
    const TRUNC_U: &str = "truncated \\uXXXX escape";
    const TRUNC_U8: &str = "truncated \\UXXXXXXXX escape";
    const MALFORMED_N: &str = "malformed \\N character escape";
    const UNKNOWN_NAME: &str = "unknown Unicode character name";
    const ILLEGAL_UNICODE: &str = "illegal Unicode character";
    const TRAILING_SLASH: &str = "\\ at end of string";

    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if byte != b'\\' {
            push_wtf8_codepoint(&mut out, byte as u32);
            idx += 1;
            continue;
        }
        if idx + 1 >= bytes.len() {
            let failure = DecodeFailure::Byte {
                pos: idx,
                byte,
                message: TRAILING_SLASH,
            };
            idx = handle_unicode_escape_failure(errors, &mut out, bytes, idx, idx, failure)?;
            continue;
        }
        let esc = bytes[idx + 1];
        match esc {
            b'\\' => {
                push_wtf8_codepoint(&mut out, b'\\' as u32);
                idx += 2;
            }
            b'\'' => {
                push_wtf8_codepoint(&mut out, b'\'' as u32);
                idx += 2;
            }
            b'"' => {
                push_wtf8_codepoint(&mut out, b'"' as u32);
                idx += 2;
            }
            b'a' => {
                push_wtf8_codepoint(&mut out, 0x07);
                idx += 2;
            }
            b'b' => {
                push_wtf8_codepoint(&mut out, 0x08);
                idx += 2;
            }
            b't' => {
                push_wtf8_codepoint(&mut out, 0x09);
                idx += 2;
            }
            b'n' => {
                push_wtf8_codepoint(&mut out, 0x0A);
                idx += 2;
            }
            b'v' => {
                push_wtf8_codepoint(&mut out, 0x0B);
                idx += 2;
            }
            b'f' => {
                push_wtf8_codepoint(&mut out, 0x0C);
                idx += 2;
            }
            b'r' => {
                push_wtf8_codepoint(&mut out, 0x0D);
                idx += 2;
            }
            b'\n' => {
                idx += 2;
            }
            b'x' => {
                let (value, count) = parse_hex_prefix(&bytes[idx + 2..], 2);
                if count < 2 {
                    let end = idx + 1 + count;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: TRUNC_X,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                push_wtf8_codepoint(&mut out, value);
                idx += 4;
            }
            b'u' => {
                let (value, count) = parse_hex_prefix(&bytes[idx + 2..], 4);
                if count < 4 {
                    let end = idx + 1 + count;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: TRUNC_U,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                push_wtf8_codepoint(&mut out, value);
                idx += 6;
            }
            b'U' => {
                let (value, count) = parse_hex_prefix(&bytes[idx + 2..], 8);
                if count < 8 {
                    let end = idx + 1 + count;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: TRUNC_U8,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                if value > 0x10FFFF {
                    let end = idx + 9;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: ILLEGAL_UNICODE,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                push_wtf8_codepoint(&mut out, value);
                idx += 10;
            }
            b'N' => {
                if idx + 2 >= bytes.len() || bytes[idx + 2] != b'{' {
                    let end = usize::min(idx + 1, bytes.len() - 1);
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: MALFORMED_N,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                let close = bytes[idx + 3..]
                    .iter()
                    .position(|&ch| ch == b'}')
                    .map(|offset| idx + 3 + offset);
                let Some(close_idx) = close else {
                    let end = bytes.len() - 1;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: MALFORMED_N,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                };
                #[cfg(feature = "stdlib_unicode_names")]
                let name_bytes = &bytes[idx + 3..close_idx];
                #[cfg(feature = "stdlib_unicode_names")]
                let name = std::str::from_utf8(name_bytes).unwrap_or("");
                #[cfg(feature = "stdlib_unicode_names")]
                let resolved = unicode_names2::character(name);
                #[cfg(not(feature = "stdlib_unicode_names"))]
                let resolved: Option<char> = None;
                if let Some(ch) = resolved {
                    push_wtf8_codepoint(&mut out, ch as u32);
                    idx = close_idx + 1;
                } else {
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end: close_idx,
                        message: UNKNOWN_NAME,
                    };
                    idx = handle_unicode_escape_failure(
                        errors, &mut out, bytes, idx, close_idx, failure,
                    )?;
                }
            }
            b'0'..=b'7' => {
                let (value, count) = parse_octal_prefix(&bytes[idx + 1..], 3);
                push_wtf8_codepoint(&mut out, value);
                idx += 1 + count;
            }
            _ => {
                push_wtf8_codepoint(&mut out, b'\\' as u32);
                push_wtf8_codepoint(&mut out, esc as u32);
                idx += 2;
            }
        }
    }
    Ok(out)
}

fn utf16_decode_config(bytes: &[u8], kind: EncodingKind) -> (Endian, String, usize) {
    match kind {
        EncodingKind::Utf16 => {
            if bytes.len() >= 2 {
                if bytes[0] == 0xFF && bytes[1] == 0xFE {
                    return (Endian::Little, "utf-16-le".to_string(), 2);
                }
                if bytes[0] == 0xFE && bytes[1] == 0xFF {
                    return (Endian::Big, "utf-16-be".to_string(), 2);
                }
            }
            let endian = native_endian();
            let label = match endian {
                Endian::Little => "utf-16-le".to_string(),
                Endian::Big => "utf-16-be".to_string(),
            };
            (endian, label, 0)
        }
        EncodingKind::Utf16LE => (Endian::Little, "utf-16-le".to_string(), 0),
        EncodingKind::Utf16BE => (Endian::Big, "utf-16-be".to_string(), 0),
        _ => (native_endian(), "utf-16-le".to_string(), 0),
    }
}

fn decode_utf16_with_errors(
    bytes: &[u8],
    errors: &str,
    endian: Endian,
    offset: usize,
) -> Result<Vec<u8>, DecodeFailure> {
    let data = if offset > 0 { &bytes[offset..] } else { bytes };
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx + 1 < data.len() {
        let unit = read_u16(data, idx, endian);
        if (0xD800..=0xDBFF).contains(&unit) {
            if idx + 3 >= data.len() {
                match errors {
                    "surrogatepass" | "surrogateescape" => {
                        push_wtf8_codepoint(&mut out, unit as u32);
                    }
                    "ignore" => {}
                    "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                    "backslashreplace" => {
                        push_backslash_bytes_vec(&mut out, &data[idx..]);
                    }
                    "strict" => {
                        return Err(DecodeFailure::Range {
                            start: offset + idx,
                            end: offset + data.len() - 1,
                            message: "unexpected end of data",
                        });
                    }
                    other => {
                        return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                    }
                }
                // Avoid double-applying trailing bytes in the post-loop remainder handler.
                idx = data.len();
                break;
            }
            let next = read_u16(data, idx + 2, endian);
            if (0xDC00..=0xDFFF).contains(&next) {
                let high = (unit as u32) - 0xD800;
                let low = (next as u32) - 0xDC00;
                let code = 0x10000 + ((high << 10) | low);
                push_wtf8_codepoint(&mut out, code);
                idx += 4;
                continue;
            }
            match errors {
                "surrogatepass" | "surrogateescape" => {
                    push_wtf8_codepoint(&mut out, unit as u32);
                }
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 2]);
                }
                "strict" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 1,
                        message: "illegal UTF-16 surrogate",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 2;
            continue;
        }
        if (0xDC00..=0xDFFF).contains(&unit) {
            match errors {
                "surrogatepass" | "surrogateescape" => {
                    push_wtf8_codepoint(&mut out, unit as u32);
                }
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 2]);
                }
                "strict" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 1,
                        message: "illegal encoding",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 2;
            continue;
        }
        push_wtf8_codepoint(&mut out, unit as u32);
        idx += 2;
    }
    if idx < data.len() {
        match errors {
            "surrogatepass" | "surrogateescape" => {
                push_wtf8_codepoint(&mut out, data[idx] as u32);
            }
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => {
                push_backslash_bytes_vec(&mut out, &data[idx..]);
            }
            "strict" => {
                let pos = offset + data.len() - 1;
                let byte = data[data.len() - 1];
                return Err(DecodeFailure::Byte {
                    pos,
                    byte,
                    message: "truncated data",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn utf32_decode_config(bytes: &[u8], kind: EncodingKind) -> (Endian, String, usize) {
    match kind {
        EncodingKind::Utf32 => {
            if bytes.len() >= 4 {
                if bytes[0] == 0xFF && bytes[1] == 0xFE && bytes[2] == 0x00 && bytes[3] == 0x00 {
                    return (Endian::Little, "utf-32-le".to_string(), 4);
                }
                if bytes[0] == 0x00 && bytes[1] == 0x00 && bytes[2] == 0xFE && bytes[3] == 0xFF {
                    return (Endian::Big, "utf-32-be".to_string(), 4);
                }
            }
            let endian = native_endian();
            let label = match endian {
                Endian::Little => "utf-32-le".to_string(),
                Endian::Big => "utf-32-be".to_string(),
            };
            (endian, label, 0)
        }
        EncodingKind::Utf32LE => (Endian::Little, "utf-32-le".to_string(), 0),
        EncodingKind::Utf32BE => (Endian::Big, "utf-32-be".to_string(), 0),
        _ => (native_endian(), "utf-32-le".to_string(), 0),
    }
}

fn decode_utf32_with_errors(
    bytes: &[u8],
    errors: &str,
    endian: Endian,
    offset: usize,
) -> Result<Vec<u8>, DecodeFailure> {
    let data = if offset > 0 { &bytes[offset..] } else { bytes };
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx + 3 < data.len() {
        let code = read_u32(data, idx, endian);
        if is_surrogate(code) {
            match errors {
                "surrogatepass" | "surrogateescape" => {
                    push_wtf8_codepoint(&mut out, code);
                }
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 4]);
                }
                "strict" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 3,
                        message: "code point in surrogate code point range(0xd800, 0xe000)",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 4;
            continue;
        }
        if code > 0x10FFFF {
            match errors {
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 4]);
                }
                "strict" | "surrogatepass" | "surrogateescape" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 3,
                        message: "code point not in range(0x110000)",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 4;
            continue;
        }
        push_wtf8_codepoint(&mut out, code);
        idx += 4;
    }
    if idx < data.len() {
        match errors {
            "surrogatepass" | "surrogateescape" => {
                for &byte in &data[idx..] {
                    push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32);
                }
            }
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => {
                push_backslash_bytes_vec(&mut out, &data[idx..]);
            }
            "strict" => {
                return Err(DecodeFailure::Range {
                    start: offset + idx,
                    end: offset + data.len() - 1,
                    message: "truncated data",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn decode_bytes_with_errors(
    bytes: &[u8],
    kind: EncodingKind,
    errors: &str,
) -> Result<(Vec<u8>, String), (DecodeFailure, String)> {
    match kind.runtime_class() {
        CodecRuntimeClass::Utf8 => match decode_utf8_bytes_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "utf-8".to_string())),
            Err(err) => Err((err, "utf-8".to_string())),
        },
        CodecRuntimeClass::Utf8Sig => {
            let data =
                if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
                    &bytes[3..]
                } else {
                    bytes
                };
            match decode_utf8_bytes_with_errors(data, errors) {
                Ok(text) => Ok((text, "utf-8".to_string())),
                Err(err) => Err((err, "utf-8".to_string())),
            }
        }
        CodecRuntimeClass::Charmap => {
            match decode_single_byte_charmap_with_errors(bytes, kind, errors) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        CodecRuntimeClass::Ascii => match decode_ascii_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "ascii".to_string())),
            Err(err) => Err((err, "ascii".to_string())),
        },
        CodecRuntimeClass::Latin1 => {
            let mut out = Vec::with_capacity(bytes.len());
            for &byte in bytes {
                push_wtf8_codepoint(&mut out, byte as u32);
            }
            Ok((out, "latin-1".to_string()))
        }
        CodecRuntimeClass::UnicodeEscape => {
            match decode_unicode_escape_with_errors(bytes, errors) {
                Ok(text) => Ok((text, "unicodeescape".to_string())),
                Err(err) => Err((err, "unicodeescape".to_string())),
            }
        }
        CodecRuntimeClass::Utf16 | CodecRuntimeClass::Utf16LE | CodecRuntimeClass::Utf16BE => {
            let (endian, label, offset) = utf16_decode_config(bytes, kind);
            match decode_utf16_with_errors(bytes, errors, endian, offset) {
                Ok(text) => Ok((text, label)),
                Err(err) => Err((err, label)),
            }
        }
        CodecRuntimeClass::Utf32 | CodecRuntimeClass::Utf32LE | CodecRuntimeClass::Utf32BE => {
            let (endian, label, offset) = utf32_decode_config(bytes, kind);
            match decode_utf32_with_errors(bytes, errors, endian, offset) {
                Ok(text) => Ok((text, label)),
                Err(err) => Err((err, label)),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum DecodeTextError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    Failure(DecodeFailure, String),
}

pub(crate) fn decode_bytes_text(
    encoding: &str,
    errors: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, String), DecodeTextError> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(DecodeTextError::UnknownEncoding(encoding.to_string()));
    };
    let errors_known = matches!(
        errors,
        "strict" | "ignore" | "replace" | "backslashreplace" | "surrogateescape" | "surrogatepass"
    );
    let result = if errors_known {
        decode_bytes_with_errors(bytes, kind, errors)
    } else {
        match decode_bytes_with_errors(bytes, kind, "strict") {
            Ok((text, label)) => return Ok((text, label)),
            Err((_failure, _label)) => {
                return Err(DecodeTextError::UnknownErrorHandler(errors.to_string()));
            }
        }
    };
    match result {
        Ok((text, label)) => Ok((text, label)),
        Err((failure, label)) => Err(DecodeTextError::Failure(failure, label)),
    }
}

#[cfg(test)]
mod encode_error_range_tests {
    use super::*;

    fn text(code_points: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &code in code_points {
            push_wtf8_codepoint(&mut bytes, code);
        }
        bytes
    }

    fn failure(code_points: &[u32], encoding: &str, errors: &str) -> (&'static str, usize, usize) {
        match encode_string_with_errors(&text(code_points), encoding, Some(errors)) {
            Err(EncodeError::InvalidChar {
                encoding,
                start,
                end,
                ..
            }) => (encoding, start, end),
            _ => panic!("{encoding}/{errors}: expected an encode failure"),
        }
    }

    /// Ranges measured on CPython 3.12: one strict failure spans the whole run
    /// of unencodable code points; UTF-16/32 report one surrogate.
    #[test]
    fn encode_failures_report_cpython_unencodable_runs() {
        let a = u32::from('a');
        let z = u32::from('z');
        let b = u32::from('b');
        let e_acute = 0xE9;
        let euro = 0x20AC;
        for (input, encoding, errors, expected) in [
            (
                vec![a, e_acute, euro, z],
                "ascii",
                "strict",
                ("ascii", 1, 3),
            ),
            (
                vec![a, e_acute, euro, z],
                "latin-1",
                "strict",
                ("latin-1", 2, 3),
            ),
            (
                vec![a, euro, euro, e_acute, b],
                "latin-1",
                "strict",
                ("latin-1", 1, 3),
            ),
            (
                vec![a, 0x100, 0x101, b],
                "ascii",
                "surrogatepass",
                ("ascii", 1, 3),
            ),
            (
                vec![a, 0xDC80, 0xD800, b],
                "ascii",
                "surrogateescape",
                ("ascii", 2, 3),
            ),
            (
                vec![a, 0x100, 0x101, b],
                "ascii",
                "surrogateescape",
                ("ascii", 1, 3),
            ),
            (
                vec![a, e_acute, euro, 0x100, z],
                "cp1252",
                "strict",
                ("charmap", 3, 4),
            ),
            (
                vec![a, euro, 0x101, 0x102, z],
                "cp437",
                "strict",
                ("charmap", 1, 4),
            ),
            (
                vec![a, 0xDC80, 0x100, b],
                "cp437",
                "surrogateescape",
                ("charmap", 1, 3),
            ),
            (
                vec![a, 0xD800, 0xDFFF, b],
                "utf-8",
                "strict",
                ("utf-8", 1, 3),
            ),
            (
                vec![a, 0xDC80, 0xD800, b],
                "utf-8",
                "surrogateescape",
                ("utf-8", 2, 3),
            ),
            (
                vec![a, 0xD800, 0xDFFF, b],
                "utf-16-le",
                "strict",
                ("utf-16-le", 1, 2),
            ),
            (
                vec![a, 0xD800, 0xDFFF, b],
                "utf-32",
                "strict",
                ("utf-32", 1, 2),
            ),
        ] {
            assert_eq!(
                failure(&input, encoding, errors),
                expected,
                "{encoding}/{errors}"
            );
        }
    }

    #[test]
    fn surrogateescape_consumes_an_escapable_charmap_run() {
        let encoded = encode_string_with_errors(
            &text(&[0x61, 0xDC80, 0xDCFF, 0x62]),
            "cp437",
            Some("surrogateescape"),
        )
        .unwrap_or_else(|_| panic!("an escapable run must encode"));
        assert_eq!(encoded, [0x61, 0x80, 0xFF, 0x62]);
    }
}

#[cfg(test)]
mod python_encode_error_tests {
    use crate::builtins::exceptions::{ExceptionValue, molt_exception_last_pending};
    use crate::{MoltObject, PyToken, alloc_string, dec_ref_bits, exception_pending};

    fn string(py: &PyToken<'_>, text: &str) -> u64 {
        let ptr = alloc_string(py, text.as_bytes());
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    /// Take the pending UnicodeEncodeError as CPython's `(args, str(exc))`.
    fn take_encode_error(py: &PyToken<'_>) -> ((String, i64, i64, String), String) {
        assert!(exception_pending(py));
        let exception = ExceptionValue::adopt(py, molt_exception_last_pending());
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            py,
            exception.bits(),
            "UnicodeEncodeError"
        ));
        crate::clear_exception(py);
        let attribute = |name: &[u8]| {
            let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
            let value = crate::molt_get_attr_name(exception.bits(), name);
            dec_ref_bits(py, name);
            ExceptionValue::adopt(py, value)
        };
        let text = |value: &ExceptionValue<'_, '_>| {
            crate::string_obj_to_owned(crate::obj_from_bits(value.bits())).unwrap()
        };
        let int =
            |value: &ExceptionValue<'_, '_>| crate::obj_from_bits(value.bits()).as_int().unwrap();
        let fields = (
            text(&attribute(b"encoding")),
            int(&attribute(b"start")),
            int(&attribute(b"end")),
            text(&attribute(b"reason")),
        );
        let message = crate::format_exception_message(
            py,
            crate::obj_from_bits(exception.bits()).as_ptr().unwrap(),
        );
        assert!(!exception_pending(py));
        (fields, message)
    }

    /// `str.encode` and `codecs.charmap_encode` report CPython 3.12's ranges.
    #[test]
    fn python_level_encode_errors_report_the_unencodable_run() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let none = MoltObject::none().bits();
            let text = string(py, "a\u{e9}\u{20ac}z");
            let ascii = string(py, "ascii");
            let strict = string(py, "strict");
            assert!(
                MoltObject::from_bits(crate::molt_string_encode(text, ascii, strict)).is_none()
            );
            assert_eq!(
                take_encode_error(py),
                (
                    ("ascii".into(), 1, 3, "ordinal not in range(128)".into()),
                    "'ascii' codec can't encode characters in position 1-2: ordinal not in range(128)"
                        .into()
                )
            );

            let mapped = string(py, "a\u{100}\u{101}z");
            let mapping = crate::alloc_dict_with_pairs(
                py,
                &[
                    MoltObject::from_int(i64::from(b'a')).bits(),
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(i64::from(b'z')).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            assert!(!mapping.is_null());
            let mapping = MoltObject::from_ptr(mapping).bits();
            for (mapping, expected) in [
                (mapping, ("charmap", "character maps to <undefined>")),
                (none, ("latin-1", "ordinal not in range(256)")),
            ] {
                let result = crate::builtins::codecs_ext::molt_codecs_charmap_encode(
                    mapped, strict, mapping,
                );
                assert!(MoltObject::from_bits(result).is_none());
                let ((encoding, start, end, reason), _) = take_encode_error(py);
                assert_eq!(
                    (encoding.as_str(), start, end, reason.as_str()),
                    (expected.0, 1, 3, expected.1)
                );
            }
            for bits in [text, ascii, mapped, strict, mapping] {
                dec_ref_bits(py, bits);
            }
        });
    }
}
