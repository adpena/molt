use molt_obj_model::MoltObject;
#[cfg(feature = "stdlib_ast")]
use rustpython_parser::{Mode as ParseMode, ParseErrorType, ast as pyast, parse as parse_python};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::types::cell_class;
use crate::builtins::numbers::index_i64_with_overflow;
use crate::builtins::platform::env_state_get;
use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_LIST,
    TYPE_ID_MODULE, TYPE_ID_STRING,
    TYPE_ID_TUPLE, alloc_bound_method_obj, alloc_bytes, alloc_code_obj, alloc_dict_with_pairs,
    alloc_function_obj, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bound_method_func_bits, builtin_classes, bytes_like_slice,
    call_callable0, call_callable1,
    call_callable2, call_callable3, call_class_init_with_args, clear_exception, dec_ref_bits,
    dict_get_in_place, ensure_function_code_bits, exception_kind_bits, exception_pending,
    format_obj, function_dict_bits, function_set_closure_bits, function_set_trampoline_ptr,
    inc_ref_bits, is_truthy, maybe_ptr_from_bits, missing_bits, module_dict_bits,
    molt_exception_last, molt_getattr_builtin, molt_getitem_method, molt_is_callable, molt_iter,
    molt_iter_next, molt_list_insert, molt_trace_enter_slot, obj_from_bits, object_class_bits,
    object_set_class_bits, object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned,
    to_f64, to_i64, type_name, type_of_bits,
};
use memchr::{memchr, memmem};

#[derive(Clone)]
struct MoltEmailMessage {
    headers: Vec<(String, String)>,
    body: String,
    content_type: String,
    filename: Option<String>,
    parts: Vec<MoltEmailMessage>,
    multipart_subtype: Option<String>,
}

struct MoltUrllibResponse {
    body: Vec<u8>,
    pos: usize,
    closed: bool,
    url: String,
    code: i64,
    reason: String,
    headers: Vec<(String, String)>,
    header_joined: HashMap<String, String>,
    headers_dict_cache: Option<u64>,
    headers_list_cache: Option<u64>,
}

struct UrllibHttpRequest {
    host: String,
    port: u16,
    path: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    timeout: Option<f64>,
}

#[derive(Clone)]
struct MoltHttpClientConnection {
    host: String,
    port: u16,
    timeout: Option<f64>,
    method: Option<String>,
    url: Option<String>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    buffer: Vec<Vec<u8>>,
    skip_host: bool,
    skip_accept_encoding: bool,
}

struct MoltHttpClientConnectionRuntime {
    next_handle: u64,
    connections: HashMap<u64, MoltHttpClientConnection>,
}

#[derive(Clone, Default)]
struct MoltHttpMessage {
    headers: Vec<(String, String)>,
    index: HashMap<String, Vec<usize>>,
    items_list_cache: Option<u64>,
}

struct MoltHttpMessageRuntime {
    next_handle: u64,
    messages: HashMap<u64, MoltHttpMessage>,
}

#[derive(Clone)]
struct MoltCookieEntry {
    name: String,
    value: String,
    domain: String,
    path: String,
}

#[derive(Clone, Default)]
struct MoltCookieJar {
    cookies: Vec<MoltCookieEntry>,
}

struct MoltSocketServerPending {
    request: Vec<u8>,
    response: Option<Vec<u8>>,
}

struct MoltSocketServerRuntime {
    next_request_id: u64,
    pending_by_server: HashMap<u64, VecDeque<u64>>,
    pending_requests: HashMap<u64, MoltSocketServerPending>,
    request_server: HashMap<u64, u64>,
    closed_servers: HashSet<u64>,
}

const THIS_ENCODED: &str = concat!(
    "Gur Mra bs Clguba, ol Gvz Crgref\n\n",
    "Ornhgvshy vf orggre guna htyl.\n",
    "Rkcyvpvg vf orggre guna vzcyvpvg.\n",
    "Fvzcyr vf orggre guna pbzcyrk.\n",
    "Pbzcyrk vf orggre guna pbzcyvpngrq.\n",
    "Syng vf orggre guna arfgrq.\n",
    "Fcnefr vf orggre guna qrafr.\n",
    "Ernqnovyvgl pbhagf.\n",
    "Fcrpvny pnfrf nera'g fcrpvny rabhtu gb oernx gur ehyrf.\n",
    "Nygubhtu cenpgvpnyvgl orngf chevgl.\n",
    "Reebef fubhyq arire cnff fvyragyl.\n",
    "Hayrff rkcyvpvgyl fvyraprq.\n",
    "Va gur snpr bs nzovthvgl, ershfr gur grzcgngvba gb thrff.\n",
    "Gurer fubhyq or bar-- naq cersrenoyl bayl bar --boivbhf jnl gb qb vg.\n",
    "Nygubhtu gung jnl znl abg or boivbhf ng svefg hayrff lbh'er Qhgpu.\n",
    "Abj vf orggre guna arire.\n",
    "Nygubhtu arire vf bsgra orggre guna *evtug* abj.\n",
    "Vs gur vzcyrzragngvba vf uneq gb rkcynva, vg'f n onq vqrn.\n",
    "Vs gur vzcyrzragngvba vf rnfl gb rkcynva, vg znl or n tbbq vqrn.\n",
    "Anzrfcnprf ner bar ubaxvat terng vqrn -- yrg'f qb zber bs gubfr!",
);

#[inline]
fn this_rot13_char(ch: char) -> char {
    match ch {
        'A'..='Z' => {
            let base = b'A';
            let idx = ch as u8 - base;
            (base + ((idx + 13) % 26)) as char
        }
        'a'..='z' => {
            let base = b'a';
            let idx = ch as u8 - base;
            (base + ((idx + 13) % 26)) as char
        }
        _ => ch,
    }
}

fn this_build_rot13_text() -> String {
    THIS_ENCODED.chars().map(this_rot13_char).collect()
}

const QUOPRI_ESCAPE: u8 = b'=';
const QUOPRI_MAX_LINE_SIZE: usize = 76;
const QUOPRI_HEX: &[u8; 16] = b"0123456789ABCDEF";
const OPCODE_PAYLOAD_312_JSON: &str = include_str!("../intrinsics/data/opcode_payload_312.json");
const OPCODE_METADATA_PAYLOAD_314_JSON: &str =
    include_str!("../intrinsics/data/opcode_metadata_payload_314.json");
const TOKEN_PAYLOAD_312_JSON: &str = include_str!("../intrinsics/data/token_payload_312.json");

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

fn email_quopri_alloc_str(_py: &crate::PyToken<'_>, value: &str) -> u64 {
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

static URLLIB_RESPONSE_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltUrllibResponse>>> =
    OnceLock::new();
static URLLIB_RESPONSE_NEXT: AtomicU64 = AtomicU64::new(1);
static HTTP_CLIENT_CONNECTION_RUNTIME: OnceLock<Mutex<MoltHttpClientConnectionRuntime>> =
    OnceLock::new();
static HTTP_MESSAGE_RUNTIME: OnceLock<Mutex<MoltHttpMessageRuntime>> = OnceLock::new();
static COOKIEJAR_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltCookieJar>>> = OnceLock::new();
static COOKIEJAR_NEXT: AtomicU64 = AtomicU64::new(1);
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
static SOCKETSERVER_RUNTIME: OnceLock<Mutex<MoltSocketServerRuntime>> = OnceLock::new();

const RE_IGNORECASE: i64 = 2;
const RE_DOTALL: i64 = 16;
const RE_MULTILINE: i64 = 8;
const RE_ASCII: i64 = 256;

fn re_literal_matches_impl(segment: &str, literal: &str, flags: i64) -> bool {
    if flags & RE_IGNORECASE != 0 {
        segment.to_lowercase() == literal.to_lowercase()
    } else {
        segment == literal
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_literal_matches(
    segment_bits: u64,
    literal_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(segment) = string_obj_to_owned(obj_from_bits(segment_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "segment must be str");
        };
        let Some(literal) = string_obj_to_owned(obj_from_bits(literal_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "literal must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let matched = re_literal_matches_impl(&segment, &literal, flags);
        MoltObject::from_bool(matched).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_literal_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    literal_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(literal) = string_obj_to_owned(obj_from_bits(literal_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "literal must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let advanced = re_literal_advance_impl(&text, pos, end, &literal, flags);
        MoltObject::from_int(advanced).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_any_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let advanced = re_any_advance_impl(&text, pos, end, flags);
        MoltObject::from_int(advanced).bits()
    })
}

fn re_is_ascii_digit(ch: char) -> bool {
    ch.is_ascii_digit()
}

fn re_is_ascii_alpha(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

fn re_is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000C}' | '\u{000B}')
}

fn re_is_word_char(ch: &str, flags: i64) -> bool {
    let mut chars = ch.chars();
    let Some(c) = chars.next() else {
        return false;
    };
    if chars.next().is_some() {
        return false;
    }
    if c == '_' {
        return true;
    }
    if re_is_ascii_alpha(c) || re_is_ascii_digit(c) {
        return true;
    }
    if flags & RE_ASCII != 0 {
        return false;
    }
    (c as u32) >= 128 && !re_is_space(c)
}

fn re_category_matches_impl(ch: &str, category: &str, flags: i64) -> bool {
    let mut ch_chars = ch.chars();
    let Some(c) = ch_chars.next() else {
        return false;
    };
    if ch_chars.next().is_some() {
        return false;
    }
    match category {
        "d" | "digit" => {
            if flags & RE_ASCII != 0 {
                c.is_ascii_digit()
            } else {
                c.is_ascii_digit() || c.is_numeric()
            }
        }
        "w" | "word" => re_is_word_char(ch, flags),
        "s" | "space" => {
            if flags & RE_ASCII != 0 {
                re_is_space(c)
            } else {
                c.is_whitespace()
            }
        }
        _ => false,
    }
}

fn re_char_in_range_impl(ch: &str, start: &str, end: &str, flags: i64) -> bool {
    if flags & RE_IGNORECASE != 0 {
        let ch_cmp = ch.to_lowercase();
        let start_cmp = start.to_lowercase();
        let end_cmp = end.to_lowercase();
        start_cmp <= ch_cmp && ch_cmp <= end_cmp
    } else {
        start <= ch && ch <= end
    }
}

fn re_char_at(chars: &[char], index: i64) -> Option<char> {
    let idx = usize::try_from(index).ok()?;
    chars.get(idx).copied()
}

fn re_anchor_matches_impl(
    kind: &str,
    text: &str,
    pos: i64,
    end: i64,
    origin: i64,
    flags: i64,
) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(chars.len()).unwrap_or(i64::MAX);
    if pos < 0 || end < 0 || origin < 0 {
        return false;
    }
    if origin > end || end > text_len || pos > end {
        return false;
    }
    if kind == "start" {
        if pos == origin {
            return true;
        }
        if flags & RE_MULTILINE != 0 && pos > origin {
            return re_char_at(&chars, pos - 1) == Some('\n');
        }
        return false;
    }
    if kind == "start_abs" {
        return pos == 0;
    }
    if kind == "end_abs" {
        if pos == end {
            return true;
        }
        return end > 0 && pos == end - 1 && re_char_at(&chars, pos) == Some('\n');
    }
    if kind == "word_boundary" || kind == "word_boundary_not" {
        let prev_is_word = if pos > 0 {
            re_char_at(&chars, pos - 1)
                .map(|c| {
                    let s = c.to_string();
                    re_is_word_char(&s, flags)
                })
                .unwrap_or(false)
        } else {
            false
        };
        let next_is_word = if pos < text_len {
            re_char_at(&chars, pos)
                .map(|c| {
                    let s = c.to_string();
                    re_is_word_char(&s, flags)
                })
                .unwrap_or(false)
        } else {
            false
        };
        let at_boundary = prev_is_word != next_is_word;
        return if kind == "word_boundary" {
            at_boundary
        } else {
            !at_boundary
        };
    }
    if flags & RE_MULTILINE != 0 {
        if pos == end {
            return true;
        }
        if pos < end {
            return re_char_at(&chars, pos) == Some('\n');
        }
        return false;
    }
    if pos == end {
        return true;
    }
    if end > origin && pos == end - 1 {
        return re_char_at(&chars, pos) == Some('\n');
    }
    false
}

fn re_backref_advance_impl(text: &str, pos: i64, end: i64, start_ref: i64, end_ref: i64) -> i64 {
    let chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(chars.len()).unwrap_or(i64::MAX);
    if pos < 0 || end < 0 || start_ref < 0 || end_ref < start_ref {
        return -1;
    }
    if end > text_len || pos > end || end_ref > text_len {
        return -1;
    }
    let ref_len = end_ref - start_ref;
    let Some(pos_end) = pos.checked_add(ref_len) else {
        return -1;
    };
    if pos_end > end {
        return -1;
    }
    let Some(start_idx) = usize::try_from(start_ref).ok() else {
        return -1;
    };
    let Some(pos_idx) = usize::try_from(pos).ok() else {
        return -1;
    };
    let Some(ref_len_usize) = usize::try_from(ref_len).ok() else {
        return -1;
    };
    for i in 0..ref_len_usize {
        if chars[start_idx + i] != chars[pos_idx + i] {
            return -1;
        }
    }
    pos_end
}

fn re_apply_scoped_flags_impl(flags: i64, add_flags: i64, clear_flags: i64) -> i64 {
    (flags | add_flags) & !clear_flags
}

fn re_extract_range_pairs(
    _py: &crate::PyToken<'_>,
    ranges_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    let iter_bits = molt_iter(ranges_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "ranges must contain (start, end) pairs",
            ));
        };
        let is_sequence = unsafe {
            let ty = object_type_id(item_ptr);
            ty == TYPE_ID_TUPLE || ty == TYPE_ID_LIST
        };
        if !is_sequence {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "ranges must contain (start, end) pairs",
            ));
        }
        let pair = unsafe { seq_vec_ref(item_ptr) };
        if pair.len() < 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "ranges must contain (start, end) pairs",
            ));
        }
        let Some(start) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "range start must be str",
            ));
        };
        let Some(end) = string_obj_to_owned(obj_from_bits(pair[1])) else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "range end must be str",
            ));
        };
        dec_ref_bits(_py, item_bits);
        out.push((start, end));
    }
    Ok(out)
}

fn re_charclass_matches_impl(
    ch: &str,
    negated: bool,
    chars: &[String],
    ranges: &[(String, String)],
    categories: &[String],
    flags: i64,
) -> bool {
    let mut hit = false;
    if flags & RE_IGNORECASE != 0 {
        for item in chars {
            if ch.to_lowercase() == item.to_lowercase() {
                hit = true;
                break;
            }
        }
    } else {
        for item in chars {
            if ch == item {
                hit = true;
                break;
            }
        }
    }
    if !hit {
        for (start, end) in ranges {
            if re_char_in_range_impl(ch, start.as_str(), end.as_str(), flags) {
                hit = true;
                break;
            }
        }
    }
    if !hit {
        for category in categories {
            if category.starts_with("posix:") {
                continue;
            }
            if re_category_matches_impl(ch, category.as_str(), flags) {
                hit = true;
                break;
            }
        }
    }
    if negated { !hit } else { hit }
}

#[allow(clippy::too_many_arguments)]
fn re_charclass_advance_impl(
    text: &str,
    pos: i64,
    end: i64,
    negated: bool,
    chars: &[String],
    ranges: &[(String, String)],
    categories: &[String],
    flags: i64,
) -> i64 {
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);
    if pos < 0 || end < 0 || pos >= end || end > text_len {
        return -1;
    }
    let Some(ch) = re_char_at(&text_chars, pos) else {
        return -1;
    };
    let mut buf = [0u8; 4];
    let ch_str = ch.encode_utf8(&mut buf);
    if re_charclass_matches_impl(ch_str, negated, chars, ranges, categories, flags) {
        pos.saturating_add(1)
    } else {
        -1
    }
}

fn re_group_values_from_sequence(
    _py: &crate::PyToken<'_>,
    group_values_bits: u64,
) -> Result<Vec<Option<String>>, u64> {
    let group_values_obj = obj_from_bits(group_values_bits);
    let Some(group_values_ptr) = group_values_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "group_values must be a sequence",
        ));
    };
    let group_values_ty = unsafe { object_type_id(group_values_ptr) };
    if group_values_ty != TYPE_ID_LIST && group_values_ty != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "group_values must be a sequence",
        ));
    }
    let mut out: Vec<Option<String>> = Vec::new();
    let elems = unsafe { seq_vec_ref(group_values_ptr) };
    for &elem_bits in elems.iter() {
        let elem_obj = obj_from_bits(elem_bits);
        if elem_obj.is_none() {
            out.push(None);
            continue;
        }
        let Some(value) = string_obj_to_owned(elem_obj) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group_values must contain str or None",
            ));
        };
        out.push(Some(value));
    }
    Ok(out)
}

fn re_expand_replacement_impl(repl: &str, group_values: &[Option<String>]) -> Result<String, ()> {
    let mut out = String::new();
    let chars: Vec<char> = repl.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' && i + 1 < chars.len() {
            let nxt = chars[i + 1];
            if nxt.is_ascii_digit() {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                let idx_str: String = chars[i + 1..j].iter().collect();
                let idx = idx_str.parse::<usize>().unwrap_or(usize::MAX);
                if idx >= group_values.len() {
                    return Err(());
                }
                if let Some(value) = &group_values[idx] {
                    out.push_str(value.as_str());
                }
                i = j;
                continue;
            }
            let escaped = match nxt {
                'n' => Some('\n'),
                't' => Some('\t'),
                'r' => Some('\r'),
                'f' => Some('\u{000C}'),
                'v' => Some('\u{000B}'),
                '\\' => Some('\\'),
                _ => None,
            };
            if let Some(mapped) = escaped {
                out.push(mapped);
            } else {
                out.push(nxt);
            }
            i += 2;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    Ok(out)
}

fn re_group_spans_from_sequence(
    _py: &crate::PyToken<'_>,
    groups_bits: u64,
) -> Result<Vec<Option<(i64, i64)>>, u64> {
    let groups_obj = obj_from_bits(groups_bits);
    let Some(groups_ptr) = groups_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a sequence",
        ));
    };
    let groups_ty = unsafe { object_type_id(groups_ptr) };
    if groups_ty != TYPE_ID_LIST && groups_ty != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a sequence",
        ));
    }
    let mut out: Vec<Option<(i64, i64)>> = Vec::new();
    let elems = unsafe { seq_vec_ref(groups_ptr) };
    for &elem_bits in elems.iter() {
        let elem_obj = obj_from_bits(elem_bits);
        if elem_obj.is_none() {
            out.push(None);
            continue;
        }
        let Some(elem_ptr) = elem_obj.as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be tuple[int, int] or None",
            ));
        };
        let elem_ty = unsafe { object_type_id(elem_ptr) };
        if elem_ty != TYPE_ID_LIST && elem_ty != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be tuple[int, int] or None",
            ));
        }
        let span = unsafe { seq_vec_ref(elem_ptr) };
        if span.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must contain start/end",
            ));
        }
        let Some(start) = to_i64(obj_from_bits(span[0])) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span start must be int",
            ));
        };
        let Some(end) = to_i64(obj_from_bits(span[1])) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span end must be int",
            ));
        };
        out.push(Some((start, end)));
    }
    Ok(out)
}

fn re_alloc_group_spans(
    _py: &crate::PyToken<'_>,
    spans: &[Option<(i64, i64)>],
) -> Result<u64, u64> {
    let mut elem_bits: Vec<u64> = Vec::with_capacity(spans.len());
    let mut owned_bits: Vec<u64> = Vec::new();
    for span in spans {
        if let Some((start, end)) = span {
            let start_bits = MoltObject::from_int(*start).bits();
            let end_bits = MoltObject::from_int(*end).bits();
            let pair_ptr = alloc_tuple(_py, &[start_bits, end_bits]);
            if pair_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return Err(MoltObject::none().bits());
            }
            let pair_bits = MoltObject::from_ptr(pair_ptr).bits();
            elem_bits.push(pair_bits);
            owned_bits.push(pair_bits);
        } else {
            elem_bits.push(MoltObject::none().bits());
        }
    }
    let out_ptr = alloc_tuple(_py, &elem_bits);
    for bits in owned_bits {
        dec_ref_bits(_py, bits);
    }
    if out_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

fn re_slice_char_bounds(index: i64, text_len: i64) -> i64 {
    if index < 0 {
        let shifted = text_len + index;
        if shifted < 0 { 0 } else { shifted }
    } else if index > text_len {
        text_len
    } else {
        index
    }
}

fn re_group_values_from_spans(text: &str, spans: &[Option<(i64, i64)>]) -> Vec<Option<String>> {
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);
    let mut out: Vec<Option<String>> = Vec::with_capacity(spans.len());
    for span in spans {
        let Some((start, end)) = span else {
            out.push(None);
            continue;
        };
        let start_idx = re_slice_char_bounds(*start, text_len);
        let end_idx = re_slice_char_bounds(*end, text_len);
        let slice = if end_idx <= start_idx {
            String::new()
        } else {
            let s = usize::try_from(start_idx).unwrap_or(0);
            let e = usize::try_from(end_idx).unwrap_or(s);
            text_chars[s..e].iter().collect()
        };
        out.push(Some(slice));
    }
    out
}

fn re_alloc_group_values(_py: &crate::PyToken<'_>, values: &[Option<String>]) -> Result<u64, u64> {
    let mut elem_bits: Vec<u64> = Vec::with_capacity(values.len());
    let mut owned_bits: Vec<u64> = Vec::new();
    for value in values {
        if let Some(text) = value {
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return Err(MoltObject::none().bits());
            }
            let bits = MoltObject::from_ptr(ptr).bits();
            elem_bits.push(bits);
            owned_bits.push(bits);
        } else {
            elem_bits.push(MoltObject::none().bits());
        }
    }
    let out_ptr = alloc_tuple(_py, &elem_bits);
    for bits in owned_bits {
        dec_ref_bits(_py, bits);
    }
    if out_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

fn re_literal_advance_impl(text: &str, pos: i64, end: i64, literal: &str, flags: i64) -> i64 {
    let text_chars: Vec<char> = text.chars().collect();
    let literal_chars: Vec<char> = literal.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);
    if pos < 0 || end < 0 || pos > end || end > text_len {
        return -1;
    }
    let literal_len = i64::try_from(literal_chars.len()).unwrap_or(i64::MAX);
    let Some(stop) = pos.checked_add(literal_len) else {
        return -1;
    };
    if stop > end {
        return -1;
    }
    let Some(start_idx) = usize::try_from(pos).ok() else {
        return -1;
    };
    let Some(stop_idx) = usize::try_from(stop).ok() else {
        return -1;
    };
    let segment: String = text_chars[start_idx..stop_idx].iter().collect();
    if re_literal_matches_impl(segment.as_str(), literal, flags) {
        stop
    } else {
        -1
    }
}

