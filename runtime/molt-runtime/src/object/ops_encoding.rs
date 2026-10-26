//! String encoding and decoding — extracted from ops.rs for maintainability.

use molt_obj_model::text_storage::push_backslash_bytes;
use molt_stdlib_text::charmap_codecs_generated::{
    decode_single_byte_charmap_with_errors, encode_single_byte_charmap_byte, single_byte_charmap,
};
pub(crate) use molt_stdlib_text::codec_errors::DecodeFailure;
pub(crate) use molt_stdlib_text::codec_registry::{
    CodecRuntimeClass, EncodingKind, normalize_encoding,
};

pub(crate) fn encoding_kind_name(kind: EncodingKind) -> &'static str {
    kind.name()
}

#[derive(Debug)]
pub(crate) enum EncodeError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    InvalidSequence {
        encoding: &'static str,
        start: usize,
        end: usize,
        reason: &'static str,
    },
}

impl EncodeError {
    /// Every text-encoding consumer publishes the scanner's original span.
    pub(crate) fn raise<T: crate::builtins::exceptions::ExceptionSentinel>(
        self,
        py: &crate::PyToken<'_>,
        object_bits: u64,
    ) -> T {
        match self {
            Self::UnknownEncoding(name) => {
                crate::raise_exception(py, "LookupError", &format!("unknown encoding: {name}"))
            }
            Self::UnknownErrorHandler(name) => crate::raise_exception(
                py,
                "LookupError",
                &format!("unknown error handler name '{name}'"),
            ),
            Self::InvalidSequence {
                encoding,
                start,
                end,
                reason,
            } => crate::raise_unicode_encode_error(py, encoding, object_bits, start, end, reason),
        }
    }
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
    // Error-handler lookup remains lazy: valid text accepts an unknown name.
    let unknown_handler = match handler {
        "surrogatepass" | "strict" | "surrogateescape" | "ignore" | "replace"
        | "backslashreplace" | "namereplace" | "xmlcharrefreplace" => None,
        other => Some(other),
    };
    let handler = if unknown_handler.is_some() {
        "strict"
    } else {
        handler
    };
    let error_encoding = kind.encode_error_label();
    let invalid_sequence = |start, end| {
        if let Some(name) = unknown_handler {
            EncodeError::UnknownErrorHandler(name.to_owned())
        } else {
            let reason = match kind.runtime_class() {
                CodecRuntimeClass::Ascii => "ordinal not in range(128)",
                CodecRuntimeClass::Latin1 => "ordinal not in range(256)",
                CodecRuntimeClass::Charmap => "character maps to <undefined>",
                _ => "surrogates not allowed",
            };
            EncodeError::InvalidSequence {
                encoding: error_encoding,
                start,
                end,
                reason,
            }
        }
    };
    let encode_charmap = |kind: EncodingKind| -> Result<Vec<u8>, EncodeError> {
        let Some(table) = single_byte_charmap(kind) else {
            unreachable!("single-byte charmap encoder called for non-charmap encoding");
        };
        let mut out = Vec::new();
        let mut points = molt_obj_model::text_storage::code_points(bytes)
            .enumerate()
            .peekable();
        while let Some((idx, code)) = points.next() {
            if code <= 0x7F {
                out.push(code as u8);
                continue;
            }
            if let Some(byte) = encode_single_byte_charmap_byte(table, code) {
                out.push(byte);
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
                "surrogateescape" | "surrogatepass" | "strict" => {
                    let mut end = idx + 1;
                    let mut escapable = (0xDC80..=0xDCFF).contains(&code);
                    if handler == "surrogateescape" && escapable {
                        out.push((code - 0xDC00) as u8);
                    }
                    while let Some((_, next)) = points.peek() {
                        let next = *next;
                        if encode_single_byte_charmap_byte(table, next).is_some() {
                            break;
                        }
                        points.next();
                        end += 1;
                        escapable &= (0xDC80..=0xDCFF).contains(&next);
                        if handler == "surrogateescape" && escapable {
                            out.push((next - 0xDC00) as u8);
                        }
                    }
                    // Unlike the ordinal and UTF-8 encoders, CPython's charmap
                    // callback sees the original whole span if escape fails.
                    if handler != "surrogateescape" || !escapable {
                        return Err(invalid_sequence(idx, end));
                    }
                }
                other => {
                    return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                }
            }
        }
        Ok(out)
    };
    let mut out = Vec::new();
    let encode_utf8 = |handler: &str,
                       bytes: &[u8],
                       out: &mut Vec<u8>|
     -> Result<Vec<u8>, EncodeError> {
        match handler {
            "surrogatepass" => Ok(bytes.to_vec()),
            "strict" => {
                if std::str::from_utf8(bytes).is_ok() {
                    return Ok(bytes.to_vec());
                }
                let mut points = molt_obj_model::text_storage::code_points(bytes).enumerate();
                while let Some((idx, code)) = points.next() {
                    if is_surrogate(code) {
                        let end = idx + 1 + points.take_while(|(_, cp)| is_surrogate(*cp)).count();
                        return Err(invalid_sequence(idx, end));
                    }
                }
                Ok(bytes.to_vec())
            }
            "surrogateescape" => {
                let mut points = molt_obj_model::text_storage::code_points(bytes).enumerate();
                while let Some((idx, code)) = points.next() {
                    if (0xDC80..=0xDCFF).contains(&code) {
                        out.push((code - 0xDC00) as u8);
                    } else if is_surrogate(code) {
                        let end = idx + 1 + points.take_while(|(_, cp)| is_surrogate(*cp)).count();
                        return Err(invalid_sequence(idx, end));
                    } else {
                        molt_obj_model::text_storage::push_code_point(out, code);
                    }
                }
                Ok(std::mem::take(out))
            }
            "ignore" | "replace" | "backslashreplace" | "namereplace" | "xmlcharrefreplace" => {
                for code in molt_obj_model::text_storage::code_points(bytes) {
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
                    molt_obj_model::text_storage::push_code_point(out, code);
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
            let mut points = molt_obj_model::text_storage::code_points(bytes).enumerate();
            while let Some((idx, code)) = points.next() {
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
                            let end = idx + 1 + points.take_while(|(_, cp)| *cp >= limit).count();
                            return Err(invalid_sequence(idx, end));
                        }
                    }
                    "surrogatepass" | "strict" => {
                        let end = idx + 1 + points.take_while(|(_, cp)| *cp >= limit).count();
                        return Err(invalid_sequence(idx, end));
                    }
                    other => {
                        return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                    }
                }
            }
            Ok(out)
        }
        CodecRuntimeClass::UnicodeEscape => {
            for code in molt_obj_model::text_storage::code_points(bytes) {
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
            for (idx, code) in molt_obj_model::text_storage::code_points(bytes).enumerate() {
                if is_surrogate(code) {
                    match handler {
                        "surrogatepass" => {
                            push_u16(&mut out, code as u16, endian);
                            continue;
                        }
                        "ignore" => continue,
                        "replace" => {
                            push_u16(&mut out, b'?' as u16, endian);
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
                        "strict" | "surrogateescape" => {
                            return Err(invalid_sequence(idx, idx + 1));
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
            for (idx, code) in molt_obj_model::text_storage::code_points(bytes).enumerate() {
                if is_surrogate(code) {
                    match handler {
                        "surrogatepass" => {
                            push_u32(&mut out, code, endian);
                            continue;
                        }
                        "ignore" => continue,
                        "replace" => {
                            push_u32(&mut out, b'?' as u32, endian);
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
                        "strict" | "surrogateescape" => {
                            return Err(invalid_sequence(idx, idx + 1));
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
            "replace" => molt_obj_model::text_storage::push_code_point(&mut out, 0xFFFD),
            "backslashreplace" => push_backslash_bytes(&mut out, &[byte]),
            "surrogateescape" => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0xDC00 + byte as u32)
            }
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

/// The first handler-created UnicodeDecodeError keeps these arguments even
/// when a later failure updates its public start/end/reason attributes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DecodeErrorSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) reason: &'static str,
}

fn decode_utf8_bytes_with_errors(
    bytes: &[u8],
    errors: &str,
    final_input: bool,
    initial_error: &mut Option<DecodeErrorSpan>,
) -> Result<(Vec<u8>, usize), DecodeFailure> {
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
        let needed = match first {
            0xC2..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF4 => 3,
            _ => {
                idx = handle_decode_failure(
                    &mut out,
                    bytes,
                    errors,
                    DecodeErrorSpan {
                        start: idx,
                        end: idx + 1,
                        reason: "invalid start byte",
                    },
                    None,
                    None,
                )?;
                continue;
            }
        };
        // CPython defers the two-byte surrogate prefix in non-final input,
        // even under strict handling; surrogatepass consumes only a complete
        // three-byte surrogate, leaving malformed sequences to the scanner.
        if first == 0xED
            && bytes
                .get(idx + 1)
                .is_some_and(|b| (0xA0..=0xBF).contains(b))
        {
            if !final_input && bytes.len() - idx == 2 {
                return Ok((out, idx));
            }
            if allow_surrogates
                && bytes
                    .get(idx + 2)
                    .is_some_and(|b| (0x80..=0xBF).contains(b))
            {
                let code = ((first & 0x0F) as u32) << 12
                    | ((bytes[idx + 1] & 0x3F) as u32) << 6
                    | (bytes[idx + 2] & 0x3F) as u32;
                initial_error.get_or_insert(DecodeErrorSpan {
                    start: idx,
                    end: idx + 1,
                    reason: "invalid continuation byte",
                });
                molt_obj_model::text_storage::push_code_point(&mut out, code);
                idx += 3;
                continue;
            }
        }
        let mut code: u32 = (first & (0x7F >> needed)) as u32;
        let mut failure = None;
        for off in 1..=needed {
            let Some(&byte) = bytes.get(idx + off) else {
                if !final_input {
                    return Ok((out, idx));
                }
                failure = Some((idx + off, "unexpected end of data"));
                break;
            };
            // CPython's UTF-8 scanner excludes the offending continuation from
            // the error span. Restricted second bytes reject overlong values,
            // surrogates (unless surrogatepass), and values above U+10FFFF.
            if !(0x80..=0xBF).contains(&byte)
                || (off == 1
                    && ((first == 0xE0 && byte < 0xA0)
                        || (first == 0xED && byte >= 0xA0)
                        || (first == 0xF0 && byte < 0x90)
                        || (first == 0xF4 && byte >= 0x90)))
            {
                failure = Some((idx + off, "invalid continuation byte"));
                break;
            }
            code = (code << 6) | (byte & 0x3F) as u32;
        }
        if let Some((end, reason)) = failure {
            idx = handle_decode_failure(
                &mut out,
                bytes,
                errors,
                DecodeErrorSpan {
                    start: idx,
                    end,
                    reason,
                },
                None,
                None,
            )?;
            continue;
        }
        molt_obj_model::text_storage::push_code_point(&mut out, code);
        idx += needed + 1;
    }
    Ok((out, idx))
}

// Resume at the handler's consumed boundary, not necessarily the scanner's
// error end. CPython surrogatepass consumes one whole surrogate; surrogateescape
// consumes only a non-ASCII byte prefix (up to four bytes).
fn handle_decode_failure(
    out: &mut Vec<u8>,
    bytes: &[u8],
    errors: &str,
    span: DecodeErrorSpan,
    surrogate: Option<(u32, usize)>,
    initial_error: Option<&mut Option<DecodeErrorSpan>>,
) -> Result<usize, DecodeFailure> {
    let DecodeErrorSpan {
        start,
        end,
        reason: message,
    } = span;
    let failure = || {
        if end == start + 1 {
            DecodeFailure::Byte {
                pos: start,
                byte: bytes[start],
                message,
            }
        } else {
            DecodeFailure::Range {
                start,
                end: end - 1,
                message,
            }
        }
    };
    match errors {
        "ignore" => Ok(end),
        "replace" => {
            molt_obj_model::text_storage::push_code_point(out, 0xfffd);
            Ok(end)
        }
        "backslashreplace" => {
            push_backslash_bytes(out, &bytes[start..end]);
            Ok(end)
        }
        "surrogatepass" => {
            if let Some((code, width)) = surrogate {
                if let Some(initial_error) = initial_error {
                    initial_error.get_or_insert(span);
                }
                molt_obj_model::text_storage::push_code_point(out, code);
                Ok(start + width)
            } else {
                Err(failure())
            }
        }
        "surrogateescape" => {
            let mut next = start;
            while next < end && next - start < 4 && bytes[next] >= 0x80 {
                molt_obj_model::text_storage::push_code_point(out, 0xdc00 + u32::from(bytes[next]));
                next += 1;
            }
            if next == start {
                Err(failure())
            } else {
                if let Some(initial_error) = initial_error {
                    initial_error.get_or_insert(span);
                }
                Ok(next)
            }
        }
        "strict" => Err(failure()),
        other => Err(DecodeFailure::UnknownErrorHandler(other.to_string())),
    }
}

/// The incremental UTF-8 consumer retains only the scanner's unconsumed suffix.
/// Incomplete input is not an error until final input, independent of the error
/// handler. Ordinary decoders use this same scanner with final_input=true.
pub(crate) fn decode_utf8_incremental(
    bytes: &[u8],
    errors: &str,
    final_input: bool,
) -> Result<(Vec<u8>, usize), DecodeTextError> {
    let mut initial_error = None;
    decode_utf8_bytes_with_errors(bytes, errors, final_input, &mut initial_error)
        .map_err(|failure| DecodeTextError::Failure(failure, "utf-8".to_string(), 0, initial_error))
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
            molt_obj_model::text_storage::push_code_point(out, 0xFFFD);
            Ok(end + 1)
        }
        "backslashreplace" => {
            if start <= end && end < bytes.len() {
                push_backslash_bytes(out, &bytes[start..=end]);
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
            molt_obj_model::text_storage::push_code_point(&mut out, byte as u32);
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
                molt_obj_model::text_storage::push_code_point(&mut out, b'\\' as u32);
                idx += 2;
            }
            b'\'' => {
                molt_obj_model::text_storage::push_code_point(&mut out, b'\'' as u32);
                idx += 2;
            }
            b'"' => {
                molt_obj_model::text_storage::push_code_point(&mut out, b'"' as u32);
                idx += 2;
            }
            b'a' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x07);
                idx += 2;
            }
            b'b' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x08);
                idx += 2;
            }
            b't' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x09);
                idx += 2;
            }
            b'n' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x0A);
                idx += 2;
            }
            b'v' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x0B);
                idx += 2;
            }
            b'f' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x0C);
                idx += 2;
            }
            b'r' => {
                molt_obj_model::text_storage::push_code_point(&mut out, 0x0D);
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
                molt_obj_model::text_storage::push_code_point(&mut out, value);
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
                molt_obj_model::text_storage::push_code_point(&mut out, value);
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
                molt_obj_model::text_storage::push_code_point(&mut out, value);
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
                let name_bytes = &bytes[idx + 3..close_idx];
                let name = std::str::from_utf8(name_bytes).unwrap_or("");
                let resolved = unicode_names2::character(name);
                if let Some(ch) = resolved {
                    molt_obj_model::text_storage::push_code_point(&mut out, ch as u32);
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
                molt_obj_model::text_storage::push_code_point(&mut out, value);
                idx += 1 + count;
            }
            _ => {
                molt_obj_model::text_storage::push_code_point(&mut out, b'\\' as u32);
                molt_obj_model::text_storage::push_code_point(&mut out, esc as u32);
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
    initial_error: &mut Option<DecodeErrorSpan>,
) -> Result<Vec<u8>, DecodeFailure> {
    let mut idx = offset;
    let mut out = Vec::new();
    while idx < bytes.len() {
        let remaining = bytes.len() - idx;
        if remaining < 2 {
            idx = handle_decode_failure(
                &mut out,
                bytes,
                errors,
                DecodeErrorSpan {
                    start: idx,
                    end: bytes.len(),
                    reason: "truncated data",
                },
                None,
                Some(&mut *initial_error),
            )?;
            continue;
        }
        let unit = read_u16(bytes, idx, endian);
        if (0xd800..=0xdbff).contains(&unit) {
            if remaining < 4 {
                idx = handle_decode_failure(
                    &mut out,
                    bytes,
                    errors,
                    DecodeErrorSpan {
                        start: idx,
                        end: bytes.len(),
                        reason: "unexpected end of data",
                    },
                    Some((u32::from(unit), 2)),
                    Some(&mut *initial_error),
                )?;
            } else {
                let next = read_u16(bytes, idx + 2, endian);
                if (0xdc00..=0xdfff).contains(&next) {
                    molt_obj_model::text_storage::push_code_point(
                        &mut out,
                        0x10000 + ((u32::from(unit) - 0xd800) << 10) + u32::from(next) - 0xdc00,
                    );
                    idx += 4;
                } else {
                    idx = handle_decode_failure(
                        &mut out,
                        bytes,
                        errors,
                        DecodeErrorSpan {
                            start: idx,
                            end: idx + 2,
                            reason: "illegal UTF-16 surrogate",
                        },
                        Some((u32::from(unit), 2)),
                        Some(&mut *initial_error),
                    )?;
                }
            }
        } else if (0xdc00..=0xdfff).contains(&unit) {
            idx = handle_decode_failure(
                &mut out,
                bytes,
                errors,
                DecodeErrorSpan {
                    start: idx,
                    end: idx + 2,
                    reason: "illegal encoding",
                },
                Some((u32::from(unit), 2)),
                Some(&mut *initial_error),
            )?;
        } else {
            molt_obj_model::text_storage::push_code_point(&mut out, u32::from(unit));
            idx += 2;
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
    initial_error: &mut Option<DecodeErrorSpan>,
) -> Result<Vec<u8>, DecodeFailure> {
    let mut idx = offset;
    let mut out = Vec::new();
    while idx < bytes.len() {
        if bytes.len() - idx < 4 {
            idx = handle_decode_failure(
                &mut out,
                bytes,
                errors,
                DecodeErrorSpan {
                    start: idx,
                    end: bytes.len(),
                    reason: "truncated data",
                },
                None,
                Some(&mut *initial_error),
            )?;
            continue;
        }
        let code = read_u32(bytes, idx, endian);
        if is_surrogate(code) {
            idx = handle_decode_failure(
                &mut out,
                bytes,
                errors,
                DecodeErrorSpan {
                    start: idx,
                    end: idx + 4,
                    reason: "code point in surrogate code point range(0xd800, 0xe000)",
                },
                Some((code, 4)),
                Some(&mut *initial_error),
            )?;
        } else if code > 0x10ffff {
            idx = handle_decode_failure(
                &mut out,
                bytes,
                errors,
                DecodeErrorSpan {
                    start: idx,
                    end: idx + 4,
                    reason: "code point not in range(0x110000)",
                },
                None,
                Some(&mut *initial_error),
            )?;
        } else {
            molt_obj_model::text_storage::push_code_point(&mut out, code);
            idx += 4;
        }
    }
    Ok(out)
}

fn decode_bytes_with_errors(
    bytes: &[u8],
    kind: EncodingKind,
    errors: &str,
) -> Result<(Vec<u8>, String), DecodeTextError> {
    let mut initial_error = None;
    let result = match kind.runtime_class() {
        CodecRuntimeClass::Utf8 => {
            match decode_utf8_bytes_with_errors(bytes, errors, true, &mut initial_error) {
                Ok((text, _)) => Ok((text, "utf-8".to_string())),
                Err(err) => Err((err, "utf-8".to_string(), 0)),
            }
        }
        CodecRuntimeClass::Utf8Sig => {
            let data =
                if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
                    &bytes[3..]
                } else {
                    bytes
                };
            match decode_utf8_bytes_with_errors(data, errors, true, &mut initial_error) {
                Ok((text, _)) => Ok((text, "utf-8".to_string())),
                Err(err) => Err((err, "utf-8".to_string(), bytes.len() - data.len())),
            }
        }
        CodecRuntimeClass::Charmap => {
            match decode_single_byte_charmap_with_errors(bytes, kind, errors) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string(), 0)),
            }
        }
        CodecRuntimeClass::Ascii => match decode_ascii_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "ascii".to_string())),
            Err(err) => Err((err, "ascii".to_string(), 0)),
        },
        CodecRuntimeClass::Latin1 => {
            let mut out = Vec::with_capacity(bytes.len());
            for &byte in bytes {
                molt_obj_model::text_storage::push_code_point(&mut out, byte as u32);
            }
            Ok((out, "latin-1".to_string()))
        }
        CodecRuntimeClass::UnicodeEscape => {
            match decode_unicode_escape_with_errors(bytes, errors) {
                Ok(text) => Ok((text, "unicodeescape".to_string())),
                Err(err) => Err((err, "unicodeescape".to_string(), 0)),
            }
        }
        CodecRuntimeClass::Utf16 | CodecRuntimeClass::Utf16LE | CodecRuntimeClass::Utf16BE => {
            let (endian, label, offset) = utf16_decode_config(bytes, kind);
            match decode_utf16_with_errors(bytes, errors, endian, offset, &mut initial_error) {
                Ok(text) => Ok((text, label)),
                Err(err) => Err((err, label, 0)),
            }
        }
        CodecRuntimeClass::Utf32 | CodecRuntimeClass::Utf32LE | CodecRuntimeClass::Utf32BE => {
            let (endian, label, offset) = utf32_decode_config(bytes, kind);
            match decode_utf32_with_errors(bytes, errors, endian, offset, &mut initial_error) {
                Ok(text) => Ok((text, label)),
                Err(err) => Err((err, label, 0)),
            }
        }
    };
    result.map_err(|(failure, label, input_start)| {
        DecodeTextError::Failure(failure, label, input_start, initial_error)
    })
}

