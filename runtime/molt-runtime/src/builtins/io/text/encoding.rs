use super::*;
use crate::object::ops::{decode_error_byte, decode_error_range};
use crate::object::ops_encoding::DecodeFailure;
use molt_runtime_text::codec_registry::{TextEncodingClass, normalize_encoding};

#[derive(Clone, Copy, Debug)]
pub(in crate::builtins::io) enum TextEncodingKind {
    Utf8,
    Ascii,
    Latin1,
    Utf16,
    Utf32,
}

pub(in crate::builtins::io) fn normalize_text_encoding(
    encoding: &str,
) -> Result<(String, TextEncodingKind), String> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(format!("unknown encoding: {encoding}"));
    };
    let Some(class) = kind.text_class() else {
        return Err(format!("unknown encoding: {encoding}"));
    };
    Ok((
        kind.name().to_string(),
        text_encoding_kind_from_class(class),
    ))
}

pub(in crate::builtins::io) fn text_encoding_kind(label: &str) -> TextEncodingKind {
    normalize_encoding(label)
        .and_then(|kind| kind.text_class())
        .map(text_encoding_kind_from_class)
        .unwrap_or(TextEncodingKind::Utf8)
}

fn text_encoding_kind_from_class(class: TextEncodingClass) -> TextEncodingKind {
    match class {
        TextEncodingClass::Utf8 => TextEncodingKind::Utf8,
        TextEncodingClass::Ascii => TextEncodingKind::Ascii,
        TextEncodingClass::SingleByte => TextEncodingKind::Latin1,
        TextEncodingClass::Utf16 => TextEncodingKind::Utf16,
        TextEncodingClass::Utf32 => TextEncodingKind::Utf32,
    }
}

pub(in crate::builtins::io) fn text_encoding_is_multibyte(kind: TextEncodingKind) -> bool {
    matches!(kind, TextEncodingKind::Utf16 | TextEncodingKind::Utf32)
}

pub(in crate::builtins::io) fn text_encoding_is_variable(kind: TextEncodingKind) -> bool {
    matches!(
        kind,
        TextEncodingKind::Utf8 | TextEncodingKind::Utf16 | TextEncodingKind::Utf32
    )
}

pub(in crate::builtins::io) fn split_fixed_pending(
    handle: &mut MoltFileHandle,
    bytes: &mut Vec<u8>,
    at_eof: bool,
    unit: usize,
) {
    if at_eof {
        handle.text_pending_bytes.clear();
        return;
    }
    let rem = bytes.len() % unit;
    if rem == 0 {
        handle.text_pending_bytes.clear();
        return;
    }
    let split = bytes.len().saturating_sub(rem);
    let pending = bytes.split_off(split);
    handle.text_pending_bytes = pending;
}

pub(in crate::builtins::io) fn split_text_pending_bytes(
    handle: &mut MoltFileHandle,
    bytes: &mut Vec<u8>,
    at_eof: bool,
    kind: TextEncodingKind,
) {
    match kind {
        TextEncodingKind::Utf8 => split_utf8_pending(handle, bytes, at_eof),
        TextEncodingKind::Utf16 => split_fixed_pending(handle, bytes, at_eof, 2),
        TextEncodingKind::Utf32 => split_fixed_pending(handle, bytes, at_eof, 4),
        TextEncodingKind::Ascii | TextEncodingKind::Latin1 => {
            handle.text_pending_bytes.clear();
        }
    }
}

