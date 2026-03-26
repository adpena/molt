use crate::PyToken;
use crate::object::{MoltFileBackend, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF, NEWLINE_KIND_LF};
use crate::*;
use num_bigint::{BigInt, Sign};
use std::io::Read;
use std::sync::Arc;

use super::io::{
    alloc_file_handle_with_state, backend_flush, backend_read_bytes, backend_seek, backend_tell,
    backend_truncate, backend_write_bytes, buffered_read_bytes, buffered_read_into,
    clear_read_buffer, clear_write_buffer, file_handle_close_ptr, file_handle_enter,
    file_handle_exit, file_read1_bytes, flush_write_buffer, handle_read_byte,
    memory_backend_vec_ref, prepend_read_buffer, reconfigure_arg_newline, reconfigure_arg_type,
    rewind_read_buffer, unread_bytes, vfs_writeback_take,
};

#[cfg(windows)]
use super::io::windows_handle_isatty;

macro_rules! file_handle_require_attached {
    ($py:expr, $handle:expr) => {
        if $handle.detached {
            return raise_exception::<_>($py, "ValueError", file_handle_detached_message($handle));
        }
    };
}

#[derive(Clone, Copy, Debug)]
enum TextEncodingKind {
    Utf8,
    Ascii,
    Latin1,
    Utf16,
    Utf32,
}