fn re_any_advance_impl(text: &str, pos: i64, end: i64, flags: i64) -> i64 {
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);
    if pos < 0 || end < 0 || pos >= end || end > text_len {
        return -1;
    }
    let Some(ch) = re_char_at(&text_chars, pos) else {
        return -1;
    };
    if flags & RE_DOTALL != 0 || ch != '\n' {
        pos + 1
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_char_in_range(
    ch_bits: u64,
    start_bits: u64,
    end_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ch) = string_obj_to_owned(obj_from_bits(ch_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "ch must be str");
        };
        let Some(start) = string_obj_to_owned(obj_from_bits(start_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "start must be str");
        };
        let Some(end) = string_obj_to_owned(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let matched = re_char_in_range_impl(&ch, &start, &end, flags);
        MoltObject::from_bool(matched).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_category_matches(
    ch_bits: u64,
    category_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ch) = string_obj_to_owned(obj_from_bits(ch_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "ch must be str");
        };
        let Some(category) = string_obj_to_owned(obj_from_bits(category_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "category must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        if category.starts_with("posix:") {
            return MoltObject::from_bool(false).bits();
        }
        let matched = re_category_matches_impl(&ch, category.as_str(), flags);
        MoltObject::from_bool(matched).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_anchor_matches(
    kind_bits: u64,
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    origin_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "kind must be str");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(origin) = to_i64(obj_from_bits(origin_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "origin must be int");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let matched = re_anchor_matches_impl(kind.as_str(), &text, pos, end, origin, flags);
        MoltObject::from_bool(matched).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_group_is_set(groups_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let spans = match re_group_spans_from_sequence(_py, groups_bits) {
            Ok(value) => value,
            Err(err_bits) => return err_bits,
        };
        let Some(index) = to_i64(obj_from_bits(index_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "index must be int");
        };
        let is_set = if let Ok(index_usize) = usize::try_from(index) {
            index_usize < spans.len() && spans[index_usize].is_some()
        } else {
            false
        };
        MoltObject::from_bool(is_set).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_backref_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    start_ref_bits: u64,
    end_ref_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(start_ref) = to_i64(obj_from_bits(start_ref_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "start_ref must be int");
        };
        let Some(end_ref) = to_i64(obj_from_bits(end_ref_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end_ref must be int");
        };
        let advanced = re_backref_advance_impl(&text, pos, end, start_ref, end_ref);
        MoltObject::from_int(advanced).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_backref_group_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    groups_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let spans = match re_group_spans_from_sequence(_py, groups_bits) {
            Ok(value) => value,
            Err(err_bits) => return err_bits,
        };
        let Some(index) = to_i64(obj_from_bits(index_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "index must be int");
        };
        let advanced = if let Ok(index_usize) = usize::try_from(index) {
            if let Some(Some((start_ref, end_ref))) = spans.get(index_usize) {
                re_backref_advance_impl(&text, pos, end, *start_ref, *end_ref)
            } else {
                -1
            }
        } else {
            -1
        };
        MoltObject::from_int(advanced).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_apply_scoped_flags(
    flags_bits: u64,
    add_flags_bits: u64,
    clear_flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let Some(add_flags) = to_i64(obj_from_bits(add_flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "add_flags must be int");
        };
        let Some(clear_flags) = to_i64(obj_from_bits(clear_flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "clear_flags must be int");
        };
        let scoped = re_apply_scoped_flags_impl(flags, add_flags, clear_flags);
        MoltObject::from_int(scoped).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_charclass_matches(
    ch_bits: u64,
    negated_bits: u64,
    chars_bits: u64,
    ranges_bits: u64,
    categories_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ch) = string_obj_to_owned(obj_from_bits(ch_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "ch must be str");
        };
        let negated = is_truthy(_py, obj_from_bits(negated_bits));
        let chars = match iterable_to_string_vec(_py, chars_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let ranges = match re_extract_range_pairs(_py, ranges_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        let categories = match iterable_to_string_vec(_py, categories_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let matched = re_charclass_matches_impl(
            &ch,
            negated,
            chars.as_slice(),
            ranges.as_slice(),
            categories.as_slice(),
            flags,
        );
        MoltObject::from_bool(matched).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_charclass_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    negated_bits: u64,
    chars_bits: u64,
    ranges_bits: u64,
    categories_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let negated = is_truthy(_py, obj_from_bits(negated_bits));
        let chars = match iterable_to_string_vec(_py, chars_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let ranges = match re_extract_range_pairs(_py, ranges_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        let categories = match iterable_to_string_vec(_py, categories_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        let advanced = re_charclass_advance_impl(
            &text,
            pos,
            end,
            negated,
            chars.as_slice(),
            ranges.as_slice(),
            categories.as_slice(),
            flags,
        );
        MoltObject::from_int(advanced).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_group_capture(
    groups_bits: u64,
    index_bits: u64,
    start_bits: u64,
    end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut spans = match re_group_spans_from_sequence(_py, groups_bits) {
            Ok(value) => value,
            Err(err_bits) => return err_bits,
        };
        let Some(index) = to_i64(obj_from_bits(index_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "index must be int");
        };
        let Some(start) = to_i64(obj_from_bits(start_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "start must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(index_usize) = usize::try_from(index).ok() else {
            return raise_exception::<_>(_py, "IndexError", "no such group");
        };
        if index_usize >= spans.len() {
            return raise_exception::<_>(_py, "IndexError", "no such group");
        }
        spans[index_usize] = Some((start, end));
        match re_alloc_group_spans(_py, spans.as_slice()) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_group_values(text_bits: u64, groups_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let spans = match re_group_spans_from_sequence(_py, groups_bits) {
            Ok(value) => value,
            Err(err_bits) => return err_bits,
        };
        let values = re_group_values_from_spans(text.as_str(), spans.as_slice());
        match re_alloc_group_values(_py, values.as_slice()) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_re_expand_replacement(repl_bits: u64, group_values_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(repl) = string_obj_to_owned(obj_from_bits(repl_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "repl must be str");
        };
        let group_values = match re_group_values_from_sequence(_py, group_values_bits) {
            Ok(values) => values,
            Err(err_bits) => return err_bits,
        };
        let expanded = match re_expand_replacement_impl(repl.as_str(), group_values.as_slice()) {
            Ok(value) => value,
            Err(()) => return raise_exception::<_>(_py, "IndexError", "no such group"),
        };
        let out_ptr = alloc_string(_py, expanded.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

fn enum_set_attr(
    _py: &crate::concurrency::gil::PyToken<'_>,
    target_bits: u64,
    name: &[u8],
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return false;
    };
    let _ = crate::molt_object_setattr(target_bits, name_bits, value_bits);
    !exception_pending(_py)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_init_member(member_bits: u64, name_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !enum_set_attr(_py, member_bits, b"_name_", name_bits)
            || !enum_set_attr(_py, member_bits, b"_value_", value_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[derive(Clone, Copy, Debug)]
enum PickleGlobal {
    CodecsEncode,
    Bytearray,
    Slice,
    Set,
    FrozenSet,
    List,
    Tuple,
    Dict,
}

#[derive(Clone, Debug)]
enum PickleStackItem {
    Value(u64),
    Mark,
    Global(PickleGlobal),
}

fn pickle_raise(_py: &crate::PyToken<'_>, message: &str) -> u64 {
    raise_exception::<u64>(_py, "RuntimeError", message)
}

fn pickle_dump_global(out: &mut String, module: &str, name: &str) {
    out.push('c');
    out.push_str(module);
    out.push('\n');
    out.push_str(name);
    out.push('\n');
}

fn pickle_decode_latin1(input: &[u8]) -> String {
    input.iter().map(|&b| char::from(b)).collect()
}

fn pickle_string_repr(_py: &crate::PyToken<'_>, value: &str) -> Result<String, u64> {
    let Some(value_bits) = alloc_string_bits(_py, value) else {
        return Err(MoltObject::none().bits());
    };
    let rendered = format_obj(_py, obj_from_bits(value_bits));
    dec_ref_bits(_py, value_bits);
    Ok(rendered)
}

fn pickle_dump_list_payload(
    _py: &crate::PyToken<'_>,
    values: &[u64],
    protocol: i64,
    out: &mut String,
) -> Result<(), u64> {
    out.push('(');
    out.push('l');
    for &item_bits in values {
        pickle_dump_obj(_py, item_bits, protocol, out)?;
        out.push('a');
    }
    Ok(())
}

fn pickle_dump_obj(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    protocol: i64,
    out: &mut String,
) -> Result<(), u64> {
    let obj = obj_from_bits(obj_bits);
    if obj.is_none() {
        out.push('N');
        return Ok(());
    }
    if let Some(value) = obj.as_bool() {
        if value {
            out.push_str("I01\n");
        } else {
            out.push_str("I00\n");
        }
        return Ok(());
    }
    if let Some(value) = obj.as_int() {
        out.push('I');
        out.push_str(value.to_string().as_str());
        out.push('\n');
        return Ok(());
    }
    if let Some(value) = obj.as_float() {
        out.push('F');
        out.push_str(value.to_string().as_str());
        out.push('\n');
        return Ok(());
    }
    let Some(ptr) = obj.as_ptr() else {
        let message = format!("pickle.dumps: unsupported type: {}", type_name(_py, obj));
        return Err(pickle_raise(_py, &message));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id == crate::TYPE_ID_BIGINT {
        out.push('I');
        out.push_str(format_obj(_py, obj).as_str());
        out.push('\n');
        return Ok(());
    }
    if type_id == TYPE_ID_STRING {
        out.push('S');
        out.push_str(format_obj(_py, obj).as_str());
        out.push('\n');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_BYTES {
        let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
            return Err(pickle_raise(
                _py,
                "pickle.dumps: internal error reading bytes payload",
            ));
        };
        pickle_dump_global(out, "_codecs", "encode");
        out.push('(');
        let latin1 = pickle_decode_latin1(raw);
        let latin1_repr = pickle_string_repr(_py, &latin1)?;
        out.push('S');
        out.push_str(&latin1_repr);
        out.push('\n');
        let encoding_repr = pickle_string_repr(_py, "latin1")?;
        out.push('S');
        out.push_str(&encoding_repr);
        out.push('\n');
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_BYTEARRAY {
        let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
            return Err(pickle_raise(
                _py,
                "pickle.dumps: internal error reading bytearray payload",
            ));
        };
        pickle_dump_global(out, "builtins", "bytearray");
        out.push('(');
        let bytes_ptr = crate::alloc_bytes(_py, raw);
        if bytes_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let dumped = pickle_dump_obj(_py, bytes_bits, protocol, out);
        dec_ref_bits(_py, bytes_bits);
        dumped?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == TYPE_ID_TUPLE {
        out.push('(');
        for &item_bits in unsafe { seq_vec_ref(ptr) }.iter() {
            pickle_dump_obj(_py, item_bits, protocol, out)?;
        }
        out.push('t');
        return Ok(());
    }
    if type_id == TYPE_ID_LIST {
        let values = unsafe { seq_vec_ref(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        return Ok(());
    }
    if type_id == TYPE_ID_DICT {
        out.push('(');
        out.push('d');
        let pairs = unsafe { crate::dict_order(ptr).clone() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            pickle_dump_obj(_py, pairs[idx], protocol, out)?;
            pickle_dump_obj(_py, pairs[idx + 1], protocol, out)?;
            out.push('s');
            idx += 2;
        }
        return Ok(());
    }
    if type_id == crate::TYPE_ID_SET {
        pickle_dump_global(out, "builtins", "set");
        out.push('(');
        let values = unsafe { crate::set_order(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_FROZENSET {
        pickle_dump_global(out, "builtins", "frozenset");
        out.push('(');
        let values = unsafe { crate::set_order(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_SLICE {
        pickle_dump_global(out, "builtins", "slice");
        out.push('(');
        pickle_dump_obj(_py, unsafe { crate::slice_start_bits(ptr) }, protocol, out)?;
        pickle_dump_obj(_py, unsafe { crate::slice_stop_bits(ptr) }, protocol, out)?;
        pickle_dump_obj(_py, unsafe { crate::slice_step_bits(ptr) }, protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    let message = format!("pickle.dumps: unsupported type: {}", type_name(_py, obj));
    Err(pickle_raise(_py, &message))
}

fn pickle_read_line<'a>(
    _py: &crate::PyToken<'_>,
    text: &'a str,
    idx: &mut usize,
) -> Result<&'a str, u64> {
    let bytes = text.as_bytes();
    if *idx > bytes.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = bytes[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&text[start..end])
}

fn pickle_parse_string_literal(text: &str) -> Result<String, &'static str> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return Err("pickle.loads: invalid string literal");
    }
    let quote = chars[0];
    if (quote != '\'' && quote != '"') || chars[chars.len() - 1] != quote {
        return Err("pickle.loads: invalid string literal");
    }
    let mut out = String::new();
    let mut idx = 1usize;
    let end = chars.len() - 1;
    while idx < end {
        let ch = chars[idx];
        if ch != '\\' {
            out.push(ch);
            idx += 1;
            continue;
        }
        idx += 1;
        if idx >= end {
            return Err("pickle.loads: invalid escape sequence");
        }
        let esc = chars[idx];
        idx += 1;
        match esc {
            'a' => out.push('\u{0007}'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{000b}'),
            '\\' | '\'' | '"' => out.push(esc),
            'x' => {
                if idx + 2 > end {
                    return Err("pickle.loads: invalid hex escape");
                }
                let hex_text: String = chars[idx..idx + 2].iter().collect();
                let Ok(value) = u32::from_str_radix(&hex_text, 16) else {
                    return Err("pickle.loads: invalid hex escape");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid hex escape");
                };
                out.push(decoded);
                idx += 2;
            }
            'u' => {
                if idx + 4 > end {
                    return Err("pickle.loads: invalid unicode escape");
                }
                let hex_text: String = chars[idx..idx + 4].iter().collect();
                let Ok(value) = u32::from_str_radix(&hex_text, 16) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                out.push(decoded);
                idx += 4;
            }
            'U' => {
                if idx + 8 > end {
                    return Err("pickle.loads: invalid unicode escape");
                }
                let hex_text: String = chars[idx..idx + 8].iter().collect();
                let Ok(value) = u32::from_str_radix(&hex_text, 16) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                out.push(decoded);
                idx += 8;
            }
            '0'..='7' => {
                let mut octal = String::new();
                octal.push(esc);
                let limit = (idx + 2).min(end);
                while idx < limit && matches!(chars[idx], '0'..='7') {
                    octal.push(chars[idx]);
                    idx += 1;
                }
                let Ok(value) = u32::from_str_radix(&octal, 8) else {
                    return Err("pickle.loads: invalid escape sequence");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid escape sequence");
                };
                out.push(decoded);
            }
            _ => return Err("pickle.loads: invalid escape sequence"),
        }
    }
    Ok(out)
}

fn pickle_parse_int_bits(_py: &crate::PyToken<'_>, text: &str) -> Result<u64, u64> {
    if let Ok(value) = text.parse::<i64>() {
        return Ok(MoltObject::from_int(value).bits());
    }
    let Some(text_bits) = alloc_string_bits(_py, text) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = unsafe { call_callable1(_py, builtin_classes(_py).int, text_bits) };
    dec_ref_bits(_py, text_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_parse_long_line_bits(_py: &crate::PyToken<'_>, text: &str) -> Result<u64, u64> {
    let trimmed = text.trim_end_matches(['L', 'l']);
    let Some(text_bits) = alloc_string_bits(_py, trimmed) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = unsafe { call_callable1(_py, builtin_classes(_py).int, text_bits) };
    dec_ref_bits(_py, text_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_parse_float_bits(_py: &crate::PyToken<'_>, text: &str) -> Result<u64, u64> {
    let Some(text_bits) = alloc_string_bits(_py, text) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = unsafe { call_callable1(_py, builtin_classes(_py).float, text_bits) };
    dec_ref_bits(_py, text_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_parse_memo_key(_py: &crate::PyToken<'_>, text: &str) -> Result<i64, u64> {
    text.parse::<i64>()
        .map_err(|_| pickle_raise(_py, "pickle.loads: invalid memo key"))
}

fn pickle_resolve_global(module: &str, name: &str) -> Option<PickleGlobal> {
    match (module, name) {
        ("_codecs", "encode") => Some(PickleGlobal::CodecsEncode),
        ("builtins", "bytearray") | ("__builtin__", "bytearray") => Some(PickleGlobal::Bytearray),
        ("builtins", "slice") | ("__builtin__", "slice") => Some(PickleGlobal::Slice),
        ("builtins", "set") | ("__builtin__", "set") => Some(PickleGlobal::Set),
        ("builtins", "frozenset") | ("__builtin__", "frozenset") => Some(PickleGlobal::FrozenSet),
        ("builtins", "list") | ("__builtin__", "list") => Some(PickleGlobal::List),
        ("builtins", "tuple") | ("__builtin__", "tuple") => Some(PickleGlobal::Tuple),
        ("builtins", "dict") | ("__builtin__", "dict") => Some(PickleGlobal::Dict),
        _ => None,
    }
}

fn pickle_global_callable_bits(_py: &crate::PyToken<'_>, global: PickleGlobal) -> Result<u64, u64> {
    match global {
        PickleGlobal::CodecsEncode => Err(pickle_raise(
            _py,
            "pickle.loads: _codecs.encode cannot be materialized as a standalone callable",
        )),
        PickleGlobal::Bytearray => Ok(builtin_classes(_py).bytearray),
        PickleGlobal::Slice => Ok(builtin_classes(_py).slice),
        PickleGlobal::Set => Ok(builtin_classes(_py).set),
        PickleGlobal::FrozenSet => Ok(builtin_classes(_py).frozenset),
        PickleGlobal::List => Ok(builtin_classes(_py).list),
        PickleGlobal::Tuple => Ok(builtin_classes(_py).tuple),
        PickleGlobal::Dict => Ok(builtin_classes(_py).dict),
    }
}

fn pickle_stack_item_to_value(
    _py: &crate::PyToken<'_>,
    item: &PickleStackItem,
) -> Result<u64, u64> {
    match item {
        PickleStackItem::Value(bits) => Ok(*bits),
        PickleStackItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleStackItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

fn pickle_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
) -> Result<Vec<PickleStackItem>, u64> {
    let mut out: Vec<PickleStackItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleStackItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

fn pickle_items_to_value_bits(
    _py: &crate::PyToken<'_>,
    items: &[PickleStackItem],
) -> Result<Vec<u64>, u64> {
    let mut out: Vec<u64> = Vec::with_capacity(items.len());
    for item in items {
        out.push(pickle_stack_item_to_value(_py, item)?);
    }
    Ok(out)
}

fn pickle_pop_stack_item(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
    message: &'static str,
) -> Result<PickleStackItem, u64> {
    stack.pop().ok_or_else(|| pickle_raise(_py, message))
}

fn pickle_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
    message: &'static str,
) -> Result<u64, u64> {
    let item = pickle_pop_stack_item(_py, stack, message)?;
    pickle_stack_item_to_value(_py, &item)
}

fn pickle_call_with_args(_py: &crate::PyToken<'_>, callable_bits: u64, args: &[u64]) -> u64 {
    match args.len() {
        0 => unsafe { call_callable0(_py, callable_bits) },
        1 => unsafe { call_callable1(_py, callable_bits, args[0]) },
        2 => unsafe { call_callable2(_py, callable_bits, args[0], args[1]) },
        3 => unsafe { call_callable3(_py, callable_bits, args[0], args[1], args[2]) },
        _ => {
            let builder_bits = crate::molt_callargs_new(args.len() as u64, 0);
            for &arg_bits in args {
                let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg_bits) };
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            crate::molt_call_bind(callable_bits, builder_bits)
        }
    }
}

fn pickle_encode_text(_py: &crate::PyToken<'_>, text: &str, encoding: &str) -> Result<u64, u64> {
    let normalized = encoding.to_ascii_lowercase();
    let bytes: Vec<u8> = match normalized.as_str() {
        "utf-8" | "utf8" => text.as_bytes().to_vec(),
        "latin1" | "latin-1" => {
            let mut out: Vec<u8> = Vec::with_capacity(text.chars().count());
            for ch in text.chars() {
                let code = ch as u32;
                if code > 0xff {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: latin1 encoding failed for _codecs.encode payload",
                    ));
                }
                out.push(code as u8);
            }
            out
        }
        "ascii" => {
            let mut out: Vec<u8> = Vec::with_capacity(text.chars().count());
            for ch in text.chars() {
                let code = ch as u32;
                if code > 0x7f {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: ascii encoding failed for _codecs.encode payload",
                    ));
                }
                out.push(code as u8);
            }
            out
        }
        _ => {
            let message = format!(
                "pickle.loads: unsupported encoding {:?} for _codecs.encode",
                encoding
            );
            return Err(pickle_raise(_py, &message));
        }
    };
    let out_ptr = crate::alloc_bytes(_py, &bytes);
    if out_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

fn pickle_apply_reduce(
    _py: &crate::PyToken<'_>,
    func_item: PickleStackItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args: Vec<u64> = unsafe { seq_vec_ref(args_ptr).to_vec() };
    match func_item {
        PickleStackItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark cannot be called")),
        PickleStackItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 2 {
                let Some(name) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                name
            } else {
                "utf-8".to_string()
            };
            pickle_encode_text(_py, &text, &encoding)
        }
        PickleStackItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(out_bits)
            }
        }
        PickleStackItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(out_bits)
            }
        }
    }
}

const PICKLE_PROTO_3: i64 = 3;
const PICKLE_PROTO_4: i64 = 4;
const PICKLE_PROTO_5: i64 = 5;

const PICKLE_OP_PROTO: u8 = 0x80;
const PICKLE_OP_STOP: u8 = b'.';
const PICKLE_OP_POP: u8 = b'0';
const PICKLE_OP_POP_MARK: u8 = b'1';
const PICKLE_OP_MARK: u8 = b'(';
const PICKLE_OP_NONE: u8 = b'N';
const PICKLE_OP_NEWTRUE: u8 = 0x88;
const PICKLE_OP_NEWFALSE: u8 = 0x89;
const PICKLE_OP_INT: u8 = b'I';
const PICKLE_OP_LONG: u8 = b'L';
const PICKLE_OP_BININT: u8 = b'J';
const PICKLE_OP_BININT1: u8 = b'K';
const PICKLE_OP_BININT2: u8 = b'M';
const PICKLE_OP_LONG1: u8 = 0x8a;
const PICKLE_OP_LONG4: u8 = 0x8b;
const PICKLE_OP_FLOAT: u8 = b'F';
const PICKLE_OP_BINFLOAT: u8 = b'G';
const PICKLE_OP_STRING: u8 = b'S';
const PICKLE_OP_BINUNICODE: u8 = b'X';
const PICKLE_OP_SHORT_BINUNICODE: u8 = 0x8c;
const PICKLE_OP_UNICODE: u8 = b'V';
const PICKLE_OP_BINBYTES: u8 = b'B';
const PICKLE_OP_SHORT_BINBYTES: u8 = b'C';
const PICKLE_OP_BINBYTES8: u8 = 0x8e;
const PICKLE_OP_BYTEARRAY8: u8 = 0x96;
const PICKLE_OP_EMPTY_TUPLE: u8 = b')';
const PICKLE_OP_TUPLE: u8 = b't';
const PICKLE_OP_TUPLE1: u8 = 0x85;
const PICKLE_OP_TUPLE2: u8 = 0x86;
const PICKLE_OP_TUPLE3: u8 = 0x87;
const PICKLE_OP_EMPTY_LIST: u8 = b']';
const PICKLE_OP_LIST: u8 = b'l';
const PICKLE_OP_APPEND: u8 = b'a';
const PICKLE_OP_APPENDS: u8 = b'e';
const PICKLE_OP_EMPTY_DICT: u8 = b'}';
const PICKLE_OP_DICT: u8 = b'd';
const PICKLE_OP_SETITEM: u8 = b's';
const PICKLE_OP_SETITEMS: u8 = b'u';
const PICKLE_OP_EMPTY_SET: u8 = 0x8f;
const PICKLE_OP_ADDITEMS: u8 = 0x90;
const PICKLE_OP_FROZENSET: u8 = 0x91;
const PICKLE_OP_GLOBAL: u8 = b'c';
const PICKLE_OP_STACK_GLOBAL: u8 = 0x93;
const PICKLE_OP_REDUCE: u8 = b'R';
const PICKLE_OP_BUILD: u8 = b'b';
const PICKLE_OP_NEWOBJ: u8 = 0x81;
const PICKLE_OP_NEWOBJ_EX: u8 = 0x92;
const PICKLE_OP_PUT: u8 = b'p';
const PICKLE_OP_BINPUT: u8 = b'q';
const PICKLE_OP_LONG_BINPUT: u8 = b'r';
const PICKLE_OP_GET: u8 = b'g';
const PICKLE_OP_BINGET: u8 = b'h';
const PICKLE_OP_LONG_BINGET: u8 = b'j';
const PICKLE_OP_MEMOIZE: u8 = 0x94;
const PICKLE_OP_PERSID: u8 = b'P';
const PICKLE_OP_BINPERSID: u8 = b'Q';
const PICKLE_OP_EXT1: u8 = 0x82;
const PICKLE_OP_EXT2: u8 = 0x83;
const PICKLE_OP_EXT4: u8 = 0x84;
const PICKLE_OP_FRAME: u8 = 0x95;
const PICKLE_OP_NEXT_BUFFER: u8 = 0x97;
const PICKLE_OP_READONLY_BUFFER: u8 = 0x98;

const PICKLE_RECURSION_LIMIT: usize = 1_000;

#[derive(Clone, Debug)]
enum PickleVmItem {
    Value(u64),
    Global(PickleGlobal),
    Mark,
}

struct PickleDumpState {
    protocol: i64,
    out: Vec<u8>,
    memo: HashMap<u64, u32>,
    next_memo: u32,
    depth: usize,
    persistent_id_bits: Option<u64>,
    buffer_callback_bits: Option<u64>,
    dispatch_table_bits: Option<u64>,
}

impl PickleDumpState {
    fn new(
        protocol: i64,
        persistent_id_bits: Option<u64>,
        buffer_callback_bits: Option<u64>,
        dispatch_table_bits: Option<u64>,
    ) -> Self {
        Self {
            protocol,
            out: Vec::with_capacity(256),
            memo: HashMap::new(),
            next_memo: 0,
            depth: 0,
            persistent_id_bits,
            buffer_callback_bits,
            dispatch_table_bits,
        }
    }

    fn push(&mut self, op: u8) {
        self.out.push(op);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
}

fn pickle_option_callable_bits(
    _py: &crate::PyToken<'_>,
    maybe_bits: u64,
    name: &str,
) -> Result<Option<u64>, u64> {
    if obj_from_bits(maybe_bits).is_none() {
        return Ok(None);
    }
    if !is_truthy(_py, obj_from_bits(molt_is_callable(maybe_bits))) {
        let message = format!("pickle {name} must be callable");
        return Err(raise_exception::<u64>(_py, "TypeError", &message));
    }
    Ok(Some(maybe_bits))
}

fn pickle_input_to_bytes(_py: &crate::PyToken<'_>, data_bits: u64) -> Result<Vec<u8>, u64> {
    if let Some(ptr) = obj_from_bits(data_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        return Ok(raw.to_vec());
    }
    if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
        return Ok(text.into_bytes());
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "pickle data must be bytes, bytearray, or str",
    ))
}

fn pickle_read_u8(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u8, u64> {
    if *idx >= data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let byte = data[*idx];
    *idx += 1;
    Ok(byte)
}

fn pickle_read_exact<'a>(
    data: &'a [u8],
    idx: &mut usize,
    n: usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if data.len().saturating_sub(*idx) < n {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let end = start + n;
    *idx = end;
    Ok(&data[start..end])
}

fn pickle_read_line_bytes<'a>(
    data: &'a [u8],
    idx: &mut usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if *idx > data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = data[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&data[start..end])
}

fn pickle_read_u16_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u16, u64> {
    let raw = pickle_read_exact(data, idx, 2, _py)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn pickle_read_u32_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u32, u64> {
    let raw = pickle_read_exact(data, idx, 4, _py)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn pickle_read_u64_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn pickle_parse_long_bytes_bits(_py: &crate::PyToken<'_>, raw: &[u8]) -> Result<u64, u64> {
    if raw.is_empty() {
        return Ok(MoltObject::from_int(0).bits());
    }
    if raw.len() > 8 {
        return Err(pickle_raise(
            _py,
            "pickle.loads: LONG payload exceeds Molt int range",
        ));
    }
    let negative = (raw[raw.len() - 1] & 0x80) != 0;
    let mut bytes = if negative { [0xff; 8] } else { [0u8; 8] };
    bytes[..raw.len()].copy_from_slice(raw);
    Ok(MoltObject::from_int(i64::from_le_bytes(bytes)).bits())
}

fn pickle_read_f64_be(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<f64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(f64::from_bits(u64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ])))
}

fn pickle_decode_utf8(_py: &crate::PyToken<'_>, raw: &[u8], ctx: &str) -> Result<String, u64> {
    String::from_utf8(raw.to_vec()).map_err(|_| {
        let msg = format!("pickle.loads: invalid UTF-8 while decoding {ctx}");
        pickle_raise(_py, &msg)
    })
}

fn pickle_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    urllib_request_attr_optional(_py, obj_bits, name)
}

fn pickle_attr_required(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<u64, u64> {
    match pickle_attr_optional(_py, obj_bits, name)? {
        Some(bits) => Ok(bits),
        None => {
            let name_text = std::str::from_utf8(name).unwrap_or("attribute");
            let msg = format!("pickle: missing required attribute {name_text}");
            Err(pickle_raise(_py, &msg))
        }
    }
}

fn pickle_emit_u32_le(state: &mut PickleDumpState, value: u32) {
    state.extend(&value.to_le_bytes());
}

fn pickle_emit_u64_le(state: &mut PickleDumpState, value: u64) {
    state.extend(&value.to_le_bytes());
}

fn pickle_emit_memo_put(state: &mut PickleDumpState, index: u32) {
    if state.protocol >= PICKLE_PROTO_4 {
        state.push(PICKLE_OP_MEMOIZE);
        return;
    }
    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINPUT);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINPUT);
        pickle_emit_u32_le(state, index);
    }
}

fn pickle_emit_memo_get(state: &mut PickleDumpState, index: u32) {
    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINGET);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINGET);
        pickle_emit_u32_le(state, index);
    }
}

fn pickle_memo_key(bits: u64) -> Option<u64> {
    let obj = obj_from_bits(bits);
    if obj.as_ptr().is_some() {
        Some(bits)
    } else {
        None
    }
}

fn pickle_memo_lookup(state: &PickleDumpState, bits: u64) -> Option<u32> {
    let key = pickle_memo_key(bits)?;
    state.memo.get(&key).copied()
}

fn pickle_memo_store(state: &mut PickleDumpState, bits: u64) -> Option<u32> {
    let key = pickle_memo_key(bits)?;
    if let Some(found) = state.memo.get(&key).copied() {
        return Some(found);
    }
    let index = state.next_memo;
    state.next_memo = state.next_memo.saturating_add(1);
    state.memo.insert(key, index);
    pickle_emit_memo_put(state, index);
    Some(index)
}

fn pickle_memo_store_if_absent(state: &mut PickleDumpState, bits: u64) -> Option<u32> {
    if let Some(found) = pickle_memo_lookup(state, bits) {
        return Some(found);
    }
    pickle_memo_store(state, bits)
}

fn pickle_emit_proto_header(state: &mut PickleDumpState) {
    state.push(PICKLE_OP_PROTO);
    state.push(state.protocol as u8);
}

fn pickle_emit_global_opcode(state: &mut PickleDumpState, module: &str, name: &str) {
    state.push(PICKLE_OP_GLOBAL);
    state.extend(module.as_bytes());
    state.push(b'\n');
    state.extend(name.as_bytes());
    state.push(b'\n');
}

fn pickle_lookup_extension_code(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<Option<i64>, u64> {
    let registry_bits = pickle_resolve_global_bits(_py, "copyreg", "_extension_registry")?;
    let Some(registry_ptr) = obj_from_bits(registry_bits).as_ptr() else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    if unsafe { object_type_id(registry_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    }
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let key_ptr = alloc_tuple(_py, &[module_bits, name_bits]);
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    let Some(key_ptr) = (!key_ptr.is_null()).then_some(key_ptr) else {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let code_bits = unsafe { dict_get_in_place(_py, registry_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(code_bits) = code_bits else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    let Some(code) = to_i64(obj_from_bits(code_bits)) else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    dec_ref_bits(_py, registry_bits);
    if code <= 0 {
        return Ok(None);
    }
    Ok(Some(code))
}

fn pickle_emit_global_ref(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<bool, u64> {
    let Some(module_bits) = pickle_attr_optional(_py, obj_bits, b"__module__")? else {
        return Ok(false);
    };
    let Some(name_bits) = pickle_attr_optional(_py, obj_bits, b"__name__")? else {
        dec_ref_bits(_py, module_bits);
        return Ok(false);
    };
    let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return Ok(false);
    };
    let Some(attr_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return Ok(false);
    };
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    if state.protocol >= 2
        && let Some(code) = pickle_lookup_extension_code(_py, &module_name, &attr_name)?
    {
        if code <= u8::MAX as i64 {
            state.push(PICKLE_OP_EXT1);
            state.push(code as u8);
            return Ok(true);
        }
        if code <= u16::MAX as i64 {
            state.push(PICKLE_OP_EXT2);
            state.extend(&(code as u16).to_le_bytes());
            return Ok(true);
        }
        if code <= u32::MAX as i64 {
            state.push(PICKLE_OP_EXT4);
            state.extend(&(code as u32).to_le_bytes());
            return Ok(true);
        }
    }
    if state.protocol >= PICKLE_PROTO_4 {
        pickle_dump_unicode_binary(_py, state, module_name.as_str())?;
        pickle_dump_unicode_binary(_py, state, attr_name.as_str())?;
        state.push(PICKLE_OP_STACK_GLOBAL);
        return Ok(true);
    }
    pickle_emit_global_opcode(state, module_name.as_str(), attr_name.as_str());
    Ok(true)
}

fn pickle_dump_unicode_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    text: &str,
) -> Result<(), u64> {
    let raw = text.as_bytes();
    if raw.len() <= u8::MAX as usize && state.protocol >= PICKLE_PROTO_4 {
        state.push(PICKLE_OP_SHORT_BINUNICODE);
        state.push(raw.len() as u8);
        state.extend(raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.push(PICKLE_OP_BINUNICODE);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
        return Ok(());
    }
    state.push(0x8d);
    pickle_emit_u64_le(state, raw.len() as u64);
    state.extend(raw);
    Ok(())
}

fn pickle_dump_bytes_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_SHORT_BINBYTES);
        state.push(raw.len() as u8);
        state.extend(raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.push(PICKLE_OP_BINBYTES);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
        return Ok(());
    }
    state.push(PICKLE_OP_BINBYTES8);
    pickle_emit_u64_le(state, raw.len() as u64);
    state.extend(raw);
    Ok(())
}

fn pickle_dump_bytearray_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if state.protocol >= PICKLE_PROTO_5 {
        state.push(PICKLE_OP_BYTEARRAY8);
        pickle_emit_u64_le(state, raw.len() as u64);
        state.extend(raw);
        return Ok(());
    }
    // Protocols 2-4: bytearray(bytes(...)) reduce path.
    pickle_emit_global_opcode(state, "builtins", "bytearray");
    let bytes_ptr = crate::alloc_bytes(_py, raw);
    if bytes_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
    let dumped = pickle_dump_obj_binary(_py, state, bytes_bits, true);
    dec_ref_bits(_py, bytes_bits);
    dumped?;
    state.push(PICKLE_OP_TUPLE1);
    state.push(PICKLE_OP_REDUCE);
    Ok(())
}

fn pickle_long_bytes_from_i64(value: i64) -> Vec<u8> {
    let mut raw = value.to_le_bytes().to_vec();
    while raw.len() > 1 {
        let last = raw[raw.len() - 1];
        let prev = raw[raw.len() - 2];
        let drop_zero = last == 0x00 && (prev & 0x80) == 0;
        let drop_ff = last == 0xff && (prev & 0x80) != 0;
        if drop_zero || drop_ff {
            raw.pop();
        } else {
            break;
        }
    }
    raw
}

fn pickle_dump_int_binary(state: &mut PickleDumpState, value: i64) {
    if (0..=u8::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT1);
        state.push(value as u8);
        return;
    }
    if (0..=u16::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT2);
        state.extend(&(value as u16).to_le_bytes());
        return;
    }
    if (i32::MIN as i64..=i32::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT);
        state.extend(&(value as i32).to_le_bytes());
        return;
    }
    let raw = pickle_long_bytes_from_i64(value);
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_LONG1);
        state.push(raw.len() as u8);
        state.extend(raw.as_slice());
    } else {
        state.push(PICKLE_OP_LONG4);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw.as_slice());
    }
}

fn pickle_dump_float_binary(state: &mut PickleDumpState, value: f64) {
    state.push(PICKLE_OP_BINFLOAT);
    state.extend(&value.to_bits().to_be_bytes());
}

fn pickle_dump_maybe_persistent(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<bool, u64> {
    let Some(callback_bits) = state.persistent_id_bits else {
        return Ok(false);
    };
    let pid_bits = unsafe { call_callable1(_py, callback_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(pid_bits).is_none() {
        return Ok(false);
    }
    if state.protocol == 0
        && let Some(pid_text) = string_obj_to_owned(obj_from_bits(pid_bits))
    {
        state.push(PICKLE_OP_PERSID);
        state.extend(pid_text.as_bytes());
        state.push(b'\n');
        return Ok(true);
    }
    pickle_dump_obj_binary(_py, state, pid_bits, false)?;
    state.push(PICKLE_OP_BINPERSID);
    Ok(true)
}

fn pickle_buffer_value_to_bytes(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        let out_ptr = crate::alloc_bytes(_py, raw);
        if out_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(out_ptr).bits());
    }
    let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
    Err(pickle_raise(_py, &msg))
}

fn pickle_buffer_value_to_memoryview(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    let view_bits = crate::molt_memoryview_new(value_bits);
    if exception_pending(_py) {
        let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(view_bits)
}

fn pickle_external_buffer_to_memoryview(
    _py: &crate::PyToken<'_>,
    item_bits: u64,
) -> Result<u64, u64> {
    if let Ok(bits) = pickle_buffer_value_to_memoryview(_py, item_bits, "out-of-band buffer") {
        return Ok(bits);
    }
    if let Some(raw_method_bits) = pickle_attr_optional(_py, item_bits, b"raw")? {
        let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
        dec_ref_bits(_py, raw_method_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return pickle_buffer_value_to_memoryview(_py, raw_bits, "out-of-band buffer");
    }
    Err(pickle_raise(
        _py,
        "pickle.loads: out-of-band buffer must be bytes-like or expose raw()",
    ))
}

fn pickle_next_external_buffer_bits(
    _py: &crate::PyToken<'_>,
    buffers_iter_bits: Option<u64>,
) -> Result<u64, u64> {
    let Some(iter_bits) = buffers_iter_bits else {
        return Err(pickle_raise(
            _py,
            "pickle.loads: NEXT_BUFFER requires buffers argument",
        ));
    };
    let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
    if done {
        return Err(pickle_raise(
            _py,
            "pickle.loads: not enough out-of-band buffers",
        ));
    }
    pickle_external_buffer_to_memoryview(_py, item_bits)
}

fn pickle_dump_maybe_out_of_band_buffer(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    readonly: bool,
) -> Result<bool, u64> {
    let Some(callback_bits) = state.buffer_callback_bits else {
        return Ok(false);
    };
    if state.protocol < PICKLE_PROTO_5 {
        return Ok(false);
    }
    let callback_result_bits = unsafe { call_callable1(_py, callback_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let in_band = is_truthy(_py, obj_from_bits(callback_result_bits));
    if !obj_from_bits(callback_result_bits).is_none() {
        dec_ref_bits(_py, callback_result_bits);
    }
    if in_band {
        return Ok(false);
    }
    state.push(PICKLE_OP_NEXT_BUFFER);
    if readonly {
        state.push(PICKLE_OP_READONLY_BUFFER);
    }
    // Do NOT memo out-of-band buffers — each reference must emit its own
    // NEXT_BUFFER opcode so every buffer slot is consumed during loads.
    Ok(true)
}

fn pickle_extract_picklebuffer_payload(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<Option<(u64, bool)>, u64> {
    let marker_bits = match pickle_attr_optional(_py, obj_bits, b"__molt_pickle_buffer__")? {
        Some(bits) => bits,
        None => return Ok(None),
    };
    let is_marker = is_truthy(_py, obj_from_bits(marker_bits));
    dec_ref_bits(_py, marker_bits);
    if !is_marker {
        return Ok(None);
    }
    let raw_method_bits = pickle_attr_required(_py, obj_bits, b"raw")?;
    let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
    dec_ref_bits(_py, raw_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let readonly = if let Some(raw_ptr) = obj_from_bits(raw_bits).as_ptr() {
        let raw_type = unsafe { object_type_id(raw_ptr) };
        if raw_type == crate::TYPE_ID_BYTEARRAY {
            false
        } else if raw_type == crate::TYPE_ID_MEMORYVIEW {
            unsafe { crate::memoryview_readonly(raw_ptr) }
        } else {
            true
        }
    } else {
        true
    };
    let payload_bits = pickle_buffer_value_to_bytes(_py, raw_bits, "PickleBuffer.raw() payload");
    if !obj_from_bits(raw_bits).is_none() {
        dec_ref_bits(_py, raw_bits);
    }
    payload_bits.map(|bits| Some((bits, readonly)))
}

fn pickle_dispatch_reducer_from_table(
    _py: &crate::PyToken<'_>,
    dispatch_table_bits: u64,
    obj_bits: u64,
) -> Result<Option<u64>, u64> {
    let Some(ptr) = obj_from_bits(dispatch_table_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }
    let type_bits = type_of_bits(_py, obj_bits);
    let reducer_bits = unsafe { dict_get_in_place(_py, ptr, type_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(reducer_bits) = reducer_bits else {
        return Ok(None);
    };
    let out_bits = unsafe { call_callable1(_py, reducer_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(Some(out_bits))
}

fn pickle_reduce_value(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<Option<u64>, u64> {
    if let Some(dispatch_bits) = state.dispatch_table_bits
        && let Some(reduced) = pickle_dispatch_reducer_from_table(_py, dispatch_bits, obj_bits)?
    {
        return Ok(Some(reduced));
    }
    if let Some(reduce_ex_bits) = pickle_attr_optional(_py, obj_bits, b"__reduce_ex__")? {
        let out_bits = unsafe {
            call_callable1(
                _py,
                reduce_ex_bits,
                MoltObject::from_int(state.protocol).bits(),
            )
        };
        dec_ref_bits(_py, reduce_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    if let Some(reduce_bits) = pickle_attr_optional(_py, obj_bits, b"__reduce__")? {
        let out_bits = unsafe { call_callable0(_py, reduce_bits) };
        dec_ref_bits(_py, reduce_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    Ok(None)
}

fn pickle_dump_items_from_iterable(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    values_bits: u64,
    dict_items: bool,
    iterator_error_prefix: &str,
) -> Result<(), u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        let value_type = type_name(_py, obj_from_bits(values_bits));
        let msg = format!("{iterator_error_prefix}{value_type}");
        return Err(pickle_raise(_py, &msg));
    }
    state.push(PICKLE_OP_MARK);
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        if dict_items {
            let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            };
            if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            }
            let fields = unsafe { seq_vec_ref(item_ptr) };
            if fields.len() != 2 {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            }
            pickle_dump_obj_binary(_py, state, fields[0], true)?;
            pickle_dump_obj_binary(_py, state, fields[1], true)?;
        } else {
            pickle_dump_obj_binary(_py, state, item_bits, true)?;
        }
    }
    if dict_items {
        state.push(PICKLE_OP_SETITEMS);
    } else {
        state.push(PICKLE_OP_APPENDS);
    }
    Ok(())
}

fn pickle_dump_reduce_value(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    reduce_bits: u64,
    obj_bits: Option<u64>,
) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(reduce_bits).as_ptr() else {
        return Err(pickle_raise(
            _py,
            "__reduce__ must return a string or tuple",
        ));
    };
    let reduce_type = unsafe { object_type_id(ptr) };
    if reduce_type == TYPE_ID_STRING {
        let Some(global_name) = string_obj_to_owned(obj_from_bits(reduce_bits)) else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(obj_bits) = obj_bits else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(module_bits) = pickle_attr_optional(_py, obj_bits, b"__module__")? else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
            dec_ref_bits(_py, module_bits);
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        dec_ref_bits(_py, module_bits);
        let resolved_bits =
            pickle_resolve_global_bits(_py, module_name.as_str(), global_name.as_str())?;
        let matches = resolved_bits == obj_bits;
        if !obj_from_bits(resolved_bits).is_none() {
            dec_ref_bits(_py, resolved_bits);
        }
        if !matches {
            let obj_type = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!(
                "Can't pickle {obj_type}: it's not the same object as {}.{}",
                module_name, global_name
            );
            return Err(pickle_raise(_py, &msg));
        }
        if state.protocol >= PICKLE_PROTO_4 {
            pickle_dump_unicode_binary(_py, state, module_name.as_str())?;
            pickle_dump_unicode_binary(_py, state, global_name.as_str())?;
            state.push(PICKLE_OP_STACK_GLOBAL);
        } else {
            pickle_emit_global_opcode(state, module_name.as_str(), global_name.as_str());
        }
        let _ = pickle_memo_store_if_absent(state, obj_bits);
        return Ok(());
    }
    if reduce_type != TYPE_ID_TUPLE {
        return Err(pickle_raise(
            _py,
            "__reduce__ must return a string or tuple",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if !(2..=6).contains(&fields.len()) {
        return Err(pickle_raise(
            _py,
            "tuple returned by __reduce__ must contain 2 through 6 elements",
        ));
    }
    let callable_bits = fields[0];
    let callable_check = molt_is_callable(callable_bits);
    if !is_truthy(_py, obj_from_bits(callable_check)) {
        return Err(pickle_raise(
            _py,
            "first item of the tuple returned by __reduce__ must be callable",
        ));
    }
    let args_bits = fields[1];
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    }
    if fields.len() >= 4 && !obj_from_bits(fields[3]).is_none() {
        let iter_bits = molt_iter(fields[3]);
        if exception_pending(_py) {
            clear_exception(_py);
            let value_type = type_name(_py, obj_from_bits(fields[3]));
            let msg = format!(
                "fourth element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
        if !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        let iter_bits = molt_iter(fields[4]);
        if exception_pending(_py) {
            clear_exception(_py);
            let value_type = type_name(_py, obj_from_bits(fields[4]));
            let msg = format!(
                "fifth element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
        if !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
    }
    if fields.len() >= 6 && !obj_from_bits(fields[5]).is_none() {
        let setter_check = molt_is_callable(fields[5]);
        if !is_truthy(_py, obj_from_bits(setter_check)) {
            let value_type = type_name(_py, obj_from_bits(fields[5]));
            let msg = format!(
                "sixth element of the tuple returned by __reduce__ must be a function, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
    }
    pickle_dump_obj_binary(_py, state, callable_bits, true)?;
    pickle_dump_obj_binary(_py, state, args_bits, true)?;
    state.push(PICKLE_OP_REDUCE);
    if let Some(bits) = obj_bits {
        let _ = pickle_memo_store_if_absent(state, bits);
    }
    let state_bits = if fields.len() >= 3 {
        Some(fields[2])
    } else {
        None
    };
    let state_setter_bits = if fields.len() >= 6 {
        Some(fields[5])
    } else {
        None
    };
    if let Some(state_bits) = state_bits
        && !obj_from_bits(state_bits).is_none()
    {
        if let Some(state_setter_bits) = state_setter_bits {
            if !obj_from_bits(state_setter_bits).is_none() {
                let Some(obj_bits) = obj_bits else {
                    return Err(pickle_raise(
                        _py,
                        "pickle reducer state_setter requires object context",
                    ));
                };
                pickle_dump_obj_binary(_py, state, state_setter_bits, true)?;
                pickle_dump_obj_binary(_py, state, obj_bits, true)?;
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_TUPLE2);
                state.push(PICKLE_OP_REDUCE);
                state.push(PICKLE_OP_POP);
            } else {
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_BUILD);
            }
        } else {
            pickle_dump_obj_binary(_py, state, state_bits, true)?;
            state.push(PICKLE_OP_BUILD);
        }
    }
    if fields.len() >= 4 && !obj_from_bits(fields[3]).is_none() {
        pickle_dump_items_from_iterable(
            _py,
            state,
            fields[3],
            false,
            "fourth element of the tuple returned by __reduce__ must be an iterator, not ",
        )?;
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        pickle_dump_items_from_iterable(
            _py,
            state,
            fields[4],
            true,
            "fifth element of the tuple returned by __reduce__ must be an iterator, not ",
        )?;
    }
    Ok(())
}

fn pickle_empty_tuple_bits(_py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let tuple_ptr = alloc_tuple(_py, &[]);
    if tuple_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn pickle_require_tuple_bits(
    _py: &crate::PyToken<'_>,
    bits: u64,
    context: &str,
) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(pickle_raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(())
}

fn pickle_require_dict_bits(_py: &crate::PyToken<'_>, bits: u64, context: &str) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(pickle_raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(())
}

fn pickle_default_newobj_args(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<(u64, Option<u64>), u64> {
    if let Some(getnewargs_ex_bits) = pickle_attr_optional(_py, obj_bits, b"__getnewargs_ex__")? {
        let out_bits = unsafe { call_callable0(_py, getnewargs_ex_bits) };
        dec_ref_bits(_py, getnewargs_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(tuple_ptr) = obj_from_bits(out_bits).as_ptr() else {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        };
        if unsafe { object_type_id(tuple_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, out_bits);
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let fields = unsafe { seq_vec_ref(tuple_ptr).to_vec() };
        if fields.len() != 2 {
            dec_ref_bits(_py, out_bits);
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let args_bits = fields[0];
        let kwargs_bits = fields[1];
        pickle_require_tuple_bits(_py, args_bits, "__getnewargs_ex__ args")?;
        pickle_require_dict_bits(_py, kwargs_bits, "__getnewargs_ex__ kwargs")?;
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, kwargs_bits);
        dec_ref_bits(_py, out_bits);
        return Ok((args_bits, Some(kwargs_bits)));
    }

    if let Some(getnewargs_bits) = pickle_attr_optional(_py, obj_bits, b"__getnewargs__")? {
        let args_bits = unsafe { call_callable0(_py, getnewargs_bits) };
        dec_ref_bits(_py, getnewargs_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if let Err(err_bits) = pickle_require_tuple_bits(_py, args_bits, "__getnewargs__ value") {
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return Err(err_bits);
        }
        return Ok((args_bits, None));
    }

    Ok((pickle_empty_tuple_bits(_py)?, None))
}

fn pickle_dataclass_state_bits(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Result<Option<u64>, u64> {
    let desc_ptr = unsafe { crate::dataclass_desc_ptr(ptr) };
    if desc_ptr.is_null() {
        return Ok(None);
    }

    if unsafe { (*desc_ptr).slots } {
        let slot_state_ptr = alloc_dict_with_pairs(_py, &[]);
        if slot_state_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let slot_state_bits = MoltObject::from_ptr(slot_state_ptr).bits();
        let mut wrote_any = false;
        let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
        let field_names = unsafe { &(*desc_ptr).field_names };
        for (name, value_bits) in field_names.iter().zip(field_values.iter().copied()) {
            let Some(name_bits) = alloc_string_bits(_py, name) else {
                dec_ref_bits(_py, slot_state_bits);
                return Err(MoltObject::none().bits());
            };
            unsafe {
                crate::dict_set_in_place(_py, slot_state_ptr, name_bits, value_bits);
            }
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, slot_state_bits);
                return Err(MoltObject::none().bits());
            }
            wrote_any = true;
        }
        if !wrote_any {
            dec_ref_bits(_py, slot_state_bits);
            return Ok(None);
        }
        let tuple_ptr = alloc_tuple(_py, &[MoltObject::none().bits(), slot_state_bits]);
        dec_ref_bits(_py, slot_state_bits);
        if tuple_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(MoltObject::from_ptr(tuple_ptr).bits()));
    }

    if !unsafe { (*desc_ptr).slots } {
        let dict_bits = unsafe { crate::dataclass_dict_bits(ptr) };
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            inc_ref_bits(_py, dict_bits);
            return Ok(Some(dict_bits));
        }
    }

    let state_ptr = alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;

    let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
    let field_names = unsafe { &(*desc_ptr).field_names };
    for (name, value_bits) in field_names.iter().zip(field_values.iter().copied()) {
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, state_bits);
            return Err(MoltObject::none().bits());
        };
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return Err(MoltObject::none().bits());
        }
        wrote_any = true;
    }

    let extra_bits = unsafe { crate::dataclass_dict_bits(ptr) };
    if extra_bits != 0
        && let Some(extra_ptr) = obj_from_bits(extra_bits).as_ptr()
        && unsafe { object_type_id(extra_ptr) } == TYPE_ID_DICT
    {
        let pairs = unsafe { crate::dict_order(extra_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            unsafe {
                crate::dict_set_in_place(_py, state_ptr, pairs[idx], pairs[idx + 1]);
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, state_bits);
                return Err(MoltObject::none().bits());
            }
            wrote_any = true;
            idx += 2;
        }
    }

    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return Ok(None);
    }
    Ok(Some(state_bits))
}

fn pickle_object_slot_state_bits(
    _py: &crate::PyToken<'_>,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
        return Ok(None);
    }

    let class_dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
    let Some(class_dict_ptr) = obj_from_bits(class_dict_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(class_dict_ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }

    let Some(offsets_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
        return Err(MoltObject::none().bits());
    };
    let offsets_bits = unsafe { dict_get_in_place(_py, class_dict_ptr, offsets_name_bits) };
    dec_ref_bits(_py, offsets_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(offsets_bits) = offsets_bits else {
        return Ok(None);
    };
    let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }

    let slot_state_ptr = alloc_dict_with_pairs(_py, &[]);
    if slot_state_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let slot_state_bits = MoltObject::from_ptr(slot_state_ptr).bits();
    let mut wrote_any = false;
    let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let name_bits = pairs[idx];
        let offset_bits = pairs[idx + 1];
        idx += 2;
        let Some(offset) = to_i64(obj_from_bits(offset_bits)) else {
            continue;
        };
        if offset < 0 {
            continue;
        }
        let value_bits = unsafe { crate::object_field_get_ptr_raw(_py, ptr, offset as usize) };
        if exception_pending(_py) {
            dec_ref_bits(_py, slot_state_bits);
            return Err(MoltObject::none().bits());
        }
        if value_bits == missing_bits(_py) {
            dec_ref_bits(_py, value_bits);
            continue;
        }
        unsafe {
            crate::dict_set_in_place(_py, slot_state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, value_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, slot_state_bits);
            return Err(MoltObject::none().bits());
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, slot_state_bits);
        return Ok(None);
    }
    Ok(Some(slot_state_bits))
}

fn pickle_object_state_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let mut dict_state_bits: Option<u64> = None;
    // Try the fast path first: trailing __dict__ slot.
    let dict_bits = unsafe { crate::instance_dict_bits(ptr) };
    if dict_bits != 0
        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
        && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        && !unsafe { crate::dict_order(dict_ptr).is_empty() }
    {
        inc_ref_bits(_py, dict_bits);
        dict_state_bits = Some(dict_bits);
    }
    // Fall back to getattr(__dict__) when the trailing slot is empty/missing.
    // The compiler may store attributes in a dict accessible only through getattr.
    if dict_state_bits.is_none()
        && !exception_pending(_py)
        && let Some(dict_name_bits) = attr_name_bits_from_bytes(_py, b"__dict__")
    {
        let missing = missing_bits(_py);
        let attr_dict_bits = molt_getattr_builtin(obj_bits, dict_name_bits, missing);
        dec_ref_bits(_py, dict_name_bits);
        if !exception_pending(_py)
            && attr_dict_bits != missing
            && let Some(dict_ptr) = obj_from_bits(attr_dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            // attr_dict_bits already carries a reference from getattr.
            dict_state_bits = Some(attr_dict_bits);
        } else if attr_dict_bits != missing && !obj_from_bits(attr_dict_bits).is_none() {
            dec_ref_bits(_py, attr_dict_bits);
        }
        // Clear AttributeError if __dict__ wasn't found.
        if exception_pending(_py) {
            clear_exception(_py);
        }
    }

    let slot_state_bits = pickle_object_slot_state_bits(_py, ptr)?;
    let Some(slot_state_bits) = slot_state_bits else {
        return Ok(dict_state_bits);
    };

    let dict_or_none_bits = dict_state_bits.unwrap_or(MoltObject::none().bits());
    let tuple_ptr = alloc_tuple(_py, &[dict_or_none_bits, slot_state_bits]);
    if let Some(bits) = dict_state_bits {
        dec_ref_bits(_py, bits);
    }
    dec_ref_bits(_py, slot_state_bits);
    if tuple_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(Some(MoltObject::from_ptr(tuple_ptr).bits()))
}

fn pickle_default_instance_state(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
    type_id: u32,
) -> Result<Option<u64>, u64> {
    if let Some(getstate_bits) = pickle_attr_optional(_py, obj_bits, b"__getstate__")? {
        let state_bits = unsafe { call_callable0(_py, getstate_bits) };
        dec_ref_bits(_py, getstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(state_bits));
    }
    if type_id == crate::TYPE_ID_DATACLASS {
        return pickle_dataclass_state_bits(_py, ptr);
    }
    if type_id == crate::TYPE_ID_OBJECT {
        return pickle_object_state_bits(_py, obj_bits, ptr);
    }
    Ok(None)
}

fn pickle_dump_default_instance(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    ptr: *mut u8,
    type_id: u32,
) -> Result<bool, u64> {
    if type_id != crate::TYPE_ID_OBJECT && type_id != crate::TYPE_ID_DATACLASS {
        return Ok(false);
    }
    let cls_bits = unsafe { object_class_bits(ptr) };
    if cls_bits == 0 || obj_from_bits(cls_bits).as_ptr().is_none() {
        return Ok(false);
    }

    let (args_bits, kwargs_bits) = pickle_default_newobj_args(_py, obj_bits)?;
    let result = (|| -> Result<(), u64> {
        let mut kwargs_effective = kwargs_bits;
        if let Some(bits) = kwargs_effective {
            let Some(dict_ptr) = obj_from_bits(bits).as_ptr() else {
                return Err(pickle_raise(_py, "pickle.dumps: kwargs must be dict"));
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                return Err(pickle_raise(_py, "pickle.dumps: kwargs must be dict"));
            }
            if unsafe { crate::dict_order(dict_ptr).is_empty() } {
                kwargs_effective = None;
            }
        }

        if let Some(kwargs_bits) = kwargs_effective {
            if state.protocol >= PICKLE_PROTO_4 {
                pickle_dump_obj_binary(_py, state, cls_bits, true)?;
                pickle_dump_obj_binary(_py, state, args_bits, true)?;
                pickle_dump_obj_binary(_py, state, kwargs_bits, true)?;
                state.push(PICKLE_OP_NEWOBJ_EX);
            } else {
                pickle_emit_global_opcode(state, "copyreg", "__newobj_ex__");
                pickle_dump_obj_binary(_py, state, cls_bits, true)?;
                pickle_dump_obj_binary(_py, state, args_bits, true)?;
                pickle_dump_obj_binary(_py, state, kwargs_bits, true)?;
                state.push(PICKLE_OP_TUPLE3);
                state.push(PICKLE_OP_REDUCE);
            }
        } else {
            pickle_dump_obj_binary(_py, state, cls_bits, true)?;
            pickle_dump_obj_binary(_py, state, args_bits, true)?;
            state.push(PICKLE_OP_NEWOBJ);
        }

        let _ = pickle_memo_store_if_absent(state, obj_bits);
        if let Some(state_bits) = pickle_default_instance_state(_py, obj_bits, ptr, type_id)? {
            if !obj_from_bits(state_bits).is_none() {
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_BUILD);
            }
            if !obj_from_bits(state_bits).is_none() {
                dec_ref_bits(_py, state_bits);
            }
        }
        Ok(())
    })();

    if !obj_from_bits(args_bits).is_none() {
        dec_ref_bits(_py, args_bits);
    }
    if let Some(bits) = kwargs_bits
        && !obj_from_bits(bits).is_none()
    {
        dec_ref_bits(_py, bits);
    }
    result.map(|()| true)
}

fn pickle_dump_obj_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    allow_persistent_id: bool,
) -> Result<(), u64> {
    if state.depth >= PICKLE_RECURSION_LIMIT {
        return Err(pickle_raise(
            _py,
            "pickle.dumps: maximum recursion depth exceeded",
        ));
    }
    state.depth += 1;
    let result = (|| -> Result<(), u64> {
        if allow_persistent_id && pickle_dump_maybe_persistent(_py, state, obj_bits)? {
            return Ok(());
        }
        if let Some(index) = pickle_memo_lookup(state, obj_bits) {
            pickle_emit_memo_get(state, index);
            return Ok(());
        }
        let obj = obj_from_bits(obj_bits);
        if obj.is_none() {
            state.push(PICKLE_OP_NONE);
            return Ok(());
        }
        if let Some(value) = obj.as_bool() {
            state.push(if value {
                PICKLE_OP_NEWTRUE
            } else {
                PICKLE_OP_NEWFALSE
            });
            return Ok(());
        }
        if let Some(value) = obj.as_int() {
            pickle_dump_int_binary(state, value);
            return Ok(());
        }
        if let Some(value) = obj.as_float() {
            pickle_dump_float_binary(state, value);
            return Ok(());
        }
        let Some(ptr) = obj.as_ptr() else {
            let type_name = type_name(_py, obj);
            let msg = format!("pickle.dumps: unsupported type: {type_name}");
            return Err(pickle_raise(_py, &msg));
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_STRING {
            let text = string_obj_to_owned(obj)
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: string conversion failed"))?;
            pickle_dump_unicode_binary(_py, state, text.as_str())?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTES {
            let raw = unsafe { bytes_like_slice(ptr) }
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: bytes conversion failed"))?;
            if state.protocol < PICKLE_PROTO_3 {
                pickle_emit_global_opcode(state, "_codecs", "encode");
                let latin1 = pickle_decode_latin1(raw);
                pickle_dump_unicode_binary(_py, state, &latin1)?;
                pickle_dump_unicode_binary(_py, state, "latin1")?;
                state.push(PICKLE_OP_TUPLE2);
                state.push(PICKLE_OP_REDUCE);
            } else {
                pickle_dump_bytes_binary(_py, state, raw)?;
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTEARRAY {
            let raw = unsafe { bytes_like_slice(ptr) }
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: bytearray conversion failed"))?;
            pickle_dump_bytearray_binary(_py, state, raw)?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if let Some((payload_bits, readonly)) = pickle_extract_picklebuffer_payload(_py, obj_bits)?
        {
            if pickle_dump_maybe_out_of_band_buffer(_py, state, obj_bits, readonly)? {
                if !obj_from_bits(payload_bits).is_none() {
                    dec_ref_bits(_py, payload_bits);
                }
                return Ok(());
            }
            let Some(payload_ptr) = obj_from_bits(payload_bits).as_ptr() else {
                return Err(pickle_raise(
                    _py,
                    "pickle.dumps: PickleBuffer.raw() must be bytes-like",
                ));
            };
            let raw = unsafe { bytes_like_slice(payload_ptr) }.ok_or_else(|| {
                pickle_raise(_py, "pickle.dumps: PickleBuffer.raw() must be bytes-like")
            })?;
            if readonly {
                pickle_dump_bytes_binary(_py, state, raw)?;
            } else {
                pickle_dump_bytearray_binary(_py, state, raw)?;
            }
            if !obj_from_bits(payload_bits).is_none() {
                dec_ref_bits(_py, payload_bits);
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_TUPLE {
            let values = unsafe { seq_vec_ref(ptr).to_vec() };
            match values.len() {
                0 => state.push(PICKLE_OP_EMPTY_TUPLE),
                1 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    state.push(PICKLE_OP_TUPLE1);
                }
                2 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    pickle_dump_obj_binary(_py, state, values[1], true)?;
                    state.push(PICKLE_OP_TUPLE2);
                }
                3 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    pickle_dump_obj_binary(_py, state, values[1], true)?;
                    pickle_dump_obj_binary(_py, state, values[2], true)?;
                    state.push(PICKLE_OP_TUPLE3);
                }
                _ => {
                    state.push(PICKLE_OP_MARK);
                    for entry in values {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_TUPLE);
                }
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_LIST {
            state.push(PICKLE_OP_EMPTY_LIST);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let values = unsafe { seq_vec_ref(ptr).to_vec() };
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            return Ok(());
        }
        if type_id == TYPE_ID_DICT {
            state.push(PICKLE_OP_EMPTY_DICT);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let pairs = unsafe { crate::dict_order(ptr).to_vec() };
            if !pairs.is_empty() {
                state.push(PICKLE_OP_MARK);
                let mut idx = 0usize;
                while idx + 1 < pairs.len() {
                    pickle_dump_obj_binary(_py, state, pairs[idx], true)?;
                    pickle_dump_obj_binary(_py, state, pairs[idx + 1], true)?;
                    idx += 2;
                }
                state.push(PICKLE_OP_SETITEMS);
            }
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_EMPTY_SET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                let values = unsafe { crate::set_order(ptr).to_vec() };
                if !values.is_empty() {
                    state.push(PICKLE_OP_MARK);
                    for entry in values {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_ADDITEMS);
                }
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "set");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::set_order(ptr).to_vec() };
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            state.push(PICKLE_OP_TUPLE1);
            state.push(PICKLE_OP_REDUCE);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_FROZENSET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_MARK);
                let values = unsafe { crate::set_order(ptr).to_vec() };
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_FROZENSET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "frozenset");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::set_order(ptr).to_vec() };
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            state.push(PICKLE_OP_TUPLE1);
            state.push(PICKLE_OP_REDUCE);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SLICE {
            pickle_emit_global_opcode(state, "builtins", "slice");
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_start_bits(ptr) }, true)?;
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_stop_bits(ptr) }, true)?;
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_step_bits(ptr) }, true)?;
            state.push(PICKLE_OP_TUPLE3);
            state.push(PICKLE_OP_REDUCE);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if pickle_emit_global_ref(_py, state, obj_bits)? {
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if let Some(reduce_bits) = pickle_reduce_value(_py, state, obj_bits)? {
            let dumped = pickle_dump_reduce_value(_py, state, reduce_bits, Some(obj_bits));
            if !obj_from_bits(reduce_bits).is_none() {
                dec_ref_bits(_py, reduce_bits);
            }
            return dumped;
        }
        if pickle_dump_default_instance(_py, state, obj_bits, ptr, type_id)? {
            return Ok(());
        }
        let type_name = type_name(_py, obj_from_bits(obj_bits));
        let message = format!("cannot pickle '{type_name}' object");
        Err(raise_exception::<u64>(_py, "TypeError", &message))
    })();
    state.depth = state.depth.saturating_sub(1);
    result
}

fn pickle_apply_dict_state(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    dict_state_bits: u64,
) -> Result<(), u64> {
    if obj_from_bits(dict_state_bits).is_none() {
        return Ok(());
    }
    let Some(state_ptr) = obj_from_bits(dict_state_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    };
    if unsafe { object_type_id(state_ptr) } != TYPE_ID_DICT {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    }

    // Use setattr for each state entry. This correctly routes values to typed
    // field slots (TYPE_ID_OBJECT), dataclass descriptor fields
    // (TYPE_ID_DATACLASS), or __dict__ for fully dynamic instances.
    let pairs = unsafe { crate::dict_order(state_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let key_bits = pairs[idx];
        let value_bits = pairs[idx + 1];
        idx += 2;
        let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    Ok(())
}

fn pickle_vm_item_to_bits(_py: &crate::PyToken<'_>, item: &PickleVmItem) -> Result<u64, u64> {
    match item {
        PickleVmItem::Value(bits) => Ok(*bits),
        PickleVmItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleVmItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

fn pickle_vm_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<Vec<PickleVmItem>, u64> {
    let mut out: Vec<PickleVmItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleVmItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

fn pickle_vm_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<u64, u64> {
    let item = stack
        .pop()
        .ok_or_else(|| pickle_raise(_py, "pickle.loads: stack underflow"))?;
    pickle_vm_item_to_bits(_py, &item)
}

fn pickle_decode_8bit_string(
    _py: &crate::PyToken<'_>,
    raw: &[u8],
    encoding: &str,
    _errors: &str,
) -> Result<u64, u64> {
    if encoding.eq_ignore_ascii_case("bytes") {
        let ptr = crate::alloc_bytes(_py, raw);
        if ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(ptr).bits());
    }
    let decoded = if encoding.eq_ignore_ascii_case("latin1")
        || encoding.eq_ignore_ascii_case("latin-1")
    {
        raw.iter().map(|&b| char::from(b)).collect::<String>()
    } else {
        String::from_utf8(raw.to_vec())
            .map_err(|_| pickle_raise(_py, "pickle.loads: unable to decode 8-bit string payload"))?
    };
    let ptr = alloc_string(_py, decoded.as_bytes());
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

fn pickle_resolve_global_bits(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<u64, u64> {
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        return Err(MoltObject::none().bits());
    };
    let imported_bits = crate::molt_module_import(module_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(MoltObject::none().bits());
    };
    let value_bits = crate::molt_object_getattribute(imported_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if !obj_from_bits(imported_bits).is_none() {
        dec_ref_bits(_py, imported_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value_bits)
}

fn pickle_resolve_global_with_hook(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    if let Some(callback_bits) = find_class_bits {
        let Some(module_bits) = alloc_string_bits(_py, module) else {
            return Err(MoltObject::none().bits());
        };
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, module_bits);
            return Err(MoltObject::none().bits());
        };
        let out_bits = unsafe { call_callable2(_py, callback_bits, module_bits, name_bits) };
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(out_bits);
    }
    pickle_resolve_global_bits(_py, module, name)
}

fn pickle_lookup_extension_bits(
    _py: &crate::PyToken<'_>,
    code: i64,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    let copyreg_bits = pickle_resolve_global_bits(_py, "copyreg", "_inverted_registry")?;
    let Some(dict_ptr) = obj_from_bits(copyreg_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    }
    let code_bits = MoltObject::from_int(code).bits();
    let entry_bits = unsafe { dict_get_in_place(_py, dict_ptr, code_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, copyreg_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(entry_bits) = entry_bits else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: unknown extension code"));
    };
    let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    if unsafe { object_type_id(entry_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let fields = unsafe { seq_vec_ref(entry_ptr) };
    if fields.len() != 2 {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let Some(module) = string_obj_to_owned(obj_from_bits(fields[0])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    let Some(name) = string_obj_to_owned(obj_from_bits(fields[1])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    dec_ref_bits(_py, copyreg_bits);
    pickle_resolve_global_with_hook(_py, &module, &name, find_class_bits)
}

fn pickle_apply_newobj(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    args_bits: u64,
    kwargs_bits: Option<u64>,
) -> Result<u64, u64> {
    let new_bits = pickle_attr_required(_py, cls_bits, b"__new__")?;
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let kw_len = if let Some(kw_bits) = kwargs_bits {
        let Some(kw_ptr) = obj_from_bits(kw_bits).as_ptr() else {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        };
        if unsafe { object_type_id(kw_ptr) } != TYPE_ID_DICT {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        }
        unsafe { crate::dict_order(kw_ptr).len() / 2 }
    } else {
        0
    };
    let builder_bits = crate::molt_callargs_new((args.len() + 1) as u64, kw_len as u64);
    let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, cls_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, new_bits);
        return Err(MoltObject::none().bits());
    }
    for arg in args {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return Err(MoltObject::none().bits());
        }
    }
    if let Some(kw_bits) = kwargs_bits {
        let kw_ptr = obj_from_bits(kw_bits).as_ptr().expect("checked above");
        let pairs = unsafe { crate::dict_order(kw_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let val_bits = pairs[idx + 1];
            let _ = unsafe { crate::molt_callargs_push_kw(builder_bits, key_bits, val_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    let out_bits = crate::molt_call_bind(new_bits, builder_bits);
    dec_ref_bits(_py, new_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    // Initialize typed field slots to the missing sentinel so that
    // uninitialized fields (from __new__ without __init__) are properly
    // recognized as absent by hasattr/getattr.
    pickle_init_missing_fields(_py, out_bits);
    Ok(out_bits)
}

/// Initialize all typed field slots (and dataclass field values) to the missing
/// sentinel. Called after NEWOBJ to ensure fields not populated by BUILD are
/// correctly absent.
fn pickle_init_missing_fields(_py: &crate::PyToken<'_>, inst_bits: u64) {
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return;
    };
    let type_id = unsafe { object_type_id(inst_ptr) };
    let missing = missing_bits(_py);

    if type_id == crate::TYPE_ID_OBJECT {
        // Initialize typed field offsets to missing.
        let class_bits = unsafe { object_class_bits(inst_ptr) };
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
            return;
        }
        let cd_bits = unsafe { crate::class_dict_bits(class_ptr) };
        let Some(cd_ptr) = obj_from_bits(cd_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(cd_ptr) } != TYPE_ID_DICT {
            return;
        }
        let Some(offsets_name) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
            return;
        };
        let offsets_bits = unsafe { crate::dict_get_in_place(_py, cd_ptr, offsets_name) };
        dec_ref_bits(_py, offsets_name);
        if exception_pending(_py) {
            clear_exception(_py);
            return;
        }
        let Some(offsets_bits) = offsets_bits else {
            return;
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
            return;
        }
        let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let offset_bits = pairs[idx + 1];
            idx += 2;
            if let Some(offset) = to_i64(obj_from_bits(offset_bits)).filter(|&v| v >= 0) {
                unsafe {
                    let slot = inst_ptr.add(offset as usize) as *mut u64;
                    let old = *slot;
                    if old != missing {
                        inc_ref_bits(_py, missing);
                        if obj_from_bits(old).as_ptr().is_some() {
                            dec_ref_bits(_py, old);
                        }
                        *slot = missing;
                    }
                }
            }
        }
    } else if type_id == crate::TYPE_ID_DATACLASS {
        // Initialize dataclass field values to missing.
        let desc_ptr = unsafe { crate::dataclass_desc_ptr(inst_ptr) };
        if desc_ptr.is_null() {
            return;
        }
        let fields = unsafe { crate::dataclass_fields_mut(inst_ptr) };
        for val in fields.iter_mut() {
            if *val != missing {
                inc_ref_bits(_py, missing);
                if obj_from_bits(*val).as_ptr().is_some() {
                    dec_ref_bits(_py, *val);
                }
                *val = missing;
            }
        }
    }
}

fn pickle_apply_build(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    state_bits: u64,
) -> Result<u64, u64> {
    if obj_from_bits(state_bits).is_none() {
        return Ok(inst_bits);
    }
    if let Some(setstate_bits) = pickle_attr_optional(_py, inst_bits, b"__setstate__")? {
        let _ = unsafe { call_callable1(_py, setstate_bits, state_bits) };
        dec_ref_bits(_py, setstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(inst_bits);
    }
    let mut dict_state_bits = state_bits;
    let mut slot_state_bits: Option<u64> = None;
    if let Some(state_ptr) = obj_from_bits(state_bits).as_ptr()
        && unsafe { object_type_id(state_ptr) } == TYPE_ID_TUPLE
    {
        let fields = unsafe { seq_vec_ref(state_ptr) };
        if fields.len() == 2 {
            dict_state_bits = fields[0];
            slot_state_bits = Some(fields[1]);
        }
    }
    pickle_apply_dict_state(_py, inst_bits, dict_state_bits)?;
    if let Some(slot_bits) = slot_state_bits
        && !obj_from_bits(slot_bits).is_none()
    {
        let Some(slot_ptr) = obj_from_bits(slot_bits).as_ptr() else {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        };
        if unsafe { object_type_id(slot_ptr) } != TYPE_ID_DICT {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        }
        let pairs = unsafe { crate::dict_order(slot_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let value_bits = pairs[idx + 1];
            let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    Ok(inst_bits)
}

fn pickle_apply_reduce_vm(
    _py: &crate::PyToken<'_>,
    callable: PickleVmItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let out_bits = match callable {
        PickleVmItem::Mark => {
            return Err(pickle_raise(_py, "pickle.loads: mark cannot be called"));
        }
        PickleVmItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 1 {
                "utf-8".to_string()
            } else {
                let Some(enc) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                enc
            };
            pickle_encode_text(_py, &text, &encoding)?
        }
        PickleVmItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
        PickleVmItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
    };
    Ok(out_bits)
}

fn pickle_apply_reduce_bits(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_memo_set(
    _py: &crate::PyToken<'_>,
    memo: &mut Vec<Option<PickleVmItem>>,
    index: usize,
    item: PickleVmItem,
) {
    if memo.len() <= index {
        memo.resize(index + 1, None);
    }
    memo[index] = Some(item);
}

fn pickle_memo_get(
    _py: &crate::PyToken<'_>,
    memo: &[Option<PickleVmItem>],
    index: usize,
) -> Result<PickleVmItem, u64> {
    if let Some(Some(item)) = memo.get(index) {
        return Ok(item.clone());
    }
    let msg = format!("pickle.loads: memo key {} missing", index);
    Err(pickle_raise(_py, &msg))
}

    protocol_bits: u64,
    _fix_imports_bits: u64,
    persistent_id_bits: u64,
    buffer_callback_bits: u64,
    dispatch_table_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(protocol) = to_i64(obj_from_bits(protocol_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle protocol must be int");
        };
        if !(-1..=PICKLE_PROTO_5).contains(&protocol) {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "pickle protocol must be in range -1..5",
            );
        }
        let actual_protocol = if protocol < 0 {
            PICKLE_PROTO_5
        } else {
            protocol
        };
        if actual_protocol <= 1 {
            return molt_pickle_dumps_protocol01(
                obj_bits,
                MoltObject::from_int(actual_protocol).bits(),
            );
        }
        let persistent_id =
            match pickle_option_callable_bits(_py, persistent_id_bits, "persistent_id") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let buffer_callback =
            match pickle_option_callable_bits(_py, buffer_callback_bits, "buffer_callback") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let dispatch_table = if obj_from_bits(dispatch_table_bits).is_none() {
            None
        } else {
            Some(dispatch_table_bits)
        };
        let mut state = PickleDumpState::new(
            actual_protocol,
            persistent_id,
            buffer_callback,
            dispatch_table,
        );
        if state.buffer_callback_bits.is_some() && actual_protocol < PICKLE_PROTO_5 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "buffer_callback requires protocol 5 or higher",
            );
        }
        pickle_emit_proto_header(&mut state);
        if let Err(err_bits) = pickle_dump_obj_binary(_py, &mut state, obj_bits, true) {
            return err_bits;
        }
        state.push(PICKLE_OP_STOP);
        let out_ptr = crate::alloc_bytes(_py, state.out.as_slice());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
fn shlex_is_safe(s: &str) -> bool {
    s.bytes().all(|b| {
        matches!(
            b,
            b'a'..=b'z'
                | b'A'..=b'Z'
                | b'0'..=b'9'
                | b'_'
                | b'@'
                | b'%'
                | b'+'
                | b'='
                | b':'
                | b','
                | b'.'
                | b'/'
                | b'-'
        )
    })
}

fn shlex_quote_impl(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }
    if shlex_is_safe(input) {
        return input.to_string();
    }
    let escaped = input.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

type CharClassParse = (Vec<char>, Vec<(char, char)>, bool, usize);

fn fnmatch_parse_char_class(pat: &[char], mut idx: usize) -> Option<CharClassParse> {
    if idx >= pat.len() || pat[idx] != '[' {
        return None;
    }
    idx += 1;
    if idx >= pat.len() {
        return None;
    }

    let mut negate = false;
    if pat[idx] == '!' {
        negate = true;
        idx += 1;
    }
    if idx >= pat.len() {
        return None;
    }

    let mut singles: Vec<char> = Vec::new();
    let mut ranges: Vec<(char, char)> = Vec::new();

    if pat[idx] == ']' {
        singles.push(']');
        idx += 1;
    }

    while idx < pat.len() && pat[idx] != ']' {
        if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
            let start = pat[idx];
            let end = pat[idx + 2];
            if start <= end {
                ranges.push((start, end));
            }
            idx += 3;
            continue;
        }
        singles.push(pat[idx]);
        idx += 1;
    }
    if idx >= pat.len() || pat[idx] != ']' {
        return None;
    }
    Some((singles, ranges, negate, idx + 1))
}

fn fnmatch_char_class_hit(
    ch: char,
    singles: &[char],
    ranges: &[(char, char)],
    negate: bool,
) -> bool {
    let mut hit = singles.contains(&ch);
    if !hit {
        hit = ranges.iter().any(|(start, end)| *start <= ch && ch <= *end);
    }
    if negate { !hit } else { hit }
}

fn fnmatch_match_impl(name: &str, pat: &str) -> bool {
    let name_chars: Vec<char> = name.chars().collect();
    let pat_chars: Vec<char> = pat.chars().collect();
    let mut pi: usize = 0;
    let mut ni: usize = 0;
    let mut star_idx: Option<usize> = None;
    let mut matched_from_star: usize = 0;

    while ni < name_chars.len() {
        if pi < pat_chars.len() && pat_chars[pi] == '*' {
            while pi < pat_chars.len() && pat_chars[pi] == '*' {
                pi += 1;
            }
            if pi == pat_chars.len() {
                return true;
            }
            star_idx = Some(pi);
            matched_from_star = ni;
            continue;
        }
        if pi < pat_chars.len() && pat_chars[pi] == '?' {
            pi += 1;
            ni += 1;
            continue;
        }
        if pi < pat_chars.len()
            && pat_chars[pi] == '['
            && let Some((singles, ranges, negate, next_idx)) =
                fnmatch_parse_char_class(&pat_chars, pi)
        {
            let hit = fnmatch_char_class_hit(name_chars[ni], &singles, &ranges, negate);
            if hit {
                pi = next_idx;
                ni += 1;
                continue;
            }
            if let Some(star) = star_idx {
                matched_from_star += 1;
                ni = matched_from_star;
                pi = star;
                continue;
            }
            return false;
        }
        if pi < pat_chars.len() && pat_chars[pi] == name_chars[ni] {
            pi += 1;
            ni += 1;
            continue;
        }
        if let Some(star) = star_idx {
            matched_from_star += 1;
            ni = matched_from_star;
            pi = star;
            continue;
        }
        return false;
    }

    while pi < pat_chars.len() && pat_chars[pi] == '*' {
        pi += 1;
    }
    pi == pat_chars.len()
}

type FnmatchByteCharClass = (Vec<u8>, Vec<(u8, u8)>, bool, usize);

fn fnmatch_parse_char_class_bytes(pat: &[u8], mut idx: usize) -> Option<FnmatchByteCharClass> {
    if idx >= pat.len() || pat[idx] != b'[' {
        return None;
    }
    idx += 1;
    if idx >= pat.len() {
        return None;
    }

    let mut negate = false;
    if pat[idx] == b'!' {
        negate = true;
        idx += 1;
    }
    if idx >= pat.len() {
        return None;
    }

    let mut singles: Vec<u8> = Vec::new();
    let mut ranges: Vec<(u8, u8)> = Vec::new();

    if pat[idx] == b']' {
        singles.push(b']');
        idx += 1;
    }

    while idx < pat.len() && pat[idx] != b']' {
        if idx + 2 < pat.len() && pat[idx + 1] == b'-' && pat[idx + 2] != b']' {
            let start = pat[idx];
            let end = pat[idx + 2];
            if start <= end {
                ranges.push((start, end));
            }
            idx += 3;
            continue;
        }
        singles.push(pat[idx]);
        idx += 1;
    }
    if idx >= pat.len() || pat[idx] != b']' {
        return None;
    }
    Some((singles, ranges, negate, idx + 1))
}

fn fnmatch_char_class_hit_bytes(ch: u8, singles: &[u8], ranges: &[(u8, u8)], negate: bool) -> bool {
    let mut hit = singles.contains(&ch);
    if !hit {
        hit = ranges.iter().any(|(start, end)| *start <= ch && ch <= *end);
    }
    if negate { !hit } else { hit }
}

fn fnmatch_match_bytes_impl(name: &[u8], pat: &[u8]) -> bool {
    let mut pi: usize = 0;
    let mut ni: usize = 0;
    let mut star_idx: Option<usize> = None;
    let mut matched_from_star: usize = 0;

    while ni < name.len() {
        if pi < pat.len() && pat[pi] == b'*' {
            while pi < pat.len() && pat[pi] == b'*' {
                pi += 1;
            }
            if pi == pat.len() {
                return true;
            }
            star_idx = Some(pi);
            matched_from_star = ni;
            continue;
        }
        if pi < pat.len() && pat[pi] == b'?' {
            pi += 1;
            ni += 1;
            continue;
        }
        if pi < pat.len()
            && pat[pi] == b'['
            && let Some((singles, ranges, negate, next_idx)) =
                fnmatch_parse_char_class_bytes(pat, pi)
        {
            let hit = fnmatch_char_class_hit_bytes(name[ni], &singles, &ranges, negate);
            if hit {
                pi = next_idx;
                ni += 1;
                continue;
            }
            if let Some(star) = star_idx {
                matched_from_star += 1;
                ni = matched_from_star;
                pi = star;
                continue;
            }
            return false;
        }
        if pi < pat.len() && pat[pi] == name[ni] {
            pi += 1;
            ni += 1;
            continue;
        }
        if let Some(star) = star_idx {
            matched_from_star += 1;
            ni = matched_from_star;
            pi = star;
            continue;
        }
        return false;
    }

    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

fn fnmatch_normcase_text(input: &str) -> String {
    if cfg!(windows) {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            if ch == '/' {
                out.push('\\');
            } else {
                out.extend(ch.to_lowercase());
            }
        }
        out
    } else {
        input.to_string()
    }
}

fn fnmatch_normcase_bytes(input: &[u8]) -> Vec<u8> {
    if cfg!(windows) {
        let mut out = Vec::with_capacity(input.len());
        for b in input {
            let mut ch = *b;
            if ch == b'/' {
                ch = b'\\';
            } else if ch.is_ascii_uppercase() {
                ch += 32;
            }
            out.push(ch);
        }
        out
    } else {
        input.to_vec()
    }
}

fn fnmatch_escape_regex_char(out: &mut String, ch: char) {
    if ch.is_alphanumeric() || ch == '_' {
        out.push(ch);
        return;
    }
    out.push('\\');
    out.push(ch);
}

fn fnmatch_translate_impl(pat: &str) -> String {
    #[derive(Clone)]
    enum Token {
        Star,
        Text(String),
    }

    let chars: Vec<char> = pat.chars().collect();
    let mut res: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let n = chars.len();
    while i < n {
        let ch = chars[i];
        i += 1;
        match ch {
            '*' => {
                if res.last().is_none_or(|token| !matches!(token, Token::Star)) {
                    res.push(Token::Star);
                }
            }
            '?' => res.push(Token::Text(".".to_string())),
            '[' => {
                let mut j = i;
                if j < n && chars[j] == '!' {
                    j += 1;
                }
                if j < n && chars[j] == ']' {
                    j += 1;
                }
                while j < n && chars[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    res.push(Token::Text("\\[".to_string()));
                    continue;
                }
                let mut stuff: String = chars[i..j].iter().collect();
                if !stuff.contains('-') {
                    stuff = stuff.replace('\\', r"\\");
                } else {
                    let mut chunks: Vec<String> = Vec::new();
                    let mut sub_i = i;
                    let mut k = if chars[sub_i] == '!' {
                        sub_i + 2
                    } else {
                        sub_i + 1
                    };
                    loop {
                        let found = chars[k..j]
                            .iter()
                            .position(|ch| *ch == '-')
                            .map(|offset| k + offset);
                        let Some(k_idx) = found else {
                            break;
                        };
                        chunks.push(chars[sub_i..k_idx].iter().collect());
                        sub_i = k_idx + 1;
                        k = sub_i + 2;
                    }
                    let chunk: String = chars[sub_i..j].iter().collect();
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    } else if let Some(last) = chunks.last_mut() {
                        last.push('-');
                    }
                    for idx in (1..chunks.len()).rev() {
                        if let (Some(prev_last), Some(next_first)) =
                            (chunks[idx - 1].chars().last(), chunks[idx].chars().next())
                            && prev_last > next_first
                        {
                            let mut updated = chunks[idx - 1].chars().collect::<Vec<_>>();
                            updated.pop();
                            let mut new_chunk: String = updated.into_iter().collect();
                            let mut next_chars = chunks[idx].chars();
                            next_chars.next();
                            new_chunk.push_str(&next_chars.collect::<String>());
                            chunks[idx - 1] = new_chunk;
                            chunks.remove(idx);
                        }
                    }
                    let escaped_chunks: Vec<String> = chunks
                        .into_iter()
                        .map(|chunk| {
                            let mut out = String::new();
                            for ch in chunk.chars() {
                                match ch {
                                    '\\' => out.push_str(r"\\"),
                                    '-' => out.push_str(r"\-"),
                                    _ => out.push(ch),
                                }
                            }
                            out
                        })
                        .collect();
                    stuff = escaped_chunks.join("-");
                }
                if stuff.contains('&') || stuff.contains('~') || stuff.contains('|') {
                    let mut escaped = String::with_capacity(stuff.len());
                    for ch in stuff.chars() {
                        if matches!(ch, '&' | '~' | '|') {
                            escaped.push('\\');
                        }
                        escaped.push(ch);
                    }
                    stuff = escaped;
                }
                i = j + 1;
                if stuff.is_empty() {
                    res.push(Token::Text("(?!)".to_string()));
                } else if stuff == "!" {
                    res.push(Token::Text(".".to_string()));
                } else {
                    if stuff.starts_with('!') {
                        stuff = format!("^{}", &stuff[1..]);
                    } else if let Some(first) = stuff.chars().next()
                        && (first == '^' || first == '[')
                    {
                        stuff = format!("\\{}", stuff);
                    }
                    res.push(Token::Text(format!("[{stuff}]")));
                }
            }
            other => {
                let mut out = String::new();
                fnmatch_escape_regex_char(&mut out, other);
                res.push(Token::Text(out));
            }
        }
    }

    let inp = res;
    let mut out: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < inp.len() {
        match &inp[idx] {
            Token::Star => break,
            Token::Text(text) => out.push(text.clone()),
        }
        idx += 1;
    }
    while idx < inp.len() {
        idx += 1;
        if idx == inp.len() {
            out.push(".*".to_string());
            break;
        }
        let mut fixed = String::new();
        while idx < inp.len() {
            match &inp[idx] {
                Token::Star => break,
                Token::Text(text) => fixed.push_str(text),
            }
            idx += 1;
        }
        if idx == inp.len() {
            out.push(".*".to_string());
            out.push(fixed);
        } else {
            out.push(format!("(?>.*?{fixed})"));
        }
    }
    let res = out.join("");
    format!("(?s:{res})\\Z")
}

fn fnmatch_bytes_from_bits(bits: u64) -> Option<Vec<u8>> {
    let obj = obj_from_bits(bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_BYTES {
            return None;
        }
        bytes_like_slice(ptr).map(|slice| slice.to_vec())
    }
}

fn iter_next_pair(_py: &crate::PyToken<'_>, iter_bits: u64) -> Result<(u64, bool), u64> {
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        return Err(MoltObject::none().bits());
    };
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return Err(MoltObject::none().bits());
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        Ok((val_bits, done))
    }
}

fn iterable_to_string_vec(_py: &crate::PyToken<'_>, values_bits: u64) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<String> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item) = string_obj_to_owned(obj_from_bits(item_bits)) else {
            return Err(raise_exception::<_>(_py, "TypeError", "expected str item"));
        };
        out.push(item);
    }
    Ok(out)
}

fn alloc_string_list(_py: &crate::PyToken<'_>, values: &[String]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
        if ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        item_bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn shlex_split_impl(
    input: &str,
    whitespace: &str,
    posix: bool,
    comments: bool,
    commenters: &str,
    _whitespace_split: bool,
    punctuation_chars: &str,
) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut quote_char: Option<char> = None;
    let mut escape = false;
    let mut it = input.chars().peekable();
    while let Some(ch) = it.next() {
        if escape {
            buf.push(ch);
            escape = false;
            continue;
        }
        if let Some(q) = quote_char {
            if ch == q {
                quote_char = None;
            } else if ch == '\\' && (q != '\'' || !posix) {
                escape = true;
            } else {
                buf.push(ch);
            }
            continue;
        }
        if comments && commenters.contains(ch) {
            while let Some(next) = it.peek() {
                if *next == '\n' || *next == '\r' {
                    break;
                }
                it.next();
            }
            if !buf.is_empty() {
                tokens.push(std::mem::take(&mut buf));
            }
            continue;
        }
        if !punctuation_chars.is_empty() && punctuation_chars.contains(ch) {
            if !buf.is_empty() {
                tokens.push(std::mem::take(&mut buf));
            }
            let mut punct = String::new();
            punct.push(ch);
            while let Some(next) = it.peek() {
                if punctuation_chars.contains(*next) {
                    punct.push(*next);
                    it.next();
                } else {
                    break;
                }
            }
            tokens.push(punct);
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote_char = Some(ch);
            continue;
        }
        if whitespace.contains(ch) {
            if !buf.is_empty() {
                tokens.push(std::mem::take(&mut buf));
            }
            continue;
        }
        buf.push(ch);
    }
    if quote_char.is_some() {
        return Err("No closing quotation".to_string());
    }
    if escape {
        if posix {
            return Err("No escaped character".to_string());
        }
        buf.push('\\');
    }
    if !buf.is_empty() {
        tokens.push(buf);
    }
    Ok(tokens)
}

fn shlex_join_impl(parts: &[String]) -> String {
    parts
        .iter()
        .map(|item| shlex_quote_impl(item))
        .collect::<Vec<String>>()
        .join(" ")
}

fn raise_os_error_from_io(_py: &crate::PyToken<'_>, err: std::io::Error) -> u64 {
    let msg = err.to_string();
    match err.kind() {
        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
        ErrorKind::PermissionDenied => raise_exception::<_>(_py, "PermissionError", &msg),
        ErrorKind::AlreadyExists => raise_exception::<_>(_py, "FileExistsError", &msg),
        ErrorKind::NotADirectory => raise_exception::<_>(_py, "NotADirectoryError", &msg),
        ErrorKind::IsADirectory => raise_exception::<_>(_py, "IsADirectoryError", &msg),
        _ => raise_exception::<_>(_py, "OSError", &msg),
    }
}

fn absolutize_path(path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_string_lossy().into_owned();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(p).to_string_lossy().into_owned()
}

fn path_is_executable(path: &Path) -> bool {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        (meta.permissions().mode() & 0o111) != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn alloc_optional_path(_py: &crate::PyToken<'_>, candidate: &Path) -> u64 {
    let out = candidate.to_string_lossy().into_owned();
    let out_ptr = alloc_string(_py, out.as_bytes());
    if out_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(out_ptr).bits()
    }
}

fn alloc_string_bits(_py: &crate::PyToken<'_>, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

fn alloc_string_tuple(_py: &crate::PyToken<'_>, values: &[String]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let Some(bits) = alloc_string_bits(_py, value) else {
            for bit in item_bits {
                dec_ref_bits(_py, bit);
            }
            return MoltObject::none().bits();
        };
        item_bits.push(bits);
    }
    let tuple_ptr = alloc_tuple(_py, &item_bits);
    if tuple_ptr.is_null() {
        for bit in item_bits {
            dec_ref_bits(_py, bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(tuple_ptr).bits();
    for bit in item_bits {
        dec_ref_bits(_py, bit);
    }
    out
}

fn alloc_qsl_list(_py: &crate::PyToken<'_>, items: &[(String, String)]) -> u64 {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(items.len());
    for (key, value) in items {
        let Some(key_bits) = alloc_string_bits(_py, key) else {
            for bit in tuple_bits {
                dec_ref_bits(_py, bit);
            }
            return MoltObject::none().bits();
        };
        let Some(value_bits) = alloc_string_bits(_py, value) else {
            dec_ref_bits(_py, key_bits);
            for bit in tuple_bits {
                dec_ref_bits(_py, bit);
            }
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(_py, &[key_bits, value_bits]);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, value_bits);
        if tuple_ptr.is_null() {
            for bit in tuple_bits {
                dec_ref_bits(_py, bit);
            }
            return MoltObject::none().bits();
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, &tuple_bits, tuple_bits.len());
    if list_ptr.is_null() {
        for bit in tuple_bits {
            dec_ref_bits(_py, bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(list_ptr).bits();
    for bit in tuple_bits {
        dec_ref_bits(_py, bit);
    }
    out
}

fn alloc_qs_dict(
    _py: &crate::PyToken<'_>,
    order: &[String],
    values: &HashMap<String, Vec<String>>,
) -> u64 {
    let mut pairs: Vec<u64> = Vec::with_capacity(order.len() * 2);
    let mut owned_bits: Vec<u64> = Vec::with_capacity(order.len() * 2);
    for key in order {
        let Some(key_bits) = alloc_string_bits(_py, key) else {
            for bit in owned_bits {
                dec_ref_bits(_py, bit);
            }
            return MoltObject::none().bits();
        };
        let mut value_bits: Vec<u64> = Vec::new();
        for value in values.get(key).into_iter().flatten() {
            let Some(bits) = alloc_string_bits(_py, value) else {
                dec_ref_bits(_py, key_bits);
                for bit in value_bits {
                    dec_ref_bits(_py, bit);
                }
                for bit in owned_bits {
                    dec_ref_bits(_py, bit);
                }
                return MoltObject::none().bits();
            };
            value_bits.push(bits);
        }
        let list_ptr = alloc_list_with_capacity(_py, &value_bits, value_bits.len());
        for bit in value_bits {
            dec_ref_bits(_py, bit);
        }
        if list_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            for bit in owned_bits {
                dec_ref_bits(_py, bit);
            }
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        pairs.push(key_bits);
        pairs.push(list_bits);
        owned_bits.push(key_bits);
        owned_bits.push(list_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    if dict_ptr.is_null() {
        for bit in owned_bits {
            dec_ref_bits(_py, bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(dict_ptr).bits();
    for bit in owned_bits {
        dec_ref_bits(_py, bit);
    }
    out
}

fn urllib_is_alpha(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

fn urllib_is_alnum(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
}

fn urllib_split_scheme(url: &str, default: &str) -> (String, String) {
    for (idx, ch) in url.char_indices() {
        if ch == ':' {
            let scheme = &url[..idx];
            if !scheme.is_empty()
                && scheme.chars().next().is_some_and(urllib_is_alpha)
                && scheme
                    .chars()
                    .all(|c| urllib_is_alnum(c) || matches!(c, '+' | '-' | '.'))
            {
                return (scheme.to_ascii_lowercase(), url[idx + 1..].to_string());
            }
            break;
        }
        if matches!(ch, '/' | '?' | '#') {
            break;
        }
    }
    (default.to_string(), url.to_string())
}

fn urllib_split_netloc(rest: &str) -> (String, String) {
    for (idx, ch) in rest.char_indices() {
        if matches!(ch, '/' | '?' | '#') {
            return (rest[..idx].to_string(), rest[idx..].to_string());
        }
    }
    (rest.to_string(), String::new())
}

fn urllib_split_query_fragment(rest: &str, allow_fragments: bool) -> (String, String, String) {
    let mut working = rest.to_string();
    let mut fragment = String::new();
    if allow_fragments && let Some(idx) = working.find('#') {
        fragment = working[idx + 1..].to_string();
        working.truncate(idx);
    }
    let mut query = String::new();
    if let Some(idx) = working.find('?') {
        query = working[idx + 1..].to_string();
        working.truncate(idx);
    }
    (working, query, fragment)
}

fn urllib_urlsplit_impl(url: &str, scheme: &str, allow_fragments: bool) -> [String; 5] {
    let (parsed_scheme, mut rest) = urllib_split_scheme(url, scheme);
    let mut netloc = String::new();
    if rest.starts_with("//") {
        let (out_netloc, out_rest) = urllib_split_netloc(&rest[2..]);
        netloc = out_netloc;
        rest = out_rest;
    }
    let (path, query, fragment) = urllib_split_query_fragment(&rest, allow_fragments);
    [parsed_scheme, netloc, path, query, fragment]
}

fn urllib_urlparse_impl(url: &str, scheme: &str, allow_fragments: bool) -> [String; 6] {
    let split = urllib_urlsplit_impl(url, scheme, allow_fragments);
    let mut path = split[2].clone();
    let mut params = String::new();
    if let Some(idx) = path.find(';') {
        params = path[idx + 1..].to_string();
        path.truncate(idx);
    }
    [
        split[0].clone(),
        split[1].clone(),
        path,
        params,
        split[3].clone(),
        split[4].clone(),
    ]
}

fn urllib_unsplit_impl(
    scheme: &str,
    netloc: &str,
    path: &str,
    query: &str,
    fragment: &str,
) -> String {
    let mut out = String::new();
    if !scheme.is_empty() {
        out.push_str(scheme);
        out.push(':');
    }
    if !netloc.is_empty() {
        out.push_str("//");
        out.push_str(netloc);
    }
    out.push_str(path);
    if !query.is_empty() {
        out.push('?');
        out.push_str(query);
    }
    if !fragment.is_empty() {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

fn urllib_quote_impl(string: &str, safe: &str) -> String {
    const ALWAYS_SAFE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-~";
    let safe_set: std::collections::HashSet<char> =
        ALWAYS_SAFE.chars().chain(safe.chars()).collect();
    let mut out = String::new();
    for ch in string.chars() {
        if safe_set.contains(&ch) {
            out.push(ch);
            continue;
        }
        let mut buf = [0u8; 4];
        for byte in ch.encode_utf8(&mut buf).as_bytes() {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

fn urllib_quote_plus_impl(string: &str, safe: &str) -> String {
    urllib_quote_impl(string, safe).replace("%20", "+")
}

fn urllib_unquote_impl(string: &str) -> String {
    if !string.contains('%') {
        return string.to_string();
    }
    let chars: Vec<char> = string.chars().collect();
    let mut out: Vec<u8> = Vec::with_capacity(string.len());
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch == '%' && idx + 2 < chars.len() {
            let h1 = chars[idx + 1];
            let h2 = chars[idx + 2];
            if let (Some(a), Some(b)) = (h1.to_digit(16), h2.to_digit(16)) {
                out.push(((a << 4) | b) as u8);
                idx += 3;
                continue;
            }
        }
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn urllib_urlencode_impl(
    _py: &crate::PyToken<'_>,
    query_bits: u64,
    doseq: bool,
    safe: &str,
) -> Result<String, u64> {
    let iter_bits = molt_iter(query_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out_pairs: Vec<String> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let item_obj = obj_from_bits(item_bits);
        let Some(item_ptr) = item_obj.as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "not a valid non-string sequence or mapping object",
            ));
        };
        let item_type = unsafe { object_type_id(item_ptr) };
        if item_type != TYPE_ID_LIST && item_type != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "not a valid non-string sequence or mapping object",
            ));
        }
        let item_fields = unsafe { seq_vec_ref(item_ptr) };
        if item_fields.len() != 2 {
            if item_fields.len() < 2 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    &format!(
                        "not enough values to unpack (expected 2, got {})",
                        item_fields.len()
                    ),
                ));
            }
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "too many values to unpack (expected 2)",
            ));
        }
        let key_text = crate::format_obj_str(_py, obj_from_bits(item_fields[0]));
        let key_enc = urllib_quote_plus_impl(&key_text, safe);
        let value_obj = obj_from_bits(item_fields[1]);
        let mut wrote_pair = false;
        if doseq && let Some(value_ptr) = value_obj.as_ptr() {
            let value_type = unsafe { object_type_id(value_ptr) };
            if value_type == TYPE_ID_LIST || value_type == TYPE_ID_TUPLE {
                let seq = unsafe { seq_vec_ref(value_ptr) };
                for value_bits in seq.iter().copied() {
                    let value_text = crate::format_obj_str(_py, obj_from_bits(value_bits));
                    let value_enc = urllib_quote_plus_impl(&value_text, safe);
                    out_pairs.push(format!("{key_enc}={value_enc}"));
                }
                wrote_pair = true;
            }
        }
        if !wrote_pair {
            let value_text = crate::format_obj_str(_py, value_obj);
            let value_enc = urllib_quote_plus_impl(&value_text, safe);
            out_pairs.push(format!("{key_enc}={value_enc}"));
        }
    }
    Ok(out_pairs.join("&"))
}

fn urllib_error_set_attr(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
    name: &str,
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
        return false;
    };
    let _ = crate::molt_object_setattr(self_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn urllib_error_init_args(_py: &crate::PyToken<'_>, self_bits: u64, args: &[u64]) -> bool {
    let args_ptr = alloc_tuple(_py, args);
    if args_ptr.is_null() {
        return false;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let _ = crate::builtins::exceptions::molt_exception_init(self_bits, args_bits);
    !exception_pending(_py)
}

fn urllib_parse_qsl_impl(
    qs: &str,
    keep_blank_values: bool,
    strict_parsing: bool,
) -> Result<Vec<(String, String)>, String> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    if qs.is_empty() {
        return Ok(pairs);
    }
    for chunk in qs.split('&') {
        if chunk.is_empty() && !keep_blank_values {
            continue;
        }
        let (key, value) = if let Some((k, v)) = chunk.split_once('=') {
            (k, v)
        } else if strict_parsing {
            return Err("bad query field".to_string());
        } else {
            (chunk, "")
        };
        if !value.is_empty() || keep_blank_values {
            let key_text = urllib_unquote_impl(&key.replace('+', " "));
            let value_text = urllib_unquote_impl(&value.replace('+', " "));
            pairs.push((key_text, value_text));
        }
    }
    Ok(pairs)
}

fn urllib_request_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if let Some(exc_name) = urllib_request_pending_exception_kind_name(_py)
            && exc_name == "AttributeError"
        {
            clear_exception(_py);
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn urllib_request_pending_exception_kind_name(_py: &crate::PyToken<'_>) -> Option<String> {
    if !exception_pending(_py) {
        return None;
    }
    let exc_bits = molt_exception_last();
    let out = maybe_ptr_from_bits(exc_bits)
        .and_then(|ptr| string_obj_to_owned(obj_from_bits(unsafe { exception_kind_bits(ptr) })));
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    out
}

fn ctypes_attr_present(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<bool, u64> {
    match urllib_request_attr_optional(_py, obj_bits, name)? {
        Some(bits) => {
            dec_ref_bits(_py, bits);
            Ok(true)
        }
        None => Ok(false),
    }
}

fn ctypes_is_scalar_ctype(_py: &crate::PyToken<'_>, ctype_bits: u64) -> Result<bool, u64> {
    let has_size = ctypes_attr_present(_py, ctype_bits, b"_size")?;
    if !has_size {
        return Ok(false);
    }
    let has_fields = ctypes_attr_present(_py, ctype_bits, b"_fields_")?;
    let has_length = ctypes_attr_present(_py, ctype_bits, b"_length")?;
    Ok(!has_fields && !has_length)
}

fn ctypes_sizeof_bits(_py: &crate::PyToken<'_>, obj_or_type_bits: u64) -> Result<u64, u64> {
    let Some(size_bits) = urllib_request_attr_optional(_py, obj_or_type_bits, b"_size")? else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "unsupported type for ctypes.sizeof",
        ));
    };
    let out = match to_i64(obj_from_bits(size_bits)) {
        Some(value) => MoltObject::from_int(value).bits(),
        None => {
            raise_exception::<u64>(_py, "TypeError", "ctypes size value must be int-compatible")
        }
    };
    dec_ref_bits(_py, size_bits);
    Ok(out)
}

fn urllib_attr_truthy(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<bool, u64> {
    match urllib_request_attr_optional(_py, obj_bits, name)? {
        Some(bits) => {
            let out = is_truthy(_py, obj_from_bits(bits));
            dec_ref_bits(_py, bits);
            Ok(out)
        }
        None => Ok(false),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_require_ffi() -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_coerce_value(ctype_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }

        if let Some(inner_bits) = match urllib_request_attr_optional(_py, value_bits, b"value") {
            Ok(bits) => bits,
            Err(bits) => return bits,
        } {
            let out = match to_i64(obj_from_bits(inner_bits)) {
                Some(num) => MoltObject::from_int(num).bits(),
                None => raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "ctypes value.value must be int-compatible",
                ),
            };
            dec_ref_bits(_py, inner_bits);
            return out;
        }

        let is_scalar = match ctypes_is_scalar_ctype(_py, ctype_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !is_scalar {
            return value_bits;
        }
        let Some(num) = to_i64(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "ctypes scalar value must be int");
        };
        MoltObject::from_int(num).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_default_value(ctype_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }

        let is_scalar = match ctypes_is_scalar_ctype(_py, ctype_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if is_scalar {
            return MoltObject::from_int(0).bits();
        }

        let has_fields = match ctypes_attr_present(_py, ctype_bits, b"_fields_") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let has_length = match ctypes_attr_present(_py, ctype_bits, b"_length") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if has_fields || has_length {
            let out_bits = unsafe { call_callable0(_py, ctype_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return out_bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ctypes_sizeof(obj_or_type_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "ffi.unsafe") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "capability 'ffi.unsafe' required",
            );
        }
        match ctypes_sizeof_bits(_py, obj_or_type_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

fn socketserver_runtime() -> &'static Mutex<MoltSocketServerRuntime> {
    SOCKETSERVER_RUNTIME.get_or_init(|| {
        Mutex::new(MoltSocketServerRuntime {
            next_request_id: 1,
            pending_by_server: HashMap::new(),
            pending_requests: HashMap::new(),
            request_server: HashMap::new(),
            closed_servers: HashSet::new(),
        })
    })
}

fn socketserver_extract_bytes(
    _py: &crate::PyToken<'_>,
    bits: u64,
    label: &str,
) -> Result<Vec<u8>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be bytes-like"),
        ));
    };
    let Some(bytes) = (unsafe { bytes_like_slice(ptr) }) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be bytes-like"),
        ));
    };
    Ok(bytes.to_vec())
}

fn socketserver_extract_request_id(_py: &crate::PyToken<'_>, bits: u64) -> Result<u64, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "request id must be int",
        ));
    };
    if value <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "request id must be positive",
        ));
    }
    Ok(value as u64)
}

fn socketserver_extract_handle_request_tuple(
    _py: &crate::PyToken<'_>,
    bits: u64,
) -> Result<(u64, u64, i64), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "get_request() must return a 3-item tuple",
        ));
    };
    let ty = unsafe { object_type_id(ptr) };
    if ty != TYPE_ID_TUPLE && ty != TYPE_ID_LIST {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "get_request() must return a 3-item tuple",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if fields.len() != 3 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "get_request() must return a 3-item tuple",
        ));
    }
    let Some(request_id) = to_i64(obj_from_bits(fields[2])) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "request id must be int",
        ));
    };
    Ok((fields[0], fields[1], request_id))
}

fn socketserver_call_service_actions(
    _py: &crate::PyToken<'_>,
    server_bits: u64,
) -> Result<(), u64> {
    let Some(method_bits) = urllib_request_attr_optional(_py, server_bits, b"service_actions")?
    else {
        return Ok(());
    };
    if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
        dec_ref_bits(_py, method_bits);
        return Ok(());
    }
    let _ = unsafe { call_callable0(_py, method_bits) };
    dec_ref_bits(_py, method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

const HTTP_SERVER_DEFAULT_REQUEST_VERSION: &str = "HTTP/0.9";
const HTTP_SERVER_HTTP11: &str = "HTTP/1.1";

fn http_server_reason_phrase(code: i64) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        414 => "Request-URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Requested Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

fn http_status_constants() -> &'static [(&'static str, i64)] {
    &[
        ("CONTINUE", 100),
        ("SWITCHING_PROTOCOLS", 101),
        ("PROCESSING", 102),
        ("EARLY_HINTS", 103),
        ("OK", 200),
        ("CREATED", 201),
        ("ACCEPTED", 202),
        ("NON_AUTHORITATIVE_INFORMATION", 203),
        ("NO_CONTENT", 204),
        ("RESET_CONTENT", 205),
        ("PARTIAL_CONTENT", 206),
        ("MULTI_STATUS", 207),
        ("ALREADY_REPORTED", 208),
        ("IM_USED", 226),
        ("MULTIPLE_CHOICES", 300),
        ("MOVED_PERMANENTLY", 301),
        ("FOUND", 302),
        ("SEE_OTHER", 303),
        ("NOT_MODIFIED", 304),
        ("USE_PROXY", 305),
        ("TEMPORARY_REDIRECT", 307),
        ("PERMANENT_REDIRECT", 308),
        ("BAD_REQUEST", 400),
        ("UNAUTHORIZED", 401),
        ("PAYMENT_REQUIRED", 402),
        ("FORBIDDEN", 403),
        ("NOT_FOUND", 404),
        ("METHOD_NOT_ALLOWED", 405),
        ("NOT_ACCEPTABLE", 406),
        ("PROXY_AUTHENTICATION_REQUIRED", 407),
        ("REQUEST_TIMEOUT", 408),
        ("CONFLICT", 409),
        ("GONE", 410),
        ("LENGTH_REQUIRED", 411),
        ("PRECONDITION_FAILED", 412),
        ("REQUEST_ENTITY_TOO_LARGE", 413),
        ("REQUEST_URI_TOO_LONG", 414),
        ("UNSUPPORTED_MEDIA_TYPE", 415),
        ("REQUESTED_RANGE_NOT_SATISFIABLE", 416),
        ("EXPECTATION_FAILED", 417),
        ("IM_A_TEAPOT", 418),
        ("MISDIRECTED_REQUEST", 421),
        ("UNPROCESSABLE_ENTITY", 422),
        ("LOCKED", 423),
        ("FAILED_DEPENDENCY", 424),
        ("TOO_EARLY", 425),
        ("UPGRADE_REQUIRED", 426),
        ("PRECONDITION_REQUIRED", 428),
        ("TOO_MANY_REQUESTS", 429),
        ("REQUEST_HEADER_FIELDS_TOO_LARGE", 431),
        ("UNAVAILABLE_FOR_LEGAL_REASONS", 451),
        ("INTERNAL_SERVER_ERROR", 500),
        ("NOT_IMPLEMENTED", 501),
        ("BAD_GATEWAY", 502),
        ("SERVICE_UNAVAILABLE", 503),
        ("GATEWAY_TIMEOUT", 504),
        ("HTTP_VERSION_NOT_SUPPORTED", 505),
        ("VARIANT_ALSO_NEGOTIATES", 506),
        ("INSUFFICIENT_STORAGE", 507),
        ("LOOP_DETECTED", 508),
        ("NOT_EXTENDED", 510),
        ("NETWORK_AUTHENTICATION_REQUIRED", 511),
        // CPython 3.12+ compatibility aliases.
        ("CONTENT_TOO_LARGE", 413),
        ("URI_TOO_LONG", 414),
        ("RANGE_NOT_SATISFIABLE", 416),
        ("UNPROCESSABLE_CONTENT", 422),
    ]
}

fn http_server_error_explain(code: i64) -> &'static str {
    match code {
        400 => "Bad request syntax or unsupported method",
        404 => "Nothing matches the given URI",
        500 => "Server got itself in trouble",
        501 => "Server does not support this operation",
        _ => "",
    }
}

fn http_server_html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn http_server_repr_single_quoted(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

fn http_server_set_attr_string(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    value: &str,
) -> Result<(), u64> {
    let Some(bits) = alloc_string_bits(_py, value) else {
        return Err(MoltObject::none().bits());
    };
    if !urllib_request_set_attr(_py, obj_bits, name, bits) {
        dec_ref_bits(_py, bits);
        return Err(MoltObject::none().bits());
    }
    dec_ref_bits(_py, bits);
    Ok(())
}

fn http_server_get_required_attr_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    label: &str,
) -> Result<u64, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, obj_bits, name)? else {
        return Err(raise_exception::<u64>(_py, "RuntimeError", label));
    };
    if obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
        return Err(raise_exception::<u64>(_py, "RuntimeError", label));
    }
    Ok(bits)
}

fn http_server_get_optional_attr_string(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<String>, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, obj_bits, name)? else {
        return Ok(None);
    };
    if obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
        return Ok(None);
    }
    let out = crate::format_obj_str(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Ok(Some(out))
}

fn http_server_write_bytes(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
    payload: &[u8],
) -> Result<(), u64> {
    let wfile_bits = http_server_get_required_attr_bits(
        _py,
        handler_bits,
        b"wfile",
        "http handler is missing wfile",
    )?;
    let Some(write_name_bits) = attr_name_bits_from_bytes(_py, b"write") else {
        dec_ref_bits(_py, wfile_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let write_bits = molt_getattr_builtin(wfile_bits, write_name_bits, missing);
    dec_ref_bits(_py, write_name_bits);
    dec_ref_bits(_py, wfile_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if write_bits == missing || !is_truthy(_py, obj_from_bits(molt_is_callable(write_bits))) {
        if write_bits != missing {
            dec_ref_bits(_py, write_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http handler wfile.write is unavailable",
        ));
    }
    let data_ptr = crate::alloc_bytes(_py, payload);
    if data_ptr.is_null() {
        dec_ref_bits(_py, write_bits);
        return Err(MoltObject::none().bits());
    }
    let data_bits = MoltObject::from_ptr(data_ptr).bits();
    let _ = unsafe { call_callable1(_py, write_bits, data_bits) };
    dec_ref_bits(_py, data_bits);
    dec_ref_bits(_py, write_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn http_server_flush(_py: &crate::PyToken<'_>, handler_bits: u64) -> Result<(), u64> {
    let Some(wfile_bits) = urllib_request_attr_optional(_py, handler_bits, b"wfile")? else {
        return Ok(());
    };
    if obj_from_bits(wfile_bits).is_none() {
        dec_ref_bits(_py, wfile_bits);
        return Ok(());
    }
    let Some(flush_name_bits) = attr_name_bits_from_bytes(_py, b"flush") else {
        dec_ref_bits(_py, wfile_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let flush_bits = molt_getattr_builtin(wfile_bits, flush_name_bits, missing);
    dec_ref_bits(_py, flush_name_bits);
    dec_ref_bits(_py, wfile_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if flush_bits == missing || !is_truthy(_py, obj_from_bits(molt_is_callable(flush_bits))) {
        if flush_bits != missing {
            dec_ref_bits(_py, flush_bits);
        }
        return Ok(());
    }
    let _ = unsafe { call_callable0(_py, flush_bits) };
    dec_ref_bits(_py, flush_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn http_server_readline(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
    limit: i64,
) -> Result<Vec<u8>, u64> {
    let rfile_bits = http_server_get_required_attr_bits(
        _py,
        handler_bits,
        b"rfile",
        "http handler is missing rfile",
    )?;
    let Some(readline_name_bits) = attr_name_bits_from_bytes(_py, b"readline") else {
        dec_ref_bits(_py, rfile_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let readline_bits = molt_getattr_builtin(rfile_bits, readline_name_bits, missing);
    dec_ref_bits(_py, readline_name_bits);
    dec_ref_bits(_py, rfile_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if readline_bits == missing || !is_truthy(_py, obj_from_bits(molt_is_callable(readline_bits))) {
        if readline_bits != missing {
            dec_ref_bits(_py, readline_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http handler rfile.readline is unavailable",
        ));
    }
    let line_bits =
        unsafe { call_callable1(_py, readline_bits, MoltObject::from_int(limit).bits()) };
    dec_ref_bits(_py, readline_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let out = socketserver_extract_bytes(_py, line_bits, "request line");
    dec_ref_bits(_py, line_bits);
    out
}

fn http_server_version_string_impl(server_version: &str, sys_version: &str) -> String {
    if sys_version.is_empty() {
        server_version.to_string()
    } else {
        format!("{server_version} {sys_version}")
    }
}

fn http_server_format_gmt_timestamp(timestamp: i64) -> String {
    const WEEKDAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTH: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    #[cfg(target_arch = "wasm32")]
    {
        // Pure-arithmetic UTC calendar decomposition (no libc dependency).
        let secs = if timestamp < 0 { 0i64 } else { timestamp };
        let day_secs = secs % 86400;
        let hour = (day_secs / 3600) as u32;
        let minute = ((day_secs % 3600) / 60) as u32;
        let second = (day_secs % 60) as u32;
        // Days since epoch → civil date (Howard Hinnant algorithm)
        let z = secs / 86400 + 719468;
        let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if m <= 2 { y + 1 } else { y };
        // Weekday: epoch (1970-01-01) was Thursday (4)
        let total_days = secs / 86400;
        let wday = ((total_days % 7 + 4) % 7) as usize;
        let month_idx = if m >= 1 && m <= 12 {
            (m - 1) as usize
        } else {
            0
        };
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAY[wday.min(6)],
            d,
            MONTH[month_idx.min(11)],
            year,
            hour,
            minute,
            second,
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs: libc::time_t = if timestamp < 0 {
            0
        } else {
            timestamp as libc::time_t
        };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            #[cfg(windows)]
            {
                libc::gmtime_s(&mut tm, &secs) == 0
            }
            #[cfg(not(windows))]
            {
                !libc::gmtime_r(&secs, &mut tm).is_null()
            }
        };
        if !ok {
            return "Thu, 01 Jan 1970 00:00:00 GMT".to_string();
        }
        let wday = usize::try_from(tm.tm_wday).unwrap_or(0).min(6);
        let month = usize::try_from(tm.tm_mon).unwrap_or(0).min(11);
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAY[wday],
            tm.tm_mday,
            MONTH[month],
            tm.tm_year + 1900,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

fn http_server_date_time_string_from_bits(
    _py: &crate::PyToken<'_>,
    timestamp_bits: u64,
) -> Result<String, u64> {
    let ts = if obj_from_bits(timestamp_bits).is_none() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(now.as_secs()).unwrap_or(i64::MAX)
    } else {
        let Some(value) = to_f64(obj_from_bits(timestamp_bits)) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "timestamp must be float or None",
            ));
        };
        if !value.is_finite() || value < 0.0 {
            0
        } else {
            value as i64
        }
    };
    Ok(http_server_format_gmt_timestamp(ts))
}

fn http_server_send_response_only_impl(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
    code: i64,
    message: Option<String>,
) -> Result<(), u64> {
    let request_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"request_version")?
            .unwrap_or_else(|| HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string());
    if request_version == HTTP_SERVER_DEFAULT_REQUEST_VERSION {
        return Ok(());
    }
    let reason = message.unwrap_or_else(|| http_server_reason_phrase(code).to_string());
    let status = format!("HTTP/1.1 {} {}\r\n", code, reason);
    http_server_write_bytes(_py, handler_bits, status.as_bytes())
}

fn http_server_send_response_impl(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
    code: i64,
    message: Option<String>,
) -> Result<(), u64> {
    http_server_send_response_only_impl(_py, handler_bits, code, message)?;
    let request_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"request_version")?
            .unwrap_or_else(|| HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string());
    if request_version == HTTP_SERVER_DEFAULT_REQUEST_VERSION {
        return Ok(());
    }
    let server_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"server_version")?
            .unwrap_or_else(|| "BaseHTTP/0.6".to_string());
    let sys_version = http_server_get_optional_attr_string(_py, handler_bits, b"sys_version")?
        .unwrap_or_default();
    let version = http_server_version_string_impl(&server_version, &sys_version);
    let date = http_server_format_gmt_timestamp(
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
        .unwrap_or(i64::MAX),
    );
    http_server_send_header_impl(_py, handler_bits, "Server", &version)?;
    http_server_send_header_impl(_py, handler_bits, "Date", &date)?;
    Ok(())
}

fn http_server_send_header_impl(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
    keyword: &str,
    value: &str,
) -> Result<(), u64> {
    let line = format!("{keyword}: {value}\r\n");
    http_server_write_bytes(_py, handler_bits, line.as_bytes())
}

fn http_server_end_headers_impl(_py: &crate::PyToken<'_>, handler_bits: u64) -> Result<(), u64> {
    http_server_write_bytes(_py, handler_bits, b"\r\n")?;
    http_server_flush(_py, handler_bits)
}

fn http_server_send_error_impl(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
    code: i64,
    message: Option<String>,
) -> Result<(), u64> {
    let short = http_server_reason_phrase(code).to_string();
    let text = message.unwrap_or_else(|| short.clone());
    let explain = http_server_error_explain(code).to_string();
    let escaped_message = http_server_html_escape(&text);
    let escaped_explain = http_server_html_escape(&explain);
    let body = format!(
        "<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        <title>Error response</title>\n    </head>\n    <body>\n        <h1>Error response</h1>\n        <p>Error code: {code}</p>\n        <p>Message: {escaped_message}.</p>\n        <p>Error code explanation: {code} - {escaped_explain}.</p>\n    </body>\n</html>\n"
    );

    http_server_send_response_impl(_py, handler_bits, code, Some(text))?;
    let request_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"request_version")?
            .unwrap_or_else(|| HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string());
    if request_version != HTTP_SERVER_DEFAULT_REQUEST_VERSION {
        http_server_send_header_impl(_py, handler_bits, "Content-Type", "text/html;charset=utf-8")?;
        http_server_send_header_impl(_py, handler_bits, "Content-Length", &body.len().to_string())?;
        http_server_end_headers_impl(_py, handler_bits)?;
    }
    http_server_write_bytes(_py, handler_bits, body.as_bytes())?;
    let _ = urllib_request_set_attr(
        _py,
        handler_bits,
        b"close_connection",
        MoltObject::from_bool(true).bits(),
    );
    Ok(())
}

fn http_server_handle_one_request_impl(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
) -> Result<bool, u64> {
    let state = http_server_read_request_impl(_py, handler_bits)?;
    if state == 0 {
        return Ok(false);
    }
    if state == 2 {
        let close = urllib_attr_truthy(_py, handler_bits, b"close_connection")?;
        return Ok(!close);
    }
    if state != 1 {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http server request parser returned invalid state",
        ));
    }

    let close_connection = http_server_compute_close_connection_impl(_py, handler_bits)?;
    if !urllib_request_set_attr(
        _py,
        handler_bits,
        b"close_connection",
        MoltObject::from_bool(close_connection).bits(),
    ) {
        return Err(MoltObject::none().bits());
    }

    let Some(prepare_headers_name_bits) = attr_name_bits_from_bytes(_py, b"_molt_prepare_headers")
    else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let prepare_headers_bits =
        molt_getattr_builtin(handler_bits, prepare_headers_name_bits, missing);
    dec_ref_bits(_py, prepare_headers_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if prepare_headers_bits != missing
        && is_truthy(_py, obj_from_bits(molt_is_callable(prepare_headers_bits)))
    {
        let _ = unsafe { call_callable0(_py, prepare_headers_bits) };
        dec_ref_bits(_py, prepare_headers_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    } else if prepare_headers_bits != missing {
        dec_ref_bits(_py, prepare_headers_bits);
    }

    let command =
        http_server_get_optional_attr_string(_py, handler_bits, b"command")?.unwrap_or_default();
    let method_name = format!("do_{command}");
    let Some(method_name_bits) = alloc_string_bits(_py, &method_name) else {
        return Err(MoltObject::none().bits());
    };
    let method_bits = molt_getattr_builtin(handler_bits, method_name_bits, missing);
    dec_ref_bits(_py, method_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if method_bits == missing {
        let message = format!(
            "Unsupported method ({})",
            http_server_repr_single_quoted(&command)
        );
        http_server_send_error_impl(_py, handler_bits, 501, Some(message))?;
    } else {
        let _ = unsafe { call_callable0(_py, method_bits) };
        dec_ref_bits(_py, method_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    let close = urllib_attr_truthy(_py, handler_bits, b"close_connection")?;
    Ok(!close)
}

fn urllib_request_set_attr(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return false;
    };
    let _ = crate::molt_object_setattr(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn urllib_request_handler_order(_py: &crate::PyToken<'_>, handler_bits: u64) -> Result<i64, u64> {
    let Some(order_bits) = urllib_request_attr_optional(_py, handler_bits, b"handler_order")?
    else {
        return Ok(500);
    };
    let out = to_i64(obj_from_bits(order_bits)).unwrap_or(500);
    dec_ref_bits(_py, order_bits);
    Ok(out)
}

fn urllib_request_ensure_handlers_list(
    _py: &crate::PyToken<'_>,
    opener_bits: u64,
) -> Result<u64, u64> {
    if let Some(list_bits) = urllib_request_attr_optional(_py, opener_bits, b"_molt_handlers")? {
        let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "opener handler registry is invalid",
            ));
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "opener handler registry is invalid",
                ));
            }
        }
        return Ok(list_bits);
    }
    let list_ptr = alloc_list_with_capacity(_py, &[], 0);
    if list_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    if !urllib_request_set_attr(_py, opener_bits, b"_molt_handlers", list_bits) {
        return Err(MoltObject::none().bits());
    }
    Ok(list_bits)
}

fn urllib_request_set_cursor(_py: &crate::PyToken<'_>, opener_bits: u64, cursor: i64) -> bool {
    urllib_request_set_attr(
        _py,
        opener_bits,
        b"_molt_open_cursor",
        MoltObject::from_int(cursor).bits(),
    )
}

fn urllib_request_get_cursor(_py: &crate::PyToken<'_>, opener_bits: u64) -> Result<i64, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, opener_bits, b"_molt_open_cursor")? else {
        return Ok(0);
    };
    let out = to_i64(obj_from_bits(bits)).unwrap_or(0);
    dec_ref_bits(_py, bits);
    Ok(out)
}

fn urllib_data_percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            let h1 = (bytes[idx + 1] as char).to_digit(16);
            let h2 = (bytes[idx + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (h1, h2) {
                out.push(((a << 4) | b) as u8);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    out
}

fn urllib_data_base64_val(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn urllib_data_base64_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !(*b as char).is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if !compact.len().is_multiple_of(4) {
        return Err("Invalid base64 data URL payload".to_string());
    }
    let mut out: Vec<u8> = Vec::with_capacity(compact.len() / 4 * 3);
    let mut idx = 0usize;
    while idx < compact.len() {
        let c0 = compact[idx];
        let c1 = compact[idx + 1];
        let c2 = compact[idx + 2];
        let c3 = compact[idx + 3];
        let Some(v0) = urllib_data_base64_val(c0) else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        let Some(v1) = urllib_data_base64_val(c1) else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        let pad2 = c2 == b'=';
        let pad3 = c3 == b'=';
        let v2 = if pad2 {
            0
        } else if let Some(v) = urllib_data_base64_val(c2) {
            v
        } else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        let v3 = if pad3 {
            0
        } else if let Some(v) = urllib_data_base64_val(c3) {
            v
        } else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        out.push((v0 << 2) | (v1 >> 4));
        if !pad2 {
            out.push(((v1 & 0x0F) << 4) | (v2 >> 2));
        }
        if !pad3 {
            out.push(((v2 & 0x03) << 6) | v3);
        }
        if pad2 && !pad3 {
            return Err("Invalid base64 data URL payload".to_string());
        }
        idx += 4;
    }
    Ok(out)
}

fn urllib_request_decode_data_url(url: &str) -> Result<Vec<u8>, String> {
    let Some(payload) = url.strip_prefix("data:") else {
        return Err("unsupported URL scheme".to_string());
    };
    let Some((meta, raw_data)) = payload.split_once(',') else {
        return Err("Malformed data URL".to_string());
    };
    let percent_decoded = urllib_data_percent_decode(raw_data);
    let is_base64 = meta
        .split(';')
        .any(|item| item.eq_ignore_ascii_case("base64"));
    if is_base64 {
        urllib_data_base64_decode(&percent_decoded)
    } else {
        Ok(percent_decoded)
    }
}

fn urllib_response_registry() -> &'static Mutex<HashMap<u64, MoltUrllibResponse>> {
    URLLIB_RESPONSE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn urllib_response_from_parts(
    body: Vec<u8>,
    url: String,
    code: i64,
    reason: String,
    headers: Vec<(String, String)>,
) -> MoltUrllibResponse {
    let mut header_joined: HashMap<String, String> = HashMap::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        let key = http_message_header_key(name);
        match header_joined.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value.clone());
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let joined = entry.get_mut();
                joined.push_str(", ");
                joined.push_str(value);
            }
        }
    }
    MoltUrllibResponse {
        body,
        pos: 0,
        closed: false,
        url,
        code,
        reason,
        headers,
        header_joined,
        headers_dict_cache: None,
        headers_list_cache: None,
    }
}

fn urllib_response_joined_header<'a>(resp: &'a MoltUrllibResponse, name: &str) -> Option<&'a str> {
    resp.header_joined
        .get(&http_message_header_key(name))
        .map(String::as_str)
}

fn urllib_response_headers_dict_bits(
    _py: &crate::PyToken<'_>,
    resp: &mut MoltUrllibResponse,
) -> Result<u64, u64> {
    if let Some(bits) = resp.headers_dict_cache {
        inc_ref_bits(_py, bits);
        return Ok(bits);
    }
    let bits = urllib_http_headers_to_dict(_py, &resp.headers)?;
    resp.headers_dict_cache = Some(bits);
    inc_ref_bits(_py, bits);
    Ok(bits)
}

fn urllib_response_headers_list_bits(
    _py: &crate::PyToken<'_>,
    resp: &mut MoltUrllibResponse,
) -> Result<u64, u64> {
    if resp.headers_list_cache.is_none() {
        let bits = urllib_http_headers_to_list(_py, &resp.headers)?;
        resp.headers_list_cache = Some(bits);
    }
    let Some(cached_bits) = resp.headers_list_cache else {
        return Err(MoltObject::none().bits());
    };
    let Some(cached_ptr) = obj_from_bits(cached_bits).as_ptr() else {
        return Err(MoltObject::none().bits());
    };
    if unsafe { object_type_id(cached_ptr) } != TYPE_ID_LIST {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "response headers cache is invalid",
        ));
    }
    let items = unsafe { seq_vec_ref(cached_ptr) };
    let list_ptr = alloc_list_with_capacity(_py, items.as_slice(), items.len());
    if list_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

fn urllib_response_store(response: MoltUrllibResponse) -> Option<i64> {
    let id = URLLIB_RESPONSE_NEXT.fetch_add(1, Ordering::Relaxed);
    let Ok(mut guard) = urllib_response_registry().lock() else {
        return None;
    };
    guard.insert(id, response);
    i64::try_from(id).ok()
}

fn urllib_response_with_mut<T>(
    handle: i64,
    f: impl FnOnce(&mut MoltUrllibResponse) -> T,
) -> Option<T> {
    let Ok(mut guard) = urllib_response_registry().lock() else {
        return None;
    };
    guard.get_mut(&(handle as u64)).map(f)
}

fn urllib_response_with<T>(handle: i64, f: impl FnOnce(&MoltUrllibResponse) -> T) -> Option<T> {
    let Ok(guard) = urllib_response_registry().lock() else {
        return None;
    };
    guard.get(&(handle as u64)).map(f)
}

fn urllib_response_drop(_py: &crate::PyToken<'_>, handle: i64) {
    if let Ok(mut guard) = urllib_response_registry().lock()
        && let Some(mut response) = guard.remove(&(handle as u64))
    {
        if let Some(bits) = response.headers_dict_cache.take() {
            dec_ref_bits(_py, bits);
        }
        if let Some(bits) = response.headers_list_cache.take() {
            dec_ref_bits(_py, bits);
        }
    }
}

fn http_client_connection_runtime() -> &'static Mutex<MoltHttpClientConnectionRuntime> {
    HTTP_CLIENT_CONNECTION_RUNTIME.get_or_init(|| {
        Mutex::new(MoltHttpClientConnectionRuntime {
            next_handle: 1,
            connections: HashMap::new(),
        })
    })
}

fn http_client_connection_store(host: String, port: u16, timeout: Option<f64>) -> Option<i64> {
    let Ok(mut guard) = http_client_connection_runtime().lock() else {
        return None;
    };
    let handle = guard.next_handle;
    guard.next_handle = guard.next_handle.saturating_add(1);
    guard.connections.insert(
        handle,
        MoltHttpClientConnection {
            host,
            port,
            timeout,
            method: None,
            url: None,
            headers: Vec::new(),
            body: Vec::new(),
            buffer: Vec::new(),
            skip_host: false,
            skip_accept_encoding: false,
        },
    );
    i64::try_from(handle).ok()
}

fn http_client_connection_with_mut<T>(
    handle: i64,
    f: impl FnOnce(&mut MoltHttpClientConnection) -> T,
) -> Option<T> {
    let Ok(mut guard) = http_client_connection_runtime().lock() else {
        return None;
    };
    guard.connections.get_mut(&(handle as u64)).map(f)
}

fn http_client_connection_with<T>(
    handle: i64,
    f: impl FnOnce(&MoltHttpClientConnection) -> T,
) -> Option<T> {
    let Ok(guard) = http_client_connection_runtime().lock() else {
        return None;
    };
    guard.connections.get(&(handle as u64)).map(f)
}

fn http_client_connection_drop(handle: i64) {
    if let Ok(mut guard) = http_client_connection_runtime().lock() {
        guard.connections.remove(&(handle as u64));
    }
}

fn http_client_connection_reset_pending(conn: &mut MoltHttpClientConnection) {
    conn.method = None;
    conn.url = None;
    conn.headers.clear();
    conn.body.clear();
    conn.buffer.clear();
    conn.skip_host = false;
    conn.skip_accept_encoding = false;
}

fn http_client_apply_default_headers(
    headers: &mut Vec<(String, String)>,
    host: &str,
    port: u16,
    skip_host: bool,
    skip_accept_encoding: bool,
) {
    if !skip_host
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        let host_value = if port == 80 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        headers.insert(0, ("Host".to_string(), host_value));
    }
    if !skip_accept_encoding
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push(("Accept-Encoding".to_string(), "identity".to_string()));
    }
}

fn http_client_alloc_buffer_list(_py: &crate::PyToken<'_>, buffer: &[Vec<u8>]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(buffer.len());
    for chunk in buffer {
        let item_ptr = alloc_bytes(_py, chunk.as_slice());
        if item_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        item_bits.push(MoltObject::from_ptr(item_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn http_message_runtime() -> &'static Mutex<MoltHttpMessageRuntime> {
    HTTP_MESSAGE_RUNTIME.get_or_init(|| {
        Mutex::new(MoltHttpMessageRuntime {
            next_handle: 1,
            messages: HashMap::new(),
        })
    })
}

#[inline]
fn http_message_header_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn http_message_from_headers(headers: Vec<(String, String)>) -> MoltHttpMessage {
    let mut index: HashMap<String, Vec<usize>> = HashMap::with_capacity(headers.len());
    for (idx, (name, _)) in headers.iter().enumerate() {
        index
            .entry(http_message_header_key(name))
            .or_default()
            .push(idx);
    }
    MoltHttpMessage {
        headers,
        index,
        items_list_cache: None,
    }
}

fn http_message_push_header(
    _py: &crate::PyToken<'_>,
    message: &mut MoltHttpMessage,
    name: String,
    value: String,
) {
    if let Some(cache_bits) = message.items_list_cache.take()
        && !obj_from_bits(cache_bits).is_none()
    {
        dec_ref_bits(_py, cache_bits);
    }
    let idx = message.headers.len();
    let key = http_message_header_key(name.as_str());
    message.headers.push((name, value));
    message.index.entry(key).or_default().push(idx);
}

fn http_message_store(headers: Vec<(String, String)>) -> Option<i64> {
    let Ok(mut guard) = http_message_runtime().lock() else {
        return None;
    };
    let handle = guard.next_handle;
    guard.next_handle = guard.next_handle.saturating_add(1);
    guard
        .messages
        .insert(handle, http_message_from_headers(headers));
    i64::try_from(handle).ok()
}

fn http_message_store_new() -> Option<i64> {
    http_message_store(Vec::new())
}

fn http_message_with_mut<T>(handle: i64, f: impl FnOnce(&mut MoltHttpMessage) -> T) -> Option<T> {
    let Ok(mut guard) = http_message_runtime().lock() else {
        return None;
    };
    guard.messages.get_mut(&(handle as u64)).map(f)
}

fn http_message_with<T>(handle: i64, f: impl FnOnce(&MoltHttpMessage) -> T) -> Option<T> {
    let Ok(guard) = http_message_runtime().lock() else {
        return None;
    };
    guard.messages.get(&(handle as u64)).map(f)
}

fn http_message_drop(_py: &crate::PyToken<'_>, handle: i64) {
    if let Ok(mut guard) = http_message_runtime().lock()
        && let Some(message) = guard.messages.remove(&(handle as u64))
        && let Some(cache_bits) = message.items_list_cache
        && !obj_from_bits(cache_bits).is_none()
    {
        dec_ref_bits(_py, cache_bits);
    }
}

fn http_message_handle_from_bits(_py: &crate::PyToken<'_>, handle_bits: u64) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "http message handle is invalid",
        ));
    };
    if handle <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "http message handle is invalid",
        ));
    }
    Ok(handle)
}

fn http_message_values_to_list_from_indices(
    _py: &crate::PyToken<'_>,
    message: &MoltHttpMessage,
    indices: &[usize],
) -> Result<u64, u64> {
    let mut item_bits: Vec<u64> = Vec::with_capacity(indices.len());
    for &idx in indices {
        let value_ptr = alloc_string(_py, message.headers[idx].1.as_bytes());
        if value_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        item_bits.push(MoltObject::from_ptr(value_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

fn cookiejar_registry() -> &'static Mutex<HashMap<u64, MoltCookieJar>> {
    COOKIEJAR_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cookiejar_store_new() -> Option<i64> {
    let id = COOKIEJAR_NEXT.fetch_add(1, Ordering::Relaxed);
    let Ok(mut guard) = cookiejar_registry().lock() else {
        return None;
    };
    guard.insert(id, MoltCookieJar::default());
    i64::try_from(id).ok()
}

fn cookiejar_with_mut<T>(handle: i64, f: impl FnOnce(&mut MoltCookieJar) -> T) -> Option<T> {
    let Ok(mut guard) = cookiejar_registry().lock() else {
        return None;
    };
    guard.get_mut(&(handle as u64)).map(f)
}

fn cookiejar_with<T>(handle: i64, f: impl FnOnce(&MoltCookieJar) -> T) -> Option<T> {
    let Ok(guard) = cookiejar_registry().lock() else {
        return None;
    };
    guard.get(&(handle as u64)).map(f)
}

fn urllib_cookiejar_domain_matches(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn urllib_cookiejar_path_matches(request_path: &str, cookie_path: &str) -> bool {
    if cookie_path == "/" {
        return true;
    }
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/')
        || request_path
            .as_bytes()
            .get(cookie_path.len())
            .copied()
            .is_some_and(|b| b == b'/')
}

fn urllib_cookiejar_default_scope(url: &str) -> (String, String) {
    let parts = urllib_urlsplit_impl(url, "", true);
    let host = urllib_http_parse_host_port(&parts[1], 80)
        .0
        .to_ascii_lowercase();
    let raw_path = if parts[2].is_empty() {
        "/".to_string()
    } else if parts[2].starts_with('/') {
        parts[2].clone()
    } else {
        format!("/{}", parts[2])
    };
    let path = match raw_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => raw_path[..idx].to_string(),
    };
    (host, path)
}

fn urllib_cookiejar_parse_set_cookie(
    set_cookie_value: &str,
    default_domain: &str,
    default_path: &str,
) -> Option<(MoltCookieEntry, bool)> {
    let mut parts = set_cookie_value.split(';');
    let first = parts.next()?.trim();
    let (name_raw, value_raw) = first.split_once('=')?;
    let name = name_raw.trim();
    if name.is_empty() {
        return None;
    }
    let mut domain = default_domain.to_ascii_lowercase();
    let mut path = if default_path.is_empty() {
        "/".to_string()
    } else {
        default_path.to_string()
    };
    let mut delete_cookie = false;
    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (key, value_opt) = match attr.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), Some(v.trim())),
            None => (attr.to_ascii_lowercase(), None),
        };
        match key.as_str() {
            "domain" => {
                if let Some(value) = value_opt {
                    let normalized = value.trim().trim_start_matches('.').to_ascii_lowercase();
                    if !normalized.is_empty() {
                        domain = normalized;
                    }
                }
            }
            "path" => {
                if let Some(value) = value_opt
                    && !value.is_empty()
                {
                    path = if value.starts_with('/') {
                        value.to_string()
                    } else {
                        format!("/{value}")
                    };
                }
            }
            "max-age" => {
                if let Some(value) = value_opt
                    && value == "0"
                {
                    delete_cookie = true;
                }
            }
            _ => {}
        }
    }
    Some((
        MoltCookieEntry {
            name: name.to_string(),
            value: value_raw.trim().to_string(),
            domain,
            path,
        },
        delete_cookie,
    ))
}

fn urllib_cookiejar_store_from_headers(
    handle: i64,
    request_url: &str,
    headers: &[(String, String)],
) {
    let (default_domain, default_path) = urllib_cookiejar_default_scope(request_url);
    for (header_name, header_value) in headers {
        if !header_name.eq_ignore_ascii_case("Set-Cookie") {
            continue;
        }
        let Some((cookie, delete_cookie)) =
            urllib_cookiejar_parse_set_cookie(header_value, &default_domain, &default_path)
        else {
            continue;
        };
        let _ = cookiejar_with_mut(handle, |jar| {
            let same_cookie = |entry: &MoltCookieEntry| {
                entry.name == cookie.name
                    && entry.domain == cookie.domain
                    && entry.path == cookie.path
            };
            if delete_cookie || cookie.value.is_empty() {
                jar.cookies.retain(|entry| !same_cookie(entry));
                return;
            }
            if let Some(existing) = jar.cookies.iter_mut().find(|entry| same_cookie(entry)) {
                *existing = cookie;
            } else {
                jar.cookies.push(cookie);
            }
        });
    }
}

fn urllib_cookiejar_header_for_url(handle: i64, request_url: &str) -> Option<String> {
    let parts = urllib_urlsplit_impl(request_url, "", true);
    let host = urllib_http_parse_host_port(&parts[1], 80)
        .0
        .to_ascii_lowercase();
    let path = if parts[2].is_empty() {
        "/".to_string()
    } else if parts[2].starts_with('/') {
        parts[2].clone()
    } else {
        format!("/{}", parts[2])
    };
    cookiejar_with(handle, |jar| {
        let mut pairs: Vec<String> = Vec::new();
        for entry in &jar.cookies {
            if urllib_cookiejar_domain_matches(&host, &entry.domain)
                && urllib_cookiejar_path_matches(&path, &entry.path)
            {
                pairs.push(format!("{}={}", entry.name, entry.value));
            }
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    })
    .flatten()
}

fn http_cookies_parse_pairs(cookie_header: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for segment in cookie_header.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let Some((name_raw, value_raw)) = segment.split_once('=') else {
            continue;
        };
        let name = name_raw.trim();
        if name.is_empty() {
            continue;
        }
        out.push((name.to_string(), value_raw.trim().to_string()));
    }
    out
}

fn http_cookies_attr_text(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    if obj_from_bits(value_bits).is_none() {
        return None;
    }
    let text = crate::format_obj_str(_py, obj_from_bits(value_bits));
    if text.is_empty() { None } else { Some(text) }
}

fn http_cookies_expires_text(_py: &crate::PyToken<'_>, expires_bits: u64) -> Option<String> {
    if obj_from_bits(expires_bits).is_none() {
        return None;
    }
    if let Some(offset_seconds) = to_i64(obj_from_bits(expires_bits)) {
        let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
            Err(_) => 0,
        };
        let absolute = now.saturating_add(offset_seconds);
        return Some(http_server_format_gmt_timestamp(absolute));
    }
    http_cookies_attr_text(_py, expires_bits)
}

struct HttpCookieMorselInput {
    name_bits: u64,
    value_bits: u64,
    path_bits: u64,
    secure_bits: u64,
    httponly_bits: u64,
    max_age_bits: u64,
    expires_bits: u64,
}

fn http_cookies_render_morsel_impl(
    _py: &crate::PyToken<'_>,
    input: HttpCookieMorselInput,
) -> String {
    let name = crate::format_obj_str(_py, obj_from_bits(input.name_bits));
    let value = crate::format_obj_str(_py, obj_from_bits(input.value_bits));
    let mut segments: Vec<String> = vec![format!("{name}={value}")];

    if let Some(expires_value) = http_cookies_expires_text(_py, input.expires_bits) {
        segments.push(format!("expires={expires_value}"));
    }

    if !obj_from_bits(input.httponly_bits).is_none()
        && is_truthy(_py, obj_from_bits(input.httponly_bits))
    {
        segments.push("HttpOnly".to_string());
    }

    if !obj_from_bits(input.max_age_bits).is_none() {
        if let Some(max_age_int) = to_i64(obj_from_bits(input.max_age_bits)) {
            segments.push(format!("Max-Age={max_age_int}"));
        } else if let Some(max_age_text) = http_cookies_attr_text(_py, input.max_age_bits) {
            segments.push(format!("Max-Age={max_age_text}"));
        }
    }

    if let Some(path_value) = http_cookies_attr_text(_py, input.path_bits) {
        segments.push(format!("Path={path_value}"));
    }

    if !obj_from_bits(input.secure_bits).is_none()
        && is_truthy(_py, obj_from_bits(input.secure_bits))
    {
        segments.push("Secure".to_string());
    }

    segments.join("; ")
}

struct HttpClientExecuteInput {
    host: String,
    port: u16,
    timeout: Option<f64>,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    skip_host: bool,
    skip_accept_encoding: bool,
}

fn urllib_http_extract_headers_mapping(
    _py: &crate::PyToken<'_>,
    mapping_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    if obj_from_bits(mapping_bits).is_none() {
        return Ok(Vec::new());
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(mapping_bits, items_name_bits, missing);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing
        || !is_truthy(_py, obj_from_bits(molt_is_callable(items_method_bits)))
    {
        if items_method_bits != missing {
            dec_ref_bits(_py, items_method_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "headers must be a mapping",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let iter_bits = molt_iter(iterable_bits);
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        };
        let item_type = unsafe { object_type_id(item_ptr) };
        if item_type != TYPE_ID_LIST && item_type != TYPE_ID_TUPLE {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        out.push((
            crate::format_obj_str(_py, obj_from_bits(fields[0])),
            crate::format_obj_str(_py, obj_from_bits(fields[1])),
        ));
        dec_ref_bits(_py, item_bits);
    }
    Ok(out)
}

fn urllib_cookiejar_handles_from_handlers(
    _py: &crate::PyToken<'_>,
    handlers: &[u64],
) -> Result<Vec<i64>, u64> {
    let mut out: Vec<i64> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for handler_bits in handlers {
        let Some(cookiejar_bits) = urllib_request_attr_optional(_py, *handler_bits, b"cookiejar")?
        else {
            continue;
        };
        if obj_from_bits(cookiejar_bits).is_none() {
            dec_ref_bits(_py, cookiejar_bits);
            continue;
        }
        let handle_opt =
            match urllib_request_attr_optional(_py, cookiejar_bits, b"_molt_cookiejar_handle") {
                Ok(value) => value,
                Err(bits) => {
                    dec_ref_bits(_py, cookiejar_bits);
                    return Err(bits);
                }
            };
        dec_ref_bits(_py, cookiejar_bits);
        let Some(handle_bits) = handle_opt else {
            continue;
        };
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            dec_ref_bits(_py, handle_bits);
            continue;
        };
        dec_ref_bits(_py, handle_bits);
        if seen.insert(handle) {
            out.push(handle);
        }
    }
    Ok(out)
}

fn urllib_cookiejar_apply_header_for_url(
    _py: &crate::PyToken<'_>,
    cookiejar_handles: &[i64],
    request_url: &str,
    headers: &mut Vec<(String, String)>,
) {
    for handle in cookiejar_handles {
        let Some(cookie_header) = urllib_cookiejar_header_for_url(*handle, request_url) else {
            continue;
        };
        let mut replaced = false;
        for (name, value) in headers.iter_mut() {
            if name.eq_ignore_ascii_case("Cookie") {
                if value.is_empty() {
                    *value = cookie_header.clone();
                } else {
                    *value = format!("{value}; {cookie_header}");
                }
                replaced = true;
                break;
            }
        }
        if !replaced {
            headers.push(("Cookie".to_string(), cookie_header));
        }
    }
}

fn urllib_cookiejar_store_headers_for_url(
    cookiejar_handles: &[i64],
    request_url: &str,
    response_headers: &[(String, String)],
) {
    for handle in cookiejar_handles {
        urllib_cookiejar_store_from_headers(*handle, request_url, response_headers);
    }
}

fn urllib_http_timeout_error(_py: &crate::PyToken<'_>) -> u64 {
    raise_exception::<_>(_py, "TimeoutError", "timed out")
}

fn urllib_http_request_timeout(
    _py: &crate::PyToken<'_>,
    request_bits: u64,
) -> Result<Option<f64>, u64> {
    let Some(timeout_bits) = urllib_request_attr_optional(_py, request_bits, b"timeout")? else {
        return Ok(None);
    };
    if obj_from_bits(timeout_bits).is_none() {
        dec_ref_bits(_py, timeout_bits);
        return Ok(None);
    }
    let timeout = to_f64(obj_from_bits(timeout_bits))
        .or_else(|| to_i64(obj_from_bits(timeout_bits)).map(|v| v as f64));
    dec_ref_bits(_py, timeout_bits);
    let Some(value) = timeout else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "timeout must be a number",
        ));
    };
    if value < 0.0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "timeout value out of range",
        ));
    }
    Ok(Some(value))
}

fn urllib_http_host_matches_no_proxy(host: &str, no_proxy: &str) -> bool {
    let normalized_host = host.trim().to_ascii_lowercase();
    if normalized_host.is_empty() {
        return false;
    }
    for raw in no_proxy.split(',') {
        let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() {
            continue;
        }
        if token == "*" {
            return true;
        }
        let needle = token.strip_prefix('.').unwrap_or(&token);
        if needle.is_empty() {
            continue;
        }
        if normalized_host == needle || normalized_host.ends_with(&format!(".{needle}")) {
            return true;
        }
    }
    false
}

fn urllib_http_parse_host_port(netloc: &str, default_port: u16) -> (String, u16) {
    let without_user = netloc.rsplit('@').next().unwrap_or(netloc);
    if without_user.starts_with('[')
        && let Some(end) = without_user.find(']')
    {
        let host = without_user[1..end].to_string();
        if let Some(port_part) = without_user[end + 1..].strip_prefix(':')
            && let Ok(port) = port_part.parse::<u16>()
        {
            return (host, port);
        }
        return (host, default_port);
    }
    if let Some((host, port_part)) = without_user.rsplit_once(':')
        && !host.is_empty()
        && !port_part.is_empty()
        && !host.contains(':')
        && let Ok(port) = port_part.parse::<u16>()
    {
        return (host.to_string(), port);
    }
    (without_user.to_string(), default_port)
}

fn urllib_http_join_url(base: &str, target: &str) -> String {
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("data:")
    {
        return target.to_string();
    }
    let base_parts = urllib_urlsplit_impl(base, "", true);
    if target.starts_with("//") {
        return format!("{}:{}", base_parts[0], target);
    }
    if target.starts_with('/') {
        return urllib_unsplit_impl(&base_parts[0], &base_parts[1], target, "", "");
    }
    let base_path = &base_parts[2];
    let base_dir = match base_path.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    let joined = if base_dir.is_empty() {
        format!("/{}", target)
    } else {
        format!("{base_dir}/{target}")
    };
    urllib_unsplit_impl(&base_parts[0], &base_parts[1], &joined, "", "")
}

fn urllib_http_headers_to_dict(
    _py: &crate::PyToken<'_>,
    headers: &[(String, String)],
) -> Result<u64, u64> {
    let mut pair_bits: Vec<u64> = Vec::with_capacity(headers.len().saturating_mul(2));
    for (name, value) in headers {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            for bits in pair_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            for bits in pair_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        pair_bits.push(name_bits);
        pair_bits.push(value_bits);
    }
    let dict = alloc_dict_with_pairs(_py, &pair_bits);
    for bits in pair_bits {
        dec_ref_bits(_py, bits);
    }
    if dict.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(dict).bits())
    }
}

