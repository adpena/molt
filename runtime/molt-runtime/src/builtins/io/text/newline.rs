use super::*;
use num_bigint::{BigInt, Sign};
use num_traits::Zero;

pub(in crate::builtins::io) const TEXT_COOKIE_VERSION: u8 = 2;
pub(in crate::builtins::io) const TEXT_COOKIE_MAX_PENDING: usize = 4;
pub(in crate::builtins::io) const TEXT_COOKIE_FIXED_LEN: usize = 16;

pub(in crate::builtins::io) struct TextCookie {
    pub(in crate::builtins::io) pos: u64,
    pub(in crate::builtins::io) pending_byte: Option<u8>,
    pub(in crate::builtins::io) pending_bytes: Vec<u8>,
    pub(in crate::builtins::io) pending_text: Vec<u8>,
}

pub(in crate::builtins::io) fn text_cookie_encode_bits(
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

pub(in crate::builtins::io) fn text_cookie_decode_value(
    value: BigInt,
) -> Result<TextCookie, String> {
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

pub(in crate::builtins::io) fn translate_universal_newlines(bytes: &[u8]) -> Vec<u8> {
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

pub(in crate::builtins::io) fn should_track_newlines(handle: &MoltFileHandle) -> bool {
    handle.text && matches!(handle.newline.as_deref(), None | Some(""))
}

pub(in crate::builtins::io) fn record_newline(handle: &mut MoltFileHandle, kind: u8) {
    if (handle.newlines_mask & kind) != 0 {
        return;
    }
    if (handle.newlines_len as usize) < handle.newlines_seen.len() {
        handle.newlines_seen[handle.newlines_len as usize] = kind;
        handle.newlines_len = handle.newlines_len.saturating_add(1);
    }
    handle.newlines_mask |= kind;
}

pub(in crate::builtins::io) fn update_newlines_from_bytes(
    handle: &mut MoltFileHandle,
    bytes: &[u8],
) {
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

pub(in crate::builtins::io) fn update_newlines_from_chars(
    handle: &mut MoltFileHandle,
    chars: &[char],
) {
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

pub(in crate::builtins::io) fn translate_write_newlines_bytes(
    bytes: &[u8],
    newline: Option<&str>,
) -> Vec<u8> {
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

pub(in crate::builtins::io) fn translate_write_newlines_str(
    text: &str,
    newline: Option<&str>,
) -> String {
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