pub(in crate::builtins::io) fn decode_text_bytes(
    _py: &PyToken<'_>,
    encoding_label: &str,
    errors: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, String), u64> {
    match crate::object::ops::decode_bytes_text(encoding_label, errors, bytes) {
        Ok((text_bytes, label)) => Ok((text_bytes, label)),
        Err(crate::object::ops::DecodeTextError::UnknownEncoding(name)) => {
            let msg = format!("unknown encoding: {name}");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(name)) => {
            let msg = format!("unknown error handler name '{name}'");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::Failure(
            DecodeFailure::Byte { pos, byte, message },
            label,
        )) => {
            let msg = decode_error_byte(&label, byte, pos, message);
            Err(raise_exception::<_>(_py, "UnicodeDecodeError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::Failure(
            DecodeFailure::Range {
                start,
                end,
                message,
            },
            label,
        )) => {
            let msg = decode_error_range(&label, start, end, message);
            Err(raise_exception::<_>(_py, "UnicodeDecodeError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::Failure(
            DecodeFailure::UnknownErrorHandler(name),
            _label,
        )) => {
            let msg = format!("unknown error handler name '{name}'");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
    }
}

pub(in crate::builtins::io) fn decode_text_bytes_for_io(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    encoding_label: &str,
    errors: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, String), u64> {
    let mut decode_label = encoding_label;
    if encoding_label == "utf-8-sig" && handle.text_bom_seen {
        decode_label = "utf-8";
    }
    let result = decode_text_bytes(_py, decode_label, errors, bytes)?;
    if encoding_label == "utf-8-sig" && !handle.text_bom_seen && !bytes.is_empty() {
        handle.text_bom_seen = true;
    }
    Ok(result)
}

pub(in crate::builtins::io) fn decode_multibyte_text(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    encoding_label: &mut String,
    encoding_kind: &mut TextEncodingKind,
    errors: &str,
    bytes: &[u8],
    at_eof: bool,
) -> Result<Vec<u8>, u64> {
    let (mut text_bytes, label) = decode_text_bytes(_py, encoding_label, errors, bytes)?;
    if (encoding_label.as_str() == "utf-16" || encoding_label.as_str() == "utf-32")
        && label != *encoding_label
    {
        *encoding_label = label.clone();
        handle.encoding = Some(label.clone());
        *encoding_kind = text_encoding_kind(encoding_label.as_str());
    }
    let newline_is_none = handle.newline.is_none();
    let combine_crlf = matches!(handle.newline.as_deref(), None | Some(""));
    let mut combined: Vec<u8> = Vec::new();
    if let Some(pending) = handle.pending_byte.take() {
        if combine_crlf && pending == b'\r' {
            if text_bytes.first() == Some(&b'\n') {
                combined.extend_from_slice(b"\r\n");
                text_bytes.remove(0);
            } else {
                combined.push(b'\r');
            }
        } else {
            combined.push(pending);
        }
    }
    combined.extend_from_slice(&text_bytes);
    if combine_crlf && !at_eof && combined.last() == Some(&b'\r') {
        combined.pop();
        handle.pending_byte = Some(b'\r');
    }
    update_newlines_from_bytes(handle, &combined);
    if newline_is_none {
        Ok(translate_universal_newlines(&combined))
    } else {
        Ok(combined)
    }
}

pub(in crate::builtins::io) fn utf8_expected_len(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if (0xC2..=0xDF).contains(&byte) {
        2
    } else if (0xE0..=0xEF).contains(&byte) {
        3
    } else if (0xF0..=0xF4).contains(&byte) {
        4
    } else {
        1
    }
}

pub(in crate::builtins::io) fn utf8_pending_len(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut cont = 0usize;
    let mut idx = bytes.len();
    while cont < 3 && idx > 0 {
        let byte = bytes[idx - 1];
        if (byte & 0xC0) == 0x80 {
            cont += 1;
            idx -= 1;
        } else {
            break;
        }
    }
    if cont == 0 {
        let byte = bytes[bytes.len() - 1];
        let needed = utf8_expected_len(byte);
        return if needed > 1 { 1 } else { 0 };
    }
    if idx == 0 {
        return 0;
    }
    let lead = bytes[idx - 1];
    let expected = utf8_expected_len(lead);
    if expected <= 1 {
        return 0;
    }
    let seq_len = cont + 1;
    if expected > seq_len { seq_len } else { 0 }
}

pub(in crate::builtins::io) fn split_utf8_pending(
    handle: &mut MoltFileHandle,
    bytes: &mut Vec<u8>,
    at_eof: bool,
) {
    if at_eof || bytes.is_empty() {
        handle.text_pending_bytes.clear();
        return;
    }
    let pending_len = utf8_pending_len(bytes);
    if pending_len == 0 {
        handle.text_pending_bytes.clear();
        return;
    }
    let split = bytes.len().saturating_sub(pending_len);
    let pending = bytes.split_off(split);
    handle.text_pending_bytes = pending;
}

pub(in crate::builtins::io) fn wtf8_char_count(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let width = utf8_expected_len(bytes[idx]);
        idx = idx.saturating_add(width).min(bytes.len());
        count += 1;
    }
    count
}

pub(in crate::builtins::io) fn wtf8_split_index(bytes: &[u8], limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() && count < limit {
        let width = utf8_expected_len(bytes[idx]);
        idx = idx.saturating_add(width).min(bytes.len());
        count += 1;
    }
    idx
}

pub(in crate::builtins::io) fn pending_text_line_end(
    bytes: &[u8],
    newline: Option<&str>,
) -> Option<usize> {
    match newline {
        None | Some("\n") => bytes.iter().position(|&b| b == b'\n').map(|idx| idx + 1),
        Some("") => {
            let mut idx = 0usize;
            while idx < bytes.len() {
                let byte = bytes[idx];
                if byte == b'\n' {
                    return Some(idx + 1);
                }
                if byte == b'\r' {
                    if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                        return Some(idx + 2);
                    }
                    return Some(idx + 1);
                }
                idx += 1;
            }
            None
        }
        Some("\r") => bytes.iter().position(|&b| b == b'\r').map(|idx| idx + 1),
        Some("\r\n") => {
            let mut idx = 0usize;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'\r' && bytes[idx + 1] == b'\n' {
                    return Some(idx + 2);
                }
                idx += 1;
            }
            None
        }
        Some(_) => bytes.iter().position(|&b| b == b'\n').map(|idx| idx + 1),
    }
}

pub(in crate::builtins::io) fn validate_decode_error_handler(errors: &str) -> Result<(), String> {
    if matches!(
        errors,
        "strict" | "ignore" | "replace" | "backslashreplace" | "surrogateescape" | "surrogatepass"
    ) {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}

pub(in crate::builtins::io) fn validate_encode_error_handler(errors: &str) -> Result<(), String> {
    if matches!(
        errors,
        "strict"
            | "ignore"
            | "replace"
            | "backslashreplace"
            | "surrogateescape"
            | "surrogatepass"
            | "namereplace"
            | "xmlcharrefreplace"
    ) {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}