fn urllib_http_headers_to_list(
    _py: &crate::PyToken<'_>,
    headers: &[(String, String)],
) -> Result<u64, u64> {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            dec_ref_bits(_py, name_bits);
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let pair_ptr = alloc_tuple(_py, &[name_bits, value_bits]);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, value_bits);
        if pair_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        tuple_bits.push(MoltObject::from_ptr(pair_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, tuple_bits.as_slice(), tuple_bits.len());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

fn http_client_extract_headers(
    _py: &crate::PyToken<'_>,
    headers_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    if obj_from_bits(headers_bits).is_none() {
        return Ok(Vec::new());
    }
    let iter_bits = molt_iter(headers_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers must be an iterable of pairs",
            ));
        };
        let is_sequence = unsafe {
            let ty = object_type_id(item_ptr);
            ty == TYPE_ID_TUPLE || ty == TYPE_ID_LIST
        };
        if !is_sequence {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "header entries must be (name, value) pairs",
            ));
        }
        let pair = unsafe { seq_vec_ref(item_ptr) };
        if pair.len() < 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "header entries must be (name, value) pairs",
            ));
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "header name must be str",
            ));
        };
        let value = crate::format_obj_str(_py, obj_from_bits(pair[1]));
        dec_ref_bits(_py, item_bits);
        out.push((name, value));
    }
    Ok(out)
}

