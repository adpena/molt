use super::functions::iterable_to_string_vec;
use molt_obj_model::MoltObject;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    alloc_list_with_capacity, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    bytes_like_slice, call_class_init_with_args, dec_ref_bits, exception_pending, is_truthy,
    missing_bits, molt_getattr_builtin, obj_from_bits, raise_exception, string_obj_to_owned,
    to_i64,
};

#[derive(Clone)]
struct MoltEmailMessage {
    headers: Vec<(String, String)>,
    body: String,
    content_type: String,
    filename: Option<String>,
    parts: Vec<MoltEmailMessage>,
    multipart_subtype: Option<String>,
}

const QUOPRI_ESCAPE: u8 = b'=';
const QUOPRI_MAX_LINE_SIZE: usize = 76;
const QUOPRI_HEX: &[u8; 16] = b"0123456789ABCDEF";

#[inline]
fn quopri_needs_quoting(byte: u8, quotetabs: bool, header: bool) -> bool {
    if matches!(byte, b' ' | b'\t') {
        return quotetabs;
    }
    if byte == b'_' {
        return header;
    }
    byte == QUOPRI_ESCAPE || !(b' '..=b'~').contains(&byte)
}

#[inline]
fn quopri_quote_byte(byte: u8, out: &mut Vec<u8>) {
    out.push(QUOPRI_ESCAPE);
    out.push(QUOPRI_HEX[(byte >> 4) as usize]);
    out.push(QUOPRI_HEX[(byte & 0x0F) as usize]);
}

#[inline]
fn quopri_write_chunk(chunk: &[u8], line_end: &[u8], out: &mut Vec<u8>) {
    if let Some(last) = chunk.last()
        && matches!(*last, b' ' | b'\t')
    {
        out.extend_from_slice(&chunk[..chunk.len() - 1]);
        quopri_quote_byte(*last, out);
        out.extend_from_slice(line_end);
        return;
    }
    if chunk == b"." {
        quopri_quote_byte(b'.', out);
        out.extend_from_slice(line_end);
        return;
    }
    out.extend_from_slice(chunk);
    out.extend_from_slice(line_end);
}

fn quopri_encode_impl(data: &[u8], quotetabs: bool, header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + (data.len() / 20));
    let mut idx = 0usize;
    while idx < data.len() {
        let start = idx;
        while idx < data.len() && data[idx] != b'\n' {
            idx += 1;
        }
        let line = &data[start..idx];
        let line_end: &[u8] = if idx < data.len() && data[idx] == b'\n' {
            idx += 1;
            b"\n"
        } else {
            b""
        };

        let mut encoded = Vec::with_capacity(line.len() * 3);
        for byte in line {
            if quopri_needs_quoting(*byte, quotetabs, header) {
                quopri_quote_byte(*byte, &mut encoded);
            } else if header && *byte == b' ' {
                encoded.push(b'_');
            } else {
                encoded.push(*byte);
            }
        }

        let mut cursor = 0usize;
        while encoded.len().saturating_sub(cursor) > QUOPRI_MAX_LINE_SIZE {
            let end = cursor + QUOPRI_MAX_LINE_SIZE - 1;
            quopri_write_chunk(&encoded[cursor..end], b"=\n", &mut out);
            cursor = end;
        }
        quopri_write_chunk(&encoded[cursor..], line_end, &mut out);
    }
    out
}

#[inline]
fn quopri_is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

#[inline]
fn quopri_unhex_pair(hi: u8, lo: u8) -> Option<u8> {
    let hi = match hi {
        b'0'..=b'9' => hi - b'0',
        b'a'..=b'f' => hi - b'a' + 10,
        b'A'..=b'F' => hi - b'A' + 10,
        _ => return None,
    };
    let lo = match lo {
        b'0'..=b'9' => lo - b'0',
        b'a'..=b'f' => lo - b'a' + 10,
        b'A'..=b'F' => lo - b'A' + 10,
        _ => return None,
    };
    Some((hi << 4) | lo)
}

fn quopri_decode_impl(data: &[u8], header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut new = Vec::with_capacity(64);
    let mut idx = 0usize;
    while idx < data.len() {
        let start = idx;
        while idx < data.len() && data[idx] != b'\n' {
            idx += 1;
        }
        let mut n = idx - start;
        let line = &data[start..idx];
        let mut partial = true;
        if idx < data.len() && data[idx] == b'\n' {
            idx += 1;
            partial = false;
            while n > 0 && matches!(line[n - 1], b' ' | b'\t' | b'\r') {
                n -= 1;
            }
        }

        let mut i = 0usize;
        while i < n {
            let c = line[i];
            if c == b'_' && header {
                new.push(b' ');
                i += 1;
            } else if c != QUOPRI_ESCAPE {
                new.push(c);
                i += 1;
            } else if i + 1 == n && !partial {
                partial = true;
                break;
            } else if i + 1 < n && line[i + 1] == QUOPRI_ESCAPE {
                new.push(QUOPRI_ESCAPE);
                i += 2;
            } else if i + 2 < n && quopri_is_hex(line[i + 1]) && quopri_is_hex(line[i + 2]) {
                if let Some(decoded) = quopri_unhex_pair(line[i + 1], line[i + 2]) {
                    new.push(decoded);
                    i += 3;
                } else {
                    new.push(c);
                    i += 1;
                }
            } else {
                new.push(c);
                i += 1;
            }
        }
        if !partial {
            out.extend_from_slice(new.as_slice());
            out.push(b'\n');
            new.clear();
        }
    }
    if !new.is_empty() {
        out.extend_from_slice(new.as_slice());
    }
    out
}