fn normalize_text_encoding(encoding: &str) -> Result<(String, TextEncodingKind), String> {
    let normalized = encoding.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Ok(("utf-8".to_string(), TextEncodingKind::Utf8)),
        "utf-8-sig" | "utf8-sig" => Ok(("utf-8-sig".to_string(), TextEncodingKind::Utf8)),
        "cp1252" | "cp-1252" | "windows-1252" => {
            Ok(("cp1252".to_string(), TextEncodingKind::Latin1))
        }
        "cp437" | "ibm437" | "437" => Ok(("cp437".to_string(), TextEncodingKind::Latin1)),
        "cp850" | "ibm850" | "850" | "cp-850" => {
            Ok(("cp850".to_string(), TextEncodingKind::Latin1))
        }
        "cp860" | "ibm860" | "860" | "cp-860" => {
            Ok(("cp860".to_string(), TextEncodingKind::Latin1))
        }
        "cp862" | "ibm862" | "862" | "cp-862" => {
            Ok(("cp862".to_string(), TextEncodingKind::Latin1))
        }
        "cp863" | "ibm863" | "863" | "cp-863" => {
            Ok(("cp863".to_string(), TextEncodingKind::Latin1))
        }
        "cp865" | "ibm865" | "865" | "cp-865" => {
            Ok(("cp865".to_string(), TextEncodingKind::Latin1))
        }
        "cp866" | "ibm866" | "866" | "cp-866" => {
            Ok(("cp866".to_string(), TextEncodingKind::Latin1))
        }
        "cp874" | "cp-874" | "windows-874" => Ok(("cp874".to_string(), TextEncodingKind::Latin1)),
        "cp1250" | "cp-1250" | "windows-1250" => {
            Ok(("cp1250".to_string(), TextEncodingKind::Latin1))
        }
        "cp1251" | "cp-1251" | "windows-1251" => {
            Ok(("cp1251".to_string(), TextEncodingKind::Latin1))
        }
        "cp1253" | "cp-1253" | "windows-1253" => {
            Ok(("cp1253".to_string(), TextEncodingKind::Latin1))
        }
        "cp1254" | "cp-1254" | "windows-1254" => {
            Ok(("cp1254".to_string(), TextEncodingKind::Latin1))
        }
        "cp1255" | "cp-1255" | "windows-1255" => {
            Ok(("cp1255".to_string(), TextEncodingKind::Latin1))
        }
        "cp1256" | "cp-1256" | "windows-1256" => {
            Ok(("cp1256".to_string(), TextEncodingKind::Latin1))
        }
        "cp1257" | "cp-1257" | "windows-1257" => {
            Ok(("cp1257".to_string(), TextEncodingKind::Latin1))
        }
        "koi8-r" | "koi8r" | "koi8_r" => Ok(("koi8-r".to_string(), TextEncodingKind::Latin1)),
        "koi8-u" | "koi8u" | "koi8_u" => Ok(("koi8-u".to_string(), TextEncodingKind::Latin1)),
        "iso-8859-2" | "iso8859-2" | "latin2" | "latin-2" => {
            Ok(("iso8859-2".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-3" | "iso8859-3" | "latin3" | "latin-3" | "latin_3" => {
            Ok(("iso8859-3".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-4" | "iso8859-4" | "latin4" | "latin-4" | "latin_4" => {
            Ok(("iso8859-4".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-5" | "iso8859-5" | "cyrillic" => {
            Ok(("iso8859-5".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-6" | "iso8859-6" | "arabic" => {
            Ok(("iso8859-6".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-7" | "iso8859-7" | "greek" => {
            Ok(("iso8859-7".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-8" | "iso8859-8" | "hebrew" => {
            Ok(("iso8859-8".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-10" | "iso8859-10" | "latin6" | "latin-6" | "latin_6" => {
            Ok(("iso8859-10".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-15" | "iso8859-15" | "latin9" | "latin-9" | "latin_9" => {
            Ok(("iso8859-15".to_string(), TextEncodingKind::Latin1))
        }
        "mac-roman" | "macroman" | "mac_roman" => {
            Ok(("mac-roman".to_string(), TextEncodingKind::Latin1))
        }
        "ascii" | "us-ascii" => Ok(("ascii".to_string(), TextEncodingKind::Ascii)),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => {
            Ok(("latin-1".to_string(), TextEncodingKind::Latin1))
        }
        "utf-16" | "utf16" => Ok(("utf-16".to_string(), TextEncodingKind::Utf16)),
        "utf-16-le" | "utf-16le" | "utf16le" => {
            Ok(("utf-16-le".to_string(), TextEncodingKind::Utf16))
        }
        "utf-16-be" | "utf-16be" | "utf16be" => {
            Ok(("utf-16-be".to_string(), TextEncodingKind::Utf16))
        }
        "utf-32" | "utf32" => Ok(("utf-32".to_string(), TextEncodingKind::Utf32)),
        "utf-32-le" | "utf-32le" | "utf32le" => {
            Ok(("utf-32-le".to_string(), TextEncodingKind::Utf32))
        }
        "utf-32-be" | "utf-32be" | "utf32be" => {
            Ok(("utf-32-be".to_string(), TextEncodingKind::Utf32))
        }
        _ => Err(format!("unknown encoding: {encoding}")),
    }
}

fn text_encoding_kind(label: &str) -> TextEncodingKind {
    match label {
        "ascii" => TextEncodingKind::Ascii,
        "latin-1" => TextEncodingKind::Latin1,
        "cp1252" => TextEncodingKind::Latin1,
        "cp437" => TextEncodingKind::Latin1,
        "cp850" => TextEncodingKind::Latin1,
        "cp860" => TextEncodingKind::Latin1,
        "cp862" => TextEncodingKind::Latin1,
        "cp863" => TextEncodingKind::Latin1,
        "cp865" => TextEncodingKind::Latin1,
        "cp866" => TextEncodingKind::Latin1,
        "cp874" => TextEncodingKind::Latin1,
        "cp1250" => TextEncodingKind::Latin1,
        "cp1251" => TextEncodingKind::Latin1,
        "cp1253" => TextEncodingKind::Latin1,
        "cp1254" => TextEncodingKind::Latin1,
        "cp1255" => TextEncodingKind::Latin1,
        "cp1256" => TextEncodingKind::Latin1,
        "cp1257" => TextEncodingKind::Latin1,
        "koi8-r" => TextEncodingKind::Latin1,
        "koi8-u" => TextEncodingKind::Latin1,
        "iso8859-2" => TextEncodingKind::Latin1,
        "iso8859-3" => TextEncodingKind::Latin1,
        "iso8859-4" => TextEncodingKind::Latin1,
        "iso8859-5" => TextEncodingKind::Latin1,
        "iso8859-6" => TextEncodingKind::Latin1,
        "iso8859-7" => TextEncodingKind::Latin1,
        "iso8859-8" => TextEncodingKind::Latin1,
        "iso8859-10" => TextEncodingKind::Latin1,
        "iso8859-15" => TextEncodingKind::Latin1,
        "mac-roman" => TextEncodingKind::Latin1,
        "utf-8-sig" => TextEncodingKind::Utf8,
        _ if label.starts_with("utf-16") => TextEncodingKind::Utf16,
        _ if label.starts_with("utf-32") => TextEncodingKind::Utf32,
        _ => TextEncodingKind::Utf8,
    }
}

fn text_encoding_is_multibyte(kind: TextEncodingKind) -> bool {
    matches!(kind, TextEncodingKind::Utf16 | TextEncodingKind::Utf32)
}

fn text_encoding_is_variable(kind: TextEncodingKind) -> bool {
    matches!(
        kind,
        TextEncodingKind::Utf8 | TextEncodingKind::Utf16 | TextEncodingKind::Utf32
    )
}

fn split_fixed_pending(
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

fn split_text_pending_bytes(
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

fn decode_text_bytes(
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

fn decode_text_bytes_for_io(
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

fn decode_multibyte_text(
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

fn utf8_expected_len(byte: u8) -> usize {
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

fn utf8_pending_len(bytes: &[u8]) -> usize {
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

fn split_utf8_pending(handle: &mut MoltFileHandle, bytes: &mut Vec<u8>, at_eof: bool) {
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

fn wtf8_char_count(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let width = utf8_expected_len(bytes[idx]);
        idx = idx.saturating_add(width).min(bytes.len());
        count += 1;
    }
    count
}

fn wtf8_split_index(bytes: &[u8], limit: usize) -> usize {
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

fn pending_text_line_end(bytes: &[u8], newline: Option<&str>) -> Option<usize> {
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

fn validate_decode_error_handler(errors: &str) -> Result<(), String> {
    if matches!(
        errors,
        "strict" | "ignore" | "replace" | "backslashreplace" | "surrogateescape" | "surrogatepass"
    ) {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}

fn validate_encode_error_handler(errors: &str) -> Result<(), String> {
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

fn decode_error_byte(label: &str, byte: u8, pos: usize, message: &str) -> String {
    format!("'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}")
}

fn decode_error_range(label: &str, start: usize, end: usize, message: &str) -> String {
    format!("'{label}' codec can't decode bytes in position {start}-{end}: {message}")
}

const TEXT_COOKIE_VERSION: u8 = 2;
const TEXT_COOKIE_MAX_PENDING: usize = 4;
const TEXT_COOKIE_FIXED_LEN: usize = 16;

struct TextCookie {
    pos: u64,
    pending_byte: Option<u8>,
    pending_bytes: Vec<u8>,
    pending_text: Vec<u8>,
}

fn text_cookie_encode_bits(
    _py: &PyToken<'_>,
    pos: u64,
    pending_byte: Option<u8>,
    pending_bytes: &[u8],
    pending_text: &[u8],
) -> Result<u64, String> {
    if pos == 0 && pending_byte.is_none() && pending_bytes.is_empty() && pending_text.is_empty() {
        return Ok(MoltObject::from_int(0).bits());
    }
    if pending_bytes.len() > TEXT_COOKIE_MAX_PENDING {
        return Err("tell overflow".to_string());
    }
    let pending_text_len: u32 = pending_text
        .len()
        .try_into()
        .map_err(|_| "tell overflow".to_string())?;
    let mut bytes =
        Vec::with_capacity(TEXT_COOKIE_FIXED_LEN + pending_bytes.len() + pending_text.len());
    bytes.push(TEXT_COOKIE_VERSION);
    if let Some(byte) = pending_byte {
        bytes.push(1);
        bytes.push(byte);
    } else {
        bytes.push(0);
        bytes.push(0);
    }
    bytes.push(pending_bytes.len() as u8);
    bytes.extend_from_slice(pending_bytes);
    bytes.extend_from_slice(&pending_text_len.to_le_bytes());
    bytes.extend_from_slice(pending_text);
    bytes.extend_from_slice(&pos.to_le_bytes());
    let value = BigInt::from_bytes_le(Sign::Plus, &bytes);
    Ok(int_bits_from_bigint(_py, value))
}

fn text_cookie_decode_value(value: BigInt) -> Result<TextCookie, String> {
    if value.sign() == Sign::Minus {
        return Err("negative seek position".to_string());
    }
    if value.is_zero() {
        return Ok(TextCookie {
            pos: 0,
            pending_byte: None,
            pending_bytes: Vec::new(),
            pending_text: Vec::new(),
        });
    }
    let (_, mut bytes) = value.to_bytes_le();
    if bytes.len() < TEXT_COOKIE_FIXED_LEN {
        bytes.resize(TEXT_COOKIE_FIXED_LEN, 0);
    }
    if bytes[0] != TEXT_COOKIE_VERSION {
        return Err("invalid seek position".to_string());
    }
    let pending_flag = bytes[1] != 0;
    let pending_byte = if pending_flag { Some(bytes[2]) } else { None };
    let pending_len = bytes[3] as usize;
    if pending_len > TEXT_COOKIE_MAX_PENDING {
        return Err("invalid seek position".to_string());
    }
    let pending_bytes = if pending_len == 0 {
        Vec::new()
    } else {
        bytes[4..4 + pending_len].to_vec()
    };
    let text_len_offset = 4 + pending_len;
    if bytes.len() < text_len_offset + 4 {
        return Err("invalid seek position".to_string());
    }
    let pending_text_len = u32::from_le_bytes(
        bytes[text_len_offset..text_len_offset + 4]
            .try_into()
            .map_err(|_| "invalid seek position".to_string())?,
    ) as usize;
    let text_offset = text_len_offset + 4;
    let pos_offset = text_offset + pending_text_len;
    if bytes.len() < pos_offset + 8 {
        bytes.resize(pos_offset + 8, 0);
    }
    let pending_text = if pending_text_len == 0 {
        Vec::new()
    } else {
        bytes[text_offset..text_offset + pending_text_len].to_vec()
    };
    let pos = u64::from_le_bytes(
        bytes[pos_offset..pos_offset + 8]
            .try_into()
            .map_err(|_| "invalid seek position".to_string())?,
    );
    Ok(TextCookie {
        pos,
        pending_byte,
        pending_bytes,
        pending_text,
    })
}

fn translate_universal_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\r' => {
                if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                    idx += 2;
                } else {
                    idx += 1;
                }
                out.push(b'\n');
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    out
}

fn should_track_newlines(handle: &MoltFileHandle) -> bool {
    handle.text && matches!(handle.newline.as_deref(), None | Some(""))
}

fn record_newline(handle: &mut MoltFileHandle, kind: u8) {
    if (handle.newlines_mask & kind) != 0 {
        return;
    }
    if (handle.newlines_len as usize) < handle.newlines_seen.len() {
        handle.newlines_seen[handle.newlines_len as usize] = kind;
        handle.newlines_len = handle.newlines_len.saturating_add(1);
    }
    handle.newlines_mask |= kind;
}

fn update_newlines_from_bytes(handle: &mut MoltFileHandle, bytes: &[u8]) {
    if !should_track_newlines(handle) || bytes.is_empty() {
        return;
    }
    let mut idx = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if byte == b'\r' {
            if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                record_newline(handle, NEWLINE_KIND_CRLF);
                idx += 2;
                continue;
            }
            record_newline(handle, NEWLINE_KIND_CR);
            idx += 1;
            continue;
        }
        if byte == b'\n' {
            record_newline(handle, NEWLINE_KIND_LF);
        }
        idx += 1;
    }
}

fn update_newlines_from_chars(handle: &mut MoltFileHandle, chars: &[char]) {
    if !should_track_newlines(handle) || chars.is_empty() {
        return;
    }
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch == '\r' {
            if idx + 1 < chars.len() && chars[idx + 1] == '\n' {
                record_newline(handle, NEWLINE_KIND_CRLF);
                idx += 2;
                continue;
            }
            record_newline(handle, NEWLINE_KIND_CR);
            idx += 1;
            continue;
        }
        if ch == '\n' {
            record_newline(handle, NEWLINE_KIND_LF);
        }
        idx += 1;
    }
}

fn translate_write_newlines_bytes(bytes: &[u8], newline: Option<&str>) -> Vec<u8> {
    let target = match newline {
        None => {
            if cfg!(windows) {
                "\r\n"
            } else {
                "\n"
            }
        }
        Some("") | Some("\n") => "\n",
        Some(value) => value,
    };
    if target == "\n" {
        return bytes.to_vec();
    }
    let target_bytes = target.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        if byte == b'\n' {
            out.extend_from_slice(target_bytes);
        } else {
            out.push(byte);
        }
    }
    out
}

fn translate_write_newlines_str(text: &str, newline: Option<&str>) -> String {
    let target = match newline {
        None => {
            if cfg!(windows) {
                "\r\n"
            } else {
                "\n"
            }
        }
        Some("") | Some("\n") => "\n",
        Some(value) => value,
    };
    if target == "\n" {
        return text.to_string();
    }
    text.replace('\n', target)
}

unsafe fn text_backend_read(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let newline = handle.newline.as_deref();
    let limit = size.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut count = 0usize;
    let mut idx = text_backend.pos;
    let start = idx;
    let len = text_backend.data.len();
    while idx < len && count < limit {
        let ch = text_backend.data[idx];
        if newline.is_none() && ch == '\r' {
            if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                idx += 2;
            } else {
                idx += 1;
            }
            out.push('\n');
            count += 1;
            continue;
        }
        out.push(ch);
        idx += 1;
        count += 1;
    }
    text_backend.pos = idx;
    update_newlines_from_chars(handle, &text_backend.data[start..idx]);
    Ok(out)
}

unsafe fn text_backend_readline(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    limit: Option<usize>,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let newline = handle.newline.as_deref();
    let max_len = limit.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut count = 0usize;
    let mut idx = text_backend.pos;
    let start = idx;
    let len = text_backend.data.len();
    while idx < len && count < max_len {
        let ch = text_backend.data[idx];
        match newline {
            None => {
                if ch == '\n' {
                    out.push('\n');
                    idx += 1;
                    break;
                }
                if ch == '\r' {
                    if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    out.push('\n');
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
            }
            Some("") => {
                if ch == '\r' {
                    if count >= max_len {
                        break;
                    }
                    out.push('\r');
                    count += 1;
                    if count >= max_len {
                        idx += 1;
                        break;
                    }
                    if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                        out.push('\n');
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
            Some("\n") => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
            Some("\r") => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\r' {
                    break;
                }
            }
            Some("\r\n") => {
                if ch == '\r' && idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                    if count >= max_len {
                        break;
                    }
                    out.push('\r');
                    count += 1;
                    if count >= max_len {
                        idx += 1;
                        break;
                    }
                    out.push('\n');
                    idx += 2;
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
            }
            Some(_) => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
        }
    }
    text_backend.pos = idx;
    update_newlines_from_chars(handle, &text_backend.data[start..idx]);
    Ok(out)
}

unsafe fn text_backend_write(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    text: &str,
) -> Result<usize, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let translated = translate_write_newlines_str(text, handle.newline.as_deref());
    let chars: Vec<char> = translated.chars().collect();
    let pos = text_backend.pos;
    if pos > text_backend.data.len() {
        text_backend.data.resize(pos, '\0');
    }
    let end = pos.saturating_add(chars.len());
    if end > text_backend.data.len() {
        text_backend.data.resize(end, '\0');
    }
    text_backend.data[pos..end].copy_from_slice(&chars);
    text_backend.pos = end;
    Ok(chars.len())
}

unsafe fn text_backend_getvalue(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    Ok(text_backend.data.iter().collect())
}

unsafe fn text_backend_seek(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
    offset: i64,
    whence: i64,
) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let len = text_backend.data.len() as i64;
    let new_pos = match whence {
        0 => offset,
        1 => text_backend.pos as i64 + offset,
        2 => len + offset,
        _ => {
            return Err(raise_exception::<_>(_py, "ValueError", "invalid whence"));
        }
    };
    if new_pos < 0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "negative seek position",
        ));
    }
    text_backend.pos = new_pos as usize;
    Ok(new_pos)
}

unsafe fn text_backend_truncate(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
    size: usize,
) -> Result<(), u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    if size < text_backend.data.len() {
        text_backend.data.truncate(size);
    } else if size > text_backend.data.len() {
        text_backend.data.resize(size, '\0');
    }
    if text_backend.pos > text_backend.data.len() {
        text_backend.pos = text_backend.data.len();
    }
    Ok(())
}

unsafe fn text_backend_tell(_py: &PyToken<'_>, backend: &mut MoltFileBackend) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    Ok(text_backend.pos as i64)
}

pub(crate) fn file_handle_detached_message(handle: &MoltFileHandle) -> &'static str {
    if handle.text {
        "underlying buffer has been detached"
    } else {
        "raw stream has been detached"
    }
}

pub(crate) fn file_handle_is_closed(handle: &MoltFileHandle) -> bool {
    if handle.closed {
        return true;
    }
    Arc::clone(&handle.state).backend.lock().unwrap().is_none()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            if size == Some(0) {
                if handle.text {
                    let out_ptr = alloc_string(_py, b"");
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let out_ptr = alloc_bytes(_py, &[]);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let text = match text_backend_read(_py, handle, backend, size) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                let out_ptr = alloc_string(_py, text.as_bytes());
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text {
                let errors = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                if let Err(msg) = validate_decode_error_handler(errors.as_str()) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8").to_string();
                let mut encoding_kind = text_encoding_kind(encoding_label.as_str());
                let mut out_text: Vec<u8> = Vec::new();
                let mut remaining = size;
                if let Some(limit) = remaining {
                    if !handle.text_pending_text.is_empty() {
                        let pending_chars = wtf8_char_count(&handle.text_pending_text);
                        if pending_chars >= limit {
                            let split = wtf8_split_index(&handle.text_pending_text, limit);
                            out_text.extend_from_slice(&handle.text_pending_text[..split]);
                            let rest = handle.text_pending_text.split_off(split);
                            handle.text_pending_text = rest;
                            let out_ptr = alloc_string(_py, &out_text);
                            if out_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(out_ptr).bits();
                        }
                        out_text.extend_from_slice(&handle.text_pending_text);
                        remaining = Some(limit - pending_chars);
                        handle.text_pending_text.clear();
                    }
                } else if !handle.text_pending_text.is_empty() {
                    out_text.extend_from_slice(&handle.text_pending_text);
                    handle.text_pending_text.clear();
                }
                loop {
                    let mut buf = Vec::new();
                    let multibyte = text_encoding_is_multibyte(encoding_kind);
                    if !multibyte && let Some(pending) = handle.pending_byte.take() {
                        buf.push(pending);
                    }
                    let mut pending_utf8_needed = 0usize;
                    if !handle.text_pending_bytes.is_empty() {
                        let pending = std::mem::take(&mut handle.text_pending_bytes);
                        if matches!(encoding_kind, TextEncodingKind::Utf8) && !pending.is_empty() {
                            let expected = utf8_expected_len(pending[0]);
                            if expected > pending.len() {
                                pending_utf8_needed = expected - pending.len();
                            }
                        }
                        buf.extend_from_slice(&pending);
                    }
                    let mut byte_limit = remaining;
                    if text_encoding_is_variable(encoding_kind) {
                        byte_limit = None;
                    }
                    if let Some(rem) = byte_limit
                        && pending_utf8_needed > rem
                    {
                        byte_limit = Some(pending_utf8_needed);
                    }
                    let (mut more, at_eof) =
                        match file_read1_bytes(_py, handle, backend, byte_limit) {
                            Ok(val) => val,
                            Err(bits) => return bits,
                        };
                    buf.append(&mut more);
                    split_text_pending_bytes(handle, &mut buf, at_eof, encoding_kind);
                    let text_bytes = if multibyte {
                        match decode_multibyte_text(
                            _py,
                            handle,
                            &mut encoding_label,
                            &mut encoding_kind,
                            errors.as_str(),
                            &buf,
                            at_eof,
                        ) {
                            Ok(text_bytes) => text_bytes,
                            Err(bits) => return bits,
                        }
                    } else {
                        if handle.newline.is_none() && buf.last() == Some(&b'\r') && !at_eof {
                            handle.pending_byte = Some(b'\r');
                            buf.pop();
                        }
                        update_newlines_from_bytes(handle, &buf);
                        let bytes = if handle.newline.is_none() {
                            translate_universal_newlines(&buf)
                        } else {
                            buf
                        };
                        match decode_text_bytes_for_io(
                            _py,
                            handle,
                            encoding_label.as_str(),
                            errors.as_str(),
                            &bytes,
                        ) {
                            Ok((text_bytes, _label)) => text_bytes,
                            Err(bits) => return bits,
                        }
                    };
                    match remaining {
                        None => {
                            out_text.extend_from_slice(&text_bytes);
                            if at_eof {
                                break;
                            }
                        }
                        Some(rem) => {
                            let text_chars = wtf8_char_count(&text_bytes);
                            if text_chars <= rem {
                                out_text.extend_from_slice(&text_bytes);
                                let new_rem = rem.saturating_sub(text_chars);
                                remaining = Some(new_rem);
                                if new_rem == 0 || at_eof {
                                    break;
                                }
                            } else {
                                let split = wtf8_split_index(&text_bytes, rem);
                                out_text.extend_from_slice(&text_bytes[..split]);
                                handle.text_pending_text = text_bytes[split..].to_vec();
                                break;
                            }
                        }
                    }
                    if at_eof {
                        break;
                    }
                }
                let out_ptr = alloc_string(_py, &out_text);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let mut buf = Vec::new();
                let mut remaining = size;
                if let Some(pending) = handle.pending_byte.take() {
                    if let Some(rem) = remaining {
                        if rem == 0 {
                            handle.pending_byte = Some(pending);
                        } else {
                            buf.push(pending);
                            remaining = Some(rem.saturating_sub(1));
                        }
                    } else {
                        buf.push(pending);
                    }
                }
                let (mut more, _at_eof) = match buffered_read_bytes(_py, handle, backend, remaining)
                {
                    Ok(val) => val,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_read1(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            if size == Some(0) {
                if handle.text {
                    let out_ptr = alloc_string(_py, b"");
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let out_ptr = alloc_bytes(_py, &[]);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let text = match text_backend_read(_py, handle, backend, size) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                let out_ptr = alloc_string(_py, text.as_bytes());
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text {
                let errors = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                if let Err(msg) = validate_decode_error_handler(errors.as_str()) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8").to_string();
                let mut encoding_kind = text_encoding_kind(encoding_label.as_str());
                let mut out_text: Vec<u8> = Vec::new();
                let mut remaining = size;
                if let Some(limit) = remaining {
                    if !handle.text_pending_text.is_empty() {
                        let pending_chars = wtf8_char_count(&handle.text_pending_text);
                        if pending_chars >= limit {
                            let split = wtf8_split_index(&handle.text_pending_text, limit);
                            out_text.extend_from_slice(&handle.text_pending_text[..split]);
                            let rest = handle.text_pending_text.split_off(split);
                            handle.text_pending_text = rest;
                            let out_ptr = alloc_string(_py, &out_text);
                            if out_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(out_ptr).bits();
                        }
                        out_text.extend_from_slice(&handle.text_pending_text);
                        remaining = Some(limit - pending_chars);
                        handle.text_pending_text.clear();
                    }
                } else if !handle.text_pending_text.is_empty() {
                    out_text.extend_from_slice(&handle.text_pending_text);
                    handle.text_pending_text.clear();
                    let out_ptr = alloc_string(_py, &out_text);
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let mut buf = Vec::new();
                let multibyte = text_encoding_is_multibyte(encoding_kind);
                if !multibyte && let Some(pending) = handle.pending_byte.take() {
                    buf.push(pending);
                }
                let mut pending_utf8_needed = 0usize;
                if !handle.text_pending_bytes.is_empty() {
                    let pending = std::mem::take(&mut handle.text_pending_bytes);
                    if matches!(encoding_kind, TextEncodingKind::Utf8) && !pending.is_empty() {
                        let expected = utf8_expected_len(pending[0]);
                        if expected > pending.len() {
                            pending_utf8_needed = expected - pending.len();
                        }
                    }
                    buf.extend_from_slice(&pending);
                }
                let mut byte_limit = remaining;
                if text_encoding_is_variable(encoding_kind) {
                    byte_limit = None;
                }
                if let Some(rem) = byte_limit
                    && pending_utf8_needed > rem
                {
                    byte_limit = Some(pending_utf8_needed);
                }
                let (mut more, at_eof) = match file_read1_bytes(_py, handle, backend, byte_limit) {
                    Ok(more) => more,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                split_text_pending_bytes(handle, &mut buf, at_eof, encoding_kind);
                let text_bytes = if multibyte {
                    match decode_multibyte_text(
                        _py,
                        handle,
                        &mut encoding_label,
                        &mut encoding_kind,
                        errors.as_str(),
                        &buf,
                        at_eof,
                    ) {
                        Ok(text_bytes) => text_bytes,
                        Err(bits) => return bits,
                    }
                } else {
                    if matches!(handle.newline.as_deref(), None | Some(""))
                        && buf.last() == Some(&b'\r')
                        && !at_eof
                    {
                        handle.pending_byte = Some(b'\r');
                        buf.pop();
                    }
                    update_newlines_from_bytes(handle, &buf);
                    let bytes = if handle.newline.is_none() {
                        translate_universal_newlines(&buf)
                    } else {
                        buf
                    };
                    match decode_text_bytes_for_io(
                        _py,
                        handle,
                        encoding_label.as_str(),
                        errors.as_str(),
                        &bytes,
                    ) {
                        Ok((text_bytes, _label)) => text_bytes,
                        Err(bits) => return bits,
                    }
                };
                match remaining {
                    None => {
                        out_text.extend_from_slice(&text_bytes);
                    }
                    Some(rem) => {
                        let text_chars = wtf8_char_count(&text_bytes);
                        if text_chars <= rem {
                            out_text.extend_from_slice(&text_bytes);
                        } else {
                            let split = wtf8_split_index(&text_bytes, rem);
                            out_text.extend_from_slice(&text_bytes[..split]);
                            handle.text_pending_text = text_bytes[split..].to_vec();
                        }
                    }
                }
                let out_ptr = alloc_string(_py, &out_text);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let mut buf = Vec::new();
                let mut remaining = size;
                if let Some(pending) = handle.pending_byte.take() {
                    if let Some(rem) = remaining {
                        if rem == 0 {
                            handle.pending_byte = Some(pending);
                        } else {
                            buf.push(pending);
                            remaining = Some(rem.saturating_sub(1));
                        }
                    } else {
                        buf.push(pending);
                    }
                }
                let (mut more, _at_eof) = match file_read1_bytes(_py, handle, backend, remaining) {
                    Ok(more) => more,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readall(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let text = match text_backend_read(_py, handle, backend, None) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                let out_ptr = alloc_string(_py, text.as_bytes());
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text {
                let errors = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                if let Err(msg) = validate_decode_error_handler(errors.as_str()) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8").to_string();
                let mut encoding_kind = text_encoding_kind(encoding_label.as_str());
                let mut out_text = Vec::new();
                if !handle.text_pending_text.is_empty() {
                    out_text.extend_from_slice(&handle.text_pending_text);
                    handle.text_pending_text.clear();
                }
                let mut buf = Vec::new();
                let multibyte = text_encoding_is_multibyte(encoding_kind);
                if !multibyte && let Some(pending) = handle.pending_byte.take() {
                    buf.push(pending);
                }
                if !handle.text_pending_bytes.is_empty() {
                    let pending = std::mem::take(&mut handle.text_pending_bytes);
                    buf.extend_from_slice(&pending);
                }
                let (mut more, _at_eof) = match buffered_read_bytes(_py, handle, backend, None) {
                    Ok(val) => val,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                split_text_pending_bytes(handle, &mut buf, true, encoding_kind);
                let text_bytes = if multibyte {
                    match decode_multibyte_text(
                        _py,
                        handle,
                        &mut encoding_label,
                        &mut encoding_kind,
                        errors.as_str(),
                        &buf,
                        true,
                    ) {
                        Ok(text_bytes) => text_bytes,
                        Err(bits) => return bits,
                    }
                } else {
                    update_newlines_from_bytes(handle, &buf);
                    let bytes = if handle.newline.is_none() {
                        translate_universal_newlines(&buf)
                    } else {
                        buf
                    };
                    match decode_text_bytes_for_io(
                        _py,
                        handle,
                        encoding_label.as_str(),
                        errors.as_str(),
                        &bytes,
                    ) {
                        Ok((text_bytes, _label)) => text_bytes,
                        Err(bits) => return bits,
                    }
                };
                out_text.extend_from_slice(&text_bytes);
                let out_ptr = alloc_string(_py, &out_text);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let mut buf = Vec::new();
                if let Some(pending) = handle.pending_byte.take() {
                    buf.push(pending);
                }
                let (mut more, _at_eof) = match buffered_read_bytes(_py, handle, backend, None) {
                    Ok(val) => val,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[derive(Default)]
struct Utf8CountState {
    remaining: u8,
}

fn utf8_char_width(first: u8) -> u8 {
    if first < 0x80 {
        1
    } else if (0xC0..=0xDF).contains(&first) {
        2
    } else if (0xE0..=0xEF).contains(&first) {
        3
    } else if (0xF0..=0xF7).contains(&first) {
        4
    } else {
        1
    }
}

fn utf8_count_push(state: &mut Utf8CountState, byte: u8, count: &mut usize) {
    if state.remaining == 0 {
        let width = utf8_char_width(byte);
        if width <= 1 {
            *count += 1;
        } else {
            state.remaining = width - 1;
        }
        return;
    }
    state.remaining = state.remaining.saturating_sub(1);
    if state.remaining == 0 {
        *count += 1;
    }
}

fn push_text_byte(
    out: &mut Vec<u8>,
    byte: u8,
    kind: TextEncodingKind,
    limit: Option<usize>,
    count: &mut usize,
    utf8_state: &mut Utf8CountState,
) -> bool {
    out.push(byte);
    match kind {
        TextEncodingKind::Utf8 | TextEncodingKind::Utf16 | TextEncodingKind::Utf32 => {
            utf8_count_push(utf8_state, byte, count)
        }
        TextEncodingKind::Ascii | TextEncodingKind::Latin1 => {
            *count += 1;
        }
    }
    match limit {
        Some(limit) => *count >= limit,
        None => false,
    }
}

fn file_readline_bytes(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    newline: Option<&str>,
    text: bool,
    size: Option<usize>,
    encoding_kind: Option<TextEncodingKind>,
) -> Result<Vec<u8>, u64> {
    let mut out: Vec<u8> = Vec::new();
    let mut char_count = 0usize;
    let mut utf8_state = Utf8CountState::default();
    let text_kind = encoding_kind.unwrap_or(TextEncodingKind::Utf8);
    loop {
        if let Some(limit) = size {
            if text {
                if char_count >= limit {
                    break;
                }
            } else if out.len() >= limit {
                break;
            }
        }
        let Some(byte) = unsafe { handle_read_byte(_py, handle, backend) }? else {
            break;
        };
        if text {
            match newline {
                None => {
                    if byte == b'\n' {
                        record_newline(handle, NEWLINE_KIND_LF);
                        push_text_byte(
                            &mut out,
                            b'\n',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = unsafe { handle_read_byte(_py, handle, backend) }? {
                            if next == b'\n' {
                                record_newline(handle, NEWLINE_KIND_CRLF);
                            } else {
                                record_newline(handle, NEWLINE_KIND_CR);
                                handle.pending_byte = Some(next);
                            }
                        } else {
                            record_newline(handle, NEWLINE_KIND_CR);
                        }
                        push_text_byte(
                            &mut out,
                            b'\n',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
                Some("") => {
                    if byte == b'\n' {
                        record_newline(handle, NEWLINE_KIND_LF);
                        push_text_byte(
                            &mut out,
                            b'\n',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = unsafe { handle_read_byte(_py, handle, backend) }? {
                            if next == b'\n' {
                                record_newline(handle, NEWLINE_KIND_CRLF);
                                if push_text_byte(
                                    &mut out,
                                    b'\r',
                                    text_kind,
                                    size,
                                    &mut char_count,
                                    &mut utf8_state,
                                ) {
                                    handle.pending_byte = Some(next);
                                    break;
                                }
                                push_text_byte(
                                    &mut out,
                                    b'\n',
                                    text_kind,
                                    size,
                                    &mut char_count,
                                    &mut utf8_state,
                                );
                                break;
                            }
                            record_newline(handle, NEWLINE_KIND_CR);
                            handle.pending_byte = Some(next);
                        } else {
                            record_newline(handle, NEWLINE_KIND_CR);
                        }
                        push_text_byte(
                            &mut out,
                            b'\r',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
                Some("\n") => {
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                    if byte == b'\n' {
                        break;
                    }
                }
                Some("\r") => {
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                    if byte == b'\r' {
                        break;
                    }
                }
                Some("\r\n") => {
                    if byte == b'\r'
                        && let Some(next) = unsafe { handle_read_byte(_py, handle, backend) }?
                    {
                        if next == b'\n' {
                            if push_text_byte(
                                &mut out,
                                b'\r',
                                text_kind,
                                size,
                                &mut char_count,
                                &mut utf8_state,
                            ) {
                                handle.pending_byte = Some(next);
                                break;
                            }
                            push_text_byte(
                                &mut out,
                                b'\n',
                                text_kind,
                                size,
                                &mut char_count,
                                &mut utf8_state,
                            );
                            break;
                        }
                        handle.pending_byte = Some(next);
                    }
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
                Some(_) => {
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
            }
        } else {
            out.push(byte);
            if byte == b'\n' {
                break;
            }
            if let Some(limit) = size
                && out.len() >= limit
            {
                break;
            }
        }
    }
    Ok(out)
}

unsafe fn read_text_chunk_multibyte(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    encoding_label: &mut String,
    encoding_kind: &mut TextEncodingKind,
    errors: &str,
) -> Result<(Vec<u8>, bool), u64> {
    unsafe {
        let mut buf = Vec::new();
        if !handle.text_pending_bytes.is_empty() {
            let pending = std::mem::take(&mut handle.text_pending_bytes);
            buf.extend_from_slice(&pending);
        }
        let (mut more, at_eof) = file_read1_bytes(_py, handle, backend, None)?;
        buf.append(&mut more);
        split_text_pending_bytes(handle, &mut buf, at_eof, *encoding_kind);
        let text_bytes = decode_multibyte_text(
            _py,
            handle,
            encoding_label,
            encoding_kind,
            errors,
            &buf,
            at_eof,
        )?;
        Ok((text_bytes, at_eof))
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn read_line_multibyte(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    newline: Option<&str>,
    size: Option<usize>,
    encoding_label: &mut String,
    encoding_kind: &mut TextEncodingKind,
    errors: &str,
) -> Result<Vec<u8>, u64> {
    unsafe {
        let mut out: Vec<u8> = Vec::new();
        let mut remaining = size;
        if !handle.text_pending_text.is_empty() {
            let mut take_len = handle.text_pending_text.len();
            let mut stop = false;
            if let Some(boundary) = pending_text_line_end(&handle.text_pending_text, newline) {
                take_len = boundary;
                stop = true;
            }
            if let Some(limit) = remaining {
                let split = wtf8_split_index(&handle.text_pending_text, limit);
                if split < take_len {
                    take_len = split;
                    stop = true;
                }
            }
            out.extend_from_slice(&handle.text_pending_text[..take_len]);
            let rest = handle.text_pending_text.split_off(take_len);
            handle.text_pending_text = rest;
            if let Some(limit) = remaining {
                let taken = wtf8_char_count(&out);
                remaining = Some(limit.saturating_sub(taken));
            }
            if stop || remaining == Some(0) {
                return Ok(out);
            }
        }
        loop {
            let (chunk, at_eof) = read_text_chunk_multibyte(
                _py,
                handle,
                backend,
                encoding_label,
                encoding_kind,
                errors,
            )?;
            if chunk.is_empty() && at_eof {
                break;
            }
            let mut take_len = chunk.len();
            let mut stop = false;
            if let Some(boundary) = pending_text_line_end(&chunk, newline) {
                take_len = boundary;
                stop = true;
            }
            if let Some(limit) = remaining {
                let split = wtf8_split_index(&chunk, limit);
                if split < take_len {
                    take_len = split;
                    stop = true;
                }
            }
            out.extend_from_slice(&chunk[..take_len]);
            let rest = chunk[take_len..].to_vec();
            if let Some(limit) = remaining {
                let taken = wtf8_char_count(&chunk[..take_len]);
                remaining = Some(limit.saturating_sub(taken));
            }
            if stop {
                handle.text_pending_text = rest;
                break;
            }
            if !rest.is_empty() {
                handle.text_pending_text = rest;
                break;
            }
            if remaining == Some(0) {
                break;
            }
            if at_eof {
                break;
            }
        }
        Ok(out)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readline(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let text = match text_backend_readline(_py, handle, backend, size) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                let out_ptr = alloc_string(_py, text.as_bytes());
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            let mut pending_out: Vec<u8> = Vec::new();
            let mut remaining = size;
            if handle.text && !handle.text_pending_text.is_empty() {
                if let Some(0) = remaining {
                    let out_ptr = alloc_string(_py, b"");
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let newline = handle.newline.as_deref();
                let mut take_len = handle.text_pending_text.len();
                let mut stop = false;
                if let Some(boundary) = pending_text_line_end(&handle.text_pending_text, newline) {
                    take_len = boundary;
                    stop = true;
                }
                if let Some(limit) = remaining {
                    let split = wtf8_split_index(&handle.text_pending_text, limit);
                    if split < take_len {
                        take_len = split;
                        stop = true;
                    }
                }
                pending_out.extend_from_slice(&handle.text_pending_text[..take_len]);
                let rest = handle.text_pending_text.split_off(take_len);
                handle.text_pending_text = rest;
                if let Some(limit) = remaining {
                    let taken = wtf8_char_count(&pending_out);
                    remaining = Some(limit.saturating_sub(taken));
                }
                if stop || remaining == Some(0) {
                    let out_ptr = alloc_string(_py, &pending_out);
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            let text = handle.text;
            let newline_owned = if text {
                handle.newline.clone()
            } else {
                Some("\n".to_string())
            };
            let newline = newline_owned.as_deref();
            let mut encoding_label = if text {
                handle
                    .encoding
                    .clone()
                    .unwrap_or_else(|| "utf-8".to_string())
            } else {
                "utf-8".to_string()
            };
            let encoding_kind = if text {
                Some(text_encoding_kind(&encoding_label))
            } else {
                None
            };
            if text
                && let Some(kind_value) = encoding_kind
                && text_encoding_is_multibyte(kind_value)
            {
                let mut kind = kind_value;
                let errors_owned = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                let errors = errors_owned.as_str();
                if let Err(msg) = validate_decode_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let line = match read_line_multibyte(
                    _py,
                    handle,
                    backend,
                    newline,
                    remaining,
                    &mut encoding_label,
                    &mut kind,
                    errors,
                ) {
                    Ok(line) => line,
                    Err(bits) => return bits,
                };
                pending_out.extend_from_slice(&line);
                let out_ptr = alloc_string(_py, &pending_out);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            let bytes = match file_readline_bytes(
                _py,
                handle,
                backend,
                newline,
                text,
                remaining,
                encoding_kind,
            ) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return raise_exception::<_>(_py, "OSError", "read failed");
                }
            };
            if text {
                let errors = handle.errors.as_deref().unwrap_or("strict");
                if let Err(msg) = validate_decode_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let text_bytes =
                    match crate::object::ops::decode_bytes_text(&encoding_label, errors, &bytes) {
                        Ok((text_bytes, _label)) => text_bytes,
                        Err(crate::object::ops::DecodeTextError::UnknownEncoding(name)) => {
                            let msg = format!("unknown encoding: {name}");
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(name)) => {
                            let msg = format!("unknown error handler name '{name}'");
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::Failure(
                            DecodeFailure::Byte { pos, byte, message },
                            label,
                        )) => {
                            let msg = decode_error_byte(&label, byte, pos, message);
                            return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
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
                            return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::Failure(
                            DecodeFailure::UnknownErrorHandler(name),
                            _label,
                        )) => {
                            let msg = format!("unknown error handler name '{name}'");
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                    };
                pending_out.extend_from_slice(&text_bytes);
                let out_ptr = alloc_string(_py, &pending_out);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let out_ptr = alloc_bytes(_py, &bytes);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readlines(handle_bits: u64, hint_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let hint_obj = obj_from_bits(hint_bits);
            let hint = if hint_obj.is_none() {
                None
            } else {
                match to_i64(hint_obj) {
                    Some(val) if val <= 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, hint_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let mut lines: Vec<u64> = Vec::new();
                let mut total = 0usize;
                loop {
                    let text = match text_backend_readline(_py, handle, backend, None) {
                        Ok(text) => text,
                        Err(bits) => return bits,
                    };
                    if text.is_empty() {
                        break;
                    }
                    total = total.saturating_add(text.chars().count());
                    let line_ptr = alloc_string(_py, text.as_bytes());
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                    if let Some(limit) = hint
                        && total >= limit
                    {
                        break;
                    }
                }
                let list_ptr = alloc_list(_py, lines.as_slice());
                if list_ptr.is_null() {
                    for bits in lines {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                for bits in lines {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::from_ptr(list_ptr).bits();
            }
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            let text = handle.text;
            let newline_owned = if text {
                handle.newline.clone()
            } else {
                Some("\n".to_string())
            };
            let newline = newline_owned.as_deref();
            let mut encoding_label = if text {
                handle.encoding.as_deref().unwrap_or("utf-8").to_string()
            } else {
                "utf-8".to_string()
            };
            let mut encoding_kind = if text {
                Some(text_encoding_kind(&encoding_label))
            } else {
                None
            };
            let mut lines: Vec<u64> = Vec::new();
            let mut total = 0usize;
            loop {
                if text {
                    if let Some(kind_value) = encoding_kind
                        && text_encoding_is_multibyte(kind_value)
                    {
                        let mut kind = kind_value;
                        let errors_owned = handle
                            .errors
                            .clone()
                            .unwrap_or_else(|| "strict".to_string());
                        let errors = errors_owned.as_str();
                        if let Err(msg) = validate_decode_error_handler(errors) {
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        let line = match read_line_multibyte(
                            _py,
                            handle,
                            backend,
                            newline,
                            None,
                            &mut encoding_label,
                            &mut kind,
                            errors,
                        ) {
                            Ok(line) => line,
                            Err(bits) => return bits,
                        };
                        encoding_kind = Some(kind);
                        if line.is_empty() {
                            break;
                        }
                        let char_count = match std::str::from_utf8(&line) {
                            Ok(text) => text.chars().count(),
                            Err(_) => line.len(),
                        };
                        let line_ptr = alloc_string(_py, &line);
                        if line_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        total = total.saturating_add(char_count);
                        lines.push(MoltObject::from_ptr(line_ptr).bits());
                        if let Some(limit) = hint
                            && total >= limit
                        {
                            break;
                        }
                        continue;
                    }
                    let mut pending_out: Vec<u8> = Vec::new();
                    let mut line_complete = false;
                    if !handle.text_pending_text.is_empty() {
                        if let Some(boundary) =
                            pending_text_line_end(&handle.text_pending_text, newline)
                        {
                            pending_out.extend_from_slice(&handle.text_pending_text[..boundary]);
                            let rest = handle.text_pending_text.split_off(boundary);
                            handle.text_pending_text = rest;
                            line_complete = true;
                        } else {
                            pending_out.extend_from_slice(&handle.text_pending_text);
                            handle.text_pending_text.clear();
                        }
                    }
                    if !line_complete {
                        let bytes = match file_readline_bytes(
                            _py,
                            handle,
                            backend,
                            newline,
                            text,
                            None,
                            encoding_kind,
                        ) {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                return raise_exception::<_>(_py, "OSError", "read failed");
                            }
                        };
                        if bytes.is_empty() && pending_out.is_empty() {
                            break;
                        }
                        let errors = handle.errors.as_deref().unwrap_or("strict");
                        if let Err(msg) = validate_decode_error_handler(errors) {
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                        let text_bytes = match crate::object::ops::decode_bytes_text(
                            encoding_label,
                            errors,
                            &bytes,
                        ) {
                            Ok((text_bytes, _label)) => text_bytes,
                            Err(crate::object::ops::DecodeTextError::UnknownEncoding(name)) => {
                                let msg = format!("unknown encoding: {name}");
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(name)) => {
                                let msg = format!("unknown error handler name '{name}'");
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::Failure(
                                DecodeFailure::Byte { pos, byte, message },
                                label,
                            )) => {
                                let msg = decode_error_byte(&label, byte, pos, message);
                                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
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
                                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::Failure(
                                DecodeFailure::UnknownErrorHandler(name),
                                _label,
                            )) => {
                                let msg = format!("unknown error handler name '{name}'");
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                        };
                        pending_out.extend_from_slice(&text_bytes);
                    }
                    let char_count = match std::str::from_utf8(&pending_out) {
                        Ok(text) => text.chars().count(),
                        Err(_) => pending_out.len(),
                    };
                    let line_ptr = alloc_string(_py, &pending_out);
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    total = total.saturating_add(char_count);
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                } else {
                    let bytes = match file_readline_bytes(
                        _py,
                        handle,
                        backend,
                        newline,
                        text,
                        None,
                        encoding_kind,
                    ) {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return raise_exception::<_>(_py, "OSError", "read failed");
                        }
                    };
                    if bytes.is_empty() {
                        break;
                    }
                    let line_ptr = alloc_bytes(_py, &bytes);
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    total = total.saturating_add(bytes.len());
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                }
                if let Some(limit) = hint
                    && total >= limit
                {
                    break;
                }
            }
            let list_ptr = alloc_list(_py, lines.as_slice());
            if list_ptr.is_null() {
                for bits in lines {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            for bits in lines {
                dec_ref_bits(_py, bits);
            }
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

fn file_readinto_impl(_py: &PyToken<'_>, handle_bits: u64, buffer_bits: u64, name: &str) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(_py, handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>(_py, "UnsupportedOperation", "read");
        }
        if handle.text {
            let msg = format!("{name}() unsupported for text files");
            return raise_exception::<_>(_py, "OSError", &msg);
        }
        let mut export = BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if molt_buffer_export(buffer_bits, &mut export) != 0 || export.readonly != 0 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if export.itemsize != 1 || export.stride != 1 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let len = export.len as usize;
        if len == 0 {
            return MoltObject::from_int(0).bits();
        }
        let buf = std::slice::from_raw_parts_mut(export.ptr as *mut u8, len);
        let backend_state = Arc::clone(&handle.state);
        let mut guard = backend_state.backend.lock().unwrap();
        let Some(backend) = guard.as_mut() else {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        };
        let n = match buffered_read_into(_py, handle, backend, buf) {
            Ok(n) => n,
            Err(bits) => return bits,
        };
        MoltObject::from_int(n as i64).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readinto(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readinto1(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto1")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_peek(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            if handle.text {
                return raise_exception::<_>(_py, "UnsupportedOperation", "peek");
            }
            if handle.buffer_size <= 0 {
                return raise_exception::<_>(_py, "UnsupportedOperation", "peek");
            }
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            if unread_bytes(handle) == 0 {
                let buf_size = handle.buffer_size as usize;
                handle.read_buf.resize(buf_size, 0);
                let n =
                    match backend_read_bytes(_py, handle.mem_bits, backend, &mut handle.read_buf) {
                        Ok(n) => n,
                        Err(bits) => return bits,
                    };
                handle.read_buf.truncate(n);
                handle.read_pos = 0;
            }
            let available = unread_bytes(handle);
            let take = size.unwrap_or(available).min(available);
            let out = if take == 0 {
                Vec::new()
            } else {
                let start = handle.read_pos;
                handle.read_buf[start..start + take].to_vec()
            };
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_getvalue(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            if handle.class_bits != builtins.bytes_io && handle.class_bits != builtins.string_io {
                return raise_exception::<_>(_py, "UnsupportedOperation", "getvalue");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            if handle.class_bits == builtins.string_io {
                let text = match text_backend_getvalue(_py, backend) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            } else {
                let data = match memory_backend_vec_ref(_py, handle) {
                    Ok(data) => data,
                    Err(bits) => return bits,
                };
                let ptr = alloc_bytes(_py, data);
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_getbuffer(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            if handle.class_bits != builtins.bytes_io {
                return raise_exception::<_>(_py, "UnsupportedOperation", "getbuffer");
            }
            let mem_bits = handle.mem_bits;
            if mem_bits == 0 {
                return raise_exception::<_>(_py, "RuntimeError", "memory backend missing");
            }
            molt_memoryview_new(mem_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_detach(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            if handle.detached {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    file_handle_detached_message(handle),
                );
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            if handle.class_bits == builtins.bytes_io || handle.class_bits == builtins.string_io {
                return raise_exception::<_>(
                    _py,
                    "UnsupportedOperation",
                    "detach is unsupported for in-memory streams",
                );
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            if handle.text {
                let buffer_bits = handle.buffer_bits;
                if buffer_bits == 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                let buffer_obj = obj_from_bits(buffer_bits);
                if let Some(buffer_ptr) = buffer_obj.as_ptr()
                    && object_type_id(buffer_ptr) == TYPE_ID_FILE_HANDLE
                {
                    let buffer_handle_ptr = file_handle_ptr(buffer_ptr);
                    if !buffer_handle_ptr.is_null() {
                        let buffer_handle = &mut *buffer_handle_ptr;
                        let mut prefix = Vec::new();
                        if let Some(pending) = handle.pending_byte.take() {
                            prefix.push(pending);
                        }
                        if !handle.text_pending_bytes.is_empty() {
                            prefix.extend_from_slice(&handle.text_pending_bytes);
                            handle.text_pending_bytes.clear();
                        }
                        handle.text_pending_text.clear();
                        buffer_handle.pending_byte = None;
                        buffer_handle.read_buf = std::mem::take(&mut handle.read_buf);
                        buffer_handle.read_pos = handle.read_pos;
                        handle.read_pos = 0;
                        prepend_read_buffer(buffer_handle, &prefix);
                    }
                }
                handle.buffer_bits = MoltObject::none().bits();
                handle.detached = true;
                handle.owns_fd = false;
                return buffer_bits;
            }
            let raw_ptr = alloc_file_handle_with_state(
                _py,
                Arc::clone(&handle.state),
                handle.readable,
                handle.writable,
                false,
                handle.closefd,
                handle.owns_fd,
                handle.line_buffering,
                handle.write_through,
                handle.buffer_size,
                handle.class_bits,
                handle.name_bits,
                handle.mode.clone(),
                None,
                None,
                None,
                None,
                0,
                handle.mem_bits,
            );
            if raw_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let raw_handle_ptr = file_handle_ptr(raw_ptr);
            if !raw_handle_ptr.is_null() {
                let raw_handle = &mut *raw_handle_ptr;
                let mut prefix = Vec::new();
                if let Some(pending) = handle.pending_byte.take() {
                    prefix.push(pending);
                }
                if !handle.text_pending_bytes.is_empty() {
                    prefix.extend_from_slice(&handle.text_pending_bytes);
                    handle.text_pending_bytes.clear();
                }
                handle.text_pending_text.clear();
                raw_handle.pending_byte = None;
                raw_handle.read_buf = std::mem::take(&mut handle.read_buf);
                raw_handle.read_pos = handle.read_pos;
                handle.read_pos = 0;
                prepend_read_buffer(raw_handle, &prefix);
            }
            handle.detached = true;
            handle.owns_fd = false;
            MoltObject::from_ptr(raw_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_reconfigure(
    handle_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    line_buffering_bits: u64,
    write_through_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.text {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not a text file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            if let Err(bits) = backend_flush(_py, backend) {
                return bits;
            }
            drop(guard);

            let missing = missing_bits(_py);
            let mut new_encoding = handle.encoding.clone();
            let mut new_encoding_original = handle.encoding_original.clone();
            if encoding_bits != missing
                && let Some(encoding) = reconfigure_arg_type(_py, encoding_bits, "encoding")
            {
                let (label, _kind) = match normalize_text_encoding(&encoding) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
                };
                new_encoding = Some(label.clone());
                new_encoding_original = Some(label);
            }
            let mut new_errors = handle.errors.clone();
            if errors_bits != missing
                && let Some(errors) = reconfigure_arg_type(_py, errors_bits, "errors")
            {
                new_errors = Some(errors);
            }
            let mut new_newline = handle.newline.clone();
            if newline_bits != missing {
                new_newline = reconfigure_arg_newline(_py, newline_bits);
            }
            let mut new_line_buffering = handle.line_buffering;
            if line_buffering_bits != missing {
                let obj = obj_from_bits(line_buffering_bits);
                if !obj.is_none() {
                    let val = match to_i64(obj) {
                        Some(val) => val != 0,
                        None => {
                            let type_name =
                                class_name_for_error(type_of_bits(_py, line_buffering_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    new_line_buffering = val;
                }
            }
            let mut new_write_through = handle.write_through;
            if write_through_bits != missing {
                let obj = obj_from_bits(write_through_bits);
                if !obj.is_none() {
                    let val = match to_i64(obj) {
                        Some(val) => val != 0,
                        None => {
                            let type_name =
                                class_name_for_error(type_of_bits(_py, write_through_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    new_write_through = val;
                }
            }

            handle.encoding = new_encoding;
            handle.errors = new_errors;
            if encoding_bits != missing {
                handle.encoding_original = new_encoding_original;
                handle.text_bom_seen = false;
                handle.text_bom_written = false;
            }
            if encoding_bits != missing || errors_bits != missing {
                handle.text_pending_bytes.clear();
                handle.pending_byte = None;
                handle.text_pending_text.clear();
            }
            if newline_bits != missing {
                handle.pending_byte = None;
                handle.text_pending_bytes.clear();
                handle.text_pending_text.clear();
                handle.newlines_mask = 0;
                handle.newlines_len = 0;
            }
            handle.newline = new_newline;
            handle.line_buffering = new_line_buffering;
            handle.write_through = new_write_through;
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_seek(handle_bits: u64, offset_bits: u64, whence_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let whence = match to_i64(obj_from_bits(whence_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(_py, whence_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let offset = match to_i64(obj_from_bits(offset_bits)) {
                        Some(val) => val,
                        None => {
                            let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    let pos = match text_backend_seek(_py, backend, offset, whence) {
                        Ok(pos) => pos,
                        Err(bits) => return bits,
                    };
                    handle.pending_byte = None;
                    handle.text_pending_bytes.clear();
                    handle.text_pending_text.clear();
                    clear_read_buffer(handle);
                    clear_write_buffer(handle);
                    return MoltObject::from_int(pos).bits();
                }
                if whence == 0 {
                    let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    let Some(value) = index_bigint_from_obj(_py, offset_bits, &msg) else {
                        return MoltObject::none().bits();
                    };
                    let cookie = match text_cookie_decode_value(value) {
                        Ok(val) => val,
                        Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
                    };
                    let pos = match backend_seek(
                        _py,
                        handle,
                        backend,
                        std::io::SeekFrom::Start(cookie.pos),
                    ) {
                        Ok(pos) => pos,
                        Err(bits) => return bits,
                    };
                    handle.pending_byte = cookie.pending_byte;
                    handle.text_pending_bytes = cookie.pending_bytes;
                    handle.text_pending_text = cookie.pending_text;
                    clear_read_buffer(handle);
                    clear_write_buffer(handle);
                    let at_start = cookie.pos == 0
                        && handle.pending_byte.is_none()
                        && handle.text_pending_bytes.is_empty()
                        && handle.text_pending_text.is_empty();
                    if let Some(original) = handle.encoding_original.as_ref() {
                        if (original == "utf-16" || original == "utf-32") && at_start {
                            handle.encoding = Some(original.clone());
                        }
                        if original == "utf-8-sig" {
                            handle.text_bom_seen = !at_start;
                            handle.text_bom_written = !at_start;
                        }
                    }
                    return match text_cookie_encode_bits(
                        _py,
                        pos,
                        handle.pending_byte,
                        &handle.text_pending_bytes,
                        &handle.text_pending_text,
                    ) {
                        Ok(bits) => bits,
                        Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
                    };
                }
            }
            let offset = match to_i64(obj_from_bits(offset_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if handle.text && offset != 0 && (whence == 1 || whence == 2) {
                let msg = if whence == 1 {
                    "can't do nonzero cur-relative seeks"
                } else {
                    "can't do nonzero end-relative seeks"
                };
                return raise_exception::<_>(_py, "UnsupportedOperation", msg);
            }
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            let mut seek_offset = offset;
            if whence == 1 {
                let unread = unread_bytes(handle) as i64;
                let pending = if handle.pending_byte.is_some() { 1 } else { 0 };
                let pending = pending + handle.text_pending_bytes.len() as i64;
                seek_offset = seek_offset.saturating_sub(unread + pending);
            }
            let from = match whence {
                0 => {
                    if seek_offset < 0 {
                        let msg = format!("negative seek position {seek_offset}");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    std::io::SeekFrom::Start(seek_offset as u64)
                }
                1 => std::io::SeekFrom::Current(seek_offset),
                2 => std::io::SeekFrom::End(seek_offset),
                _ => return raise_exception::<_>(_py, "ValueError", "invalid whence"),
            };
            let pos = match backend_seek(_py, handle, backend, from) {
                Ok(pos) => pos,
                Err(bits) => return bits,
            };
            handle.pending_byte = None;
            handle.text_pending_bytes.clear();
            handle.text_pending_text.clear();
            clear_read_buffer(handle);
            if handle.text {
                match text_cookie_encode_bits(_py, pos, None, &[], &[]) {
                    Ok(bits) => bits,
                    Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
                }
            } else {
                MoltObject::from_int(pos as i64).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_tell(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let pos = match text_backend_tell(_py, backend) {
                    Ok(pos) => pos,
                    Err(bits) => return bits,
                };
                return MoltObject::from_int(pos).bits();
            }
            let pos = match backend_tell(_py, backend) {
                Ok(pos) => pos,
                Err(bits) => return bits,
            } as i64;
            let unread = unread_bytes(handle) as i64;
            let buffered_write = handle.write_buf.len() as i64;
            let logical = pos - unread + buffered_write;
            if handle.text {
                if logical < 0 {
                    return raise_exception::<_>(_py, "OSError", "tell failed");
                }
                match text_cookie_encode_bits(
                    _py,
                    logical as u64,
                    handle.pending_byte,
                    &handle.text_pending_bytes,
                    &handle.text_pending_text,
                ) {
                    Ok(bits) => bits,
                    Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
                }
            } else {
                MoltObject::from_int(logical).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_fileno(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_ref() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            match backend {
                MoltFileBackend::File(file) => {
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        MoltObject::from_int(file.as_raw_fd() as i64).bits()
                    }
                    #[cfg(windows)]
                    {
                        let fd_guard = backend_state.crt_fd.lock().unwrap();
                        if let Some(fd) = *fd_guard {
                            MoltObject::from_int(fd).bits()
                        } else {
                            raise_exception::<_>(_py, "UnsupportedOperation", "fileno")
                        }
                    }
                    #[cfg(not(any(unix, windows)))]
                    {
                        raise_exception::<_>(
                            _py,
                            "OSError",
                            "fileno is unsupported on this platform",
                        )
                    }
                }
                MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => {
                    raise_exception::<_>(_py, "UnsupportedOperation", "fileno")
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_truncate(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "truncate");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let size = if obj_from_bits(size_bits).is_none() {
                    match text_backend_tell(_py, backend) {
                        Ok(pos) => pos as usize,
                        Err(bits) => return bits,
                    }
                } else {
                    let val = match to_i64(obj_from_bits(size_bits)) {
                        Some(val) => val,
                        None => {
                            let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    if val < 0 {
                        return raise_exception::<_>(_py, "OSError", "Invalid argument");
                    }
                    val as usize
                };
                if let Err(bits) = text_backend_truncate(_py, backend, size) {
                    return bits;
                }
                return MoltObject::from_int(size as i64).bits();
            }
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            let size = if obj_from_bits(size_bits).is_none() {
                let pos = match backend_tell(_py, backend) {
                    Ok(pos) => pos as i64,
                    Err(bits) => return bits,
                };
                let unread = unread_bytes(handle) as i64;
                let buffered_write = handle.write_buf.len() as i64;
                let logical = pos - unread + buffered_write;
                if logical < 0 {
                    return raise_exception::<_>(_py, "OSError", "Invalid argument");
                }
                logical as u64
            } else {
                let val = match to_i64(obj_from_bits(size_bits)) {
                    Some(val) => val,
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if val < 0 {
                    return raise_exception::<_>(_py, "OSError", "Invalid argument");
                }
                val as u64
            };
            if let Err(bits) = backend_truncate(_py, handle, backend, size) {
                return bits;
            }
            clear_read_buffer(handle);
            MoltObject::from_int(size as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            MoltObject::from_bool(handle.readable).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_writable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            MoltObject::from_bool(handle.writable).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_seekable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let seekable = match backend {
                MoltFileBackend::File(file) => file.stream_position().is_ok(),
                MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => true,
            };
            MoltObject::from_bool(seekable).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_isatty(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_ref() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            match backend {
                MoltFileBackend::File(file) => {
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        let isatty = libc::isatty(file.as_raw_fd()) == 1;
                        MoltObject::from_bool(isatty).bits()
                    }
                    #[cfg(windows)]
                    {
                        let fd_guard = backend_state.crt_fd.lock().unwrap();
                        if let Some(fd) = *fd_guard {
                            let isatty = unsafe { libc::isatty(fd as libc::c_int) == 1 };
                            return MoltObject::from_bool(isatty).bits();
                        }
                        use std::os::windows::io::AsRawHandle;
                        let handle = file.as_raw_handle();
                        let isatty = windows_handle_isatty(handle as *mut std::ffi::c_void);
                        MoltObject::from_bool(isatty).bits()
                    }
                    #[cfg(not(any(unix, windows)))]
                    {
                        let _ = file;
                        MoltObject::from_bool(false).bits()
                    }
                }
                MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => {
                    MoltObject::from_bool(false).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_iter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
        }
        inc_ref_bits(_py, handle_bits);
        handle_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_next(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_bits = molt_file_readline(handle_bits, MoltObject::from_int(-1).bits());
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let line_obj = obj_from_bits(line_bits);
        let empty = if let Some(ptr) = line_obj.as_ptr() {
            unsafe {
                match object_type_id(ptr) {
                    TYPE_ID_STRING => string_len(ptr) == 0,
                    TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr) == 0,
                    _ => false,
                }
            }
        } else {
            false
        };
        if empty {
            dec_ref_bits(_py, line_bits);
            return raise_exception::<_>(_py, "StopIteration", "");
        }
        line_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_enter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            file_handle_enter(_py, ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_exit(handle_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            file_handle_exit(_py, ptr, exc_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_exit_method(
    handle_bits: u64,
    _exc_type_bits: u64,
    exc_bits: u64,
    _tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { molt_file_exit(handle_bits, exc_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let data_obj = obj_from_bits(data_bits);
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let text = match string_obj_to_owned(data_obj) {
                    Some(text) => text,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "write expects str for text mode",
                        );
                    }
                };
                let written = match text_backend_write(_py, handle, backend, &text) {
                    Ok(count) => count,
                    Err(bits) => return bits,
                };
                return MoltObject::from_int(written as i64).bits();
            }
            if unread_bytes(handle) > 0
                && let Err(bits) = rewind_read_buffer(_py, handle, backend)
            {
                return bits;
            }
            let (bytes, written_len, flush_newline): (Vec<u8>, usize, bool) = if handle.text {
                let Some(data_ptr) = data_obj.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects str for text mode",
                    );
                };
                if object_type_id(data_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects str for text mode",
                    );
                }
                let raw = std::slice::from_raw_parts(string_bytes(data_ptr), string_len(data_ptr));
                let errors = handle.errors.as_deref().unwrap_or("strict");
                let newline = handle.newline.as_deref();
                if let Err(msg) = validate_encode_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let translated = translate_write_newlines_bytes(raw, newline);
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                let mut mark_bom_written = false;
                if encoding_label == "utf-8-sig" {
                    if handle.text_bom_written {
                        encoding_label = "utf-8";
                    } else {
                        mark_bom_written = true;
                    }
                }
                let bytes = match crate::object::ops::encode_string_with_errors(
                    &translated,
                    encoding_label,
                    Some(errors),
                ) {
                    Ok(bytes) => bytes,
                    Err(crate::object::ops::EncodeError::UnknownEncoding(name)) => {
                        let msg = format!("unknown encoding: {name}");
                        return raise_exception::<_>(_py, "LookupError", &msg);
                    }
                    Err(crate::object::ops::EncodeError::UnknownErrorHandler(name)) => {
                        let msg = format!("unknown error handler name '{name}'");
                        return raise_exception::<_>(_py, "LookupError", &msg);
                    }
                    Err(crate::object::ops::EncodeError::InvalidChar {
                        encoding,
                        code,
                        pos,
                        limit,
                    }) => {
                        let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                        return raise_unicode_encode_error::<_>(
                            _py,
                            encoding,
                            data_obj.bits(),
                            pos,
                            pos + 1,
                            &reason,
                        );
                    }
                };
                if mark_bom_written {
                    handle.text_bom_written = true;
                }
                let written_len = crate::object::ops_string::utf8_codepoint_count_cached(
                    _py,
                    raw,
                    Some(data_ptr as usize),
                ) as usize;
                let flush_newline = translated.contains(&b'\n');
                (bytes, written_len, flush_newline)
            } else {
                let Some(data_ptr) = data_obj.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects bytes or bytearray",
                    );
                };
                let type_id = object_type_id(data_ptr);
                if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects bytes or bytearray",
                    );
                }
                let len = bytes_len(data_ptr);
                let raw = std::slice::from_raw_parts(bytes_data(data_ptr), len);
                (raw.to_vec(), len, raw.contains(&b'\n'))
            };
            let should_flush = handle.write_through || (handle.line_buffering && flush_newline);
            if handle.buffer_size == 0 {
                let mut written = 0usize;
                while written < bytes.len() {
                    let n =
                        match backend_write_bytes(_py, handle.mem_bits, backend, &bytes[written..])
                        {
                            Ok(n) => n,
                            Err(bits) => return bits,
                        };
                    if n == 0 {
                        return raise_exception::<_>(_py, "OSError", "write failed");
                    }
                    written += n;
                }
                if should_flush && let Err(bits) = backend_flush(_py, backend) {
                    return bits;
                }
            } else {
                handle.write_buf.extend_from_slice(&bytes);
                let need_flush =
                    should_flush || handle.write_buf.len() >= handle.buffer_size as usize;
                if need_flush && let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            MoltObject::from_int(written_len as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_writelines(handle_bits: u64, lines_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
        }
        let iter_bits = molt_iter(lines_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "writelines() argument must be iterable",
            );
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let line_bits = elems[0];
                let _ = molt_file_write(handle_bits, line_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_flush(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if let MoltFileBackend::Text(_) = backend {
                return MoltObject::none().bits();
            }
            if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                return bits;
            }
            if let Err(bits) = backend_flush(_py, backend) {
                return bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
        }
        let flush_result = unsafe {
            if let Some(handle_ptr) = file_handle_ptr(ptr).as_mut() {
                let handle = &mut *handle_ptr;
                let backend_state = Arc::clone(&handle.state);
                let mut guard = backend_state.backend.lock().unwrap();
                if let Some(backend) = guard.as_mut() {
                    flush_write_buffer(_py, handle, backend)
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        };
        if let Err(bits) = flush_result {
            return bits;
        }

        // ── VFS writeback ────────────────────────────────────────────
        // If this handle was opened for writing on a VFS mount, read
        // the final bytearray content and flush it to the VFS backend.
        unsafe {
            if let Some(handle_ptr) = file_handle_ptr(ptr).as_ref() {
                let handle = &*handle_ptr;
                if let Some((vfs_backend, vfs_path)) = vfs_writeback_take(&handle.state) {
                    // Read the bytearray content that the runtime wrote into.
                    let mem = handle.mem_bits;
                    if mem != 0 {
                        if let Some(mem_ptr) = obj_from_bits(mem).as_ptr() {
                            if object_type_id(mem_ptr) == TYPE_ID_BYTEARRAY {
                                let vec_ptr = bytearray_vec_ptr(mem_ptr);
                                if !vec_ptr.is_null() {
                                    let data = &*vec_ptr;
                                    let _ = vfs_backend.open_write(&vfs_path, data);
                                }
                            }
                        }
                    }
                    // For text-mode handles, the buffer layer holds the
                    // bytearray, not the outer handle. Walk through the
                    // buffer handle's mem_bits instead.
                    if mem == 0 && handle.buffer_bits != 0 {
                        if let Some(buf_ptr) = obj_from_bits(handle.buffer_bits).as_ptr() {
                            if object_type_id(buf_ptr) == TYPE_ID_FILE_HANDLE {
                                let buf_handle = &*file_handle_ptr(buf_ptr);
                                let buf_mem = buf_handle.mem_bits;
                                if buf_mem != 0 {
                                    if let Some(mem_ptr) = obj_from_bits(buf_mem).as_ptr() {
                                        if object_type_id(mem_ptr) == TYPE_ID_BYTEARRAY {
                                            let vec_ptr = bytearray_vec_ptr(mem_ptr);
                                            if !vec_ptr.is_null() {
                                                let data = &*vec_ptr;
                                                let _ = vfs_backend.open_write(&vfs_path, data);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        file_handle_close_ptr(ptr);
        MoltObject::none().bits()
    })
}