fn http_client_response_handle_from_bits(
    _py: &crate::PyToken<'_>,
    handle_bits: u64,
) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response handle is invalid",
        ));
    };
    Ok(handle)
}

fn http_client_connection_handle_from_bits(
    _py: &crate::PyToken<'_>,
    handle_bits: u64,
) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "connection handle is invalid",
        ));
    };
    if handle <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "connection handle is invalid",
        ));
    }
    Ok(handle)
}

fn http_client_execute_request(
    _py: &crate::PyToken<'_>,
    mut input: HttpClientExecuteInput,
) -> Result<i64, u64> {
    http_client_apply_default_headers(
        &mut input.headers,
        input.host.as_str(),
        input.port,
        input.skip_host,
        input.skip_accept_encoding,
    );
    let request_target = if input.url.is_empty() {
        "/".to_string()
    } else {
        input.url.clone()
    };
    let host_header = if input.port == 80 {
        input.host.clone()
    } else {
        format!("{}:{}", input.host, input.port)
    };
    let req = UrllibHttpRequest {
        host: input.host.clone(),
        port: input.port,
        path: request_target.clone(),
        method: input.method,
        headers: input.headers,
        body: input.body,
        timeout: input.timeout,
    };
    let (code, reason, resp_headers, resp_body) =
        match urllib_http_try_inmemory_dispatch(_py, &req, &request_target, &host_header) {
            Ok(Some(value)) => value,
            Ok(None) => match urllib_http_send_request(&req, &request_target, &host_header) {
                Ok(value) => value,
                Err(err) => {
                    if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock {
                        return Err(raise_exception::<u64>(_py, "TimeoutError", "timed out"));
                    }
                    return Err(raise_exception::<u64>(_py, "OSError", &err.to_string()));
                }
            },
            Err(bits) => return Err(bits),
        };
    let response_url = if input.url.starts_with("http://") || input.url.starts_with("https://") {
        input.url
    } else if request_target.starts_with('/') {
        format!("http://{host_header}{request_target}")
    } else {
        format!("http://{host_header}/{request_target}")
    };
    let Some(handle) = urllib_response_store(urllib_response_from_parts(
        resp_body,
        response_url,
        code,
        reason,
        resp_headers,
    )) else {
        return Err(MoltObject::none().bits());
    };
    Ok(handle)
}