fn quopri_expect_bytes_like(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<Vec<u8>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("quopri {arg_name} expects bytes-like");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
        let msg = format!("quopri {arg_name} expects bytes-like");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    Ok(raw.to_vec())
}

fn quopri_expect_single_byte(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<u8, u64> {
    let bytes = quopri_expect_bytes_like(_py, bits, arg_name)?;
    if bytes.len() != 1 {
        let msg = format!("quopri {arg_name} expects single-byte bytes");
        return Err(raise_exception::<u64>(_py, "ValueError", &msg));
    }
    Ok(bytes[0])
}

#[inline]
fn email_quopri_header_safe(byte: u8) -> bool {
    matches!(byte, b'-' | b'!' | b'*' | b'+' | b'/')
        || byte.is_ascii_alphabetic()
        || byte.is_ascii_digit()
}

#[inline]
fn email_quopri_body_safe(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'!' | b'"' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+'
            | b',' | b'-' | b'.' | b'/' | b'0'..=b'9' | b':' | b';' | b'<' | b'>' | b'?'
            | b'@' | b'A'..=b'Z' | b'[' | b'\\' | b']' | b'^' | b'_' | b'`' | b'a'..=b'z'
            | b'{' | b'|' | b'}' | b'~' | b'\t'
    )
}

#[inline]
fn email_quopri_push_escape(byte: u8, out: &mut String) {
    out.push('=');
    out.push(QUOPRI_HEX[(byte >> 4) as usize] as char);
    out.push(QUOPRI_HEX[(byte & 0x0F) as usize] as char);
}

#[inline]
fn email_quopri_push_header_mapped(byte: u8, out: &mut String) {
    if email_quopri_header_safe(byte) {
        out.push(byte as char);
    } else if byte == b' ' {
        out.push('_');
    } else {
        email_quopri_push_escape(byte, out);
    }
}

#[inline]
fn email_quopri_push_body_mapped(byte: u8, out: &mut String) {
    if email_quopri_body_safe(byte) {
        out.push(byte as char);
    } else {
        email_quopri_push_escape(byte, out);
    }
}

fn email_quopri_expect_int_octet(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<u8, u64> {
    let value = match to_i64(obj_from_bits(bits)) {
        Some(value) => value,
        None => {
            let msg = format!("email.quoprimime {arg_name} expects int");
            return Err(raise_exception::<u64>(_py, "TypeError", &msg));
        }
    };
    if !(0..=255).contains(&value) {
        let msg = format!("email.quoprimime {arg_name} out of range");
        return Err(raise_exception::<u64>(_py, "ValueError", &msg));
    }
    Ok(value as u8)
}

fn email_quopri_expect_string(
    _py: &crate::PyToken<'_>,
    bits: u64,
    arg_name: &str,
) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(value) => Ok(value),
        None => {
            let msg = format!("email.quoprimime {arg_name} expects str");
            Err(raise_exception::<u64>(_py, "TypeError", &msg))
        }
    }
}

pub(super) fn email_quopri_alloc_str(_py: &crate::PyToken<'_>, value: &str) -> u64 {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn email_quopri_splitlines(value: &str) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\n' => {
                lines.push(value[start..idx].to_string());
                idx += 1;
                start = idx;
            }
            b'\r' => {
                lines.push(value[start..idx].to_string());
                idx += 1;
                if idx < bytes.len() && bytes[idx] == b'\n' {
                    idx += 1;
                }
                start = idx;
            }
            _ => idx += 1,
        }
    }
    if start < bytes.len() {
        lines.push(value[start..].to_string());
    }
    lines
}

#[inline]
fn email_quopri_is_hex_char(ch: char) -> bool {
    ch.is_ascii_hexdigit()
}

fn email_quopri_decode_hex_pair(hi: char, lo: char) -> Option<char> {
    let hi = hi.to_digit(16)?;
    let lo = lo.to_digit(16)?;
    let value = ((hi << 4) | lo) as u8;
    Some(value as char)
}

static EMAIL_MESSAGE_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltEmailMessage>>> = OnceLock::new();
static EMAIL_MESSAGE_NEXT: AtomicU64 = AtomicU64::new(1);
fn email_message_registry() -> &'static Mutex<HashMap<u64, MoltEmailMessage>> {
    EMAIL_MESSAGE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn email_message_register(message: MoltEmailMessage) -> u64 {
    let id = EMAIL_MESSAGE_NEXT.fetch_add(1, Ordering::Relaxed);
    let mut registry = email_message_registry()
        .lock()
        .expect("email message registry lock poisoned");
    registry.insert(id, message);
    id
}

fn email_message_handle_tag(id: u64) -> String {
    format!("molt-email-message-{id}")
}

