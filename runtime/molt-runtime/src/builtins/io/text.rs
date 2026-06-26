use super::*;
use num_bigint::{BigInt, Sign};
use num_traits::Zero;

#[path = "text/encoding.rs"]
mod encoding;
pub(super) use encoding::*;

pub(super) const TEXT_COOKIE_VERSION: u8 = 2;
pub(super) const TEXT_COOKIE_MAX_PENDING: usize = 4;
pub(super) const TEXT_COOKIE_FIXED_LEN: usize = 16;

pub(super) struct TextCookie {
    pub(super) pos: u64,
    pub(super) pending_byte: Option<u8>,
    pub(super) pending_bytes: Vec<u8>,
    pub(super) pending_text: Vec<u8>,
}

pub(super) fn text_cookie_encode_bits(
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

pub(super) fn text_cookie_decode_value(value: BigInt) -> Result<TextCookie, String> {
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

pub(super) fn translate_universal_newlines(bytes: &[u8]) -> Vec<u8> {
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

pub(super) fn should_track_newlines(handle: &MoltFileHandle) -> bool {
    handle.text && matches!(handle.newline.as_deref(), None | Some(""))
}

pub(super) fn record_newline(handle: &mut MoltFileHandle, kind: u8) {
    if (handle.newlines_mask & kind) != 0 {
        return;
    }
    if (handle.newlines_len as usize) < handle.newlines_seen.len() {
        handle.newlines_seen[handle.newlines_len as usize] = kind;
        handle.newlines_len = handle.newlines_len.saturating_add(1);
    }
    handle.newlines_mask |= kind;
}

pub(super) fn update_newlines_from_bytes(handle: &mut MoltFileHandle, bytes: &[u8]) {
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

pub(super) fn update_newlines_from_chars(handle: &mut MoltFileHandle, chars: &[char]) {
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

pub(super) fn translate_write_newlines_bytes(bytes: &[u8], newline: Option<&str>) -> Vec<u8> {
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

pub(super) fn translate_write_newlines_str(text: &str, newline: Option<&str>) -> String {
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

pub(super) unsafe fn text_backend_read(
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

pub(super) unsafe fn text_backend_readline(
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

pub(super) unsafe fn text_backend_write(
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

pub(super) unsafe fn text_backend_getvalue(
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

pub(super) unsafe fn text_backend_seek(
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

pub(super) unsafe fn text_backend_truncate(
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

pub(super) unsafe fn text_backend_tell(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    Ok(text_backend.pos as i64)
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

pub(super) fn file_readline_bytes(
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
pub(super) unsafe fn read_line_multibyte(
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