fn urllib_http_extract_request_headers(
    _py: &crate::PyToken<'_>,
    request_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    let Some(headers_bits) = urllib_request_attr_optional(_py, request_bits, b"headers")? else {
        return Ok(Vec::new());
    };
    if obj_from_bits(headers_bits).is_none() {
        dec_ref_bits(_py, headers_bits);
        return Ok(Vec::new());
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        dec_ref_bits(_py, headers_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(headers_bits, items_name_bits, missing);
    dec_ref_bits(_py, headers_bits);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing
        || !is_truthy(_py, obj_from_bits(molt_is_callable(items_method_bits)))
    {
        if items_method_bits != missing {
            dec_ref_bits(_py, items_method_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "headers must be a mapping",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let iter_bits = molt_iter(iterable_bits);
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        };
        let item_type = unsafe { object_type_id(item_ptr) };
        if item_type != TYPE_ID_LIST && item_type != TYPE_ID_TUPLE {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        out.push((
            crate::format_obj_str(_py, obj_from_bits(fields[0])),
            crate::format_obj_str(_py, obj_from_bits(fields[1])),
        ));
        dec_ref_bits(_py, item_bits);
    }
    Ok(out)
}

fn urllib_http_extract_method_and_body(
    _py: &crate::PyToken<'_>,
    request_bits: u64,
) -> Result<(String, Vec<u8>), u64> {
    let body = match urllib_request_attr_optional(_py, request_bits, b"data")? {
        Some(bits) if !obj_from_bits(bits).is_none() => {
            let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                dec_ref_bits(_py, bits);
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "Request data must be bytes-like",
                ));
            };
            let Some(bytes) = (unsafe { bytes_like_slice(ptr) }) else {
                dec_ref_bits(_py, bits);
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "Request data must be bytes-like",
                ));
            };
            let payload = bytes.to_vec();
            dec_ref_bits(_py, bits);
            payload
        }
        Some(bits) => {
            dec_ref_bits(_py, bits);
            Vec::new()
        }
        None => Vec::new(),
    };
    let method = match urllib_request_attr_optional(_py, request_bits, b"method")? {
        Some(bits) if !obj_from_bits(bits).is_none() => {
            let value = crate::format_obj_str(_py, obj_from_bits(bits));
            dec_ref_bits(_py, bits);
            value
        }
        Some(bits) => {
            dec_ref_bits(_py, bits);
            String::new()
        }
        None => String::new(),
    };
    let normalized = if method.trim().is_empty() {
        if body.is_empty() {
            "GET".to_string()
        } else {
            "POST".to_string()
        }
    } else {
        method
    };
    Ok((normalized, body))
}

fn urllib_http_find_proxy_for_scheme(
    _py: &crate::PyToken<'_>,
    opener_bits: u64,
    scheme: &str,
    host: &str,
) -> Result<Option<String>, u64> {
    let mut proxy: Option<String> = None;
    let mut saw_proxy_handler = false;
    let list_bits = urllib_request_ensure_handlers_list(_py, opener_bits)?;
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "opener handler registry is invalid",
        ));
    };
    let handlers: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    for handler_bits in handlers {
        let Some(proxies_bits) = urllib_request_attr_optional(_py, handler_bits, b"proxies")?
        else {
            continue;
        };
        saw_proxy_handler = true;
        if obj_from_bits(proxies_bits).is_none() {
            dec_ref_bits(_py, proxies_bits);
            continue;
        }
        let Some(get_name_bits) = attr_name_bits_from_bytes(_py, b"get") else {
            dec_ref_bits(_py, proxies_bits);
            return Err(MoltObject::none().bits());
        };
        let missing = missing_bits(_py);
        let get_method_bits = molt_getattr_builtin(proxies_bits, get_name_bits, missing);
        dec_ref_bits(_py, get_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, proxies_bits);
            return Err(MoltObject::none().bits());
        }
        if get_method_bits == missing
            || !is_truthy(_py, obj_from_bits(molt_is_callable(get_method_bits)))
        {
            if get_method_bits != missing {
                dec_ref_bits(_py, get_method_bits);
            }
            dec_ref_bits(_py, proxies_bits);
            continue;
        }
        let key_ptr = alloc_string(_py, scheme.as_bytes());
        if key_ptr.is_null() {
            dec_ref_bits(_py, get_method_bits);
            dec_ref_bits(_py, proxies_bits);
            return Err(MoltObject::none().bits());
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let out_bits = unsafe { call_callable1(_py, get_method_bits, key_bits) };
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, get_method_bits);
        dec_ref_bits(_py, proxies_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(out_bits).is_none() {
            proxy = Some(crate::format_obj_str(_py, obj_from_bits(out_bits)));
            dec_ref_bits(_py, out_bits);
            break;
        }
        dec_ref_bits(_py, out_bits);
    }
    if proxy.is_none() && !saw_proxy_handler {
        let env_key = format!("{}_proxy", scheme.to_ascii_lowercase());
        proxy = env_state_get(&env_key).or_else(|| env_state_get(&env_key.to_ascii_uppercase()));
    }
    let no_proxy = env_state_get("no_proxy").or_else(|| env_state_get("NO_PROXY"));
    if let (Some(rule), Some(_proxy_url)) = (no_proxy.as_deref(), proxy.as_ref())
        && urllib_http_host_matches_no_proxy(host, rule)
    {
        proxy = None;
    }
    Ok(proxy)
}

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

fn urllib_http_parse_basic_realm(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.to_ascii_lowercase().starts_with("basic") {
        return None;
    }
    let rest = trimmed.get(5..)?.trim();
    if rest.is_empty() {
        return None;
    }
    for part in rest.split(',') {
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("realm") {
            let raw = val.trim();
            if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
                return Some(raw[1..raw.len() - 1].to_string());
            }
            return Some(raw.to_string());
        }
    }
    None
}

fn urllib_proxy_find_basic_credentials(
    _py: &crate::PyToken<'_>,
    handlers: &[u64],
    proxy_url: &str,
    realm: Option<&str>,
) -> Result<Option<(String, String)>, u64> {
    for handler_bits in handlers {
        let Some(passwd_bits) = urllib_request_attr_optional(_py, *handler_bits, b"passwd")? else {
            continue;
        };
        if obj_from_bits(passwd_bits).is_none() {
            dec_ref_bits(_py, passwd_bits);
            continue;
        }
        let Some(find_name_bits) = attr_name_bits_from_bytes(_py, b"find_user_password") else {
            dec_ref_bits(_py, passwd_bits);
            return Err(MoltObject::none().bits());
        };
        let missing = missing_bits(_py);
        let find_bits = molt_getattr_builtin(passwd_bits, find_name_bits, missing);
        dec_ref_bits(_py, find_name_bits);
        dec_ref_bits(_py, passwd_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if find_bits == missing || !is_truthy(_py, obj_from_bits(molt_is_callable(find_bits))) {
            if find_bits != missing {
                dec_ref_bits(_py, find_bits);
            }
            continue;
        }
        let realm_bits = if let Some(realm_value) = realm {
            let realm_ptr = alloc_string(_py, realm_value.as_bytes());
            if realm_ptr.is_null() {
                dec_ref_bits(_py, find_bits);
                return Err(MoltObject::none().bits());
            }
            MoltObject::from_ptr(realm_ptr).bits()
        } else {
            MoltObject::none().bits()
        };
        let proxy_ptr = alloc_string(_py, proxy_url.as_bytes());
        if proxy_ptr.is_null() {
            if !obj_from_bits(realm_bits).is_none() {
                dec_ref_bits(_py, realm_bits);
            }
            dec_ref_bits(_py, find_bits);
            return Err(MoltObject::none().bits());
        }
        let proxy_bits = MoltObject::from_ptr(proxy_ptr).bits();
        let creds_bits = unsafe { call_callable2(_py, find_bits, realm_bits, proxy_bits) };
        dec_ref_bits(_py, proxy_bits);
        if !obj_from_bits(realm_bits).is_none() {
            dec_ref_bits(_py, realm_bits);
        }
        dec_ref_bits(_py, find_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(creds_bits).is_none() {
            continue;
        }
        let Some(creds_ptr) = obj_from_bits(creds_bits).as_ptr() else {
            dec_ref_bits(_py, creds_bits);
            continue;
        };
        let ty = unsafe { object_type_id(creds_ptr) };
        if ty != TYPE_ID_TUPLE && ty != TYPE_ID_LIST {
            dec_ref_bits(_py, creds_bits);
            continue;
        }
        let fields = unsafe { seq_vec_ref(creds_ptr) };
        if fields.len() != 2
            || obj_from_bits(fields[0]).is_none()
            || obj_from_bits(fields[1]).is_none()
        {
            dec_ref_bits(_py, creds_bits);
            continue;
        }
        let user = crate::format_obj_str(_py, obj_from_bits(fields[0]));
        let pass = crate::format_obj_str(_py, obj_from_bits(fields[1]));
        dec_ref_bits(_py, creds_bits);
        return Ok(Some((user, pass)));
    }
    Ok(None)
}

fn urllib_http_find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    for (key, value) in headers.iter().rev() {
        if key.eq_ignore_ascii_case(name) {
            return Some(value.as_str());
        }
    }
    None
}

type HttpResponseParts = (i64, String, Vec<(String, String)>, Vec<u8>);

fn urllib_http_parse_response_bytes(raw: &[u8]) -> Result<HttpResponseParts, String> {
    let marker = b"\r\n\r\n";
    let Some(split) = raw.windows(marker.len()).position(|w| w == marker) else {
        return Err("Malformed HTTP response".to_string());
    };
    let head = &raw[..split];
    let mut body = raw[split + marker.len()..].to_vec();
    let head_text = String::from_utf8_lossy(head);
    let mut lines = head_text.split("\r\n");
    let Some(status_line) = lines.next() else {
        return Err("Malformed HTTP response".to_string());
    };
    let mut parts = status_line.splitn(3, ' ');
    let _http_version = parts.next().unwrap_or("");
    let code = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| "Malformed HTTP status line".to_string())?;
    let reason = parts.next().unwrap_or("").to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    if let Some(length) =
        urllib_http_find_header(&headers, "Content-Length").and_then(|v| v.parse::<usize>().ok())
    {
        if body.len() < length {
            return Err("Incomplete HTTP response body".to_string());
        }
        if body.len() > length {
            body.truncate(length);
        }
    }
    Ok((code, reason, headers, body))
}

fn http_parse_header_pairs(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_value = String::new();

    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if current_name.is_some() {
                if !current_value.is_empty() {
                    current_value.push(' ');
                }
                current_value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if let Some(prev_name) = current_name.take() {
                out.push((prev_name, current_value.trim().to_string()));
            }
            current_name = Some(name.trim().to_string());
            current_value.clear();
            current_value.push_str(value.trim_start());
        }
    }

    if let Some(last_name) = current_name {
        out.push((last_name, current_value.trim().to_string()));
    }
    out
}