fn email_message_bits_from_id(_py: &crate::PyToken<'_>, id: u64) -> u64 {
    let tag = email_message_handle_tag(id);
    let ptr = alloc_string(_py, tag.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn email_message_id_from_bits(_py: &crate::PyToken<'_>, message_bits: u64) -> Result<u64, u64> {
    if let Some(text) = string_obj_to_owned(obj_from_bits(message_bits))
        && let Some(raw) = text.strip_prefix("molt-email-message-")
        && let Ok(id) = raw.parse::<u64>()
        && id > 0
    {
        return Ok(id);
    }
    let Some(raw) = to_i64(obj_from_bits(message_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "email message handle is invalid",
        ));
    };
    if raw <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "email message handle is invalid",
        ));
    }
    let Ok(id) = u64::try_from(raw) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "email message handle is invalid",
        ));
    };
    Ok(id)
}

static EMAIL_MSGID_NEXT: AtomicU64 = AtomicU64::new(1);

fn email_message_default() -> MoltEmailMessage {
    MoltEmailMessage {
        headers: Vec::new(),
        body: String::new(),
        content_type: "text/plain".to_string(),
        filename: None,
        parts: Vec::new(),
        multipart_subtype: None,
    }
}

fn email_header_get(headers: &[(String, String)], name: &str) -> Option<String> {
    for (header_name, value) in headers.iter().rev() {
        if header_name.eq_ignore_ascii_case(name) {
            return Some(value.clone());
        }
    }
    None
}

fn email_fold_header(name: &str, value: &str) -> String {
    let prefix = format!("{name}: ");
    if prefix.len() + value.len() <= 78 {
        return format!("{prefix}{value}");
    }
    let mut out = prefix;
    let mut remaining = value.trim();
    let mut first = true;
    while !remaining.is_empty() {
        let max_len = if first { 72 } else { 74 };
        let take = remaining
            .char_indices()
            .take_while(|(idx, _)| *idx < max_len)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or_else(|| remaining.len().min(max_len));
        let (chunk, rest) = remaining.split_at(take);
        if !first {
            out.push(' ');
        }
        out.push_str(chunk.trim_end());
        if !rest.is_empty() {
            out.push('\n');
            first = false;
        }
        remaining = rest.trim_start();
    }
    out
}

fn email_serialize_message(message: &MoltEmailMessage) -> String {
    let mut out = String::new();
    for (name, value) in &message.headers {
        out.push_str(&email_fold_header(name, value));
        out.push('\n');
    }
    if message.parts.is_empty() {
        out.push_str(&format!("Content-Type: {}\n", message.content_type));
        if let Some(filename) = &message.filename {
            out.push_str(&format!(
                "Content-Disposition: attachment; filename=\"{}\"\n",
                filename
            ));
        }
        out.push('\n');
        out.push_str(&message.body);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    let subtype = message
        .multipart_subtype
        .as_deref()
        .unwrap_or("mixed")
        .to_string();
    let boundary = "==MOLT_BOUNDARY==";
    out.push_str(&format!(
        "Content-Type: multipart/{}; boundary=\"{}\"\n\n",
        subtype, boundary
    ));
    for part in &message.parts {
        out.push_str(&format!("--{}\n", boundary));
        out.push_str(&format!("Content-Type: {}\n", part.content_type));
        if let Some(filename) = &part.filename {
            out.push_str(&format!(
                "Content-Disposition: attachment; filename=\"{}\"\n",
                filename
            ));
        }
        out.push('\n');
        out.push_str(&part.body);
        if !part.body.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&format!("--{}--\n", boundary));
    out
}

fn email_parse_simple_message(raw: &str) -> MoltEmailMessage {
    let mut message = email_message_default();
    let normalized = raw.replace("\r\n", "\n");
    let mut split = normalized.splitn(2, "\n\n");
    let header_block = split.next().unwrap_or_default();
    let body_block = split.next().unwrap_or_default();
    let mut last_header: Option<usize> = None;
    for line in header_block.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(idx) = last_header
                && let Some((_, value)) = message.headers.get_mut(idx)
            {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let name = line[..colon].trim().to_string();
        let value = line[colon + 1..].trim().to_string();
        if name.eq_ignore_ascii_case("content-type") {
            let base = value
                .split(';')
                .next()
                .unwrap_or(value.as_str())
                .trim()
                .to_string();
            message.content_type = if base.is_empty() {
                "text/plain".to_string()
            } else {
                base
            };
            continue;
        }
        message.headers.push((name, value));
        last_header = Some(message.headers.len().saturating_sub(1));
    }
    message.body = body_block.to_string();
    message
}

fn email_month_number(token: &str) -> Option<i64> {
    match token.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn email_month_name(month: i64) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Jan",
    }
}

fn email_weekday_mon0(year: i64, month: i64, day: i64) -> i64 {
    // Sakamoto algorithm (returns 0=Sunday..6=Saturday).
    let t = [0i64, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year;
    if month < 3 {
        y -= 1;
    }
    let m_index = usize::try_from(month.saturating_sub(1))
        .unwrap_or(0)
        .min(t.len().saturating_sub(1));
    let sun0 = (y + y / 4 - y / 100 + y / 400 + t[m_index] + day).rem_euclid(7);
    // Convert Sunday=0..Saturday=6 to Monday=0..Sunday=6.
    (sun0 + 6).rem_euclid(7)
}

fn email_weekday_name_mon0(mon0: i64) -> &'static str {
    match mon0 {
        0 => "Mon",
        1 => "Tue",
        2 => "Wed",
        3 => "Thu",
        4 => "Fri",
        5 => "Sat",
        6 => "Sun",
        _ => "Mon",
    }
}

