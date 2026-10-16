use super::*;
use molt_stdlib_text::codec_registry::{EncodingKind, normalize_encoding};
use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;

pub(in crate::builtins::io) const TEXT_COOKIE_VERSION: u8 = 3;
pub(in crate::builtins::io) const TEXT_COOKIE_MAX_PENDING: usize = 4;
pub(in crate::builtins::io) const TEXT_COOKIE_FIXED_LEN: usize = 17;

pub(in crate::builtins::io) struct TextCookie {
    pub(in crate::builtins::io) pos: u64,
    pub(in crate::builtins::io) pending_byte: Option<u8>,
    pub(in crate::builtins::io) pending_bytes: Vec<u8>,
    pub(in crate::builtins::io) pending_text: Vec<u8>,
    pub(in crate::builtins::io) decoder: Option<EncodingKind>,
    // A plain byte offset resets the decoder; a stateful cookie restores it.
    pub(in crate::builtins::io) bom_seen: Option<bool>,
}

pub(in crate::builtins::io) fn text_cookie_decoder(
    handle: &MoltFileHandle,
) -> Option<EncodingKind> {
    let original = normalize_encoding(handle.encoding_original.as_deref()?)?;
    if !handle.text_bom_seen {
        return None;
    }
    let active = normalize_encoding(handle.encoding.as_deref()?)?;
    match (original, active) {
        (EncodingKind::Utf16, EncodingKind::Utf16LE | EncodingKind::Utf16BE)
        | (EncodingKind::Utf32, EncodingKind::Utf32LE | EncodingKind::Utf32BE) => Some(active),
        _ => None,
    }
}

pub(in crate::builtins::io) fn text_cookie_restore_decoder(
    handle: &mut MoltFileHandle,
    cookie: &TextCookie,
) {
    let Some(original) = handle
        .encoding_original
        .as_deref()
        .and_then(normalize_encoding)
    else {
        return;
    };
    let at_start = cookie.pos == 0
        && cookie.pending_byte.is_none()
        && cookie.pending_bytes.is_empty()
        && cookie.pending_text.is_empty();
    handle.text_bom_written = !at_start;
    handle.text_bom_seen = cookie
        .bom_seen
        .unwrap_or(original == EncodingKind::Utf8Sig && !at_start);
    let active = match original {
        EncodingKind::Utf16 => match cookie.decoder {
            Some(kind @ (EncodingKind::Utf16LE | EncodingKind::Utf16BE)) => kind,
            _ if at_start || cookie.bom_seen == Some(false) => original,
            _ if cfg!(target_endian = "little") => EncodingKind::Utf16LE,
            _ => EncodingKind::Utf16BE,
        },
        EncodingKind::Utf32 => match cookie.decoder {
            Some(kind @ (EncodingKind::Utf32LE | EncodingKind::Utf32BE)) => kind,
            _ if at_start || cookie.bom_seen == Some(false) => original,
            _ if cfg!(target_endian = "little") => EncodingKind::Utf32LE,
            _ => EncodingKind::Utf32BE,
        },
        EncodingKind::Utf8Sig => {
            handle.text_bom_seen = cookie.bom_seen.unwrap_or(!at_start);
            original
        }
        _ => return,
    };
    handle.encoding = Some(active.name().to_owned());
}

pub(in crate::builtins::io) fn text_cookie_encode_bits(
    py: &PyToken<'_>,
    pos: u64,
    pending_byte: Option<u8>,
    pending_bytes: &[u8],
    pending_text: &[u8],
    decoder: Option<EncodingKind>,
    bom_seen: bool,
) -> Result<u64, String> {
    // A decoder-free position is the actual byte offset, including nonzero
    // offsets. Logging and other public consumers legitimately compare it.
    if pending_byte.is_none()
        && pending_bytes.is_empty()
        && pending_text.is_empty()
        && decoder.is_none()
    {
        return Ok(int_bits_from_i128(py, i128::from(pos)));
    }
    if pending_bytes.len() > TEXT_COOKIE_MAX_PENDING {
        return Err("tell overflow".to_string());
    }
    let pending_text_len: u32 = pending_text
        .len()
        .try_into()
        .map_err(|_| "tell overflow".to_string())?;
    let decoder = match decoder {
        None => 0,
        Some(EncodingKind::Utf16LE) => 1,
        Some(EncodingKind::Utf16BE) => 2,
        Some(EncodingKind::Utf32LE) => 3,
        Some(EncodingKind::Utf32BE) => 4,
        _ => return Err("invalid decoder state".to_string()),
    };
    let capacity = TEXT_COOKIE_FIXED_LEN
        .checked_add(pending_bytes.len())
        .and_then(|length| length.checked_add(pending_text.len()))
        .ok_or_else(|| "tell overflow".to_owned())?;
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(&pos.to_le_bytes());
    bytes.push(u8::from(pending_byte.is_some()) | (u8::from(bom_seen) << 1));
    bytes.push(pending_byte.unwrap_or(0));
    bytes.push(pending_bytes.len() as u8);
    bytes.push(decoder);
    bytes.extend_from_slice(&pending_text_len.to_le_bytes());
    bytes.extend_from_slice(pending_bytes);
    bytes.extend_from_slice(pending_text);
    // A nonzero high byte makes every stateful cookie disjoint from all u64
    // byte offsets, even when its saved position and payload bytes are zero.
    bytes.push(TEXT_COOKIE_VERSION);
    Ok(int_bits_from_bigint(
        py,
        BigInt::from_bytes_le(Sign::Plus, &bytes),
    ))
}

pub(in crate::builtins::io) fn text_cookie_decode_value(
    value: &BigInt,
) -> Result<TextCookie, String> {
    if value.sign() == Sign::Minus {
        return Err("negative seek position".to_string());
    }
    if let Some(pos) = value.to_u64() {
        return Ok(TextCookie {
            pos,
            pending_byte: None,
            pending_bytes: Vec::new(),
            pending_text: Vec::new(),
            decoder: None,
            bom_seen: None,
        });
    }
    let (_, bytes) = value.to_bytes_le();
    if bytes.len() < TEXT_COOKIE_FIXED_LEN
        || bytes.last() != Some(&TEXT_COOKIE_VERSION)
        || bytes[8] & !3 != 0
        || (bytes[8] & 1 == 0 && bytes[9] != 0)
    {
        return Err("invalid seek position".to_string());
    }
    let pending_len = usize::from(bytes[10]);
    let text_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let expected_len = TEXT_COOKIE_FIXED_LEN
        .checked_add(pending_len)
        .and_then(|n| n.checked_add(text_len));
    // Validate against supplied bytes before slicing or allocating. A forged
    // length must never trigger padding/allocation proportional to that length.
    if pending_len > TEXT_COOKIE_MAX_PENDING || expected_len != Some(bytes.len()) {
        return Err("invalid seek position".to_string());
    }
    let decoder = match bytes[11] {
        0 => None,
        1 => Some(EncodingKind::Utf16LE),
        2 => Some(EncodingKind::Utf16BE),
        3 => Some(EncodingKind::Utf32LE),
        4 => Some(EncodingKind::Utf32BE),
        _ => return Err("invalid seek position".to_string()),
    };
    Ok(TextCookie {
        pos: u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        pending_byte: (bytes[8] & 1 != 0).then_some(bytes[9]),
        pending_bytes: bytes[16..16 + pending_len].to_vec(),
        pending_text: bytes[16 + pending_len..bytes.len() - 1].to_vec(),
        decoder,
        bom_seen: Some(bytes[8] & 2 != 0),
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