fn urllib_http_build_request_bytes(
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Vec<u8> {
    let mut request = Vec::<u8>::new();
    request.extend_from_slice(format!("{} {} HTTP/1.1\r\n", req.method, request_target).as_bytes());
    let mut has_host = false;
    let mut has_connection = false;
    let mut has_content_length = false;
    for (name, value) in &req.headers {
        if name.eq_ignore_ascii_case("Host") {
            has_host = true;
        }
        if name.eq_ignore_ascii_case("Connection") {
            has_connection = true;
        }
        if name.eq_ignore_ascii_case("Content-Length") {
            has_content_length = true;
        }
        request.extend_from_slice(name.as_bytes());
        request.extend_from_slice(b": ");
        request.extend_from_slice(value.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    if !has_host {
        request.extend_from_slice(b"Host: ");
        request.extend_from_slice(host_header.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    if !has_connection {
        request.extend_from_slice(b"Connection: close\r\n");
    }
    if !req.body.is_empty() && !has_content_length {
        request.extend_from_slice(format!("Content-Length: {}\r\n", req.body.len()).as_bytes());
    }
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(&req.body);
    request
}

fn urllib_http_try_inmemory_dispatch(
    _py: &crate::PyToken<'_>,
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Result<Option<HttpResponseParts>, u64> {
    let module_name_ptr = alloc_string(_py, b"socketserver");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        let kind = urllib_request_pending_exception_kind_name(_py).unwrap_or_default();
        if kind == "ImportError" || kind == "TypeError" {
            clear_exception(_py);
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        if !obj_from_bits(module_bits).is_none() {
            dec_ref_bits(_py, module_bits);
        }
        return Ok(None);
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        dec_ref_bits(_py, module_bits);
        return Ok(None);
    }

    let Some(lookup_name_bits) = attr_name_bits_from_bytes(_py, b"_lookup_server") else {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let lookup_bits = molt_getattr_builtin(module_bits, lookup_name_bits, missing);
    dec_ref_bits(_py, lookup_name_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    }
    if lookup_bits == missing {
        dec_ref_bits(_py, module_bits);
        return Ok(None);
    }

    let host_ptr = alloc_string(_py, req.host.as_bytes());
    if host_ptr.is_null() {
        dec_ref_bits(_py, lookup_bits);
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    }
    let host_bits = MoltObject::from_ptr(host_ptr).bits();
    let port_bits = MoltObject::from_int(req.port as i64).bits();
    let server_bits = unsafe { call_callable2(_py, lookup_bits, host_bits, port_bits) };
    dec_ref_bits(_py, host_bits);
    dec_ref_bits(_py, lookup_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(server_bits).is_none() {
        return Ok(None);
    }

    let Some(dispatch_bits) = (match urllib_request_attr_optional(_py, server_bits, b"_dispatch") {
        Ok(value) => value,
        Err(bits) => {
            dec_ref_bits(_py, server_bits);
            return Err(bits);
        }
    }) else {
        dec_ref_bits(_py, server_bits);
        return Ok(None);
    };
    if !is_truthy(_py, obj_from_bits(molt_is_callable(dispatch_bits))) {
        dec_ref_bits(_py, dispatch_bits);
        dec_ref_bits(_py, server_bits);
        return Ok(None);
    }

    let request = urllib_http_build_request_bytes(req, request_target, host_header);
    let request_ptr = crate::alloc_bytes(_py, &request);
    if request_ptr.is_null() {
        dec_ref_bits(_py, dispatch_bits);
        dec_ref_bits(_py, server_bits);
        return Err(MoltObject::none().bits());
    }
    let request_bits = MoltObject::from_ptr(request_ptr).bits();
    let timeout_bits = MoltObject::from_float(req.timeout.unwrap_or(5.0)).bits();
    let response_bits = unsafe { call_callable2(_py, dispatch_bits, request_bits, timeout_bits) };
    dec_ref_bits(_py, request_bits);
    dec_ref_bits(_py, dispatch_bits);
    dec_ref_bits(_py, server_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }

    let Some(response_ptr) = obj_from_bits(response_bits).as_ptr() else {
        if !obj_from_bits(response_bits).is_none() {
            dec_ref_bits(_py, response_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "socketserver dispatch must return bytes-like payload",
        ));
    };
    let Some(raw_bytes) = (unsafe { bytes_like_slice(response_ptr) }) else {
        dec_ref_bits(_py, response_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "socketserver dispatch must return bytes-like payload",
        ));
    };
    let raw = raw_bytes.to_vec();
    dec_ref_bits(_py, response_bits);
    match urllib_http_parse_response_bytes(&raw) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(msg) => Err(raise_exception::<u64>(_py, "ValueError", &msg)),
    }
}

fn urllib_http_send_request(
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Result<HttpResponseParts, std::io::Error> {
    let request = urllib_http_build_request_bytes(req, request_target, host_header);
    let mut raw = Vec::new();
    {
        let _release = crate::concurrency::GilReleaseGuard::new();
        let mut stream = TcpStream::connect((req.host.as_str(), req.port))?;
        if let Some(timeout) = req.timeout {
            let timeout = Duration::from_secs_f64(timeout);
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
        }
        stream.write_all(&request)?;
        if let Err(err) = stream.read_to_end(&mut raw) {
            if (err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock)
                && !raw.is_empty()
                && let Ok(parsed) = urllib_http_parse_response_bytes(&raw)
            {
                return Ok(parsed);
            }
            return Err(err);
        }
    }
    match urllib_http_parse_response_bytes(&raw) {
        Ok(parsed) => Ok(parsed),
        Err(msg) => Err(std::io::Error::new(ErrorKind::InvalidData, msg)),
    }
}

fn urllib_http_make_response_bits(_py: &crate::PyToken<'_>, handle: i64) -> u64 {
    let marker_ptr = alloc_string(_py, b"__molt_urllib_response__");
    if marker_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let marker_bits = MoltObject::from_ptr(marker_ptr).bits();
    let handle_bits = MoltObject::from_int(handle).bits();
    let tuple_ptr = alloc_tuple(_py, &[marker_bits, handle_bits]);
    dec_ref_bits(_py, marker_bits);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn urllib_request_response_handle_from_bits(
    _py: &crate::PyToken<'_>,
    response_bits: u64,
) -> Result<i64, u64> {
    let Some(ptr) = obj_from_bits(response_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    };
    let ty = unsafe { object_type_id(ptr) };
    if ty != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if fields.len() != 2 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    }
    let Some(tag) = string_obj_to_owned(obj_from_bits(fields[0])) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    };
    if tag != "__molt_urllib_response__" {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    }
    let Some(handle) = to_i64(obj_from_bits(fields[1])) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object handle is invalid",
        ));
    };
    Ok(handle)
}

fn urllib_error_class_bits(_py: &crate::PyToken<'_>, class_name: &[u8]) -> Result<u64, u64> {
    let module_name_ptr = alloc_string(_py, b"urllib.error");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, class_name) else {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let class_bits = molt_getattr_builtin(module_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if class_bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "urllib.error class is unavailable",
        ));
    }
    Ok(class_bits)
}

fn urllib_raise_url_error(_py: &crate::PyToken<'_>, reason: &str) -> u64 {
    let class_bits = match urllib_error_class_bits(_py, b"URLError") {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        dec_ref_bits(_py, class_bits);
        return raise_exception::<u64>(_py, "TypeError", "URLError class is invalid");
    };
    let reason_ptr = alloc_string(_py, reason.as_bytes());
    if reason_ptr.is_null() {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
    let exc_bits = unsafe { call_class_init_with_args(_py, class_ptr, &[reason_bits]) };
    dec_ref_bits(_py, reason_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::molt_raise(exc_bits)
}

fn urllib_raise_http_error(
    _py: &crate::PyToken<'_>,
    url: &str,
    code: i64,
    reason: &str,
    headers: &[(String, String)],
    fp_bits: u64,
) -> u64 {
    let class_bits = match urllib_error_class_bits(_py, b"HTTPError") {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        dec_ref_bits(_py, class_bits);
        return raise_exception::<u64>(_py, "TypeError", "HTTPError class is invalid");
    };
    let url_ptr = alloc_string(_py, url.as_bytes());
    if url_ptr.is_null() {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let reason_ptr = alloc_string(_py, reason.as_bytes());
    if reason_ptr.is_null() {
        let url_bits = MoltObject::from_ptr(url_ptr).bits();
        dec_ref_bits(_py, url_bits);
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let url_bits = MoltObject::from_ptr(url_ptr).bits();
    let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
    let code_bits = MoltObject::from_int(code).bits();
    let headers_bits = match urllib_http_headers_to_dict(_py, headers) {
        Ok(bits) => bits,
        Err(bits) => {
            dec_ref_bits(_py, url_bits);
            dec_ref_bits(_py, reason_bits);
            dec_ref_bits(_py, class_bits);
            return bits;
        }
    };
    let exc_bits = unsafe {
        call_class_init_with_args(
            _py,
            class_ptr,
            &[url_bits, code_bits, reason_bits, headers_bits, fp_bits],
        )
    };
    dec_ref_bits(_py, headers_bits);
    dec_ref_bits(_py, reason_bits);
    dec_ref_bits(_py, url_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::molt_raise(exc_bits)
}

#[derive(Clone)]
struct TextWrapOptions {
    width: i64,
    initial_indent: String,
    subsequent_indent: String,
    expand_tabs: bool,
    replace_whitespace: bool,
    fix_sentence_endings: bool,
    break_long_words: bool,
    drop_whitespace: bool,
    break_on_hyphens: bool,
    tabsize: i64,
    max_lines: Option<i64>,
    placeholder: String,
}

fn textwrap_default_options(width: i64) -> TextWrapOptions {
    TextWrapOptions {
        width,
        initial_indent: String::new(),
        subsequent_indent: String::new(),
        expand_tabs: true,
        replace_whitespace: true,
        fix_sentence_endings: false,
        break_long_words: true,
        drop_whitespace: true,
        break_on_hyphens: true,
        tabsize: 8,
        max_lines: None,
        placeholder: " [...]".to_string(),
    }
}

#[inline]
fn textwrap_char_len(value: &str) -> i64 {
    value.chars().count() as i64
}

#[inline]
fn textwrap_is_ascii_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

#[inline]
fn textwrap_is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[inline]
fn textwrap_is_word_punct(ch: char) -> bool {
    textwrap_is_word_char(ch) || matches!(ch, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

#[inline]
fn textwrap_is_letter(ch: char) -> bool {
    ch.is_alphabetic()
}

#[inline]
fn textwrap_chunk_is_whitespace(chunk: &str) -> bool {
    chunk.chars().all(char::is_whitespace)
}

#[inline]
fn textwrap_normalize_index(len: usize, idx: i64) -> usize {
    let len_i64 = len as i64;
    let mut normalized = if idx < 0 {
        len_i64.saturating_add(idx)
    } else {
        idx
    };
    if normalized < 0 {
        normalized = 0;
    }
    if normalized > len_i64 {
        normalized = len_i64;
    }
    normalized as usize
}

fn textwrap_slice_prefix(value: &str, end: i64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let end = textwrap_normalize_index(chars.len(), end);
    chars[..end].iter().collect()
}

fn textwrap_slice_suffix(value: &str, start: i64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let start = textwrap_normalize_index(chars.len(), start);
    chars[start..].iter().collect()
}

fn textwrap_rfind_before(value: &str, needle: char, stop: i64) -> Option<usize> {
    let chars: Vec<char> = value.chars().collect();
    let stop = textwrap_normalize_index(chars.len(), stop);
    chars[..stop].iter().rposition(|ch| *ch == needle)
}

fn textwrap_expand_tabs(text: &str, tabsize: i64) -> String {
    let tabsize = tabsize.max(0) as usize;
    let mut out = String::with_capacity(text.len());
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\t' {
            if tabsize == 0 {
                continue;
            }
            let spaces = tabsize - (col % tabsize);
            out.extend(std::iter::repeat_n(' ', spaces));
            col = col.saturating_add(spaces);
            continue;
        }
        out.push(ch);
        if matches!(ch, '\n' | '\r') {
            col = 0;
        } else {
            col = col.saturating_add(1);
        }
    }
    out
}

fn textwrap_munge_whitespace(text: &str, options: &TextWrapOptions) -> String {
    let mut out = if options.expand_tabs {
        textwrap_expand_tabs(text, options.tabsize)
    } else {
        text.to_string()
    };
    if options.replace_whitespace {
        out = out
            .chars()
            .map(|ch| {
                if textwrap_is_ascii_whitespace(ch) {
                    ' '
                } else {
                    ch
                }
            })
            .collect();
    }
    out
}

fn textwrap_split_simple(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        let is_ws = textwrap_is_ascii_whitespace(chars[idx]);
        let start = idx;
        idx += 1;
        while idx < chars.len() && textwrap_is_ascii_whitespace(chars[idx]) == is_ws {
            idx += 1;
        }
        chunks.push(chars[start..idx].iter().collect());
    }
    chunks
}

fn textwrap_should_split_hyphen(chars: &[char], idx: usize) -> bool {
    if chars.get(idx).copied() != Some('-') {
        return false;
    }
    let left_ok =
        (idx >= 2 && textwrap_is_letter(chars[idx - 2]) && textwrap_is_letter(chars[idx - 1]))
            || (idx >= 3
                && textwrap_is_letter(chars[idx - 3])
                && chars[idx - 2] == '-'
                && textwrap_is_letter(chars[idx - 1]));
    if !left_ok {
        return false;
    }
    (idx + 2 < chars.len()
        && textwrap_is_letter(chars[idx + 1])
        && textwrap_is_letter(chars[idx + 2]))
        || (idx + 3 < chars.len()
            && textwrap_is_letter(chars[idx + 1])
            && chars[idx + 2] == '-'
            && textwrap_is_letter(chars[idx + 3]))
}

fn textwrap_hyphen_run(chars: &[char], idx: usize) -> usize {
    let mut run = 0usize;
    while idx + run < chars.len() && chars[idx + run] == '-' {
        run += 1;
    }
    run
}

fn textwrap_split_hyphenated_token(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    while idx < chars.len() {
        let dash_run = textwrap_hyphen_run(&chars, idx);
        if dash_run >= 2
            && idx > 0
            && idx + dash_run < chars.len()
            && textwrap_is_word_punct(chars[idx - 1])
            && textwrap_is_word_char(chars[idx + dash_run])
        {
            if start < idx {
                out.push(chars[start..idx].iter().collect());
            }
            out.push(chars[idx..idx + dash_run].iter().collect());
            idx += dash_run;
            start = idx;
            continue;
        }
        if textwrap_should_split_hyphen(&chars, idx) {
            idx += 1;
            if start < idx {
                out.push(chars[start..idx].iter().collect());
            }
            start = idx;
            continue;
        }
        idx += 1;
    }
    if start < chars.len() {
        out.push(chars[start..].iter().collect());
    }
    out
}

fn textwrap_split_chunks(text: &str, break_on_hyphens: bool) -> Vec<String> {
    if !break_on_hyphens {
        return textwrap_split_simple(text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        if textwrap_is_ascii_whitespace(chars[idx]) {
            let start = idx;
            idx += 1;
            while idx < chars.len() && textwrap_is_ascii_whitespace(chars[idx]) {
                idx += 1;
            }
            chunks.push(chars[start..idx].iter().collect());
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < chars.len() && !textwrap_is_ascii_whitespace(chars[idx]) {
            idx += 1;
        }
        let token: String = chars[start..idx].iter().collect();
        chunks.extend(textwrap_split_hyphenated_token(&token));
    }
    chunks
}

fn textwrap_chunk_has_sentence_end(chunk: &str) -> bool {
    let chars: Vec<char> = chunk.chars().collect();
    if chars.len() < 2 {
        return false;
    }
    let mut idx = chars.len();
    if matches!(chars[idx - 1], '"' | '\'') {
        idx -= 1;
        if idx < 2 {
            return false;
        }
    }
    matches!(chars[idx - 1], '.' | '!' | '?') && chars[idx - 2].is_ascii_lowercase()
}

fn textwrap_fix_sentence_endings(chunks: &mut [String]) {
    let mut idx = 0usize;
    while idx + 1 < chunks.len() {
        if chunks[idx + 1] == " " && textwrap_chunk_has_sentence_end(&chunks[idx]) {
            chunks[idx + 1] = "  ".to_string();
            idx += 2;
        } else {
            idx += 1;
        }
    }
}

fn textwrap_handle_long_word(
    chunks: &mut Vec<String>,
    cur_line: &mut Vec<String>,
    cur_len: i64,
    width: i64,
    break_long_words: bool,
    break_on_hyphens: bool,
) {
    let space_left = if width < 1 { 1 } else { width - cur_len };
    if break_long_words {
        let mut end = space_left;
        if let Some(chunk) = chunks.last_mut() {
            if break_on_hyphens
                && textwrap_char_len(chunk) > space_left
                && let Some(hyphen) = textwrap_rfind_before(chunk, '-', space_left)
                && hyphen > 0
                && chunk.chars().take(hyphen).any(|ch| ch != '-')
            {
                end = hyphen as i64 + 1;
            }
            let left = textwrap_slice_prefix(chunk, end);
            let right = textwrap_slice_suffix(chunk, end);
            cur_line.push(left);
            *chunk = right;
        }
    } else if cur_line.is_empty()
        && let Some(chunk) = chunks.pop()
    {
        cur_line.push(chunk);
    }
}

fn textwrap_wrap_chunks(
    mut chunks: Vec<String>,
    options: &TextWrapOptions,
) -> Result<Vec<String>, String> {
    if options.width <= 0 {
        return Err(format!("invalid width {:?} (must be > 0)", options.width));
    }
    if let Some(max_lines) = options.max_lines {
        let indent = if max_lines > 1 {
            &options.subsequent_indent
        } else {
            &options.initial_indent
        };
        let placeholder_lstrip = options.placeholder.trim_start_matches(char::is_whitespace);
        if textwrap_char_len(indent) + textwrap_char_len(placeholder_lstrip) > options.width {
            return Err("placeholder too large for max width".to_string());
        }
    }

    let mut lines: Vec<String> = Vec::new();
    chunks.reverse();

    while !chunks.is_empty() {
        let mut cur_line: Vec<String> = Vec::new();
        let mut cur_len = 0i64;
        let indent = if lines.is_empty() {
            &options.initial_indent
        } else {
            &options.subsequent_indent
        };
        let width = options.width - textwrap_char_len(indent);

        if options.drop_whitespace
            && !chunks.is_empty()
            && !lines.is_empty()
            && chunks
                .last()
                .map(|chunk| textwrap_chunk_is_whitespace(chunk))
                .unwrap_or(false)
        {
            chunks.pop();
        }

        while let Some(last) = chunks.last() {
            let last_len = textwrap_char_len(last);
            if cur_len + last_len <= width {
                cur_len += last_len;
                if let Some(chunk) = chunks.pop() {
                    cur_line.push(chunk);
                }
            } else {
                break;
            }
        }

        if !chunks.is_empty()
            && chunks
                .last()
                .map(|chunk| textwrap_char_len(chunk) > width)
                .unwrap_or(false)
        {
            textwrap_handle_long_word(
                &mut chunks,
                &mut cur_line,
                cur_len,
                width,
                options.break_long_words,
                options.break_on_hyphens,
            );
            cur_len = cur_line.iter().map(|chunk| textwrap_char_len(chunk)).sum();
        }

        if options.drop_whitespace
            && !cur_line.is_empty()
            && cur_line
                .last()
                .map(|chunk| textwrap_chunk_is_whitespace(chunk))
                .unwrap_or(false)
            && let Some(last) = cur_line.pop()
        {
            cur_len -= textwrap_char_len(&last);
        }

        if cur_line.is_empty() {
            continue;
        }

        let allow_full_line = if let Some(max_lines) = options.max_lines {
            (lines.len() as i64 + 1) < max_lines
                || ((chunks.is_empty()
                    || (options.drop_whitespace
                        && chunks.len() == 1
                        && textwrap_chunk_is_whitespace(&chunks[0])))
                    && cur_len <= width)
        } else {
            true
        };

        if allow_full_line {
            lines.push(format!("{indent}{}", cur_line.concat()));
            continue;
        }

        let placeholder_len = textwrap_char_len(&options.placeholder);
        loop {
            let can_append_placeholder = cur_line
                .last()
                .map(|last| {
                    !textwrap_chunk_is_whitespace(last) && cur_len + placeholder_len <= width
                })
                .unwrap_or(false);
            if can_append_placeholder {
                cur_line.push(options.placeholder.clone());
                lines.push(format!("{indent}{}", cur_line.concat()));
                break;
            }
            if let Some(last) = cur_line.pop() {
                cur_len -= textwrap_char_len(&last);
                continue;
            }
            if let Some(prev_line) = lines.last_mut() {
                let trimmed = prev_line.trim_end_matches(char::is_whitespace).to_string();
                if textwrap_char_len(&trimmed) + placeholder_len <= options.width {
                    *prev_line = trimmed + &options.placeholder;
                    return Ok(lines);
                }
            }
            let placeholder_lstrip = options.placeholder.trim_start_matches(char::is_whitespace);
            lines.push(format!("{indent}{placeholder_lstrip}"));
            break;
        }
        break;
    }

    Ok(lines)
}

fn textwrap_wrap_impl(text: &str, options: &TextWrapOptions) -> Result<Vec<String>, String> {
    let munged = textwrap_munge_whitespace(text, options);
    let mut chunks = textwrap_split_chunks(&munged, options.break_on_hyphens);
    if options.fix_sentence_endings {
        textwrap_fix_sentence_endings(&mut chunks);
    }
    textwrap_wrap_chunks(chunks, options)
}

fn textwrap_line_is_space(line: &str) -> bool {
    !line.is_empty() && line.chars().all(char::is_whitespace)
}

fn textwrap_splitlines_keepends(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut line_start = 0usize;
    let mut iter = text.char_indices().peekable();
    while let Some((idx, ch)) = iter.next() {
        let mut end = idx + ch.len_utf8();
        let is_break = match ch {
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => true,
            '\r' => {
                if let Some((next_idx, next_ch)) = iter.peek().copied()
                    && next_ch == '\n'
                {
                    end = next_idx + next_ch.len_utf8();
                    iter.next();
                }
                true
            }
            _ => false,
        };
        if is_break {
            out.push(text[line_start..end].to_string());
            line_start = end;
        }
    }
    if line_start < text.len() {
        out.push(text[line_start..].to_string());
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn textwrap_parse_options_ex(
    _py: &crate::PyToken<'_>,
    width_bits: u64,
    initial_indent_bits: u64,
    subsequent_indent_bits: u64,
    expand_tabs_bits: u64,
    replace_whitespace_bits: u64,
    fix_sentence_endings_bits: u64,
    break_long_words_bits: u64,
    drop_whitespace_bits: u64,
    break_on_hyphens_bits: u64,
    tabsize_bits: u64,
    max_lines_placeholder_bits: u64,
) -> Result<TextWrapOptions, u64> {
    let Some(width) = to_i64(obj_from_bits(width_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "width must be int",
        ));
    };
    let Some(initial_indent) = string_obj_to_owned(obj_from_bits(initial_indent_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "initial_indent must be str",
        ));
    };
    let Some(subsequent_indent) = string_obj_to_owned(obj_from_bits(subsequent_indent_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "subsequent_indent must be str",
        ));
    };
    let Some(tabsize) = to_i64(obj_from_bits(tabsize_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "tabsize must be int",
        ));
    };
    let Some(max_lines_placeholder_ptr) = obj_from_bits(max_lines_placeholder_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    };
    if unsafe { object_type_id(max_lines_placeholder_ptr) } != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    }
    let max_lines_placeholder = unsafe { seq_vec_ref(max_lines_placeholder_ptr) };
    if max_lines_placeholder.len() != 2 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "max_lines_placeholder must be tuple(max_lines, placeholder)",
        ));
    }
    let max_lines_bits = max_lines_placeholder[0];
    let placeholder_bits = max_lines_placeholder[1];

    let max_lines = if obj_from_bits(max_lines_bits).is_none() {
        None
    } else {
        let Some(value) = to_i64(obj_from_bits(max_lines_bits)) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "max_lines must be int or None",
            ));
        };
        Some(value)
    };
    let Some(placeholder) = string_obj_to_owned(obj_from_bits(placeholder_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "placeholder must be str",
        ));
    };
    Ok(TextWrapOptions {
        width,
        initial_indent,
        subsequent_indent,
        expand_tabs: is_truthy(_py, obj_from_bits(expand_tabs_bits)),
        replace_whitespace: is_truthy(_py, obj_from_bits(replace_whitespace_bits)),
        fix_sentence_endings: is_truthy(_py, obj_from_bits(fix_sentence_endings_bits)),
        break_long_words: is_truthy(_py, obj_from_bits(break_long_words_bits)),
        drop_whitespace: is_truthy(_py, obj_from_bits(drop_whitespace_bits)),
        break_on_hyphens: is_truthy(_py, obj_from_bits(break_on_hyphens_bits)),
        tabsize,
        max_lines,
        placeholder,
    })
}

fn textwrap_indent_with_predicate(
    _py: &crate::PyToken<'_>,
    text: &str,
    prefix: &str,
    predicate_bits: Option<u64>,
) -> u64 {
    let mut out = String::with_capacity(text.len().saturating_add(prefix.len() * 4));
    for line in textwrap_splitlines_keepends(text) {
        let should_prefix = if let Some(predicate) = predicate_bits {
            let Some(line_bits) = alloc_string_bits(_py, &line) else {
                return MoltObject::none().bits();
            };
            let result_bits = unsafe { call_callable1(_py, predicate, line_bits) };
            dec_ref_bits(_py, line_bits);
            if exception_pending(_py) {
                if !obj_from_bits(result_bits).is_none() {
                    dec_ref_bits(_py, result_bits);
                }
                return MoltObject::none().bits();
            }
            let truthy = is_truthy(_py, obj_from_bits(result_bits));
            if !obj_from_bits(result_bits).is_none() {
                dec_ref_bits(_py, result_bits);
            }
            truthy
        } else {
            !textwrap_line_is_space(&line)
        };
        if should_prefix {
            out.push_str(prefix);
        }
        out.push_str(&line);
    }
    let out_ptr = alloc_string(_py, out.as_bytes());
    if out_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(out_ptr).bits()
    }
}

// ─── textwrap.dedent ────────────────────────────────────────────────────────

fn textwrap_dedent_impl(text: &str) -> String {
    // CPython textwrap.dedent: remove common leading whitespace from all lines.
    let mut margin: Option<&str> = None;
    let lines: Vec<&str> = text.split('\n').collect();
    for &line in &lines {
        let stripped = line.trim_start();
        if stripped.is_empty() {
            continue;
        }
        let indent = &line[..line.len() - stripped.len()];
        if let Some(m) = margin {
            // Find common prefix between margin and indent
            let common_len = m
                .chars()
                .zip(indent.chars())
                .take_while(|(a, b)| a == b)
                .count();
            // Need byte length of common prefix
            let byte_len = m
                .char_indices()
                .nth(common_len)
                .map(|(i, _)| i)
                .unwrap_or(m.len());
            margin = Some(&m[..byte_len]);
        } else {
            margin = Some(indent);
        }
    }
    let margin = margin.unwrap_or("");
    if margin.is_empty() {
        return text.to_string();
    }
    let margin_len = margin.len();
    let mut result = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.trim_start().is_empty() {
            // Whitespace-only line: strip all leading whitespace
            result.push_str(line.trim_start());
        } else if line.len() >= margin_len && &line[..margin_len] == margin {
            result.push_str(&line[margin_len..]);
        } else {
            result.push_str(line);
        }
    }
    result
}
#[derive(Clone)]
struct PkgutilModuleInfo {
    module_finder: String,
    name: String,
    ispkg: bool,
}

fn pkgutil_join(base: &str, name: &str) -> String {
    if base.is_empty() {
        return name.to_string();
    }
    Path::new(base).join(name).to_string_lossy().into_owned()
}

fn pkgutil_iter_modules_in_path(path: &str, prefix: &str) -> Vec<PkgutilModuleInfo> {
    let entries = match fs::read_dir(path) {
        Ok(read_dir) => read_dir,
        Err(_) => return Vec::new(),
    };

    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut yielded: HashSet<String> = HashSet::new();
    let mut results: Vec<PkgutilModuleInfo> = Vec::new();
    for entry in names {
        if entry == "__pycache__" {
            continue;
        }
        let full = pkgutil_join(path, &entry);
        if !entry.contains('.') {
            if let Ok(dir_entries) = fs::read_dir(&full) {
                let mut ispkg = false;
                for item in dir_entries.flatten() {
                    if item.file_name().to_string_lossy() == "__init__.py" {
                        ispkg = true;
                        break;
                    }
                }
                if ispkg && yielded.insert(entry.clone()) {
                    results.push(PkgutilModuleInfo {
                        module_finder: path.to_string(),
                        name: format!("{prefix}{entry}"),
                        ispkg: true,
                    });
                }
            }
            continue;
        }
        if !entry.ends_with(".py") {
            continue;
        }
        let modname = &entry[..entry.len().saturating_sub(3)];
        if modname.is_empty() || modname == "__init__" || modname.contains('.') {
            continue;
        }
        if yielded.insert(modname.to_string()) {
            results.push(PkgutilModuleInfo {
                module_finder: path.to_string(),
                name: format!("{prefix}{modname}"),
                ispkg: false,
            });
        }
    }
    results
}

fn pkgutil_iter_modules_impl(paths: &[String], prefix: &str) -> Vec<PkgutilModuleInfo> {
    let mut yielded: HashSet<String> = HashSet::new();
    let mut out: Vec<PkgutilModuleInfo> = Vec::new();
    for path in paths {
        for info in pkgutil_iter_modules_in_path(path, prefix) {
            if yielded.insert(info.name.clone()) {
                out.push(info);
            }
        }
    }
    out
}

fn pkgutil_walk_packages_impl(paths: &[String], prefix: &str) -> Vec<PkgutilModuleInfo> {
    let mut out: Vec<PkgutilModuleInfo> = Vec::new();
    let infos = pkgutil_iter_modules_impl(paths, prefix);
    for info in infos {
        out.push(info.clone());
        if !info.ispkg {
            continue;
        }
        let mut pkg_name = info.name.clone();
        if !prefix.is_empty() && pkg_name.starts_with(prefix) {
            pkg_name = pkg_name[prefix.len()..].to_string();
        }
        let subdir = pkgutil_join(&info.module_finder, &pkg_name);
        let subprefix = format!("{}.", info.name);
        let nested = pkgutil_walk_packages_impl(&[subdir], &subprefix);
        out.extend(nested);
    }
    out
}