fn email_parse_datetime_like(value: &str) -> Option<(i64, i64, i64, i64, i64, i64, i64)> {
    let mut text = value.trim();
    if let Some(comma) = text.find(',') {
        text = text[comma + 1..].trim();
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let day = parts[0].parse::<i64>().ok()?;
    let month = email_month_number(parts[1])?;
    let year = parts[2].parse::<i64>().ok()?;
    let mut time_iter = parts[3].split(':');
    let hour = time_iter.next()?.parse::<i64>().ok()?;
    let minute = time_iter.next()?.parse::<i64>().ok()?;
    let second = time_iter.next()?.parse::<i64>().ok()?;
    let tz = parts[4];
    if tz.len() != 5 {
        return None;
    }
    let sign = match &tz[0..1] {
        "+" => 1i64,
        "-" => -1i64,
        _ => return None,
    };
    let tz_hours = tz[1..3].parse::<i64>().ok()?;
    let tz_minutes = tz[3..5].parse::<i64>().ok()?;
    let offset = sign * (tz_hours * 3600 + tz_minutes * 60);
    Some((year, month, day, hour, minute, second, offset))
}

fn email_utils_format_datetime_impl(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> String {
    let wday = email_weekday_mon0(year, month, day);
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} +0000",
        email_weekday_name_mon0(wday),
        day,
        email_month_name(month),
        year,
        hour,
        minute,
        second
    )
}

fn email_utils_parse_addresses(values: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for value in values {
        for token in value.split(',') {
            let entry = token.trim();
            if entry.is_empty() {
                continue;
            }
            if let (Some(start), Some(end)) = (entry.rfind('<'), entry.rfind('>'))
                && start < end
            {
                let name = entry[..start].trim().trim_matches('"').to_string();
                let addr = entry[start + 1..end].trim().to_string();
                out.push((name, addr));
                continue;
            }
            out.push((String::new(), entry.to_string()));
        }
    }
    out
}

fn email_header_encode_word_impl(text: &str, charset: Option<&str>) -> Result<String, String> {
    let active = charset.unwrap_or("utf-8");
    let lower = active.to_ascii_lowercase();
    if text.is_ascii() && (charset.is_none() || lower == "ascii" || lower == "us-ascii") {
        return Ok(text.to_string());
    }
    match lower.as_str() {
        "utf-8" | "utf8" => {
            let encoded = urllib_base64_encode(text.as_bytes());
            Ok(format!("=?utf-8?b?{}?=", encoded))
        }
        "ascii" | "us-ascii" => {
            if text.is_ascii() {
                Ok(text.to_string())
            } else {
                Err("non-ASCII header text with ASCII charset".to_string())
            }
        }
        _ => Err("unsupported email header charset".to_string()),
    }
}

fn email_address_addr_spec_impl(username: &str, domain: &str) -> String {
    if !username.is_empty() && !domain.is_empty() {
        format!("{username}@{domain}")
    } else if !domain.is_empty() {
        format!("@{domain}")
    } else {
        username.to_string()
    }
}

fn email_address_format_impl(display_name: &str, username: &str, domain: &str) -> String {
    let addr_spec = email_address_addr_spec_impl(username, domain);
    if !display_name.is_empty() && !addr_spec.is_empty() {
        format!("{display_name} <{addr_spec}>")
    } else if !display_name.is_empty() {
        display_name.to_string()
    } else {
        addr_spec
    }
}