#[derive(Debug)]
pub(crate) enum DecodeTextError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    Failure(DecodeFailure, String, usize, Option<DecodeErrorSpan>),
}

impl DecodeTextError {
    /// Project decoder facts to Python's five-field error, keeping the exact
    /// input alive as immutable bytes. Scanner Range endpoints are inclusive;
    /// UnicodeDecodeError.end is exclusive. Success paths allocate nothing here.
    pub(crate) fn raise<T: crate::builtins::exceptions::ExceptionSentinel>(
        self,
        py: &crate::PyToken<'_>,
        input: &[u8],
    ) -> T {
        let (encoding, start, end, reason, input_start, initial_error) = match self {
            Self::UnknownEncoding(name) => {
                return crate::raise_exception(
                    py,
                    "LookupError",
                    &format!("unknown encoding: {name}"),
                );
            }
            Self::UnknownErrorHandler(name)
            | Self::Failure(DecodeFailure::UnknownErrorHandler(name), _, _, _) => {
                // These registered handlers accept encoding errors only. Like
                // CPython, consult them only after an actual decoding failure.
                if matches!(name.as_str(), "xmlcharrefreplace" | "namereplace") {
                    return crate::raise_exception(
                        py,
                        "TypeError",
                        "don't know how to handle UnicodeDecodeError in error callback",
                    );
                }
                return crate::raise_exception(
                    py,
                    "LookupError",
                    &format!("unknown error handler name '{name}'"),
                );
            }
            Self::Failure(
                DecodeFailure::Byte { pos, message, .. },
                encoding,
                input_start,
                initial_error,
            ) => (encoding, pos, pos + 1, message, input_start, initial_error),
            Self::Failure(
                DecodeFailure::Range {
                    start,
                    end,
                    message,
                },
                encoding,
                input_start,
                initial_error,
            ) => (
                encoding,
                start,
                end + 1,
                message,
                input_start,
                initial_error,
            ),
        };
        let object = crate::alloc_bytes(py, &input[input_start..]);
        if object.is_null() {
            return T::exception_sentinel();
        }
        let object_bits = crate::MoltObject::from_ptr(object).bits();
        let result = crate::raise_unicode_decode_error(
            py,
            &encoding,
            object_bits,
            start,
            end,
            reason,
            initial_error.map(|span| (span.start, span.end, span.reason)),
        );
        crate::dec_ref_bits(py, object_bits);
        result
    }
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
    if errors_known {
        decode_bytes_with_errors(bytes, kind, errors)
    } else {
        match decode_bytes_with_errors(bytes, kind, "strict") {
            Ok(result) => Ok(result),
            Err(_) => Err(DecodeTextError::UnknownErrorHandler(errors.to_string())),
        }
    }
}