fn alloc_pkgutil_module_info_list(_py: &crate::PyToken<'_>, values: &[PkgutilModuleInfo]) -> u64 {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(values.len());
    for entry in values {
        let finder_ptr = alloc_string(_py, entry.module_finder.as_bytes());
        if finder_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let name_ptr = alloc_string(_py, entry.name.as_bytes());
        if name_ptr.is_null() {
            let finder_bits = MoltObject::from_ptr(finder_ptr).bits();
            dec_ref_bits(_py, finder_bits);
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let finder_bits = MoltObject::from_ptr(finder_ptr).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let ispkg_bits = MoltObject::from_bool(entry.ispkg).bits();
        let tuple_ptr = alloc_tuple(_py, &[finder_bits, name_bits, ispkg_bits]);
        dec_ref_bits(_py, finder_bits);
        dec_ref_bits(_py, name_bits);
        if tuple_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, tuple_bits.as_slice(), tuple_bits.len());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn compileall_compile_file_impl(fullname: &str) -> bool {
    let mut handle = match fs::File::open(fullname) {
        Ok(handle) => handle,
        Err(_) => return false,
    };
    let mut one = [0u8; 1];
    handle.read(&mut one).is_ok()
}

fn compileall_compile_dir_impl(dir: &str, maxlevels: i64) -> bool {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut success = true;
    for entry in names {
        if entry == "__pycache__" {
            continue;
        }
        let full = pkgutil_join(dir, &entry);
        if entry.ends_with(".py") {
            if !compileall_compile_file_impl(&full) {
                success = false;
            }
            continue;
        }
        if maxlevels <= 0 {
            continue;
        }
        if fs::read_dir(&full).is_err() {
            continue;
        }
        if !compileall_compile_dir_impl(&full, maxlevels - 1) {
            success = false;
        }
    }
    success
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

    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
fn opcode_num_popped_312(opcode: i64, oparg: i64) -> Option<i64> {
    match opcode {
        0 => Some(0),                 // CACHE
        1 => Some(1),                 // POP_TOP
        2 => Some(0),                 // PUSH_NULL
        3 => Some(1),                 // INTERPRETER_EXIT
        4 => Some(1 + 1),             // END_FOR
        5 => Some(2),                 // END_SEND
        9 => Some(0),                 // NOP
        11 => Some(1),                // UNARY_NEGATIVE
        12 => Some(1),                // UNARY_NOT
        15 => Some(1),                // UNARY_INVERT
        17 => Some(0),                // RESERVED
        25 => Some(2),                // BINARY_SUBSCR
        26 => Some(3),                // BINARY_SLICE
        27 => Some(4),                // STORE_SLICE
        30 => Some(1),                // GET_LEN
        31 => Some(1),                // MATCH_MAPPING
        32 => Some(1),                // MATCH_SEQUENCE
        33 => Some(2),                // MATCH_KEYS
        35 => Some(1),                // PUSH_EXC_INFO
        36 => Some(2),                // CHECK_EXC_MATCH
        37 => Some(2),                // CHECK_EG_MATCH
        49 => Some(4),                // WITH_EXCEPT_START
        50 => Some(1),                // GET_AITER
        51 => Some(1),                // GET_ANEXT
        52 => Some(1),                // BEFORE_ASYNC_WITH
        53 => Some(1),                // BEFORE_WITH
        54 => Some(2),                // END_ASYNC_FOR
        55 => Some(3),                // CLEANUP_THROW
        60 => Some(3),                // STORE_SUBSCR
        61 => Some(2),                // DELETE_SUBSCR
        68 => Some(1),                // GET_ITER
        69 => Some(1),                // GET_YIELD_FROM_ITER
        71 => Some(0),                // LOAD_BUILD_CLASS
        74 => Some(0),                // LOAD_ASSERTION_ERROR
        75 => Some(0),                // RETURN_GENERATOR
        83 => Some(1),                // RETURN_VALUE
        85 => Some(0),                // SETUP_ANNOTATIONS
        87 => Some(0),                // LOAD_LOCALS
        89 => Some(1),                // POP_EXCEPT
        90 => Some(1),                // STORE_NAME
        91 => Some(0),                // DELETE_NAME
        92 => Some(1),                // UNPACK_SEQUENCE
        93 => Some(1),                // FOR_ITER
        94 => Some(1),                // UNPACK_EX
        95 => Some(2),                // STORE_ATTR
        96 => Some(1),                // DELETE_ATTR
        97 => Some(1),                // STORE_GLOBAL
        98 => Some(0),                // DELETE_GLOBAL
        99 => Some((oparg - 2) + 2),  // SWAP
        100 => Some(0),               // LOAD_CONST
        101 => Some(0),               // LOAD_NAME
        102 => Some(oparg),           // BUILD_TUPLE
        103 => Some(oparg),           // BUILD_LIST
        104 => Some(oparg),           // BUILD_SET
        105 => Some(oparg * 2),       // BUILD_MAP
        106 => Some(1),               // LOAD_ATTR
        107 => Some(2),               // COMPARE_OP
        108 => Some(2),               // IMPORT_NAME
        109 => Some(1),               // IMPORT_FROM
        110 => Some(0),               // JUMP_FORWARD
        114 => Some(1),               // POP_JUMP_IF_FALSE
        115 => Some(1),               // POP_JUMP_IF_TRUE
        116 => Some(0),               // LOAD_GLOBAL
        117 => Some(2),               // IS_OP
        118 => Some(2),               // CONTAINS_OP
        119 => Some(oparg + 1),       // RERAISE
        120 => Some((oparg - 1) + 1), // COPY
        121 => Some(0),               // RETURN_CONST
        122 => Some(2),               // BINARY_OP
        123 => Some(2),               // SEND
        124 => Some(0),               // LOAD_FAST
        125 => Some(1),               // STORE_FAST
        126 => Some(0),               // DELETE_FAST
        127 => Some(0),               // LOAD_FAST_CHECK
        128 => Some(1),               // POP_JUMP_IF_NOT_NONE
        129 => Some(1),               // POP_JUMP_IF_NONE
        130 => Some(oparg),           // RAISE_VARARGS
        131 => Some(1),               // GET_AWAITABLE
        132 => Some(
            (if (oparg & 0x01) != 0 { 1 } else { 0 })
                + (if (oparg & 0x02) != 0 { 1 } else { 0 })
                + (if (oparg & 0x04) != 0 { 1 } else { 0 })
                + (if (oparg & 0x08) != 0 { 1 } else { 0 })
                + 1,
        ), // MAKE_FUNCTION
        133 => Some((if oparg == 3 { 1 } else { 0 }) + 2), // BUILD_SLICE
        134 => Some(0),               // JUMP_BACKWARD_NO_INTERRUPT
        135 => Some(0),               // MAKE_CELL
        136 => Some(0),               // LOAD_CLOSURE
        137 => Some(0),               // LOAD_DEREF
        138 => Some(1),               // STORE_DEREF
        139 => Some(0),               // DELETE_DEREF
        140 => Some(0),               // JUMP_BACKWARD
        141 => Some(3),               // LOAD_SUPER_ATTR
        142 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 3), // CALL_FUNCTION_EX
        143 => Some(0),               // LOAD_FAST_AND_CLEAR
        144 => Some(0),               // EXTENDED_ARG
        145 => Some((oparg - 1) + 2), // LIST_APPEND
        146 => Some((oparg - 1) + 2), // SET_ADD
        147 => Some(2),               // MAP_ADD
        149 => Some(0),               // COPY_FREE_VARS
        150 => Some(1),               // YIELD_VALUE
        151 => Some(0),               // RESUME
        152 => Some(3),               // MATCH_CLASS
        155 => Some((if (oparg & 0x04) == 0x04 { 1 } else { 0 }) + 1), // FORMAT_VALUE
        156 => Some(oparg + 1),       // BUILD_CONST_KEY_MAP
        157 => Some(oparg),           // BUILD_STRING
        162 => Some((oparg - 1) + 2), // LIST_EXTEND
        163 => Some((oparg - 1) + 2), // SET_UPDATE
        164 => Some(1),               // DICT_MERGE
        165 => Some(1),               // DICT_UPDATE
        171 => Some(oparg + 2),       // CALL
        172 => Some(0),               // KW_NAMES
        173 => Some(1),               // CALL_INTRINSIC_1
        174 => Some(2),               // CALL_INTRINSIC_2
        175 => Some(1),               // LOAD_FROM_DICT_OR_GLOBALS
        176 => Some(1),               // LOAD_FROM_DICT_OR_DEREF
        237 => Some(3),               // INSTRUMENTED_LOAD_SUPER_ATTR
        238 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_NONE
        239 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
        240 => Some(0),               // INSTRUMENTED_RESUME
        241 => Some(0),               // INSTRUMENTED_CALL
        242 => Some(1),               // INSTRUMENTED_RETURN_VALUE
        243 => Some(1),               // INSTRUMENTED_YIELD_VALUE
        244 => Some(0),               // INSTRUMENTED_CALL_FUNCTION_EX
        245 => Some(0),               // INSTRUMENTED_JUMP_FORWARD
        246 => Some(0),               // INSTRUMENTED_JUMP_BACKWARD
        247 => Some(0),               // INSTRUMENTED_RETURN_CONST
        248 => Some(0),               // INSTRUMENTED_FOR_ITER
        249 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_FALSE
        250 => Some(0),               // INSTRUMENTED_POP_JUMP_IF_TRUE
        251 => Some(2),               // INSTRUMENTED_END_FOR
        252 => Some(2),               // INSTRUMENTED_END_SEND
        253 => Some(0),               // INSTRUMENTED_INSTRUCTION
        _ => None,
    }
}

fn opcode_num_pushed_312(opcode: i64, oparg: i64) -> Option<i64> {
    match opcode {
        0 => Some(0),                                            // CACHE
        1 => Some(0),                                            // POP_TOP
        2 => Some(1),                                            // PUSH_NULL
        3 => Some(0),                                            // INTERPRETER_EXIT
        4 => Some(0),                                            // END_FOR
        5 => Some(1),                                            // END_SEND
        9 => Some(0),                                            // NOP
        11 => Some(1),                                           // UNARY_NEGATIVE
        12 => Some(1),                                           // UNARY_NOT
        15 => Some(1),                                           // UNARY_INVERT
        17 => Some(0),                                           // RESERVED
        25 => Some(1),                                           // BINARY_SUBSCR
        26 => Some(1),                                           // BINARY_SLICE
        27 => Some(0),                                           // STORE_SLICE
        30 => Some(2),                                           // GET_LEN
        31 => Some(2),                                           // MATCH_MAPPING
        32 => Some(2),                                           // MATCH_SEQUENCE
        33 => Some(3),                                           // MATCH_KEYS
        35 => Some(2),                                           // PUSH_EXC_INFO
        36 => Some(2),                                           // CHECK_EXC_MATCH
        37 => Some(2),                                           // CHECK_EG_MATCH
        49 => Some(5),                                           // WITH_EXCEPT_START
        50 => Some(1),                                           // GET_AITER
        51 => Some(2),                                           // GET_ANEXT
        52 => Some(2),                                           // BEFORE_ASYNC_WITH
        53 => Some(2),                                           // BEFORE_WITH
        54 => Some(0),                                           // END_ASYNC_FOR
        55 => Some(2),                                           // CLEANUP_THROW
        60 => Some(0),                                           // STORE_SUBSCR
        61 => Some(0),                                           // DELETE_SUBSCR
        68 => Some(1),                                           // GET_ITER
        69 => Some(1),                                           // GET_YIELD_FROM_ITER
        71 => Some(1),                                           // LOAD_BUILD_CLASS
        74 => Some(1),                                           // LOAD_ASSERTION_ERROR
        75 => Some(0),                                           // RETURN_GENERATOR
        83 => Some(0),                                           // RETURN_VALUE
        85 => Some(0),                                           // SETUP_ANNOTATIONS
        87 => Some(1),                                           // LOAD_LOCALS
        89 => Some(0),                                           // POP_EXCEPT
        90 => Some(0),                                           // STORE_NAME
        91 => Some(0),                                           // DELETE_NAME
        92 => Some(oparg),                                       // UNPACK_SEQUENCE
        93 => Some(2),                                           // FOR_ITER
        94 => Some((oparg & 0xFF) + (oparg >> 8) + 1),           // UNPACK_EX
        95 => Some(0),                                           // STORE_ATTR
        96 => Some(0),                                           // DELETE_ATTR
        97 => Some(0),                                           // STORE_GLOBAL
        98 => Some(0),                                           // DELETE_GLOBAL
        99 => Some((oparg - 2) + 2),                             // SWAP
        100 => Some(1),                                          // LOAD_CONST
        101 => Some(1),                                          // LOAD_NAME
        102 => Some(1),                                          // BUILD_TUPLE
        103 => Some(1),                                          // BUILD_LIST
        104 => Some(1),                                          // BUILD_SET
        105 => Some(1),                                          // BUILD_MAP
        106 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_ATTR
        107 => Some(1),                                          // COMPARE_OP
        108 => Some(1),                                          // IMPORT_NAME
        109 => Some(2),                                          // IMPORT_FROM
        110 => Some(0),                                          // JUMP_FORWARD
        114 => Some(0),                                          // POP_JUMP_IF_FALSE
        115 => Some(0),                                          // POP_JUMP_IF_TRUE
        116 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_GLOBAL
        117 => Some(1),                                          // IS_OP
        118 => Some(1),                                          // CONTAINS_OP
        119 => Some(oparg),                                      // RERAISE
        120 => Some((oparg - 1) + 2),                            // COPY
        121 => Some(0),                                          // RETURN_CONST
        122 => Some(1),                                          // BINARY_OP
        123 => Some(2),                                          // SEND
        124 => Some(1),                                          // LOAD_FAST
        125 => Some(0),                                          // STORE_FAST
        126 => Some(0),                                          // DELETE_FAST
        127 => Some(1),                                          // LOAD_FAST_CHECK
        128 => Some(0),                                          // POP_JUMP_IF_NOT_NONE
        129 => Some(0),                                          // POP_JUMP_IF_NONE
        130 => Some(0),                                          // RAISE_VARARGS
        131 => Some(1),                                          // GET_AWAITABLE
        132 => Some(1),                                          // MAKE_FUNCTION
        133 => Some(1),                                          // BUILD_SLICE
        134 => Some(0),                                          // JUMP_BACKWARD_NO_INTERRUPT
        135 => Some(0),                                          // MAKE_CELL
        136 => Some(1),                                          // LOAD_CLOSURE
        137 => Some(1),                                          // LOAD_DEREF
        138 => Some(0),                                          // STORE_DEREF
        139 => Some(0),                                          // DELETE_DEREF
        140 => Some(0),                                          // JUMP_BACKWARD
        141 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // LOAD_SUPER_ATTR
        142 => Some(1),                                          // CALL_FUNCTION_EX
        143 => Some(1),                                          // LOAD_FAST_AND_CLEAR
        144 => Some(0),                                          // EXTENDED_ARG
        145 => Some((oparg - 1) + 1),                            // LIST_APPEND
        146 => Some((oparg - 1) + 1),                            // SET_ADD
        147 => Some(0),                                          // MAP_ADD
        149 => Some(0),                                          // COPY_FREE_VARS
        150 => Some(1),                                          // YIELD_VALUE
        151 => Some(0),                                          // RESUME
        152 => Some(1),                                          // MATCH_CLASS
        155 => Some(1),                                          // FORMAT_VALUE
        156 => Some(1),                                          // BUILD_CONST_KEY_MAP
        157 => Some(1),                                          // BUILD_STRING
        162 => Some((oparg - 1) + 1),                            // LIST_EXTEND
        163 => Some((oparg - 1) + 1),                            // SET_UPDATE
        164 => Some(0),                                          // DICT_MERGE
        165 => Some(0),                                          // DICT_UPDATE
        171 => Some(1),                                          // CALL
        172 => Some(0),                                          // KW_NAMES
        173 => Some(1),                                          // CALL_INTRINSIC_1
        174 => Some(1),                                          // CALL_INTRINSIC_2
        175 => Some(1),                                          // LOAD_FROM_DICT_OR_GLOBALS
        176 => Some(1),                                          // LOAD_FROM_DICT_OR_DEREF
        237 => Some((if (oparg & 1) != 0 { 1 } else { 0 }) + 1), // INSTRUMENTED_LOAD_SUPER_ATTR
        238 => Some(0),                                          // INSTRUMENTED_POP_JUMP_IF_NONE
        239 => Some(0), // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
        240 => Some(0), // INSTRUMENTED_RESUME
        241 => Some(0), // INSTRUMENTED_CALL
        242 => Some(0), // INSTRUMENTED_RETURN_VALUE
        243 => Some(1), // INSTRUMENTED_YIELD_VALUE
        244 => Some(0), // INSTRUMENTED_CALL_FUNCTION_EX
        245 => Some(0), // INSTRUMENTED_JUMP_FORWARD
        246 => Some(0), // INSTRUMENTED_JUMP_BACKWARD
        247 => Some(0), // INSTRUMENTED_RETURN_CONST
        248 => Some(0), // INSTRUMENTED_FOR_ITER
        249 => Some(0), // INSTRUMENTED_POP_JUMP_IF_FALSE
        250 => Some(0), // INSTRUMENTED_POP_JUMP_IF_TRUE
        251 => Some(0), // INSTRUMENTED_END_FOR
        252 => Some(1), // INSTRUMENTED_END_SEND
        253 => Some(0), // INSTRUMENTED_INSTRUCTION
        _ => None,
    }
}

fn opcode_is_noarg_pseudo_312(opcode: i64) -> bool {
    matches!(opcode, 256..=259)
}

fn opcode_stack_effect_pseudo_312(opcode: i64) -> Option<i64> {
    match opcode {
        256 => Some(1),  // SETUP_FINALLY (max jump/non-jump)
        257 => Some(2),  // SETUP_CLEANUP (max jump/non-jump)
        258 => Some(1),  // SETUP_WITH (max jump/non-jump)
        259 => Some(0),  // POP_BLOCK
        260 => Some(0),  // JUMP
        261 => Some(0),  // JUMP_NO_INTERRUPT
        262 => Some(1),  // LOAD_METHOD
        263 => Some(-1), // LOAD_SUPER_METHOD
        264 => Some(-1), // LOAD_ZERO_SUPER_METHOD
        265 => Some(-1), // LOAD_ZERO_SUPER_ATTR
        266 => Some(-1), // STORE_FAST_MAYBE_NULL
        _ => None,
    }
}

#[inline]
fn opcode_is_noarg_312(opcode: i64) -> bool {
    opcode < 90 || opcode_is_noarg_pseudo_312(opcode)
}

#[inline]
fn opcode_stack_effect_core_312(opcode: i64, oparg: i64) -> Option<i64> {
    if let Some(effect) = opcode_stack_effect_pseudo_312(opcode) {
        return Some(effect);
    }
    let popped = opcode_num_popped_312(opcode, oparg)?;
    let pushed = opcode_num_pushed_312(opcode, oparg)?;
    if popped < 0 || pushed < 0 {
        return None;
    }
    pushed.checked_sub(popped)
}

fn token_payload_json_value_to_bits(
    _py: &crate::PyToken<'_>,
    value: &JsonValue,
) -> Result<u64, u64> {
    match value {
        JsonValue::Null => Ok(MoltObject::none().bits()),
        JsonValue::Bool(flag) => Ok(MoltObject::from_bool(*flag).bits()),
        JsonValue::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Ok(MoltObject::from_int(integer).bits());
            }
            if let Some(integer) = number.as_u64() {
                let Ok(integer_i64) = i64::try_from(integer) else {
                    return Err(raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "token payload number is out of range",
                    ));
                };
                return Ok(MoltObject::from_int(integer_i64).bits());
            }
            if let Some(float_value) = number.as_f64() {
                return Ok(MoltObject::from_float(float_value).bits());
            }
            Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "token payload number is invalid",
            ))
        }
        JsonValue::String(text) => {
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        JsonValue::Array(items) => {
            let mut item_bits: Vec<u64> = Vec::with_capacity(items.len());
            for item in items {
                let bits = match token_payload_json_value_to_bits(_py, item) {
                    Ok(bits) => bits,
                    Err(err_bits) => {
                        for owned in item_bits {
                            dec_ref_bits(_py, owned);
                        }
                        return Err(err_bits);
                    }
                };
                item_bits.push(bits);
            }
            let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            if list_ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(list_ptr).bits())
            }
        }
        JsonValue::Object(entries) => {
            let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
            let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
            for (key, item) in entries {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    for owned in owned_bits {
                        dec_ref_bits(_py, owned);
                    }
                    return Err(MoltObject::none().bits());
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let value_bits = match token_payload_json_value_to_bits(_py, item) {
                    Ok(bits) => bits,
                    Err(err_bits) => {
                        dec_ref_bits(_py, key_bits);
                        for owned in owned_bits {
                            dec_ref_bits(_py, owned);
                        }
                        return Err(err_bits);
                    }
                };
                pairs.push(key_bits);
                pairs.push(value_bits);
                owned_bits.push(key_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            if dict_ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(dict_ptr).bits())
            }
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ArgparseOptionalKind {
    Value,
    StoreTrue,
}

#[derive(Clone)]
struct ArgparseOptionalSpec {
    flag: String,
    dest: String,
    kind: ArgparseOptionalKind,
    required: bool,
    default: JsonValue,
}

#[derive(Clone)]
struct ArgparseSubparsersSpec {
    dest: String,
    required: bool,
    parsers: HashMap<String, ArgparseSpec>,
}

#[derive(Clone)]
struct ArgparseSpec {
    optionals: Vec<ArgparseOptionalSpec>,
    positionals: Vec<String>,
    subparsers: Option<ArgparseSubparsersSpec>,
}

fn argparse_choice_list(parsers: &HashMap<String, ArgparseSpec>) -> String {
    let mut keys: Vec<&str> = parsers.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys.join(", ")
}

fn argparse_decode_spec(value: &JsonValue) -> Result<ArgparseSpec, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "argparse spec must be a JSON object".to_string())?;

    let mut optionals: Vec<ArgparseOptionalSpec> = Vec::new();
    if let Some(raw_optionals) = obj.get("optionals") {
        let items = raw_optionals
            .as_array()
            .ok_or_else(|| "argparse optionals must be a JSON array".to_string())?;
        for item in items {
            let item_obj = item
                .as_object()
                .ok_or_else(|| "argparse optional spec must be object".to_string())?;
            let flag = item_obj
                .get("flag")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string flag".to_string())?
                .to_string();
            let dest = item_obj
                .get("dest")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string dest".to_string())?
                .to_string();
            let kind = item_obj
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string kind".to_string())?;
            let parsed_kind = match kind {
                "value" => ArgparseOptionalKind::Value,
                "store_true" => ArgparseOptionalKind::StoreTrue,
                _ => return Err(format!("unsupported argparse optional kind: {kind}")),
            };
            let required = item_obj
                .get("required")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let default = item_obj.get("default").cloned().unwrap_or_else(|| {
                if parsed_kind == ArgparseOptionalKind::StoreTrue {
                    JsonValue::Bool(false)
                } else {
                    JsonValue::Null
                }
            });
            optionals.push(ArgparseOptionalSpec {
                flag,
                dest,
                kind: parsed_kind,
                required,
                default,
            });
        }
    }

    let mut positionals: Vec<String> = Vec::new();
    if let Some(raw_positionals) = obj.get("positionals") {
        let items = raw_positionals
            .as_array()
            .ok_or_else(|| "argparse positionals must be a JSON array".to_string())?;
        for item in items {
            let item_obj = item
                .as_object()
                .ok_or_else(|| "argparse positional spec must be object".to_string())?;
            let dest = item_obj
                .get("dest")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse positional spec missing string dest".to_string())?
                .to_string();
            positionals.push(dest);
        }
    }

    let subparsers = if let Some(raw_subparsers) = obj.get("subparsers") {
        let sp_obj = raw_subparsers
            .as_object()
            .ok_or_else(|| "argparse subparsers spec must be object".to_string())?;
        let dest = sp_obj
            .get("dest")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "argparse subparsers spec missing string dest".to_string())?
            .to_string();
        let required = sp_obj
            .get("required")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let parsers_obj = sp_obj
            .get("parsers")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| "argparse subparsers spec missing parsers object".to_string())?;
        let mut parsers: HashMap<String, ArgparseSpec> = HashMap::new();
        for (name, parser_spec) in parsers_obj {
            let parsed = argparse_decode_spec(parser_spec)?;
            parsers.insert(name.clone(), parsed);
        }
        Some(ArgparseSubparsersSpec {
            dest,
            required,
            parsers,
        })
    } else {
        None
    };

    Ok(ArgparseSpec {
        optionals,
        positionals,
        subparsers,
    })
}

fn argparse_parse_with_spec(
    spec: &ArgparseSpec,
    argv: &[String],
) -> Result<JsonMap<String, JsonValue>, String> {
    let mut out: JsonMap<String, JsonValue> = JsonMap::new();
    let mut optional_dest_seen: HashSet<String> = HashSet::new();
    for opt in &spec.optionals {
        out.insert(opt.dest.clone(), opt.default.clone());
    }

    let mut pos_index = 0usize;
    let mut index = 0usize;

    while index < argv.len() {
        let token = &argv[index];
        if token.starts_with('-') && token != "-" {
            let Some(opt) = spec.optionals.iter().find(|entry| entry.flag == *token) else {
                return Err(format!("unrecognized arguments: {token}"));
            };
            optional_dest_seen.insert(opt.dest.clone());
            match opt.kind {
                ArgparseOptionalKind::StoreTrue => {
                    out.insert(opt.dest.clone(), JsonValue::Bool(true));
                    index += 1;
                }
                ArgparseOptionalKind::Value => {
                    if index + 1 >= argv.len() {
                        return Err(format!("argument {}: expected one argument", opt.flag));
                    }
                    let value = argv[index + 1].clone();
                    out.insert(opt.dest.clone(), JsonValue::String(value));
                    index += 2;
                }
            }
            continue;
        }

        if pos_index < spec.positionals.len() {
            let dest = spec.positionals[pos_index].clone();
            out.insert(dest, JsonValue::String(token.clone()));
            pos_index += 1;
            index += 1;
            continue;
        }

        if let Some(subparsers) = &spec.subparsers {
            if let Some(child_spec) = subparsers.parsers.get(token) {
                out.insert(subparsers.dest.clone(), JsonValue::String(token.clone()));
                let child = argparse_parse_with_spec(child_spec, &argv[index + 1..])?;
                for (key, value) in child {
                    out.insert(key, value);
                }
                break;
            }
            let choices = argparse_choice_list(&subparsers.parsers);
            return Err(format!(
                "argument {}: invalid choice: '{}' (choose from {})",
                subparsers.dest, token, choices
            ));
        }

        return Err(format!("unrecognized arguments: {token}"));
    }

    if pos_index < spec.positionals.len() {
        let missing = spec.positionals[pos_index..].join(", ");
        return Err(format!("the following arguments are required: {missing}"));
    }

    for opt in &spec.optionals {
        if opt.required && !optional_dest_seen.contains(&opt.dest) {
            return Err(format!(
                "the following arguments are required: {}",
                opt.flag
            ));
        }
    }

    if let Some(subparsers) = &spec.subparsers
        && subparsers.required
        && !out.contains_key(&subparsers.dest)
    {
        return Err(format!(
            "the following arguments are required: {}",
            subparsers.dest
        ));
    }

    Ok(out)
}

fn bisect_normalize_bounds(
    _py: &crate::PyToken<'_>,
    seq_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
) -> Result<(i64, i64), u64> {
    let lo_err = format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(_py, obj_from_bits(lo_bits))
    );
    let Some(lo) = index_i64_with_overflow(_py, lo_bits, lo_err.as_str(), None) else {
        return Err(MoltObject::none().bits());
    };
    if lo < 0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "lo must be non-negative",
        ));
    }

    let seq_len_bits = crate::molt_len(seq_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(seq_len) = to_i64(obj_from_bits(seq_len_bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "object has no usable length for bisect",
        ));
    };
    if !obj_from_bits(seq_len_bits).is_none() {
        dec_ref_bits(_py, seq_len_bits);
    }

    let hi = if obj_from_bits(hi_bits).is_none() {
        seq_len
    } else {
        let hi_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(hi_bits))
        );
        let Some(value) = index_i64_with_overflow(_py, hi_bits, hi_err.as_str(), None) else {
            return Err(MoltObject::none().bits());
        };
        value
    };
    Ok((lo, hi))
}

fn bisect_find_index(
    _py: &crate::PyToken<'_>,
    seq_bits: u64,
    x_bits: u64,
    mut lo: i64,
    mut hi: i64,
    key_bits: u64,
    left: bool,
) -> Result<i64, u64> {
    while lo < hi {
        let mid = (lo + hi) / 2;
        let mid_bits = MoltObject::from_int(mid).bits();
        let item_bits = molt_getitem_method(seq_bits, mid_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }

        let mut key_result_bits = item_bits;
        let mut release_key = false;
        if !obj_from_bits(key_bits).is_none() {
            key_result_bits = unsafe { call_callable1(_py, key_bits, item_bits) };
            if exception_pending(_py) {
                if !obj_from_bits(item_bits).is_none() {
                    dec_ref_bits(_py, item_bits);
                }
                return Err(MoltObject::none().bits());
            }
            release_key = true;
        }

        let lt_bits = if left {
            crate::molt_lt(key_result_bits, x_bits)
        } else {
            crate::molt_lt(x_bits, key_result_bits)
        };
        if exception_pending(_py) {
            if release_key && !obj_from_bits(key_result_bits).is_none() {
                dec_ref_bits(_py, key_result_bits);
            }
            if !obj_from_bits(item_bits).is_none() {
                dec_ref_bits(_py, item_bits);
            }
            return Err(MoltObject::none().bits());
        }

        if left {
            if is_truthy(_py, obj_from_bits(lt_bits)) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        } else if is_truthy(_py, obj_from_bits(lt_bits)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }

        if release_key && !obj_from_bits(key_result_bits).is_none() {
            dec_ref_bits(_py, key_result_bits);
        }
        if !obj_from_bits(item_bits).is_none() {
            dec_ref_bits(_py, item_bits);
        }
    }
    Ok(lo)
}

fn bisect_insert_at(
    _py: &crate::PyToken<'_>,
    seq_bits: u64,
    pos: i64,
    x_bits: u64,
) -> Result<(), u64> {
    let missing = missing_bits(_py);
    let Some(insert_name_bits) = attr_name_bits_from_bytes(_py, b"insert") else {
        return Err(MoltObject::none().bits());
    };
    let insert_bits = molt_getattr_builtin(seq_bits, insert_name_bits, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let pos_bits = MoltObject::from_int(pos).bits();
    let out_bits = unsafe { call_callable2(_py, insert_bits, pos_bits, x_bits) };
    if !obj_from_bits(insert_bits).is_none() {
        dec_ref_bits(_py, insert_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(_py, out_bits);
    }
    Ok(())
}
fn parse_stat_mode(_py: &crate::PyToken<'_>, mode_bits: u64) -> Result<i64, u64> {
    let Some(mode) = to_i64(obj_from_bits(mode_bits)) else {
        return Err(raise_exception::<_>(_py, "TypeError", "mode must be int"));
    };
    Ok(mode)
}

    width_bits: u64,
    initial_indent_bits: u64,
    subsequent_indent_bits: u64,
    expand_tabs_bits: u64,
    replace_whitespace_bits: u64,
    fix_sentence_endings_bits: u64,
    break_long_words_bits: u64,
    drop_whitespace_bits: u64,
    break_on_hyphens_bits: u64,
    tabsize_bits: u64,
    max_lines_placeholder_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let options = match textwrap_parse_options_ex(
            _py,
            width_bits,
            initial_indent_bits,
            subsequent_indent_bits,
            expand_tabs_bits,
            replace_whitespace_bits,
            fix_sentence_endings_bits,
            break_long_words_bits,
            drop_whitespace_bits,
            break_on_hyphens_bits,
            tabsize_bits,
            max_lines_placeholder_bits,
        ) {
            Ok(options) => options,
            Err(bits) => return bits,
        };
        let lines = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &lines)
    })
}
fn http_server_read_request_impl(_py: &crate::PyToken<'_>, handler_bits: u64) -> Result<i64, u64> {
    let request_line = http_server_readline(_py, handler_bits, 65537)?;
    if request_line.is_empty() {
        return Ok(0);
    }
    let line_text = String::from_utf8_lossy(&request_line)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    http_server_set_attr_string(_py, handler_bits, b"requestline", &line_text)?;
    http_server_set_attr_string(
        _py,
        handler_bits,
        b"request_version",
        HTTP_SERVER_DEFAULT_REQUEST_VERSION,
    )?;
    http_server_set_attr_string(_py, handler_bits, b"command", "")?;
    http_server_set_attr_string(_py, handler_bits, b"path", "")?;
    http_server_set_attr_string(_py, handler_bits, b"_molt_connection_header", "")?;

    let parts: Vec<&str> = line_text.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(0);
    }
    let command: String;
    let path: String;
    let mut request_version = HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string();

    if parts.len() >= 3 {
        command = parts[0].to_string();
        path = parts[1].to_string();
        if parts.len() > 3 {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad request version ({})",
                    http_server_repr_single_quoted(parts[3])
                )),
            )?;
            return Ok(2);
        }
        let version = parts[2];
        let Some(version_tail) = version.strip_prefix("HTTP/") else {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad request version ({})",
                    http_server_repr_single_quoted(version)
                )),
            )?;
            return Ok(2);
        };
        let mut chunks = version_tail.split('.');
        let major = chunks.next().unwrap_or_default();
        let minor = chunks.next().unwrap_or_default();
        if major.is_empty()
            || minor.is_empty()
            || chunks.next().is_some()
            || !major.chars().all(|ch| ch.is_ascii_digit())
            || !minor.chars().all(|ch| ch.is_ascii_digit())
        {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad request version ({})",
                    http_server_repr_single_quoted(version)
                )),
            )?;
            return Ok(2);
        }
        request_version = version.to_string();
    } else if parts.len() == 2 {
        command = parts[0].to_string();
        path = parts[1].to_string();
        if command != "GET" {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad HTTP/0.9 request type ({})",
                    http_server_repr_single_quoted(&command)
                )),
            )?;
            return Ok(2);
        }
    } else {
        http_server_send_error_impl(
            _py,
            handler_bits,
            400,
            Some(format!(
                "Bad request syntax ({})",
                http_server_repr_single_quoted(&line_text)
            )),
        )?;
        return Ok(2);
    }

    http_server_set_attr_string(_py, handler_bits, b"command", &command)?;
    http_server_set_attr_string(_py, handler_bits, b"path", &path)?;
    http_server_set_attr_string(_py, handler_bits, b"request_version", &request_version)?;

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut connection_header = String::new();
    loop {
        let line = http_server_readline(_py, handler_bits, 65537)?;
        if line.is_empty() || line == b"\r\n" || line == b"\n" {
            break;
        }
        let line_text = String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if let Some((key, value)) = line_text.split_once(':') {
            let key_text = key.trim().to_string();
            let value_text = value.trim_start().to_string();
            if key_text.eq_ignore_ascii_case("Connection") {
                connection_header = value_text.to_ascii_lowercase();
            }
            headers.push((key_text, value_text));
        }
    }
    let headers_bits = urllib_http_headers_to_list(_py, &headers)?;
    if !urllib_request_set_attr(_py, handler_bits, b"_molt_header_pairs", headers_bits) {
        dec_ref_bits(_py, headers_bits);
        return Err(MoltObject::none().bits());
    }
    if !urllib_request_set_attr(_py, handler_bits, b"headers", headers_bits) {
        dec_ref_bits(_py, headers_bits);
        return Err(MoltObject::none().bits());
    }
    dec_ref_bits(_py, headers_bits);
    http_server_set_attr_string(
        _py,
        handler_bits,
        b"_molt_connection_header",
        &connection_header,
    )?;
    Ok(1)
}

fn http_server_compute_close_connection_impl(
    _py: &crate::PyToken<'_>,
    handler_bits: u64,
) -> Result<bool, u64> {
    let connection =
        match http_server_get_optional_attr_string(_py, handler_bits, b"_molt_connection_header") {
            Ok(Some(value)) => value.to_ascii_lowercase(),
            Ok(None) => String::new(),
            Err(bits) => return Err(bits),
        };
    if connection == "close" {
        return Ok(true);
    }
    if connection == "keep-alive" {
        return Ok(false);
    }
    let request_version =
        match http_server_get_optional_attr_string(_py, handler_bits, b"request_version") {
            Ok(Some(value)) => value,
            Ok(None) => HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string(),
            Err(bits) => return Err(bits),
        };
    Ok(request_version != HTTP_SERVER_HTTP11)
}

    code_bits: u64,
    message_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::format_obj_str(_py, obj_from_bits(message_bits)))
        };
        match http_server_send_response_impl(_py, handler_bits, code, message) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}
#[inline]
fn urllib_response_is_data(resp: &MoltUrllibResponse) -> bool {
    resp.code < 0
}

fn urllib_response_read_vec(
    resp: &mut MoltUrllibResponse,
    size_opt: Option<i64>,
) -> Result<Vec<u8>, String> {
    if resp.closed {
        if urllib_response_is_data(resp) {
            return Err("I/O operation on closed file.".to_string());
        }
        return Ok(Vec::new());
    }
    let total = resp.body.len();
    let start = resp.pos.min(total);
    let end = match size_opt {
        Some(value) if value >= 0 => {
            let wanted = usize::try_from(value).unwrap_or(0);
            total.min(start.saturating_add(wanted))
        }
        _ => total,
    };
    resp.pos = end;
    Ok(resp.body[start..end].to_vec())
}

fn urllib_response_readinto_len(
    resp: &mut MoltUrllibResponse,
    out_buf: &mut [u8],
) -> Result<usize, String> {
    if resp.closed {
        if urllib_response_is_data(resp) {
            return Err("I/O operation on closed file.".to_string());
        }
        return Ok(0);
    }
    let out_len = out_buf.len();
    let total = resp.body.len();
    let start = resp.pos.min(total);
    let end = total.min(start.saturating_add(out_len));
    let read_len = end.saturating_sub(start);
    if read_len > 0 {
        out_buf[..read_len].copy_from_slice(&resp.body[start..end]);
    }
    resp.pos = end;
    Ok(read_len)
}

    buffer_bits: u64,
) -> u64 {
    molt_urllib_request_response_readinto(handle_bits, buffer_bits)
}

fn urllib_response_readline_vec(
    resp: &mut MoltUrllibResponse,
    size_opt: Option<i64>,
) -> Result<Vec<u8>, String> {
    if resp.closed {
        if urllib_response_is_data(resp) {
            return Err("I/O operation on closed file.".to_string());
        }
        return Ok(Vec::new());
    }
    let total = resp.body.len();
    let start = resp.pos.min(total);
    let max_end = match size_opt {
        Some(value) if value >= 0 => {
            let wanted = usize::try_from(value).unwrap_or(0);
            total.min(start.saturating_add(wanted))
        }
        _ => total,
    };
    if start >= max_end {
        return Ok(Vec::new());
    }
    let slice = &resp.body[start..max_end];
    let end = match slice.iter().position(|b| *b == b'\n') {
        Some(offset) => start.saturating_add(offset).saturating_add(1),
        None => max_end,
    };
    resp.pos = end;
    Ok(resp.body[start..end].to_vec())
}