fn email_get_int_attr(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<i64, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        let name_text = std::str::from_utf8(name).unwrap_or("attribute");
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            &format!("datetime object missing {name_text}"),
        ));
    }
    let Some(value) = to_i64(obj_from_bits(value_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "datetime field must be int",
        ));
    };
    Ok(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = email_message_register(email_message_default());
        email_message_bits_from_id(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_from_bytes(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let raw = if let Some(ptr) = obj_from_bits(data_bits).as_ptr() {
            if let Some(bytes) = unsafe { bytes_like_slice(ptr) } {
                String::from_utf8_lossy(bytes).into_owned()
            } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
                text
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "message_from_bytes argument must be bytes-like",
                );
            }
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
            text
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "message_from_bytes argument must be bytes-like",
            );
        };
        let id = email_message_register(email_parse_simple_message(&raw));
        email_message_bits_from_id(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_set(
    message_bits: u64,
    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header value must be str");
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        message.headers.push((name, value));
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_get(message_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if let Some(value) = email_header_get(&message.headers, &name) {
            let value_ptr = alloc_string(_py, value.as_bytes());
            if value_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(value_ptr).bits()
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_set_content(message_bits: u64, content_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(content) = string_obj_to_owned(obj_from_bits(content_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "content must be str");
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        message.body = content;
        message.content_type = "text/plain".to_string();
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_add_alternative(
    message_bits: u64,
    content_bits: u64,
    subtype_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let Some(content) = string_obj_to_owned(obj_from_bits(content_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "alternative content must be str");
        };
        let Some(subtype) = string_obj_to_owned(obj_from_bits(subtype_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "alternative subtype must be str");
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if message.parts.is_empty() {
            let mut first = email_message_default();
            first.content_type = "text/plain".to_string();
            first.body = message.body.clone();
            message.parts.push(first);
            message.body.clear();
        }
        let mut alt = email_message_default();
        alt.content_type = format!("text/{}", subtype);
        alt.body = content;
        message.parts.push(alt);
        message.content_type = "multipart/alternative".to_string();
        message.multipart_subtype = Some("alternative".to_string());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_add_attachment(
    message_bits: u64,
    data_bits: u64,
    maintype_bits: u64,
    subtype_bits: u64,
    filename_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let payload = if let Some(ptr) = obj_from_bits(data_bits).as_ptr() {
            if let Some(bytes) = unsafe { bytes_like_slice(ptr) } {
                String::from_utf8_lossy(bytes).into_owned()
            } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
                text
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attachment payload must be bytes-like or str",
                );
            }
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
            text
        } else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "attachment payload must be bytes-like or str",
            );
        };
        let Some(maintype) = string_obj_to_owned(obj_from_bits(maintype_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maintype must be str");
        };
        let Some(subtype) = string_obj_to_owned(obj_from_bits(subtype_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "subtype must be str");
        };
        let filename = if obj_from_bits(filename_bits).is_none() {
            None
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "filename must be str or None");
            };
            Some(value)
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get_mut(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if message.parts.is_empty() {
            let mut first = email_message_default();
            first.content_type = "text/plain".to_string();
            first.body = message.body.clone();
            message.parts.push(first);
            message.body.clear();
        }
        let mut part = email_message_default();
        part.content_type = format!("{}/{}", maintype, subtype);
        part.body = payload;
        part.filename = filename;
        message.parts.push(part);
        message.content_type = "multipart/mixed".to_string();
        message.multipart_subtype = Some("mixed".to_string());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_is_multipart(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        MoltObject::from_bool(!message.parts.is_empty()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_payload(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let (body, parts) = {
            let registry = email_message_registry()
                .lock()
                .expect("email message registry lock poisoned");
            let Some(message) = registry.get(&id) else {
                return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
            };
            (message.body.clone(), message.parts.clone())
        };
        if parts.is_empty() {
            let body_ptr = alloc_string(_py, body.as_bytes());
            if body_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(body_ptr).bits();
        }
        let mut handles: Vec<u64> = Vec::with_capacity(parts.len());
        for part in parts {
            let handle = email_message_register(part);
            handles.push(email_message_bits_from_id(_py, handle));
        }
        let list_ptr = alloc_list_with_capacity(_py, handles.as_slice(), handles.len());
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_content(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let out_ptr = alloc_string(_py, message.body.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_content_type(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let out_ptr = alloc_string(_py, message.content_type.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_filename(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        if let Some(filename) = &message.filename {
            let out_ptr = alloc_string(_py, filename.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        } else {
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_as_string(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let rendered = email_serialize_message(message);
        let out_ptr = alloc_string(_py, rendered.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_items(message_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match email_message_id_from_bits(_py, message_bits) {
            Ok(id) => id,
            Err(err) => return err,
        };
        let registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        let Some(message) = registry.get(&id) else {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        };
        let mut pair_bits: Vec<u64> = Vec::with_capacity(message.headers.len());
        for (name, value) in &message.headers {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let value_ptr = alloc_string(_py, value.as_bytes());
            if value_ptr.is_null() {
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                dec_ref_bits(_py, name_bits);
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[name_bits, value_bits]);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, value_bits);
            if tuple_ptr.is_null() {
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            pair_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list_with_capacity(_py, pair_bits.as_slice(), pair_bits.len());
        for bits in pair_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_drop(message_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let Ok(id) = email_message_id_from_bits(_py, message_bits) else {
            return;
        };
        let mut registry = email_message_registry()
            .lock()
            .expect("email message registry lock poisoned");
        registry.remove(&id);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_make_msgid(domain_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let domain = if obj_from_bits(domain_bits).is_none() {
            "localhost".to_string()
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(domain_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "domain must be str or None");
            };
            value
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let seq = EMAIL_MSGID_NEXT.fetch_add(1, Ordering::Relaxed);
        let out = format!("<{}.{}@{}>", now, seq, domain);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_getaddresses(values_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let values = match iterable_to_string_vec(_py, values_bits) {
            Ok(v) => v,
            Err(err_bits) => return err_bits,
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let pairs = email_utils_parse_addresses(values.as_slice());
        let mut out_bits: Vec<u64> = Vec::with_capacity(pairs.len());
        for (name, addr) in pairs {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let addr_ptr = alloc_string(_py, addr.as_bytes());
            if addr_ptr.is_null() {
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                dec_ref_bits(_py, name_bits);
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let addr_bits = MoltObject::from_ptr(addr_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[name_bits, addr_bits]);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, addr_bits);
            if tuple_ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list_with_capacity(_py, out_bits.as_slice(), out_bits.len());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_parsedate_tz(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "date value must be str");
        };
        let Some((year, month, day, hour, minute, second, offset)) =
            email_parse_datetime_like(value.as_str())
        else {
            return MoltObject::none().bits();
        };
        // Match CPython email.utils.parsedate_tz behavior: slots 6/7 default to
        // (weekday=0, yearday=1) rather than computed calendar values.
        let wday = 0i64;
        let yday = 1i64;
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(year).bits(),
                MoltObject::from_int(month).bits(),
                MoltObject::from_int(day).bits(),
                MoltObject::from_int(hour).bits(),
                MoltObject::from_int(minute).bits(),
                MoltObject::from_int(second).bits(),
                MoltObject::from_int(wday).bits(),
                MoltObject::from_int(yday).bits(),
                MoltObject::from_int(-1).bits(),
                MoltObject::from_int(offset).bits(),
            ],
        );
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_format_datetime(dt_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let year = match email_get_int_attr(_py, dt_bits, b"year") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let month = match email_get_int_attr(_py, dt_bits, b"month") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let day = match email_get_int_attr(_py, dt_bits, b"day") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let hour = match email_get_int_attr(_py, dt_bits, b"hour") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let minute = match email_get_int_attr(_py, dt_bits, b"minute") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let second = match email_get_int_attr(_py, dt_bits, b"second") {
            Ok(v) => v,
            Err(bits) => return bits,
        };
        let out = email_utils_format_datetime_impl(year, month, day, hour, minute, second);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_utils_parsedate_to_datetime(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "date value must be str");
        };
        let Some((year, month, day, hour, minute, second, offset)) =
            email_parse_datetime_like(value.as_str())
        else {
            return raise_exception::<_>(_py, "ValueError", "invalid date value");
        };
        if offset != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "non-UTC email date offsets are not yet supported",
            );
        }
        let module_name_ptr = alloc_string(_py, b"datetime");
        if module_name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
        let module_bits = crate::molt_module_import(module_name_bits);
        dec_ref_bits(_py, module_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(datetime_name_bits) = attr_name_bits_from_bytes(_py, b"datetime") else {
            dec_ref_bits(_py, module_bits);
            return MoltObject::none().bits();
        };
        let Some(timezone_name_bits) = attr_name_bits_from_bytes(_py, b"timezone") else {
            dec_ref_bits(_py, datetime_name_bits);
            dec_ref_bits(_py, module_bits);
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let datetime_class_bits = molt_getattr_builtin(module_bits, datetime_name_bits, missing);
        let timezone_class_bits = molt_getattr_builtin(module_bits, timezone_name_bits, missing);
        dec_ref_bits(_py, datetime_name_bits);
        dec_ref_bits(_py, timezone_name_bits);
        dec_ref_bits(_py, module_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if datetime_class_bits == missing || timezone_class_bits == missing {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "datetime module is missing required classes",
            );
        }
        let Some(utc_name_bits) = attr_name_bits_from_bytes(_py, b"utc") else {
            dec_ref_bits(_py, datetime_class_bits);
            dec_ref_bits(_py, timezone_class_bits);
            return MoltObject::none().bits();
        };
        let utc_bits = molt_getattr_builtin(timezone_class_bits, utc_name_bits, missing);
        dec_ref_bits(_py, utc_name_bits);
        dec_ref_bits(_py, timezone_class_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, datetime_class_bits);
            return MoltObject::none().bits();
        }
        if utc_bits == missing {
            dec_ref_bits(_py, datetime_class_bits);
            return raise_exception::<_>(_py, "RuntimeError", "datetime.timezone.utc missing");
        }
        let Some(datetime_class_ptr) = obj_from_bits(datetime_class_bits).as_ptr() else {
            dec_ref_bits(_py, utc_bits);
            dec_ref_bits(_py, datetime_class_bits);
            return raise_exception::<_>(_py, "TypeError", "datetime class is invalid");
        };
        let out_bits = unsafe {
            call_class_init_with_args(
                _py,
                datetime_class_ptr,
                &[
                    MoltObject::from_int(year).bits(),
                    MoltObject::from_int(month).bits(),
                    MoltObject::from_int(day).bits(),
                    MoltObject::from_int(hour).bits(),
                    MoltObject::from_int(minute).bits(),
                    MoltObject::from_int(second).bits(),
                    MoltObject::from_int(0).bits(),
                    utc_bits,
                ],
            )
        };
        dec_ref_bits(_py, utc_bits);
        dec_ref_bits(_py, datetime_class_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_policy_new(name_bits: u64, utf8_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "policy name must be str");
        };
        let utf8 = is_truthy(_py, obj_from_bits(utf8_bits));
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_obj_bits = MoltObject::from_ptr(name_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[name_obj_bits, MoltObject::from_bool(utf8).bits()]);
        dec_ref_bits(_py, name_obj_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_headerregistry_value(name_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let value = crate::format_obj_str(_py, obj_from_bits(value_bits));
        let out_ptr = alloc_string(_py, value.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_header_encode_word(text_bits: u64, charset_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header text must be str");
        };
        let charset = if obj_from_bits(charset_bits).is_none() {
            None
        } else {
            let Some(value) = string_obj_to_owned(obj_from_bits(charset_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "charset must be str or None");
            };
            Some(value)
        };
        let encoded = match email_header_encode_word_impl(text.as_str(), charset.as_deref()) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "RuntimeError", &msg),
        };
        let out_ptr = alloc_string(_py, encoded.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_address_addr_spec(username_bits: u64, domain_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(username) = string_obj_to_owned(obj_from_bits(username_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "username must be str");
        };
        let Some(domain) = string_obj_to_owned(obj_from_bits(domain_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "domain must be str");
        };
        let out = email_address_addr_spec_impl(username.as_str(), domain.as_str());
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_address_format(
    display_name_bits: u64,
    username_bits: u64,
    domain_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(display_name) = string_obj_to_owned(obj_from_bits(display_name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "display_name must be str");
        };
        let Some(username) = string_obj_to_owned(obj_from_bits(username_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "username must be str");
        };
        let Some(domain) = string_obj_to_owned(obj_from_bits(domain_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "domain must be str");
        };
        let out =
            email_address_format_impl(display_name.as_str(), username.as_str(), domain.as_str());
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

pub extern "C" fn molt_quopri_encode(data_bits: u64, quotetabs_bits: u64, header_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "encodestring") {
            Ok(data) => data,
            Err(bits) => return bits,
        };
        let quotetabs = is_truthy(_py, obj_from_bits(quotetabs_bits));
        let header = is_truthy(_py, obj_from_bits(header_bits));
        let out = quopri_encode_impl(data.as_slice(), quotetabs, header);
        let ptr = crate::alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_decode(data_bits: u64, header_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "decodestring") {
            Ok(data) => data,
            Err(bits) => return bits,
        };
        let header = is_truthy(_py, obj_from_bits(header_bits));
        let out = quopri_decode_impl(data.as_slice(), header);
        let ptr = crate::alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_needs_quoting(
    c_bits: u64,
    quotetabs_bits: u64,
    header_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "needsquoting") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        let quotetabs = is_truthy(_py, obj_from_bits(quotetabs_bits));
        let header = is_truthy(_py, obj_from_bits(header_bits));
        MoltObject::from_bool(quopri_needs_quoting(byte, quotetabs, header)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_quote(c_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "quote") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        let mut out = Vec::with_capacity(3);
        quopri_quote_byte(byte, &mut out);
        let ptr = crate::alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_ishex(c_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "ishex") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(quopri_is_hex(byte)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_unhex(s_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytes = match quopri_expect_bytes_like(_py, s_bits, "unhex") {
            Ok(bytes) => bytes,
            Err(bits) => return bits,
        };
        if bytes.is_empty() {
            return MoltObject::from_int(0).bits();
        }
        let mut out = 0i64;
        for byte in bytes {
            let value = match byte {
                b'0'..=b'9' => i64::from(byte - b'0'),
                b'a'..=b'f' => i64::from(byte - b'a' + 10),
                b'A'..=b'F' => i64::from(byte - b'A' + 10),
                _ => return raise_exception::<_>(_py, "ValueError", "quopri unhex expects hex"),
            };
            out = out.saturating_mul(16).saturating_add(value);
        }
        MoltObject::from_int(out).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_check(octet_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let octet = match email_quopri_expect_int_octet(_py, octet_bits, "header_check") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut mapped = String::new();
        email_quopri_push_header_mapped(octet, &mut mapped);
        let same = mapped.len() == 1 && mapped.as_bytes()[0] == octet;
        MoltObject::from_bool(!same).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_body_check(octet_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let octet = match email_quopri_expect_int_octet(_py, octet_bits, "body_check") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(!email_quopri_body_safe(octet)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_length(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "email.quoprimime.header_length")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut total = 0i64;
        for byte in data {
            total += if email_quopri_header_safe(byte) || byte == b' ' {
                1
            } else {
                3
            };
        }
        MoltObject::from_int(total).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_body_length(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = match quopri_expect_bytes_like(_py, data_bits, "email.quoprimime.body_length") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut total = 0i64;
        for byte in data {
            total += if email_quopri_body_safe(byte) { 1 } else { 3 };
        }
        MoltObject::from_int(total).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_quote(c_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let c = match email_quopri_expect_string(_py, c_bits, "quote") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut it = c.chars();
        let Some(ch) = it.next() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "ord() expected a character, but string of length 0 found",
            );
        };
        if it.next().is_some() {
            let msg = format!(
                "ord() expected a character, but string of length {} found",
                c.chars().count()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if (ch as u32) > 255 {
            return raise_exception::<_>(_py, "IndexError", "list index out of range");
        }
        let mut out = String::with_capacity(3);
        email_quopri_push_escape(ch as u8, &mut out);
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_unquote(s_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let s = match email_quopri_expect_string(_py, s_bits, "unquote") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 3 || chars[0] != '=' {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "invalid literal for int() with base 16",
            );
        }
        let Some(ch) = email_quopri_decode_hex_pair(chars[1], chars[2]) else {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "invalid literal for int() with base 16",
            );
        };
        let out: String = [ch].into_iter().collect();
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_encode(
    header_bytes_bits: u64,
    charset_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let header_bytes = match quopri_expect_bytes_like(
            _py,
            header_bytes_bits,
            "email.quoprimime.header_encode",
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let charset = match email_quopri_expect_string(_py, charset_bits, "header_encode charset") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if header_bytes.is_empty() {
            return email_quopri_alloc_str(_py, "");
        }
        let mut encoded = String::with_capacity(header_bytes.len() * 3);
        for byte in header_bytes {
            email_quopri_push_header_mapped(byte, &mut encoded);
        }
        let out = format!("=?{charset}?q?{encoded}?=");
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_decode(s_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let s = match email_quopri_expect_string(_py, s_bits, "header_decode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let replaced = s.replace('_', " ");
        let chars: Vec<char> = replaced.chars().collect();
        let mut out = String::with_capacity(replaced.len());
        let mut idx = 0usize;
        while idx < chars.len() {
            if chars[idx] == '='
                && idx + 2 < chars.len()
                && email_quopri_is_hex_char(chars[idx + 1])
                && email_quopri_is_hex_char(chars[idx + 2])
                && let Some(ch) = email_quopri_decode_hex_pair(chars[idx + 1], chars[idx + 2])
            {
                out.push(ch);
                idx += 3;
                continue;
            }
            out.push(chars[idx]);
            idx += 1;
        }
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_body_encode(
    body_bits: u64,
    maxlinelen_bits: u64,
    eol_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let body = match email_quopri_expect_string(_py, body_bits, "body_encode body") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let maxlinelen = match to_i64(obj_from_bits(maxlinelen_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "maxlinelen must be int"),
        };
        if maxlinelen < 4 {
            return raise_exception::<_>(_py, "ValueError", "maxlinelen must be at least 4");
        }
        let eol = match email_quopri_expect_string(_py, eol_bits, "body_encode eol") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if body.is_empty() {
            return email_quopri_alloc_str(_py, body.as_str());
        }

        let mut quoted = String::with_capacity(body.len() + 8);
        for ch in body.chars() {
            let code = ch as u32;
            if code <= 255 {
                let byte = code as u8;
                if matches!(byte, b'\r' | b'\n') {
                    quoted.push(ch);
                } else {
                    email_quopri_push_body_mapped(byte, &mut quoted);
                }
            } else {
                quoted.push(ch);
            }
        }

        let soft_break = format!("={eol}");
        let maxlinelen1 = (maxlinelen as usize) - 1;
        let mut encoded_lines: Vec<String> = Vec::new();
        for line in email_quopri_splitlines(quoted.as_str()) {
            let chars: Vec<char> = line.chars().collect();
            let mut start = 0usize;
            let laststart = (chars.len() as isize) - 1 - (maxlinelen as isize);
            while (start as isize) <= laststart {
                let stop = start + maxlinelen1;
                if chars[stop - 2] == '=' {
                    encoded_lines.push(chars[start..stop - 1].iter().collect());
                    start = stop - 2;
                } else if chars[stop - 1] == '=' {
                    encoded_lines.push(chars[start..stop].iter().collect());
                    start = stop - 1;
                } else {
                    let mut segment: String = chars[start..stop].iter().collect();
                    segment.push('=');
                    encoded_lines.push(segment);
                    start = stop;
                }
            }

            if !chars.is_empty() && matches!(chars[chars.len() - 1], ' ' | '\t') {
                let room = (start as isize) - laststart;
                let mut q = String::new();
                if room >= 3 {
                    email_quopri_push_escape(chars[chars.len() - 1] as u8, &mut q);
                } else if room == 2 {
                    q.push(chars[chars.len() - 1]);
                    q.push_str(soft_break.as_str());
                } else {
                    q.push_str(soft_break.as_str());
                    email_quopri_push_escape(chars[chars.len() - 1] as u8, &mut q);
                }
                let mut segment: String = chars[start..chars.len() - 1].iter().collect();
                segment.push_str(q.as_str());
                encoded_lines.push(segment);
            } else {
                encoded_lines.push(chars[start..].iter().collect());
            }
        }

        if matches!(quoted.chars().last(), Some('\r' | '\n')) {
            encoded_lines.push(String::new());
        }

        let out = encoded_lines.join(eol.as_str());
        email_quopri_alloc_str(_py, out.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_decode(encoded_bits: u64, eol_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let encoded = match email_quopri_expect_string(_py, encoded_bits, "decode encoded") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let eol = match email_quopri_expect_string(_py, eol_bits, "decode eol") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if encoded.is_empty() {
            return email_quopri_alloc_str(_py, encoded.as_str());
        }

        let mut decoded = String::new();
        for line in email_quopri_splitlines(encoded.as_str()) {
            let line = line.trim_end_matches(char::is_whitespace);
            if line.is_empty() {
                decoded.push_str(eol.as_str());
                continue;
            }
            let chars: Vec<char> = line.chars().collect();
            let mut idx = 0usize;
            let n = chars.len();
            while idx < n {
                let c = chars[idx];
                if c != '=' {
                    decoded.push(c);
                    idx += 1;
                } else if idx + 1 == n {
                    idx += 1;
                    continue;
                } else if idx + 2 < n
                    && email_quopri_is_hex_char(chars[idx + 1])
                    && email_quopri_is_hex_char(chars[idx + 2])
                {
                    if let Some(ch) = email_quopri_decode_hex_pair(chars[idx + 1], chars[idx + 2]) {
                        decoded.push(ch);
                        idx += 3;
                    } else {
                        decoded.push(c);
                        idx += 1;
                    }
                } else {
                    decoded.push(c);
                    idx += 1;
                }
                if idx == n {
                    decoded.push_str(eol.as_str());
                }
            }
        }

        if !encoded.ends_with('\r')
            && !encoded.ends_with('\n')
            && !eol.is_empty()
            && decoded.ends_with(eol.as_str())
        {
            let trim = decoded.len() - eol.len();
            decoded.truncate(trim);
        }
        email_quopri_alloc_str(_py, decoded.as_str())
    })
}

// Duplicated from functions.rs for self-containedness
fn urllib_base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if input.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut idx = 0usize;
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            0
        };
        let b2 = if idx + 2 < input.len() {
            input[idx + 2]
        } else {
            0
        };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if idx + 1 < input.len() {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if idx + 2 < input.len() {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        idx += 3;
    }
    out
}