#[cfg(test)]
mod encode_tests {
    use super::*;

    #[test]
    fn named_codecs_work_without_the_public_unicodedata_surface() {
        // CPython 3.12.15, 3.13.16, 3.14.8 and 3.15.0 agree on these bytes.
        // No public unicodedata export or optional feature participates here.
        assert_eq!(
            encode_string_with_errors("\u{20ac}\u{378}".as_bytes(), "ascii", Some("namereplace"))
                .unwrap(),
            br"\N{EURO SIGN}\u0378",
        );
        assert_eq!(
            decode_bytes_text("unicode_escape", "strict", br"\N{LATIN CAPITAL LETTER A}")
                .unwrap()
                .0,
            b"A",
        );
    }

    #[test]
    fn utf16_utf32_handlers_resume_at_the_consumed_byte() {
        // CPython v3.12.13 Python/codecs.c: surrogatepass consumes one
        // complete surrogate unit; surrogateescape consumes only non-ASCII
        // bytes from the start of the reported error. Neither swallows a tail.
        type Success<'a> = (&'a str, &'a str, &'a [u8], &'a [u8]);
        let successes: &[Success<'_>] = &[
            ("utf-16-le", "surrogatepass", b"\x00\xd8", b"\xed\xa0\x80"),
            ("utf-16-be", "surrogatepass", b"\xdc\x00", b"\xed\xb0\x80"),
            (
                "utf-32-le",
                "surrogatepass",
                b"\x00\xd8\x00\x00",
                b"\xed\xa0\x80",
            ),
            (
                "utf-32-be",
                "surrogatepass",
                b"\x00\x00\xdc\x00",
                b"\xed\xb0\x80",
            ),
            (
                "utf-16-be",
                "surrogateescape",
                b"\xdc\x00A",
                b"\xed\xb3\x9cA",
            ),
            (
                "utf-16-be",
                "surrogateescape",
                b"\xdc\x80",
                b"\xed\xb3\x9c\xed\xb2\x80",
            ),
            (
                "utf-32-le",
                "surrogateescape",
                b"\xff\xff\xff\xff",
                b"\xed\xb3\xbf\xed\xb3\xbf\xed\xb3\xbf\xed\xb3\xbf",
            ),
            ("utf-16-le", "replace", b"\x00\xd8\xff", b"\xef\xbf\xbd"),
            ("utf-16-le", "ignore", b"\x00\xd8\xff", b""),
            (
                "utf-16-le",
                "backslashreplace",
                b"\x00\xd8\xff",
                b"\\x00\\xd8\\xff",
            ),
        ];
        for &(encoding, errors, input, expected) in successes {
            assert_eq!(
                decode_bytes_text(encoding, errors, input).unwrap().0,
                expected,
                "{encoding}/{errors}: {input:?}"
            );
        }
        type Failure<'a> = (&'a str, &'a str, &'a [u8], &'a str, usize, usize, &'a str);
        let failures: &[Failure<'_>] = &[
            (
                "utf-16",
                "surrogatepass",
                b"\xff\xfe\x00\xd8\xff",
                "utf-16-le",
                4,
                5,
                "truncated data",
            ),
            (
                "utf-16",
                "surrogatepass",
                b"\xfe\xff\xdc\x00\xff",
                "utf-16-be",
                4,
                5,
                "truncated data",
            ),
            (
                "utf-32",
                "surrogatepass",
                b"\xff\xfe\x00\x00\x00\xd8\x00\x00\xff",
                "utf-32-le",
                8,
                9,
                "truncated data",
            ),
            (
                "utf-32",
                "surrogatepass",
                b"\x00\x00\xfe\xff\x00\x00\xdc\x00\xff",
                "utf-32-be",
                8,
                9,
                "truncated data",
            ),
            (
                "utf-16-le",
                "surrogateescape",
                b"\x00\xdc",
                "utf-16-le",
                0,
                2,
                "illegal encoding",
            ),
            (
                "utf-16-le",
                "surrogateescape",
                b"\x80\xd8A",
                "utf-16-le",
                2,
                3,
                "truncated data",
            ),
            (
                "utf-32-le",
                "surrogateescape",
                b"\x80\x00\x11\x00",
                "utf-32-le",
                1,
                4,
                "truncated data",
            ),
        ];
        for &(encoding, errors, input, label, start, end, reason) in failures {
            let DecodeTextError::Failure(failure, actual_label, input_start, _) =
                decode_bytes_text(encoding, errors, input).unwrap_err()
            else {
                panic!("expected a decode failure")
            };
            let actual = match failure {
                DecodeFailure::Byte { pos, message, .. } => (pos, pos + 1, message),
                DecodeFailure::Range {
                    start,
                    end,
                    message,
                } => (start, end + 1, message),
                other => panic!("unexpected failure: {other:?}"),
            };
            assert_eq!(
                (actual_label.as_str(), input_start, actual),
                (label, 0, (start, end, reason)),
                "{encoding}/{errors}: {input:?}"
            );
        }
    }

    #[test]
    fn utf8_failure_facts_and_consumption_match_cpython() {
        // Pinned CPython3.12 stringlib/codecs.h + unicode_decode_utf8:
        // each expected span excludes the first bad continuation byte.
        type Case<'a> = (&'a [u8], usize, usize, &'a str);
        let cases: &[Case<'_>] = &[
            (b"a\x80z", 1, 2, "invalid start byte"),
            (b"a\xc0\x80z", 1, 2, "invalid start byte"),
            (b"a\xc1\xbfz", 1, 2, "invalid start byte"),
            (b"a\xf5\x80z", 1, 2, "invalid start byte"),
            (b"a\xc2z", 1, 2, "invalid continuation byte"),
            (b"a\xe1\x80z", 1, 3, "invalid continuation byte"),
            (b"a\xf1\x80\x80z", 1, 4, "invalid continuation byte"),
            (b"a\xe0\x9f\x80z", 1, 2, "invalid continuation byte"),
            (b"a\xed\xa0\x80z", 1, 2, "invalid continuation byte"),
            (b"a\xf0\x8f\x80\x80z", 1, 2, "invalid continuation byte"),
            (b"a\xf4\x90\x80\x80z", 1, 2, "invalid continuation byte"),
            (b"a\xc2", 1, 2, "unexpected end of data"),
            (b"a\xe1\x80", 1, 3, "unexpected end of data"),
            (b"a\xf1\x80\x80", 1, 4, "unexpected end of data"),
            (b"a\xed\xa0", 1, 2, "invalid continuation byte"),
        ];
        for &(input, start, end, reason) in cases {
            let failure =
                decode_utf8_bytes_with_errors(input, "strict", true, &mut None).unwrap_err();
            let actual = match failure {
                DecodeFailure::Byte { pos, message, .. } => (pos, pos + 1, message),
                DecodeFailure::Range {
                    start,
                    end,
                    message,
                } => (start, end + 1, message),
                other => panic!("unexpected failure: {other:?}"),
            };
            assert_eq!(actual, (start, end, reason), "input={input:?}");
        }
        for errors in [
            "strict",
            "replace",
            "ignore",
            "backslashreplace",
            "surrogateescape",
            "surrogatepass",
        ] {
            for input in [
                b"a\xc2".as_slice(),
                b"a\xe1\x80",
                b"a\xf1\x80\x80",
                b"a\xed\xa0",
            ] {
                assert_eq!(
                    decode_utf8_incremental(input, errors, false).unwrap(),
                    (b"a".to_vec(), 1)
                );
            }
        }
        assert_eq!(
            decode_utf8_incremental(b"a\xff\xe1\x80", "replace", false).unwrap(),
            ("a�".as_bytes().to_vec(), 2)
        );
        assert_eq!(
            decode_utf8_incremental(b"a\xe1\x80z", "replace", false).unwrap(),
            ("a�z".as_bytes().to_vec(), 4)
        );
        assert!(decode_utf8_incremental(b"a\xff", "strict", false).is_err());
        assert_eq!(
            decode_utf8_incremental(b"a\xed\xa0\x80", "surrogatepass", true).unwrap(),
            (b"a\xed\xa0\x80".to_vec(), 4)
        );
    }

    #[test]
    fn decoder_failures_publish_typed_input_and_exclusive_spans() {
        use crate::*;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::concurrency::gil::with_gil(|py| {
            let py = &py;
            // CPython3.12 Objects/unicodeobject.c make_decode_exception and
            // UTF16/32/unicodeescape scanners: original bytes, exclusive end.
            type Span<'a> = (usize, usize, &'a str);
            type Case<'a> = (&'a [u8], &'a str, &'a str, Span<'a>, Option<Span<'a>>);
            let cases: &[Case<'_>] = &[
                (
                    b"a\xffz",
                    "utf-8",
                    "strict",
                    (1, 2, "invalid start byte"),
                    None,
                ),
                (
                    b"\xef\xbb\xbfa\xffz",
                    "utf-8-sig",
                    "strict",
                    (1, 2, "invalid start byte"),
                    None,
                ),
                (
                    b"a\xffz",
                    "ascii",
                    "strict",
                    (1, 2, "ordinal not in range(128)"),
                    None,
                ),
                (
                    b"A\x00\x00\xd8",
                    "utf-16-le",
                    "strict",
                    (2, 4, "unexpected end of data"),
                    None,
                ),
                (
                    b"\x00\x00\x11\x00",
                    "utf-32-le",
                    "strict",
                    (0, 4, "code point not in range(0x110000)"),
                    None,
                ),
                (
                    b"\\x1",
                    "unicode_escape",
                    "strict",
                    (0, 3, "truncated \\xXX escape"),
                    None,
                ),
                // make_decode_exception reuses the first exception's args;
                // the latest attributes still describe the fatal byte span.
                (
                    b"\xed\xa0\x80\xff",
                    "utf-8",
                    "surrogatepass",
                    (3, 4, "invalid start byte"),
                    Some((0, 1, "invalid continuation byte")),
                ),
                (
                    b"\x00\xd8\xff",
                    "utf-16-le",
                    "surrogatepass",
                    (2, 3, "truncated data"),
                    Some((0, 3, "unexpected end of data")),
                ),
                (
                    b"\x00\xd8\x00\x00\xff",
                    "utf-32-le",
                    "surrogatepass",
                    (4, 5, "truncated data"),
                    Some((
                        0,
                        4,
                        "code point in surrogate code point range(0xd800, 0xe000)",
                    )),
                ),
                (
                    b"\xdc\x80A",
                    "utf-16-be",
                    "surrogateescape",
                    (2, 3, "truncated data"),
                    Some((0, 2, "illegal encoding")),
                ),
                (
                    b"\x80\x00\x11\x00",
                    "utf-32-le",
                    "surrogateescape",
                    (1, 4, "truncated data"),
                    Some((0, 4, "code point not in range(0x110000)")),
                ),
            ];
            for &(input, encoding, errors, latest, initial) in cases {
                let (start, end, reason) = initial.unwrap_or(latest);
                let failure = decode_bytes_text(encoding, errors, input).unwrap_err();
                let _: u64 = failure.raise(py, input);
                let error = molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "UnicodeDecodeError"
                ));
                let ptr = obj_from_bits(error).as_ptr().unwrap();
                let args = unsafe { crate::builtins::exceptions::exception_args_bits(ptr) };
                let args = unsafe {
                    crate::object::seq_access::pin_tuple(py, obj_from_bits(args).as_ptr().unwrap())
                }
                .unwrap();
                assert_eq!(args.len(), 5);
                let label = if encoding == "unicode_escape" {
                    "unicodeescape"
                } else if encoding == "utf-8-sig" {
                    "utf-8"
                } else {
                    encoding
                };
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(args[0])).as_deref(),
                    Some(label)
                );
                let object = obj_from_bits(args[1]).as_ptr().unwrap();
                assert_eq!(unsafe { object_type_id(object) }, TYPE_ID_BYTES);
                assert_eq!(
                    unsafe { bytes_like_slice(object) }.unwrap(),
                    if encoding == "utf-8-sig" {
                        &input[3..]
                    } else {
                        input
                    }
                );
                assert_eq!(to_i64(obj_from_bits(args[2])), Some(start as i64));
                assert_eq!(to_i64(obj_from_bits(args[3])), Some(end as i64));
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(args[4])).as_deref(),
                    Some(reason)
                );
                use molt_obj_model::ExceptionTypedField;
                let [latest_start, latest_end, latest_reason] = [
                    ExceptionTypedField::UnicodeStart,
                    ExceptionTypedField::UnicodeEnd,
                    ExceptionTypedField::UnicodeReason,
                ]
                .map(|field| {
                    crate::builtins::exceptions::exception_typed_field_get(py, ptr, field)
                        .unwrap()
                        .unwrap()
                });
                assert_eq!(to_i64(obj_from_bits(latest_start)), Some(latest.0 as i64));
                assert_eq!(to_i64(obj_from_bits(latest_end)), Some(latest.1 as i64));
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(latest_reason)).as_deref(),
                    Some(latest.2)
                );
                for bits in [latest_start, latest_end, latest_reason] {
                    dec_ref_bits(py, bits);
                }
                drop(args);
                molt_exception_clear();
                dec_ref_bits(py, error);
                assert!(!exception_pending(py));
            }
            assert!(decode_bytes_text("ascii", "unknown-but-unused", b"valid").is_ok());
            let _: u64 = decode_bytes_text("ascii", "missing-handler", b"\xff")
                .unwrap_err()
                .raise(py, b"\xff");
            let error = molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "LookupError"
            ));
            molt_exception_clear();
            dec_ref_bits(py, error);
        });
    }

    #[test]
    fn encoding_failure_spans_follow_each_cpython_scanner() {
        // CPython Objects/unicodeobject.c unicode_encode_ucs1,
        // charmap_encoding_error, _PyUnicode_EncodeUTF16/32 and
        // Objects/stringlib/codecs.h utf8_encode: code-point offsets, not bytes.
        type EncodingFailureCase<'a> = (&'a [u32], &'a str, &'a str, usize, usize, &'a str);
        let cases: &[EncodingFailureCase<'_>] = &[
            (&[97, 0xe9, 0x20ac, 122], "ascii", "strict", 1, 3, "ascii"),
            (
                &[0xe9, 97, 0x20ac, 0x10000, 122],
                "latin-1",
                "strict",
                2,
                4,
                "latin-1",
            ),
            (
                &[97, 0x20ac, 0x416, 0x10000, 122],
                "cp1252",
                "strict",
                2,
                4,
                "charmap",
            ),
            (&[97, 0xd800, 0xd801, 122], "utf-8", "strict", 1, 3, "utf-8"),
            (
                &[97, 0xd800, 0xd801, 122],
                "utf-8-sig",
                "strict",
                1,
                3,
                "utf-8",
            ),
            (
                &[97, 0xd800, 0xd801, 122],
                "utf-16-le",
                "strict",
                1,
                2,
                "utf-16-le",
            ),
            (
                &[97, 0xd800, 0xd801, 122],
                "utf-32-be",
                "strict",
                1,
                2,
                "utf-32-be",
            ),
            (
                &[97, 0xdc80, 0xd800, 0xdc81, 122],
                "ascii",
                "surrogateescape",
                2,
                4,
                "ascii",
            ),
            (
                &[97, 0xdc80, 0xd800, 0xdc81, 122],
                "latin-1",
                "surrogateescape",
                2,
                4,
                "latin-1",
            ),
            (
                &[97, 0xdc80, 0xd800, 0xdc81, 122],
                "cp1252",
                "surrogateescape",
                1,
                4,
                "charmap",
            ),
            (
                &[97, 0xdc80, 0xd800, 0xdc81, 122],
                "utf-8",
                "surrogateescape",
                2,
                4,
                "utf-8",
            ),
            (
                &[97, 0xdc80, 0xdcff, 122],
                "utf-16-be",
                "surrogateescape",
                1,
                2,
                "utf-16-be",
            ),
            (
                &[97, 0xdc80, 0xdcff, 122],
                "utf-32-le",
                "surrogateescape",
                1,
                2,
                "utf-32-le",
            ),
            (
                &[97, 0xe9, 120, 0x20ac, 122],
                "ascii",
                "surrogatepass",
                1,
                2,
                "ascii",
            ),
        ];
        for &(points, codec, policy, expected_start, expected_end, label) in cases {
            let mut bytes = Vec::new();
            for &point in points {
                molt_obj_model::text_storage::push_code_point(&mut bytes, point);
            }
            let EncodeError::InvalidSequence {
                encoding,
                start,
                end,
                reason,
            } = encode_string_with_errors(&bytes, codec, Some(policy)).unwrap_err()
            else {
                panic!("{codec}/{policy}: expected UnicodeEncodeError");
            };
            assert_eq!(
                (encoding, start, end),
                (label, expected_start, expected_end),
                "{codec}/{policy}"
            );
            let expected_reason = match label {
                "ascii" => "ordinal not in range(128)",
                "latin-1" => "ordinal not in range(256)",
                "charmap" => "character maps to <undefined>",
                _ => "surrogates not allowed",
            };
            assert_eq!(reason, expected_reason);
        }
    }

    #[test]
    fn encoding_builtin_handlers_preserve_bytes_and_lookup_is_lazy() {
        let input = b"a\xed\xa0\x80\xed\xa0\x81z"; // a, D800, D801, z
        for (codec, expected) in [
            ("utf-16-le", &b"a\0?\0?\0z\0"[..]),
            ("utf-16-be", &b"\0a\0?\0?\0z"[..]),
            ("utf-32-le", &b"a\0\0\0?\0\0\0?\0\0\0z\0\0\0"[..]),
            ("utf-32-be", &b"\0\0\0a\0\0\0?\0\0\0?\0\0\0z"[..]),
        ] {
            assert_eq!(
                encode_string_with_errors(input, codec, Some("replace")).unwrap(),
                expected
            );
        }
        for codec in ["ascii", "latin-1", "cp1252", "utf-8"] {
            assert_eq!(
                encode_string_with_errors(
                    b"a\xed\xb2\x80\xed\xb3\xbfz",
                    codec,
                    Some("surrogateescape")
                )
                .unwrap(),
                b"a\x80\xffz"
            );
            assert!(encode_string_with_errors(b"valid", codec, Some("missing-handler")).is_ok());
            assert!(
                matches!(encode_string_with_errors(input, codec, Some("missing-handler")), Err(EncodeError::UnknownErrorHandler(name)) if name == "missing-handler")
            );
        }
        assert_eq!(
            encode_string_with_errors(input, "utf-16-le", Some("surrogatepass")).unwrap(),
            b"a\0\0\xd8\x01\xd8z\0"
        );
        assert_eq!(
            encode_string_with_errors(input, "utf-32-be", Some("surrogatepass")).unwrap(),
            b"\0\0\0a\0\0\xd8\0\0\0\xd8\x01\0\0\0z"
        );
    }
}