fn urllib_response_seek_pos(
    resp: &MoltUrllibResponse,
    offset: i64,
    whence: i64,
) -> Result<usize, String> {
    let base = match whence {
        0 => 0_i128,
        1 => i128::try_from(resp.pos).unwrap_or(i128::MAX),
        2 => i128::try_from(resp.body.len()).unwrap_or(i128::MAX),
        _ => return Err(format!("whence value {whence} unsupported")),
    };
    let target = base.saturating_add(i128::from(offset));
    if target < 0 {
        return Err(format!("negative seek value {target}"));
    }
    let as_u128 = target as u128;
    if as_u128 > (usize::MAX as u128) {
        return Err("seek position out of range".to_string());
    }
    Ok(as_u128 as usize)
}

    offset_bits: u64,
    whence_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(offset) = to_i64(obj_from_bits(offset_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "offset must be int");
        };
        let Some(whence) = to_i64(obj_from_bits(whence_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "whence must be int");
        };
        let Some(out) = urllib_response_with_mut(handle, |resp| {
            if !urllib_response_is_data(resp) {
                return Err(raise_exception::<u64>(_py, "UnsupportedOperation", "seek"));
            }
            if resp.closed {
                return Err(raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "I/O operation on closed file.",
                ));
            }
            let pos = match urllib_response_seek_pos(resp, offset, whence) {
                Ok(pos) => pos,
                Err(msg) => return Err(raise_exception::<u64>(_py, "ValueError", &msg)),
            };
            resp.pos = pos;
            Ok(pos)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(pos) => MoltObject::from_int(pos as i64).bits(),
            Err(bits) => bits,
        }
    })
}
fn urllib_response_message_bits(_py: &crate::PyToken<'_>, handle: i64) -> u64 {
    let Some(headers) = urllib_response_with(handle, |resp| resp.headers.clone()) else {
        return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
    };
    let Some(message_handle) = http_message_store(headers) else {
        return MoltObject::none().bits();
    };
    MoltObject::from_int(message_handle).bits()
}

    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name = crate::format_obj_str(_py, obj_from_bits(name_bits));
        let value = crate::format_obj_str(_py, obj_from_bits(value_bits));
        let Some(()) = http_message_with_mut(handle, |message| {
            http_message_push_header(_py, message, name, value);
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        MoltObject::none().bits()
    })
}
// --- Begin stdlib_ast-gated compile infrastructure ---
#[cfg(feature = "stdlib_ast")]
fn compile_error_type(error: &ParseErrorType) -> &'static str {
    if error.is_tab_error() {
        "TabError"
    } else if error.is_indentation_error() {
        "IndentationError"
    } else {
        "SyntaxError"
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flag_for_name(name: &str) -> i64 {
    match name {
        "nested_scopes" => 0x0010,
        "generators" => 0,
        "division" => 0x20000,
        "absolute_import" => 0x40000,
        "with_statement" => 0x80000,
        "print_function" => 0x100000,
        "unicode_literals" => 0x200000,
        "barry_as_FLUFL" => 0x400000,
        "generator_stop" => 0x800000,
        "annotations" => 0x1000000,
        _ => 0,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_is_docstring_stmt(stmt: &pyast::Stmt) -> bool {
    match stmt {
        pyast::Stmt::Expr(node) => match node.value.as_ref() {
            pyast::Expr::Constant(expr) => matches!(expr.value, pyast::Constant::Str(_)),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flags_from_stmts(stmts: &[pyast::Stmt]) -> i64 {
    let mut idx = 0usize;
    if let Some(first) = stmts.first()
        && codeop_is_docstring_stmt(first)
    {
        idx = 1;
    }
    let mut out = 0i64;
    for stmt in &stmts[idx..] {
        let pyast::Stmt::ImportFrom(node) = stmt else {
            break;
        };
        let Some(module) = node.module.as_ref() else {
            break;
        };
        let level_is_zero = match node.level.as_ref() {
            None => true,
            Some(value) => value.to_u32() == 0,
        };
        if module.as_str() != "__future__" || !level_is_zero {
            break;
        }
        for alias in &node.names {
            out |= codeop_future_flag_for_name(alias.name.as_str());
        }
    }
    out
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flags_from_parsed(parsed: &pyast::Mod) -> i64 {
    match parsed {
        pyast::Mod::Module(module) => codeop_future_flags_from_stmts(&module.body),
        pyast::Mod::Interactive(module) => codeop_future_flags_from_stmts(&module.body),
        _ => 0,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_stmt_is_compound(stmt: &pyast::Stmt) -> bool {
    matches!(
        stmt,
        pyast::Stmt::FunctionDef(_)
            | pyast::Stmt::AsyncFunctionDef(_)
            | pyast::Stmt::ClassDef(_)
            | pyast::Stmt::If(_)
            | pyast::Stmt::For(_)
            | pyast::Stmt::AsyncFor(_)
            | pyast::Stmt::While(_)
            | pyast::Stmt::With(_)
            | pyast::Stmt::AsyncWith(_)
            | pyast::Stmt::Try(_)
            | pyast::Stmt::TryStar(_)
            | pyast::Stmt::Match(_)
    )
}

#[cfg(feature = "stdlib_ast")]
fn codeop_source_incomplete_after_success(source: &str, mode: &str, parsed: &pyast::Mod) -> bool {
    if mode != "single" {
        return false;
    }
    if source.trim_end().ends_with(':') {
        return true;
    }
    if source.contains('\n')
        && !source.ends_with('\n')
        && let pyast::Mod::Interactive(module) = parsed
        && let Some(first) = module.body.first()
    {
        return codeop_stmt_is_compound(first);
    }
    false
}

#[cfg(feature = "stdlib_ast")]
fn codeop_source_has_missing_indented_suite(source: &str) -> bool {
    let lines: Vec<&str> = source.split('\n').collect();
    let leading_indent = |line: &str| -> usize {
        line.chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .count()
    };

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.ends_with(':') {
            continue;
        }
        let indent = leading_indent(line);
        let mut next_idx = idx + 1;
        while next_idx < lines.len() {
            let next_line = lines[next_idx];
            let next_trimmed = next_line.trim();
            if next_trimmed.is_empty() || next_trimmed.starts_with('#') {
                next_idx += 1;
                continue;
            }
            if leading_indent(next_line) <= indent {
                return true;
            }
            break;
        }
    }
    false
}

#[cfg(feature = "stdlib_ast")]
fn codeop_parse_error_is_incomplete(error: &ParseErrorType, source: &str) -> bool {
    let trimmed = source.trim_end();
    let trailing_backslash_newline = source.ends_with("\\\n") || source.ends_with("\\\r\n");
    match error {
        ParseErrorType::Eof => !trailing_backslash_newline,
        ParseErrorType::UnrecognizedToken(_, expected) => expected.as_deref() == Some("Indent"),
        ParseErrorType::Lexical(lex) => {
            let text = lex.to_string();
            if text.contains("unexpected EOF") {
                return true;
            }
            if text.contains("line continuation") {
                return !trailing_backslash_newline;
            }
            if text.contains("unexpected string") {
                return true;
            }
            (text.contains("expected an indented block")
                || text.contains("unindent does not match any outer indentation level"))
                && trimmed.ends_with(':')
        }
        _ => false,
    }
}

#[cfg(feature = "stdlib_ast")]
enum CodeopCompileStatus {
    Compiled {
        next_flags: i64,
    },
    Incomplete,
    Error {
        error_type: &'static str,
        message: String,
    },
}

#[cfg(feature = "stdlib_ast")]
fn codeop_compile_status(
    source: &str,
    filename: &str,
    mode: &str,
    flags: i64,
    incomplete_input: bool,
) -> CodeopCompileStatus {
    let parse_mode = match mode {
        "exec" => ParseMode::Module,
        "eval" => ParseMode::Expression,
        "single" => ParseMode::Interactive,
        _ => {
            return CodeopCompileStatus::Error {
                error_type: "ValueError",
                message: "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            };
        }
    };
    match parse_python(source, parse_mode, filename) {
        Ok(parsed) => match compile_validate_nonlocal_semantics(&parsed) {
            Ok(()) => {
                if codeop_source_has_missing_indented_suite(source) {
                    return CodeopCompileStatus::Error {
                        error_type: "SyntaxError",
                        message: "expected an indented block".to_string(),
                    };
                }
                if incomplete_input && codeop_source_incomplete_after_success(source, mode, &parsed)
                {
                    return CodeopCompileStatus::Incomplete;
                }
                CodeopCompileStatus::Compiled {
                    next_flags: flags | codeop_future_flags_from_parsed(&parsed),
                }
            }
            Err(message) => CodeopCompileStatus::Error {
                error_type: "SyntaxError",
                message,
            },
        },
        Err(err) => {
            if incomplete_input && codeop_parse_error_is_incomplete(&err.error, source) {
                CodeopCompileStatus::Incomplete
            } else {
                CodeopCompileStatus::Error {
                    error_type: compile_error_type(&err.error),
                    message: err.error.to_string(),
                }
            }
        }
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeobj_from_filename_bits(_py: &crate::PyToken<'_>, filename_bits: u64) -> u64 {
    let name_ptr = alloc_string(_py, b"<module>");
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let varnames_ptr = alloc_tuple(_py, &[]);
    if varnames_ptr.is_null() {
        dec_ref_bits(_py, name_bits);
        return MoltObject::none().bits();
    }
    let varnames_bits = MoltObject::from_ptr(varnames_ptr).bits();
    let code_ptr = alloc_code_obj(
        _py,
        filename_bits,
        name_bits,
        1,
        MoltObject::none().bits(),
        varnames_bits,
        0,
        0,
        0,
    );
    dec_ref_bits(_py, varnames_bits);
    if code_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(code_ptr).bits()
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_bound_names_in_target(target: &pyast::Expr, out: &mut HashSet<String>) {
    match target {
        pyast::Expr::Name(node) => {
            out.insert(node.id.as_str().to_string());
        }
        pyast::Expr::Tuple(node) => {
            for elt in &node.elts {
                collect_bound_names_in_target(elt, out);
            }
        }
        pyast::Expr::List(node) => {
            for elt in &node.elts {
                collect_bound_names_in_target(elt, out);
            }
        }
        pyast::Expr::Starred(node) => {
            collect_bound_names_in_target(node.value.as_ref(), out);
        }
        _ => {}
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_import_binding(alias: &pyast::Alias, out: &mut HashSet<String>) {
    if let Some(asname) = alias.asname.as_ref() {
        out.insert(asname.as_str().to_string());
        return;
    }
    let raw = alias.name.as_str();
    let base = raw.split('.').next().unwrap_or(raw);
    if !base.is_empty() {
        out.insert(base.to_string());
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_arg_bindings(args: &pyast::Arguments, out: &mut HashSet<String>) {
    for arg in &args.posonlyargs {
        out.insert(arg.def.arg.as_str().to_string());
    }
    for arg in &args.args {
        out.insert(arg.def.arg.as_str().to_string());
    }
    if let Some(vararg) = args.vararg.as_ref() {
        out.insert(vararg.arg.as_str().to_string());
    }
    for arg in &args.kwonlyargs {
        out.insert(arg.def.arg.as_str().to_string());
    }
    if let Some(kwarg) = args.kwarg.as_ref() {
        out.insert(kwarg.arg.as_str().to_string());
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_function_scope_info(
    stmt: &pyast::Stmt,
    local_bindings: &mut HashSet<String>,
    nonlocal_decls: &mut HashSet<String>,
    global_decls: &mut HashSet<String>,
) {
    match stmt {
        pyast::Stmt::FunctionDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::AsyncFunctionDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::ClassDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::Global(node) => {
            for name in &node.names {
                global_decls.insert(name.as_str().to_string());
            }
        }
        pyast::Stmt::Nonlocal(node) => {
            for name in &node.names {
                nonlocal_decls.insert(name.as_str().to_string());
            }
        }
        pyast::Stmt::Assign(node) => {
            for target in &node.targets {
                collect_bound_names_in_target(target, local_bindings);
            }
        }
        pyast::Stmt::AnnAssign(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
        }
        pyast::Stmt::AugAssign(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
        }
        pyast::Stmt::For(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::AsyncFor(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::With(node) => {
            for item in &node.items {
                if let Some(target) = item.optional_vars.as_ref() {
                    collect_bound_names_in_target(target.as_ref(), local_bindings);
                }
            }
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::AsyncWith(node) => {
            for item in &node.items {
                if let Some(target) = item.optional_vars.as_ref() {
                    collect_bound_names_in_target(target.as_ref(), local_bindings);
                }
            }
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::If(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::While(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::Try(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                if let Some(name) = handler.name.as_ref() {
                    local_bindings.insert(name.as_str().to_string());
                }
                for child in &handler.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.finalbody {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::TryStar(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                if let Some(name) = handler.name.as_ref() {
                    local_bindings.insert(name.as_str().to_string());
                }
                for child in &handler.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.finalbody {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::Match(node) => {
            for case in &node.cases {
                for child in &case.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
        }
        pyast::Stmt::Import(node) => {
            for alias in &node.names {
                collect_import_binding(alias, local_bindings);
            }
        }
        pyast::Stmt::ImportFrom(node) => {
            for alias in &node.names {
                collect_import_binding(alias, local_bindings);
            }
        }
        _ => {}
    }
}

#[cfg(feature = "stdlib_ast")]
fn walk_nested_function_scopes(
    stmts: &[pyast::Stmt],
    enclosing_function_bindings: &[HashSet<String>],
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            pyast::Stmt::FunctionDef(node) => {
                validate_function_scope(&node.args, &node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncFunctionDef(node) => {
                validate_function_scope(&node.args, &node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::ClassDef(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::If(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::For(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncFor(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::While(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::With(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncWith(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::Try(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                for handler in &node.handlers {
                    let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                    walk_nested_function_scopes(&handler.body, enclosing_function_bindings)?;
                }
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.finalbody, enclosing_function_bindings)?;
            }
            pyast::Stmt::TryStar(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                for handler in &node.handlers {
                    let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                    walk_nested_function_scopes(&handler.body, enclosing_function_bindings)?;
                }
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.finalbody, enclosing_function_bindings)?;
            }
            pyast::Stmt::Match(node) => {
                for case in &node.cases {
                    walk_nested_function_scopes(&case.body, enclosing_function_bindings)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_control_flow_stmt(
    stmt: &pyast::Stmt,
    in_function: bool,
    in_loop: bool,
) -> Result<(), String> {
    fn validate_delete_target(target: &pyast::Expr) -> Result<(), String> {
        match target {
            pyast::Expr::Name(_) | pyast::Expr::Attribute(_) | pyast::Expr::Subscript(_) => Ok(()),
            pyast::Expr::Tuple(node) => {
                for elt in &node.elts {
                    validate_delete_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::List(node) => {
                for elt in &node.elts {
                    validate_delete_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::Constant(_) => Err("cannot delete literal".to_string()),
            _ => Err("cannot delete expression".to_string()),
        }
    }

    fn validate_assign_target(target: &pyast::Expr) -> Result<(), String> {
        match target {
            pyast::Expr::ListComp(_) => Err(
                "cannot assign to list comprehension here. Maybe you meant '==' instead of '='?"
                    .to_string(),
            ),
            pyast::Expr::Tuple(node) => {
                for elt in &node.elts {
                    validate_assign_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::List(node) => {
                for elt in &node.elts {
                    validate_assign_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::Starred(node) => validate_assign_target(node.value.as_ref()),
            _ => Ok(()),
        }
    }

    match stmt {
        pyast::Stmt::Return(_) => {
            if !in_function {
                return Err("'return' outside function".to_string());
            }
        }
        pyast::Stmt::Break(_) => {
            if !in_loop {
                return Err("'break' outside loop".to_string());
            }
        }
        pyast::Stmt::Continue(_) => {
            if !in_loop {
                return Err("'continue' not properly in loop".to_string());
            }
        }
        pyast::Stmt::Delete(node) => {
            for target in &node.targets {
                validate_delete_target(target)?;
            }
        }
        pyast::Stmt::Assign(node) => {
            for target in &node.targets {
                validate_assign_target(target)?;
            }
        }
        pyast::Stmt::AnnAssign(node) => {
            validate_assign_target(node.target.as_ref())?;
        }
        pyast::Stmt::AugAssign(node) => {
            validate_assign_target(node.target.as_ref())?;
        }
        pyast::Stmt::FunctionDef(node) => {
            validate_control_flow_stmts(&node.body, true, false)?;
            return Ok(());
        }
        pyast::Stmt::AsyncFunctionDef(node) => {
            validate_control_flow_stmts(&node.body, true, false)?;
            return Ok(());
        }
        pyast::Stmt::ClassDef(node) => {
            validate_control_flow_stmts(&node.body, false, false)?;
            return Ok(());
        }
        pyast::Stmt::If(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::For(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::AsyncFor(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::While(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::With(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::AsyncWith(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::Try(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                validate_control_flow_stmts(&handler.body, in_function, in_loop)?;
            }
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            validate_control_flow_stmts(&node.finalbody, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::TryStar(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                validate_control_flow_stmts(&handler.body, in_function, in_loop)?;
            }
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            validate_control_flow_stmts(&node.finalbody, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::Match(node) => {
            for case in &node.cases {
                validate_control_flow_stmts(&case.body, in_function, in_loop)?;
            }
            return Ok(());
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_control_flow_stmts(
    stmts: &[pyast::Stmt],
    in_function: bool,
    in_loop: bool,
) -> Result<(), String> {
    for stmt in stmts {
        validate_control_flow_stmt(stmt, in_function, in_loop)?;
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_function_scope(
    args: &pyast::Arguments,
    body: &[pyast::Stmt],
    enclosing_function_bindings: &[HashSet<String>],
) -> Result<(), String> {
    let mut local_bindings: HashSet<String> = HashSet::new();
    collect_arg_bindings(args, &mut local_bindings);
    let mut nonlocal_decls: HashSet<String> = HashSet::new();
    let mut global_decls: HashSet<String> = HashSet::new();
    for stmt in body {
        collect_function_scope_info(
            stmt,
            &mut local_bindings,
            &mut nonlocal_decls,
            &mut global_decls,
        );
    }
    for name in &nonlocal_decls {
        if global_decls.contains(name) {
            return Err(format!("name '{name}' is nonlocal and global"));
        }
        let mut found = false;
        for scope in enclosing_function_bindings.iter().rev() {
            if scope.contains(name) {
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!("no binding for nonlocal '{name}' found"));
        }
    }
    for name in &nonlocal_decls {
        local_bindings.remove(name);
    }
    for name in &global_decls {
        local_bindings.remove(name);
    }
    let mut next_enclosing = enclosing_function_bindings.to_vec();
    next_enclosing.push(local_bindings);
    walk_nested_function_scopes(body, &next_enclosing)
}

#[cfg(feature = "stdlib_ast")]
fn compile_validate_nonlocal_semantics(parsed: &pyast::Mod) -> Result<(), String> {
    match parsed {
        pyast::Mod::Module(module) => {
            validate_control_flow_stmts(&module.body, false, false)?;
            walk_nested_function_scopes(&module.body, &[])
        }
        pyast::Mod::Interactive(module) => {
            validate_control_flow_stmts(&module.body, false, false)?;
            walk_nested_function_scopes(&module.body, &[])
        }
        _ => Ok(()),
    }
}

#[cfg(feature = "stdlib_ast")]
fn compile_validate_source(
    source: &str,
    filename: &str,
    mode: &str,
) -> Result<(), (&'static str, String)> {
    let parse_mode = match mode {
        "exec" => ParseMode::Module,
        "eval" => ParseMode::Expression,
        "single" => ParseMode::Interactive,
        _ => {
            return Err((
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            ));
        }
    };
    match parse_python(source, parse_mode, filename) {
        Ok(parsed) => match compile_validate_nonlocal_semantics(&parsed) {
            Ok(()) => Ok(()),
            Err(message) => Err(("SyntaxError", message)),
        },
        Err(err) => Err((compile_error_type(&err.error), err.error.to_string())),
    }
}
// --- Stubs when stdlib_ast is disabled ---

#[cfg(not(feature = "stdlib_ast"))]
#[cfg(not(feature = "stdlib_ast"))]
#[cfg(not(feature = "stdlib_ast"))]
/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let slot = self_ptr.add(offset as usize) as *mut u64;
            let bits = *slot;
            inc_ref_bits(_py, bits);
            bits
        })
    }
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let slot = self_ptr.add(offset as usize) as *mut u64;
            let old_bits = *slot;
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, bits);
            *slot = bits;
            MoltObject::none().bits()
        })
    }
}

fn logging_percent_lookup_mapping_value(
    _py: &crate::PyToken<'_>,
    mapping_ptr: *mut u8,
    key: &str,
) -> Option<u64> {
    let key_ptr = alloc_string(_py, key.as_bytes());
    if key_ptr.is_null() {
        return None;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let value = unsafe { dict_get_in_place(_py, mapping_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    value
}

fn logging_percent_render_str(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    let rendered_bits = crate::molt_str_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_repr(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    let rendered_bits = crate::molt_repr_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_value(
    _py: &crate::PyToken<'_>,
    spec: char,
    value_bits: u64,
) -> Option<String> {
    match spec {
        'd' => {
            if let Some(value) = to_i64(obj_from_bits(value_bits)) {
                return Some(value.to_string());
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            logging_percent_render_str(_py, value_bits)
        }
        'f' => {
            if let Some(value) = to_f64(obj_from_bits(value_bits)) {
                return Some(format!("{value:.6}"));
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            logging_percent_render_str(_py, value_bits)
        }
        'r' => logging_percent_render_repr(_py, value_bits),
        _ => logging_percent_render_str(_py, value_bits),
    }
}

fn logging_config_dict_lookup(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
    key: &str,
) -> Result<Option<u64>, u64> {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config object must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config object must be dict",
        ));
    }
    let Some(key_bits) = alloc_string_bits(_py, key) else {
        return Err(MoltObject::none().bits());
    };
    let value = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value)
}

fn logging_config_dict_items(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
) -> Result<Vec<(u64, u64)>, u64> {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(dict_bits, items_name_bits, missing);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section missing items()",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, iterable_bits) };
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() must produce an iterable of pairs",
        ));
    };
    if unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() iterable materialization failed",
        ));
    }
    let entries: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    for item_bits in entries {
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        };
        if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be key/value pairs",
            ));
        }
        let key_bits = fields[0];
        let value_bits = fields[1];
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, value_bits);
        pairs.push((key_bits, value_bits));
    }
    dec_ref_bits(_py, list_bits);
    Ok(pairs)
}

fn logging_config_name_list(_py: &crate::PyToken<'_>, seq_bits: u64) -> Result<Vec<String>, u64> {
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, seq_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list must be iterable",
        ));
    };
    if unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list materialization failed",
        ));
    }
    let entries: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    let mut names: Vec<String> = Vec::new();
    for item_bits in entries {
        let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
            dec_ref_bits(_py, list_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config handler references must be strings",
            ));
        };
        names.push(name);
    }
    dec_ref_bits(_py, list_bits);
    Ok(names)
}

fn logging_config_call_method1(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    method_name: &[u8],
    arg_bits: u64,
) -> Result<u64, u64> {
    let Some(method_bits) = urllib_request_attr_optional(_py, obj_bits, method_name)? else {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "logging object method is missing",
        ));
    };
    let out_bits = unsafe { call_callable1(_py, method_bits, arg_bits) };
    dec_ref_bits(_py, method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out_bits)
}

fn logging_config_clear_logger_handlers(
    _py: &crate::PyToken<'_>,
    logger_bits: u64,
) -> Result<(), u64> {
    let Some(handlers_bits) = urllib_request_attr_optional(_py, logger_bits, b"handlers")? else {
        return Ok(());
    };
    let Some(handlers_ptr) = obj_from_bits(handlers_bits).as_ptr() else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    let ty = unsafe { object_type_id(handlers_ptr) };
    let snapshot: Vec<u64> = if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
        unsafe { seq_vec_ref(handlers_ptr).to_vec() }
    } else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    dec_ref_bits(_py, handlers_bits);
    for handler_bits in snapshot {
        let out_bits =
            logging_config_call_method1(_py, logger_bits, b"removeHandler", handler_bits)?;
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
    }
    Ok(())
}

fn logging_config_resolve_ext_stream(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
) -> Result<u64, u64> {
    let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
        return Ok(value_bits);
    };
    if text == "ext://sys.stdout" {
        return pickle_resolve_global_bits(_py, "sys", "stdout");
    }
    if text == "ext://sys.stderr" {
        return pickle_resolve_global_bits(_py, "sys", "stderr");
    }
    if text == "ext://sys.stdin" {
        return pickle_resolve_global_bits(_py, "sys", "stdin");
    }
    Err(raise_exception::<u64>(
        _py,
        "ValueError",
        "unsupported logging stream ext target",
    ))
}

    defaults_bits: u64,
    disable_existing_loggers_bits: u64,
    encoding_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = (
            config_file_bits,
            defaults_bits,
            disable_existing_loggers_bits,
            encoding_bits,
        );
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "logging.config.fileConfig is not implemented in Molt yet",
        )
    })
}

fn imghdr_detect_kind(header: &[u8]) -> Option<&'static str> {
    if header.len() >= 10 && (header[6..10] == *b"JFIF" || header[6..10] == *b"Exif")
        || header.starts_with(b"\xFF\xD8\xFF\xDB")
    {
        return Some("jpeg");
    }
    if header.starts_with(b"\x89PNG\r\n\x1A\n") {
        return Some("png");
    }
    if header.len() >= 6 && (header[..6] == *b"GIF87a" || header[..6] == *b"GIF89a") {
        return Some("gif");
    }
    if header.len() >= 2 && (header[..2] == *b"MM" || header[..2] == *b"II") {
        return Some("tiff");
    }
    if header.starts_with(b"\x01\xDA") {
        return Some("rgb");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'1' | b'4')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("pbm");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'2' | b'5')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("pgm");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'3' | b'6')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("ppm");
    }
    if header.starts_with(b"\x59\xA6\x6A\x95") {
        return Some("rast");
    }
    if header.starts_with(b"#define ") {
        return Some("xbm");
    }
    if header.starts_with(b"BM") {
        return Some("bmp");
    }
    if header.starts_with(b"RIFF") && header.len() >= 12 && header[8..12] == *b"WEBP" {
        return Some("webp");
    }
    if header.starts_with(b"\x76\x2f\x31\x01") {
        return Some("exr");
    }
    None
}

const ZIPFILE_CENTRAL_SIG: [u8; 4] = *b"PK\x01\x02";
const ZIPFILE_EOCD_SIG: [u8; 4] = *b"PK\x05\x06";
const ZIPFILE_ZIP64_EOCD_SIG: [u8; 4] = *b"PK\x06\x06";
const ZIPFILE_ZIP64_LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";
const ZIPFILE_ZIP64_LIMIT: u64 = 0xFFFF_FFFF;
const ZIPFILE_ZIP64_COUNT_LIMIT: u16 = 0xFFFF;
const ZIPFILE_ZIP64_EXTRA_ID: u16 = 0x0001;

fn zipfile_read_u16_le(data: &[u8], offset: usize, err: &'static str) -> Result<u16, &'static str> {
    let end = offset.checked_add(2).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn zipfile_read_u32_le(data: &[u8], offset: usize, err: &'static str) -> Result<u32, &'static str> {
    let end = offset.checked_add(4).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn zipfile_read_u64_le(data: &[u8], offset: usize, err: &'static str) -> Result<u64, &'static str> {
    let end = offset.checked_add(8).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn zipfile_find_eocd_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let start = data.len().saturating_sub(22 + 65_535);
    data[start..]
        .windows(4)
        .rposition(|window| window == ZIPFILE_EOCD_SIG)
        .map(|idx| start + idx)
}

fn zipfile_read_zip64_eocd(
    data: &[u8],
    eocd_offset: usize,
) -> Result<(usize, usize), &'static str> {
    let locator_offset = eocd_offset.checked_sub(20).ok_or("zip64 locator missing")?;
    let locator_sig = data
        .get(locator_offset..locator_offset + 4)
        .ok_or("zip64 locator missing")?;
    if locator_sig != ZIPFILE_ZIP64_LOCATOR_SIG {
        return Err("zip64 locator missing");
    }
    let zip64_eocd_offset = zipfile_read_u64_le(data, locator_offset + 8, "zip64 locator missing")?;
    let zip64_eocd_offset =
        usize::try_from(zip64_eocd_offset).map_err(|_| "zip64 eocd offset overflow")?;
    let eocd_sig = data
        .get(zip64_eocd_offset..zip64_eocd_offset + 4)
        .ok_or("zip64 eocd missing")?;
    if eocd_sig != ZIPFILE_ZIP64_EOCD_SIG {
        return Err("zip64 eocd missing");
    }
    let cd_size = zipfile_read_u64_le(data, zip64_eocd_offset + 40, "zip64 eocd missing")?;
    let cd_offset = zipfile_read_u64_le(data, zip64_eocd_offset + 48, "zip64 eocd missing")?;
    let cd_size = usize::try_from(cd_size).map_err(|_| "zip64 central directory too large")?;
    let cd_offset =
        usize::try_from(cd_offset).map_err(|_| "zip64 central directory offset too large")?;
    Ok((cd_offset, cd_size))
}

fn zipfile_parse_zip64_extra(
    extra: &[u8],
    mut comp_size: u64,
    mut uncomp_size: u64,
    mut local_offset: u64,
) -> Result<(u64, u64, u64), &'static str> {
    let mut pos = 0usize;
    while pos + 4 <= extra.len() {
        let header_id = zipfile_read_u16_le(extra, pos, "zip64 extra missing")?;
        let data_size = usize::from(zipfile_read_u16_le(extra, pos + 2, "zip64 extra missing")?);
        pos += 4;
        let Some(data_end) = pos.checked_add(data_size) else {
            return Err("zip64 extra missing");
        };
        if data_end > extra.len() {
            break;
        }
        if header_id == ZIPFILE_ZIP64_EXTRA_ID {
            let mut cursor = pos;
            if uncomp_size == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing size");
                }
                uncomp_size = zipfile_read_u64_le(extra, cursor, "zip64 extra missing size")?;
                cursor += 8;
            }
            if comp_size == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing comp size");
                }
                comp_size = zipfile_read_u64_le(extra, cursor, "zip64 extra missing comp size")?;
                cursor += 8;
            }
            if local_offset == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing offset");
                }
                local_offset = zipfile_read_u64_le(extra, cursor, "zip64 extra missing offset")?;
            }
            return Ok((comp_size, uncomp_size, local_offset));
        }
        pos = data_end;
    }
    Err("zip64 extra missing")
}

fn zipfile_parse_central_directory_impl(
    data: &[u8],
) -> Result<Vec<(String, [u64; 5])>, &'static str> {
    if data.len() < 22 {
        return Err("file is not a zip file");
    }
    let Some(eocd_offset) = zipfile_find_eocd_offset(data) else {
        return Err("end of central directory not found");
    };

    let mut cd_size = u64::from(zipfile_read_u32_le(
        data,
        eocd_offset + 12,
        "end of central directory not found",
    )?);
    let mut cd_offset = u64::from(zipfile_read_u32_le(
        data,
        eocd_offset + 16,
        "end of central directory not found",
    )?);
    let total_entries =
        zipfile_read_u16_le(data, eocd_offset + 10, "end of central directory not found")?;
    if total_entries == ZIPFILE_ZIP64_COUNT_LIMIT
        || cd_size == ZIPFILE_ZIP64_LIMIT
        || cd_offset == ZIPFILE_ZIP64_LIMIT
    {
        let (zip64_offset, zip64_size) = zipfile_read_zip64_eocd(data, eocd_offset)?;
        cd_offset = zip64_offset as u64;
        cd_size = zip64_size as u64;
    }

    let pos_start = usize::try_from(cd_offset).map_err(|_| "central directory offset overflow")?;
    let cd_size = usize::try_from(cd_size).map_err(|_| "central directory size overflow")?;
    let Some(end) = pos_start.checked_add(cd_size) else {
        return Err("central directory overflow");
    };
    if end > data.len() {
        return Err("end of central directory not found");
    }

    let mut out: Vec<(String, [u64; 5])> = Vec::new();
    let mut pos = pos_start;
    while pos + 46 <= end {
        if data[pos..pos + 4] != ZIPFILE_CENTRAL_SIG {
            break;
        }
        let comp_method = u64::from(zipfile_read_u16_le(
            data,
            pos + 10,
            "invalid central directory entry",
        )?);
        let mut comp_size = u64::from(zipfile_read_u32_le(
            data,
            pos + 20,
            "invalid central directory entry",
        )?);
        let mut uncomp_size = u64::from(zipfile_read_u32_le(
            data,
            pos + 24,
            "invalid central directory entry",
        )?);
        let name_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 28,
            "invalid central directory entry",
        )?);
        let extra_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 30,
            "invalid central directory entry",
        )?);
        let comment_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 32,
            "invalid central directory entry",
        )?);
        let mut local_offset = u64::from(zipfile_read_u32_le(
            data,
            pos + 42,
            "invalid central directory entry",
        )?);

        let name_start = pos + 46;
        let Some(name_end) = name_start.checked_add(name_len) else {
            return Err("invalid central directory entry");
        };
        let Some(extra_end) = name_end.checked_add(extra_len) else {
            return Err("invalid central directory entry");
        };
        let Some(record_end) = extra_end.checked_add(comment_len) else {
            return Err("invalid central directory entry");
        };
        if record_end > end || record_end > data.len() {
            return Err("invalid central directory entry");
        }

        let name_bytes = &data[name_start..name_end];
        let name = match std::str::from_utf8(name_bytes) {
            Ok(value) => value.to_string(),
            Err(_) => String::from_utf8_lossy(name_bytes).into_owned(),
        };

        if comp_size == ZIPFILE_ZIP64_LIMIT
            || uncomp_size == ZIPFILE_ZIP64_LIMIT
            || local_offset == ZIPFILE_ZIP64_LIMIT
        {
            let extra = &data[name_end..extra_end];
            let (parsed_comp, parsed_uncomp, parsed_offset) =
                zipfile_parse_zip64_extra(extra, comp_size, uncomp_size, local_offset)?;
            comp_size = parsed_comp;
            uncomp_size = parsed_uncomp;
            local_offset = parsed_offset;
        }

        out.push((
            name,
            [
                local_offset,
                comp_size,
                comp_method,
                name_len as u64,
                uncomp_size,
            ],
        ));
        pos = record_end;
    }
    Ok(out)
}

fn zipfile_build_zip64_extra_impl(size: u64, comp_size: u64, offset: Option<u64>) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::with_capacity(if offset.is_some() { 24 } else { 16 });
    data.extend_from_slice(size.to_le_bytes().as_slice());
    data.extend_from_slice(comp_size.to_le_bytes().as_slice());
    if let Some(offset) = offset {
        data.extend_from_slice(offset.to_le_bytes().as_slice());
    }
    let mut out: Vec<u8> = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(ZIPFILE_ZIP64_EXTRA_ID.to_le_bytes().as_slice());
    out.extend_from_slice((data.len() as u16).to_le_bytes().as_slice());
    out.extend_from_slice(data.as_slice());
    out
}

fn zipfile_trim_trailing_slashes(path: &str) -> &str {
    let mut end = path.len();
    while end > 0 && path.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

fn zipfile_ancestry(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = zipfile_trim_trailing_slashes(path).to_string();
    while !zipfile_trim_trailing_slashes(current.as_str()).is_empty() {
        out.push(current.clone());
        if let Some(idx) = current.rfind('/') {
            current.truncate(idx);
            while current.ends_with('/') {
                current.pop();
            }
        } else {
            current.clear();
        }
    }
    out
}

fn zipfile_parent_of(path: &str) -> &str {
    let trimmed = zipfile_trim_trailing_slashes(path);
    if let Some(idx) = trimmed.rfind('/') {
        &trimmed[..idx]
    } else {
        ""
    }
}

fn zipfile_escape_regex_char(ch: char, out: &mut String) {
    if matches!(
        ch,
        '.' | '^' | '$' | '*' | '+' | '?' | '{' | '}' | '[' | ']' | '\\' | '|' | '(' | ')'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn zipfile_escape_character_class(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(ch, '\\' | ']' | '^' | '-') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn zipfile_separate_pattern(pattern: &str) -> Vec<(bool, String)> {
    let bytes = pattern.as_bytes();
    let mut idx = 0usize;
    let mut out: Vec<(bool, String)> = Vec::new();
    while idx < bytes.len() {
        if bytes[idx] != b'[' {
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'[' {
                idx += 1;
            }
            out.push((false, pattern[start..idx].to_string()));
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < bytes.len() && bytes[idx] != b']' {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b']' {
            idx += 1;
            out.push((true, pattern[start..idx].to_string()));
            continue;
        }
        out.push((false, pattern[start..].to_string()));
        break;
    }
    out
}

fn zipfile_translate_chunk(chunk: &str, star_pattern: &str, qmark_pattern: &str) -> String {
    let chars: Vec<char> = chunk.chars().collect();
    let mut idx = 0usize;
    let mut out = String::new();
    while idx < chars.len() {
        match chars[idx] {
            '*' => {
                if idx + 1 < chars.len() && chars[idx + 1] == '*' {
                    out.push_str(".*");
                    idx += 2;
                } else {
                    out.push_str(star_pattern);
                    idx += 1;
                }
            }
            '?' => {
                out.push_str(qmark_pattern);
                idx += 1;
            }
            ch => {
                zipfile_escape_regex_char(ch, &mut out);
                idx += 1;
            }
        }
    }
    out
}

fn zipfile_star_not_empty(pattern: &str, seps: &str) -> String {
    let mut out = String::new();
    let mut segment = String::new();
    for ch in pattern.chars() {
        if seps.contains(ch) {
            if !segment.is_empty() {
                if segment == "*" {
                    out.push_str("?*");
                } else {
                    out.push_str(segment.as_str());
                }
                segment.clear();
            }
            out.push(ch);
        } else {
            segment.push(ch);
        }
    }
    if !segment.is_empty() {
        if segment == "*" {
            out.push_str("?*");
        } else {
            out.push_str(segment.as_str());
        }
    }
    out
}

fn zipfile_contains_invalid_rglob_segment(pattern: &str, seps: &str) -> bool {
    let mut segment = String::new();
    for ch in pattern.chars() {
        if seps.contains(ch) {
            if segment.contains("**") && segment != "**" {
                return true;
            }
            segment.clear();
        } else {
            segment.push(ch);
        }
    }
    segment.contains("**") && segment != "**"
}

fn zipfile_translate_glob_impl(
    pattern: &str,
    seps: &str,
    py313_plus: bool,
) -> Result<String, &'static str> {
    let mut effective_pattern = pattern.to_string();
    let mut star_pattern = "[^/]*".to_string();
    let mut qmark_pattern = ".".to_string();
    if py313_plus {
        if zipfile_contains_invalid_rglob_segment(pattern, seps) {
            return Err("** must appear alone in a path segment");
        }
        effective_pattern = zipfile_star_not_empty(pattern, seps);
        let escaped = zipfile_escape_character_class(seps);
        star_pattern = format!("[^{escaped}]*");
        qmark_pattern = "[^/]".to_string();
    }

    let mut core = String::new();
    for (is_set, chunk) in zipfile_separate_pattern(effective_pattern.as_str()) {
        if is_set {
            core.push_str(chunk.as_str());
            continue;
        }
        core.push_str(
            zipfile_translate_chunk(
                chunk.as_str(),
                star_pattern.as_str(),
                qmark_pattern.as_str(),
            )
            .as_str(),
        );
    }

    let with_dirs = format!("{core}[/]?");
    if py313_plus {
        Ok(format!("(?s:{with_dirs})\\Z"))
    } else {
        Ok(with_dirs)
    }
}

fn zipfile_normalize_member_path_impl(member: &str) -> Option<String> {
    let replaced = member.replace('\\', "/");
    let mut stack: Vec<String> = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if let Some(last) = stack.last()
                && last != ".."
            {
                stack.pop();
                continue;
            }
            stack.push("..".to_string());
            continue;
        }
        stack.push(segment.to_string());
    }
    let normalized = stack.join("/");
    if normalized.is_empty() || normalized == "." {
        return None;
    }
    if normalized == ".." || normalized.starts_with("../") {
        return None;
    }
    Some(normalized)
}
#[cfg(test)]
mod zipfile_path_lowering_tests {
    use super::{
        zipfile_ancestry, zipfile_build_zip64_extra_impl, zipfile_normalize_member_path_impl,
        zipfile_parse_central_directory_impl, zipfile_translate_glob_impl,
    };

    #[test]
    fn zipfile_ancestry_preserves_posix_structure() {
        assert_eq!(
            zipfile_ancestry("//b//d///f//"),
            vec![
                String::from("//b//d///f"),
                String::from("//b//d"),
                String::from("//b"),
            ]
        );
    }

    #[test]
    fn zipfile_translate_glob_legacy_matches_shape() {
        let translated =
            zipfile_translate_glob_impl("*.txt", "/", false).expect("legacy translation");
        assert_eq!(translated, String::from("[^/]*\\.txt[/]?"));
    }

    #[test]
    fn zipfile_translate_glob_modern_matches_shape() {
        let translated =
            zipfile_translate_glob_impl("**/*", "/", true).expect("modern translation");
        assert_eq!(translated, String::from("(?s:.*/[^/][^/]*[/]?)\\Z"));
    }

    #[test]
    fn zipfile_translate_glob_rejects_invalid_rglob_segment() {
        let err = zipfile_translate_glob_impl("**foo", "/", true).expect_err("invalid segment");
        assert_eq!(err, "** must appear alone in a path segment");
    }

    #[test]
    fn zipfile_normalize_member_path_blocks_traversal() {
        assert_eq!(
            zipfile_normalize_member_path_impl("safe/../leaf.txt"),
            Some(String::from("leaf.txt"))
        );
        assert_eq!(zipfile_normalize_member_path_impl("../escape.txt"), None);
        assert_eq!(zipfile_normalize_member_path_impl("./"), None);
    }

    #[test]
    fn zipfile_parse_central_directory_roundtrip_shape() {
        let name = b"a.txt";
        let payload = b"hello";
        let name_len = name.len() as u16;
        let payload_len = payload.len() as u32;

        let mut archive: Vec<u8> = Vec::new();
        archive.extend_from_slice(b"PK\x03\x04");
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(name_len.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(name);
        archive.extend_from_slice(payload);

        let cd_offset = archive.len() as u32;
        archive.extend_from_slice(b"PK\x01\x02");
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(name_len.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(name);

        let cd_size = (46 + name.len()) as u32;
        archive.extend_from_slice(b"PK\x05\x06");
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(1u16.to_le_bytes().as_slice());
        archive.extend_from_slice(1u16.to_le_bytes().as_slice());
        archive.extend_from_slice(cd_size.to_le_bytes().as_slice());
        archive.extend_from_slice(cd_offset.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());

        let parsed = zipfile_parse_central_directory_impl(archive.as_slice())
            .expect("central directory parse should succeed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, String::from("a.txt"));
        assert_eq!(
            parsed[0].1,
            [
                0,
                payload.len() as u64,
                0,
                name.len() as u64,
                payload.len() as u64
            ]
        );
    }

    #[test]
    fn zipfile_build_zip64_extra_shape() {
        let out = zipfile_build_zip64_extra_impl(7, 11, Some(13));
        assert_eq!(
            out,
            vec![
                0x01, 0x00, 24, 0, 7, 0, 0, 0, 0, 0, 0, 0, 11, 0, 0, 0, 0, 0, 0, 0, 13, 0, 0, 0, 0,
                0, 0, 0,
            ]
        );
    }
}

#[cfg(test)]
mod tokenize_encoding_tests {
    use super::{find_encoding_cookie, skip_encoding_ws};

    #[test]
    fn skip_encoding_ws_trims_python_prefix_whitespace() {
        assert_eq!(skip_encoding_ws(b" \t\x0ccoding"), b"coding");
    }

    #[test]
    fn find_encoding_cookie_handles_standard_cookie() {
        assert_eq!(find_encoding_cookie(b"# coding: utf-8"), Some("utf-8"));
        assert_eq!(
            find_encoding_cookie(b"# -*- coding: latin-1 -*-"),
            Some("latin-1")
        );
    }

    #[test]
    fn find_encoding_cookie_rejects_non_cookie_lines() {
        assert_eq!(find_encoding_cookie(b"print('hi')"), None);
        assert_eq!(find_encoding_cookie(b"# comment only"), None);
    }
}

/// Tokenize a UTF-8 source string into a list of (type, string, start, end, line) tuples.
/// Token types: 0=ENDMARKER, 1=NAME, 2=NUMBER, 4=NEWLINE, 54=OP, 64=COMMENT, 65=NL, 67=ENCODING
fn make_token_tuple(
    _py: &crate::PyToken<'_>,
    tok_type: i64,
    string: &str,
    start: (i64, i64),
    end: (i64, i64),
    line_bits: u64,
) -> u64 {
    let type_bits = MoltObject::from_int(tok_type).bits();
    let string_ptr = crate::alloc_string(_py, string.as_bytes());
    let string_bits = if string_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(string_ptr).bits()
    };
    let start_elems = [
        MoltObject::from_int(start.0).bits(),
        MoltObject::from_int(start.1).bits(),
    ];
    let start_ptr = crate::alloc_tuple(_py, &start_elems);
    let start_bits = if start_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(start_ptr).bits()
    };
    let end_elems = [
        MoltObject::from_int(end.0).bits(),
        MoltObject::from_int(end.1).bits(),
    ];
    let end_ptr = crate::alloc_tuple(_py, &end_elems);
    let end_bits = if end_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(end_ptr).bits()
    };
    let elems = [type_bits, string_bits, start_bits, end_bits, line_bits];
    let tuple_ptr = crate::alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn skip_encoding_ws(bytes: &[u8]) -> &[u8] {
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b' ' | b'\t' | b'\x0c' => idx += 1,
            _ => break,
        }
    }
    &bytes[idx..]
}

fn find_encoding_cookie(line: &[u8]) -> Option<&str> {
    let stripped = skip_encoding_ws(line);
    if !stripped.starts_with(b"#") {
        return None;
    }
    let coding_idx = memmem::find(stripped, b"coding")?;
    let mut rest = &stripped[coding_idx + "coding".len()..];
    rest = skip_encoding_ws(rest);
    let (sep, rest) = rest.split_first()?;
    if *sep != b':' && *sep != b'=' {
        return None;
    }
    let rest = skip_encoding_ws(rest);
    if rest.is_empty() {
        return None;
    }
    let end = rest
        .iter()
        .position(|&b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&rest[..end]).ok()
}

/// Detect Python source file encoding from the first two lines.
/// `first_bits`: first line bytes, `second_bits`: second line bytes
/// Returns (encoding_name, has_bom) tuple.
