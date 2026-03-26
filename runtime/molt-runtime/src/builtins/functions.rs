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

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_dedent(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let result = textwrap_dedent_impl(&text);
        let out_ptr = alloc_string(_py, result.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_shorten(
    text_bits: u64,
    width_bits: u64,
    placeholder_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let placeholder = if obj_from_bits(placeholder_bits).is_none() {
            " [...]".to_string()
        } else {
            string_obj_to_owned(obj_from_bits(placeholder_bits))
                .unwrap_or_else(|| " [...]".to_string())
        };
        // Collapse whitespace and truncate
        let collapsed: String = text.split_whitespace().collect::<Vec<&str>>().join(" ");
        if (collapsed.len() as i64) <= width {
            let out_ptr = alloc_string(_py, collapsed.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        let ph_len = placeholder.len() as i64;
        let max_text = width - ph_len;
        if max_text < 0 {
            let out_ptr = alloc_string(_py, placeholder.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        // Find last space before max_text
        let mut truncate_at = max_text as usize;
        if truncate_at < collapsed.len() {
            // Find last space at or before truncate_at
            if let Some(pos) = collapsed[..truncate_at].rfind(' ') {
                truncate_at = pos;
            }
        }
        let result = format!("{}{}", &collapsed[..truncate_at].trim_end(), placeholder);
        let out_ptr = alloc_string(_py, result.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

// ─── logging filter intrinsics ──────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_filter_check(filter_name_bits: u64, record_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let filter_name = string_obj_to_owned(obj_from_bits(filter_name_bits)).unwrap_or_default();
        let record_name = string_obj_to_owned(obj_from_bits(record_name_bits)).unwrap_or_default();
        let result = filter_name.is_empty()
            || record_name == filter_name
            || record_name.starts_with(&format!("{}.", filter_name));
        MoltObject::from_int(if result { 1 } else { 0 }).bits()
    })
}

// ─── logging file handler intrinsics ────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_file_handler_emit(
    msg_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    encoding_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(msg) = string_obj_to_owned(obj_from_bits(msg_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "msg must be str");
        };
        let Some(filename) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "filename must be str");
        };
        let mode = string_obj_to_owned(obj_from_bits(mode_bits)).unwrap_or_else(|| "a".to_string());
        let _encoding = string_obj_to_owned(obj_from_bits(encoding_bits));

        use std::fs::OpenOptions;
        use std::io::Write;
        let open_result = if mode.contains('w') {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&filename)
        } else {
            OpenOptions::new().append(true).create(true).open(&filename)
        };
        match open_result {
            Ok(mut f) => {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.write_all(b"\n");
            }
            Err(e) => {
                return raise_exception::<_>(
                    _py,
                    "IOError",
                    &format!("cannot open {}: {}", filename, e),
                );
            }
        }
        MoltObject::none().bits()
    })
}

// ─── copy.replace intrinsic ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_copy_replace(obj_bits: u64, changes_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // copy.replace creates a modified shallow copy.
        // For Molt's supported types, apply changes dict on top of a shallow copy.
        let _ = changes_bits; // changes are applied Python-side
        crate::builtins::copy_mod::molt_copy_copy(obj_bits)
    })
}

// ─── pprint format/isreadable/isrecursive with context ──────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_pprint_format_object(
    obj_bits: u64,
    max_depth_bits: u64,
    level_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        use std::collections::HashSet;
        let max_depth = crate::builtins::pprint_ext::i64_from_bits_default(max_depth_bits, -1);
        let level = crate::builtins::pprint_ext::i64_from_bits_default(level_bits, 0);
        let mut seen = HashSet::new();
        let (repr, readable, recursive) = crate::builtins::pprint_ext::safe_repr_inner(
            _py, obj_bits, &mut seen, level, max_depth, -1,
        );
        // Return a tuple (repr_str, readable_bool, recursive_bool)
        let repr_ptr = alloc_string(_py, repr.as_bytes());
        if repr_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let repr_bits = MoltObject::from_ptr(repr_ptr).bits();
        let readable_bits = MoltObject::from_int(if readable { 1 } else { 0 }).bits();
        let recursive_bits = MoltObject::from_int(if recursive { 1 } else { 0 }).bits();
        let tup_ptr = crate::alloc_tuple(_py, &[repr_bits, readable_bits, recursive_bits]);
        if tup_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tup_ptr).bits()
    })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let id = email_message_register(email_message_default());
        email_message_bits_from_id(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_from_bytes(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_message_get(message_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_quote(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.quote argument must be str");
        };
        let out = shlex_quote_impl(&text);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_split(text_bits: u64, whitespace_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split argument must be str");
        };
        let Some(whitespace) = string_obj_to_owned(obj_from_bits(whitespace_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split whitespace must be str");
        };
        let parts = match shlex_split_impl(&text, &whitespace, true, false, "#", true, "") {
            Ok(parts) => parts,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &parts)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_split_ex(
    text_bits: u64,
    whitespace_bits: u64,
    posix_bits: u64,
    comments_bits: u64,
    whitespace_split_bits: u64,
    commenters_bits: u64,
    punctuation_chars_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split argument must be str");
        };
        let Some(whitespace) = string_obj_to_owned(obj_from_bits(whitespace_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split whitespace must be str");
        };
        let Some(commenters) = string_obj_to_owned(obj_from_bits(commenters_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "shlex.split commenters must be str");
        };
        let Some(punctuation_chars) = string_obj_to_owned(obj_from_bits(punctuation_chars_bits))
        else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "shlex.split punctuation_chars must be str",
            );
        };
        let posix = is_truthy(_py, obj_from_bits(posix_bits));
        let comments = is_truthy(_py, obj_from_bits(comments_bits));
        let whitespace_split = is_truthy(_py, obj_from_bits(whitespace_split_bits));
        let parts = match shlex_split_impl(
            &text,
            &whitespace,
            posix,
            comments,
            &commenters,
            whitespace_split,
            &punctuation_chars,
        ) {
            Ok(parts) => parts,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &parts)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shlex_join(words_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match iterable_to_string_vec(_py, words_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let out = shlex_join_impl(&parts);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_this_payload() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(s_bits) = alloc_string_bits(_py, THIS_ENCODED) else {
            return MoltObject::none().bits();
        };

        let mut pairs: Vec<u64> = Vec::with_capacity(52 * 2);
        let mut owned_pairs: Vec<u64> = Vec::with_capacity(52 * 2);
        for base in [b'A', b'a'] {
            for idx in 0u8..26u8 {
                let key = [(base + idx) as char];
                let value = [(base + ((idx + 13) % 26)) as char];
                let key_text: String = key.into_iter().collect();
                let value_text: String = value.into_iter().collect();
                let Some(key_bits) = alloc_string_bits(_py, &key_text) else {
                    dec_ref_bits(_py, s_bits);
                    for bits in owned_pairs {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                let Some(value_bits) = alloc_string_bits(_py, &value_text) else {
                    dec_ref_bits(_py, s_bits);
                    dec_ref_bits(_py, key_bits);
                    for bits in owned_pairs {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                pairs.push(key_bits);
                pairs.push(value_bits);
                owned_pairs.push(key_bits);
                owned_pairs.push(value_bits);
            }
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            dec_ref_bits(_py, s_bits);
            for bits in owned_pairs {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_pairs {
            dec_ref_bits(_py, bits);
        }

        let zen_text = this_build_rot13_text();
        let Some(zen_bits) = alloc_string_bits(_py, &zen_text) else {
            dec_ref_bits(_py, s_bits);
            dec_ref_bits(_py, dict_bits);
            return MoltObject::none().bits();
        };

        let payload_ptr = alloc_tuple(
            _py,
            &[
                s_bits,
                dict_bits,
                zen_bits,
                MoltObject::from_int(97).bits(),
                MoltObject::from_int(25).bits(),
            ],
        );
        dec_ref_bits(_py, s_bits);
        dec_ref_bits(_py, dict_bits);
        dec_ref_bits(_py, zen_bits);
        if payload_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(payload_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_encode(data_bits: u64, quotetabs_bits: u64, header_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let byte = match quopri_expect_single_byte(_py, c_bits, "ishex") {
            Ok(byte) => byte,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(quopri_is_hex(byte)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_quopri_unhex(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let octet = match email_quopri_expect_int_octet(_py, octet_bits, "body_check") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(!email_quopri_body_safe(octet)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_email_quoprimime_header_length(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_payload_312_json() -> u64 {
    crate::with_gil_entry!(_py, {
        email_quopri_alloc_str(_py, OPCODE_PAYLOAD_312_JSON)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload_312_json() -> u64 {
    crate::with_gil_entry!(_py, { email_quopri_alloc_str(_py, TOKEN_PAYLOAD_312_JSON) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_token_payload_312() -> u64 {
    crate::with_gil_entry!(_py, {
        let parsed: JsonValue = match serde_json::from_str(TOKEN_PAYLOAD_312_JSON) {
            Ok(value) => value,
            Err(err) => {
                let msg = format!("invalid token payload json: {err}");
                return raise_exception::<u64>(_py, "RuntimeError", msg.as_str());
            }
        };
        match token_payload_json_value_to_bits(_py, &parsed) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_metadata_payload_314_json() -> u64 {
    crate::with_gil_entry!(_py, {
        email_quopri_alloc_str(_py, OPCODE_METADATA_PAYLOAD_314_JSON)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_get_specialization_stats() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_opcode_stack_effect(opcode_bits: u64, oparg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let opcode_obj = obj_from_bits(opcode_bits);
        let Some(opcode) = to_i64(opcode_obj) else {
            let msg = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, opcode_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let oparg_obj = obj_from_bits(oparg_bits);
        let opcode_noarg = opcode_is_noarg_312(opcode);
        if oparg_obj.is_none() {
            if opcode_noarg {
                return match opcode_stack_effect_core_312(opcode, 0) {
                    Some(effect) => MoltObject::from_int(effect).bits(),
                    None => raise_exception::<_>(_py, "ValueError", "invalid opcode or oparg"),
                };
            }
            return raise_exception::<_>(
                _py,
                "ValueError",
                "stack_effect: opcode requires oparg but oparg was not specified",
            );
        }
        if opcode_noarg {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "stack_effect: opcode does not permit oparg but oparg was specified",
            );
        }

        let Some(oparg) = to_i64(oparg_obj) else {
            let msg = format!(
                "'{}' object cannot be interpreted as an integer",
                type_name(_py, oparg_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let Some(effect) = opcode_stack_effect_core_312(opcode, oparg) else {
            return raise_exception::<_>(_py, "ValueError", "invalid opcode or oparg");
        };
        MoltObject::from_int(effect).bits()
    })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_parse(spec_json_bits: u64, argv_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(spec_json) = string_obj_to_owned(obj_from_bits(spec_json_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "argparse spec_json must be str");
        };
        let argv = match iterable_to_string_vec(_py, argv_bits) {
            Ok(values) => values,
            Err(bits) => return bits,
        };

        let spec_value: JsonValue = match serde_json::from_str(spec_json.as_str()) {
            Ok(value) => value,
            Err(err) => {
                let msg = format!("invalid argparse spec json: {err}");
                return raise_exception::<_>(_py, "ValueError", msg.as_str());
            }
        };
        let spec = match argparse_decode_spec(&spec_value) {
            Ok(spec) => spec,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg.as_str()),
        };
        let parsed = match argparse_parse_with_spec(&spec, argv.as_slice()) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg.as_str()),
        };
        let payload = match serde_json::to_string(&JsonValue::Object(parsed)) {
            Ok(payload) => payload,
            Err(err) => {
                let msg = format!("argparse payload encode failed: {err}");
                return raise_exception::<_>(_py, "RuntimeError", msg.as_str());
            }
        };
        let out_ptr = alloc_string(_py, payload.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatchcase(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
                if fnmatch_bytes_from_bits(pat_bits).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a bytes pattern on a string-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            return MoltObject::from_bool(fnmatch_match_impl(&name, &pat)).bits();
        }
        if let Some(name) = fnmatch_bytes_from_bits(name_bits) {
            let Some(pat) = fnmatch_bytes_from_bits(pat_bits) else {
                if string_obj_to_owned(obj_from_bits(pat_bits)).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a string pattern on a bytes-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            return MoltObject::from_bool(fnmatch_match_bytes_impl(&name, &pat)).bits();
        }
        raise_exception::<_>(_py, "TypeError", "expected str or bytes name")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
                if fnmatch_bytes_from_bits(pat_bits).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a bytes pattern on a string-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            let name_norm = fnmatch_normcase_text(&name);
            let pat_norm = fnmatch_normcase_text(&pat);
            return MoltObject::from_bool(fnmatch_match_impl(&name_norm, &pat_norm)).bits();
        }
        if let Some(name) = fnmatch_bytes_from_bits(name_bits) {
            let Some(pat) = fnmatch_bytes_from_bits(pat_bits) else {
                if string_obj_to_owned(obj_from_bits(pat_bits)).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a string pattern on a bytes-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            let name_norm = fnmatch_normcase_bytes(&name);
            let pat_norm = fnmatch_normcase_bytes(&pat);
            return MoltObject::from_bool(fnmatch_match_bytes_impl(&name_norm, &pat_norm)).bits();
        }
        raise_exception::<_>(_py, "TypeError", "expected str or bytes name")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_filter(names_bits: u64, pat_bits: u64, invert_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let pat_str = string_obj_to_owned(obj_from_bits(pat_bits));
        let pat_bytes = if pat_str.is_none() {
            fnmatch_bytes_from_bits(pat_bits)
        } else {
            None
        };
        if pat_str.is_none() && pat_bytes.is_none() {
            return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
        }
        let invert = is_truthy(_py, obj_from_bits(invert_bits));
        let iter_bits = molt_iter(names_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut out_bits: Vec<u64> = Vec::new();
        loop {
            let (item_bits, done) = match iter_next_pair(_py, iter_bits) {
                Ok(value) => value,
                Err(bits) => {
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return bits;
                }
            };
            if done {
                break;
            }
            if let Some(pat) = &pat_str {
                let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "TypeError", "expected str item");
                };
                let name_norm = fnmatch_normcase_text(&name);
                let pat_norm = fnmatch_normcase_text(pat);
                let matched = fnmatch_match_impl(&name_norm, &pat_norm);
                if matched != invert {
                    inc_ref_bits(_py, item_bits);
                    out_bits.push(item_bits);
                }
            } else if let Some(pat) = &pat_bytes {
                let Some(name) = fnmatch_bytes_from_bits(item_bits) else {
                    if string_obj_to_owned(obj_from_bits(item_bits)).is_some() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot use a string pattern on a bytes-like object",
                        );
                    }
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "TypeError", "expected bytes item");
                };
                let name_norm = fnmatch_normcase_bytes(&name);
                let pat_norm = fnmatch_normcase_bytes(pat);
                let matched = fnmatch_match_bytes_impl(&name_norm, &pat_norm);
                if matched != invert {
                    let ptr = alloc_bytes(_py, &name);
                    if ptr.is_null() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    out_bits.push(MoltObject::from_ptr(ptr).bits());
                }
            }
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
pub extern "C" fn molt_fnmatch_translate(pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "expected str pattern");
        };
        let out = fnmatch_translate_impl(&pat);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_left(
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (lo, hi) = match bisect_normalize_bounds(_py, seq_bits, lo_bits, hi_bits) {
            Ok(bounds) => bounds,
            Err(bits) => return bits,
        };
        let pos = match bisect_find_index(_py, seq_bits, x_bits, lo, hi, key_bits, true) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_int(pos).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_right(
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (lo, hi) = match bisect_normalize_bounds(_py, seq_bits, lo_bits, hi_bits) {
            Ok(bounds) => bounds,
            Err(bits) => return bits,
        };
        let pos = match bisect_find_index(_py, seq_bits, x_bits, lo, hi, key_bits, false) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_int(pos).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_insort_left(
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (lo, hi) = match bisect_normalize_bounds(_py, seq_bits, lo_bits, hi_bits) {
            Ok(bounds) => bounds,
            Err(bits) => return bits,
        };
        let search_x_bits = if obj_from_bits(key_bits).is_none() {
            x_bits
        } else {
            let bits = unsafe { call_callable1(_py, key_bits, x_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            bits
        };
        let pos = match bisect_find_index(_py, seq_bits, search_x_bits, lo, hi, key_bits, true) {
            Ok(value) => value,
            Err(bits) => {
                if !obj_from_bits(key_bits).is_none() && !obj_from_bits(search_x_bits).is_none() {
                    dec_ref_bits(_py, search_x_bits);
                }
                return bits;
            }
        };
        if !obj_from_bits(key_bits).is_none() && !obj_from_bits(search_x_bits).is_none() {
            dec_ref_bits(_py, search_x_bits);
        }
        if let Err(bits) = bisect_insert_at(_py, seq_bits, pos, x_bits) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bisect_insort_right(
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (lo, hi) = match bisect_normalize_bounds(_py, seq_bits, lo_bits, hi_bits) {
            Ok(bounds) => bounds,
            Err(bits) => return bits,
        };
        let search_x_bits = if obj_from_bits(key_bits).is_none() {
            x_bits
        } else {
            let bits = unsafe { call_callable1(_py, key_bits, x_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            bits
        };
        let pos = match bisect_find_index(_py, seq_bits, search_x_bits, lo, hi, key_bits, false) {
            Ok(value) => value,
            Err(bits) => {
                if !obj_from_bits(key_bits).is_none() && !obj_from_bits(search_x_bits).is_none() {
                    dec_ref_bits(_py, search_x_bits);
                }
                return bits;
            }
        };
        if !obj_from_bits(key_bits).is_none() && !obj_from_bits(search_x_bits).is_none() {
            dec_ref_bits(_py, search_x_bits);
        }
        if let Err(bits) = bisect_insert_at(_py, seq_bits, pos, x_bits) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        fn stat_target_minor(_py: &crate::PyToken<'_>) -> i64 {
            let state = crate::runtime_state(_py);
            if let Some(info) = state.sys_version_info.lock().unwrap().as_ref()
                && info.major == 3
            {
                return info.minor;
            }
            if let Ok(raw) = std::env::var("MOLT_PYTHON_VERSION")
                && let Some((major_raw, minor_raw)) = raw.split_once('.')
                && major_raw.trim() == "3"
                && let Ok(minor) = minor_raw.trim().parse::<i64>()
            {
                return minor;
            }
            if let Ok(raw) = std::env::var("MOLT_SYS_VERSION_INFO") {
                let mut parts = raw.split(',');
                if let (Some(major_raw), Some(minor_raw)) = (parts.next(), parts.next())
                    && major_raw.trim() == "3"
                    && let Ok(minor) = minor_raw.trim().parse::<i64>()
                {
                    return minor;
                }
            }
            12
        }

        let has_313_constants = stat_target_minor(_py) >= 13;
        const S_IFMT_MASK: i64 = 0o170000;
        const S_IFSOCK: i64 = 0o140000;
        const S_IFLNK: i64 = 0o120000;
        const S_IFREG: i64 = 0o100000;
        const S_IFBLK: i64 = 0o060000;
        const S_IFDIR: i64 = 0o040000;
        const S_IFCHR: i64 = 0o020000;
        const S_IFIFO: i64 = 0o010000;
        const S_IFDOOR: i64 = 0;
        const S_IFPORT: i64 = 0;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        const S_IFWHT: i64 = 0o160000;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        const S_IFWHT: i64 = 0;
        const S_ISUID: i64 = 0o004000;
        const S_ISGID: i64 = 0o002000;
        const S_ISVTX: i64 = 0o001000;
        const S_IRUSR: i64 = 0o000400;
        const S_IWUSR: i64 = 0o000200;
        const S_IXUSR: i64 = 0o000100;
        const S_IRGRP: i64 = 0o000040;
        const S_IWGRP: i64 = 0o000020;
        const S_IXGRP: i64 = 0o000010;
        const S_IROTH: i64 = 0o000004;
        const S_IWOTH: i64 = 0o000002;
        const S_IXOTH: i64 = 0o000001;
        const ST_MODE: i64 = 0;
        const ST_INO: i64 = 1;
        const ST_DEV: i64 = 2;
        const ST_NLINK: i64 = 3;
        const ST_UID: i64 = 4;
        const ST_GID: i64 = 5;
        const ST_SIZE: i64 = 6;
        const ST_ATIME: i64 = 7;
        const ST_MTIME: i64 = 8;
        const ST_CTIME: i64 = 9;
        const UF_NODUMP: i64 = 0x00000001;
        const UF_IMMUTABLE: i64 = 0x00000002;
        const UF_APPEND: i64 = 0x00000004;
        const UF_OPAQUE: i64 = 0x00000008;
        const UF_NOUNLINK: i64 = 0x00000010;
        const UF_SETTABLE: i64 = 0x0000ffff;
        const UF_COMPRESSED: i64 = 0x00000020;
        const UF_TRACKED: i64 = 0x00000040;
        const UF_DATAVAULT: i64 = 0x00000080;
        const UF_HIDDEN: i64 = 0x00008000;
        const SF_ARCHIVED: i64 = 0x00010000;
        const SF_IMMUTABLE: i64 = 0x00020000;
        const SF_APPEND: i64 = 0x00040000;
        const SF_SETTABLE: i64 = 0x3fff0000;
        const SF_RESTRICTED: i64 = 0x00080000;
        const SF_NOUNLINK: i64 = 0x00100000;
        const SF_SNAPSHOT: i64 = 0x00200000;
        const SF_FIRMLINK: i64 = 0x00800000;
        const SF_DATALESS: i64 = 0x40000000;
        const SF_SUPPORTED: i64 = 0x009f0000;
        const SF_SYNTHETIC: i64 = 0xc0000000;
        const FILE_ATTRIBUTE_ARCHIVE: i64 = 32;
        const FILE_ATTRIBUTE_COMPRESSED: i64 = 2048;
        const FILE_ATTRIBUTE_DEVICE: i64 = 64;
        const FILE_ATTRIBUTE_DIRECTORY: i64 = 16;
        const FILE_ATTRIBUTE_ENCRYPTED: i64 = 16384;
        const FILE_ATTRIBUTE_HIDDEN: i64 = 2;
        const FILE_ATTRIBUTE_INTEGRITY_STREAM: i64 = 32768;
        const FILE_ATTRIBUTE_NORMAL: i64 = 128;
        const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: i64 = 8192;
        const FILE_ATTRIBUTE_NO_SCRUB_DATA: i64 = 131072;
        const FILE_ATTRIBUTE_OFFLINE: i64 = 4096;
        const FILE_ATTRIBUTE_READONLY: i64 = 1;
        const FILE_ATTRIBUTE_REPARSE_POINT: i64 = 1024;
        const FILE_ATTRIBUTE_SPARSE_FILE: i64 = 512;
        const FILE_ATTRIBUTE_SYSTEM: i64 = 4;
        const FILE_ATTRIBUTE_TEMPORARY: i64 = 256;
        const FILE_ATTRIBUTE_VIRTUAL: i64 = 65536;
        let payload = [
            MoltObject::from_int(S_IFMT_MASK).bits(),
            MoltObject::from_int(S_IFSOCK).bits(),
            MoltObject::from_int(S_IFLNK).bits(),
            MoltObject::from_int(S_IFREG).bits(),
            MoltObject::from_int(S_IFBLK).bits(),
            MoltObject::from_int(S_IFDIR).bits(),
            MoltObject::from_int(S_IFCHR).bits(),
            MoltObject::from_int(S_IFIFO).bits(),
            MoltObject::from_int(S_IFDOOR).bits(),
            MoltObject::from_int(S_IFPORT).bits(),
            MoltObject::from_int(S_IFWHT).bits(),
            MoltObject::from_int(S_ISUID).bits(),
            MoltObject::from_int(S_ISGID).bits(),
            MoltObject::from_int(S_ISVTX).bits(),
            MoltObject::from_int(S_IRUSR).bits(),
            MoltObject::from_int(S_IWUSR).bits(),
            MoltObject::from_int(S_IXUSR).bits(),
            MoltObject::from_int(S_IRGRP).bits(),
            MoltObject::from_int(S_IWGRP).bits(),
            MoltObject::from_int(S_IXGRP).bits(),
            MoltObject::from_int(S_IROTH).bits(),
            MoltObject::from_int(S_IWOTH).bits(),
            MoltObject::from_int(S_IXOTH).bits(),
            MoltObject::from_int(ST_MODE).bits(),
            MoltObject::from_int(ST_INO).bits(),
            MoltObject::from_int(ST_DEV).bits(),
            MoltObject::from_int(ST_NLINK).bits(),
            MoltObject::from_int(ST_UID).bits(),
            MoltObject::from_int(ST_GID).bits(),
            MoltObject::from_int(ST_SIZE).bits(),
            MoltObject::from_int(ST_ATIME).bits(),
            MoltObject::from_int(ST_MTIME).bits(),
            MoltObject::from_int(ST_CTIME).bits(),
            MoltObject::from_int(UF_NODUMP).bits(),
            MoltObject::from_int(UF_IMMUTABLE).bits(),
            MoltObject::from_int(UF_APPEND).bits(),
            MoltObject::from_int(UF_OPAQUE).bits(),
            MoltObject::from_int(UF_NOUNLINK).bits(),
            MoltObject::from_int(UF_COMPRESSED).bits(),
            MoltObject::from_int(UF_HIDDEN).bits(),
            MoltObject::from_int(SF_ARCHIVED).bits(),
            MoltObject::from_int(SF_IMMUTABLE).bits(),
            MoltObject::from_int(SF_APPEND).bits(),
            MoltObject::from_int(SF_NOUNLINK).bits(),
            MoltObject::from_int(SF_SNAPSHOT).bits(),
            MoltObject::from_int(if has_313_constants { UF_SETTABLE } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { UF_TRACKED } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { UF_DATAVAULT } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_SETTABLE } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_RESTRICTED } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_FIRMLINK } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_DATALESS } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_SUPPORTED } else { 0 }).bits(),
            MoltObject::from_int(if has_313_constants { SF_SYNTHETIC } else { 0 }).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_ARCHIVE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_COMPRESSED).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_DEVICE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_DIRECTORY).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_ENCRYPTED).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_HIDDEN).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_INTEGRITY_STREAM).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_NORMAL).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_NOT_CONTENT_INDEXED).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_NO_SCRUB_DATA).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_OFFLINE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_READONLY).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_REPARSE_POINT).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_SPARSE_FILE).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_SYSTEM).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_TEMPORARY).bits(),
            MoltObject::from_int(FILE_ATTRIBUTE_VIRTUAL).bits(),
        ];
        let tuple_ptr = alloc_tuple(_py, &payload);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

fn parse_stat_mode(_py: &crate::PyToken<'_>, mode_bits: u64) -> Result<i64, u64> {
    let Some(mode) = to_i64(obj_from_bits(mode_bits)) else {
        return Err(raise_exception::<_>(_py, "TypeError", "mode must be int"));
    };
    Ok(mode)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_ifmt(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFMT_MASK: i64 = 0o170000;
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(mode & S_IFMT_MASK).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_imode(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IMODE_MASK: i64 = 0o7777;
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(mode & S_IMODE_MASK).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isdir(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o040000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isreg(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o100000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_ischr(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o020000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isblk(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o060000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isfifo(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o010000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_islnk(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o120000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_issock(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o140000).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isdoor(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFDOOR: i64 = 0;
        if S_IFDOOR == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == S_IFDOOR).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isport(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFPORT: i64 = 0;
        if S_IFPORT == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == S_IFPORT).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_iswht(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        const S_IFWHT: i64 = 0o160000;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        const S_IFWHT: i64 = 0;
        if S_IFWHT == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == S_IFWHT).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_filemode(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFMT_MASK: i64 = 0o170000;
        const S_IFSOCK: i64 = 0o140000;
        const S_IFLNK: i64 = 0o120000;
        const S_IFREG: i64 = 0o100000;
        const S_IFBLK: i64 = 0o060000;
        const S_IFDIR: i64 = 0o040000;
        const S_IFCHR: i64 = 0o020000;
        const S_IFIFO: i64 = 0o010000;
        const S_IFDOOR: i64 = 0;
        const S_IFPORT: i64 = 0;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        const S_IFWHT: i64 = 0o160000;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        const S_IFWHT: i64 = 0;
        const S_ISUID: i64 = 0o004000;
        const S_ISGID: i64 = 0o002000;
        const S_ISVTX: i64 = 0o001000;
        const S_IRUSR: i64 = 0o000400;
        const S_IWUSR: i64 = 0o000200;
        const S_IXUSR: i64 = 0o000100;
        const S_IRGRP: i64 = 0o000040;
        const S_IWGRP: i64 = 0o000020;
        const S_IXGRP: i64 = 0o000010;
        const S_IROTH: i64 = 0o000004;
        const S_IWOTH: i64 = 0o000002;
        const S_IXOTH: i64 = 0o000001;
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        let file_type = mode & S_IFMT_MASK;
        let mut out = String::with_capacity(10);
        let type_char = if file_type == S_IFLNK {
            'l'
        } else if file_type == S_IFSOCK {
            's'
        } else if file_type == S_IFREG {
            '-'
        } else if file_type == S_IFBLK {
            'b'
        } else if file_type == S_IFDIR {
            'd'
        } else if file_type == S_IFCHR {
            'c'
        } else if file_type == S_IFIFO {
            'p'
        } else if S_IFDOOR != 0 && file_type == S_IFDOOR {
            'D'
        } else if S_IFPORT != 0 && file_type == S_IFPORT {
            'P'
        } else if S_IFWHT != 0 && file_type == S_IFWHT {
            'w'
        } else {
            '?'
        };
        out.push(type_char);
        out.push(if (mode & S_IRUSR) != 0 { 'r' } else { '-' });
        out.push(if (mode & S_IWUSR) != 0 { 'w' } else { '-' });
        out.push(match ((mode & S_IXUSR) != 0, (mode & S_ISUID) != 0) {
            (true, true) => 's',
            (false, true) => 'S',
            (true, false) => 'x',
            (false, false) => '-',
        });
        out.push(if (mode & S_IRGRP) != 0 { 'r' } else { '-' });
        out.push(if (mode & S_IWGRP) != 0 { 'w' } else { '-' });
        out.push(match ((mode & S_IXGRP) != 0, (mode & S_ISGID) != 0) {
            (true, true) => 's',
            (false, true) => 'S',
            (true, false) => 'x',
            (false, false) => '-',
        });
        out.push(if (mode & S_IROTH) != 0 { 'r' } else { '-' });
        out.push(if (mode & S_IWOTH) != 0 { 'w' } else { '-' });
        out.push(match ((mode & S_IXOTH) != 0, (mode & S_ISVTX) != 0) {
            (true, true) => 't',
            (false, true) => 'T',
            (true, false) => 'x',
            (false, false) => '-',
        });
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_wrap(text_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let options = textwrap_default_options(width);
        let lines = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_wrap_ex(
    text_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_fill(text_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let options = textwrap_default_options(width);
        let out = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines.join("\n"),
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_fill_ex(
    text_bits: u64,
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
        let out = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines.join("\n"),
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_indent(text_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        textwrap_indent_with_predicate(_py, &text, &prefix, None)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_indent_ex(
    text_bits: u64,
    prefix_bits: u64,
    predicate_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let predicate = if obj_from_bits(predicate_bits).is_none() {
            None
        } else {
            Some(predicate_bits)
        };
        textwrap_indent_with_predicate(_py, &text, &prefix, predicate)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_quote(string_bits: u64, safe_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let Some(safe) = string_obj_to_owned(obj_from_bits(safe_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "safe must be str");
        };
        let out = urllib_quote_impl(&string, &safe);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_quote_plus(string_bits: u64, safe_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let Some(safe) = string_obj_to_owned(obj_from_bits(safe_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "safe must be str");
        };
        let out = urllib_quote_impl(&string, &safe).replace("%20", "+");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_unquote(string_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let out = urllib_unquote_impl(&string);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_unquote_plus(string_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let out = urllib_unquote_impl(&string.replace('+', " "));
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_parse_qsl(
    qs_bits: u64,
    keep_blank_values_bits: u64,
    strict_parsing_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(qs) = string_obj_to_owned(obj_from_bits(qs_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "qs must be str");
        };
        let keep_blank_values = is_truthy(_py, obj_from_bits(keep_blank_values_bits));
        let strict_parsing = is_truthy(_py, obj_from_bits(strict_parsing_bits));
        let pairs = match urllib_parse_qsl_impl(&qs, keep_blank_values, strict_parsing) {
            Ok(pairs) => pairs,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_qsl_list(_py, &pairs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_parse_qs(
    qs_bits: u64,
    keep_blank_values_bits: u64,
    strict_parsing_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(qs) = string_obj_to_owned(obj_from_bits(qs_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "qs must be str");
        };
        let keep_blank_values = is_truthy(_py, obj_from_bits(keep_blank_values_bits));
        let strict_parsing = is_truthy(_py, obj_from_bits(strict_parsing_bits));
        let pairs = match urllib_parse_qsl_impl(&qs, keep_blank_values, strict_parsing) {
            Ok(pairs) => pairs,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let mut order: Vec<String> = Vec::new();
        let mut values: HashMap<String, Vec<String>> = HashMap::new();
        for (key, value) in pairs {
            if !values.contains_key(&key) {
                order.push(key.clone());
            }
            values.entry(key).or_default().push(value);
        }
        alloc_qs_dict(_py, &order, &values)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlencode(query_bits: u64, doseq_bits: u64, safe_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let doseq = is_truthy(_py, obj_from_bits(doseq_bits));
        let Some(safe) = string_obj_to_owned(obj_from_bits(safe_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "safe must be str");
        };
        let out = match urllib_urlencode_impl(_py, query_bits, doseq, &safe) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlsplit(
    url_bits: u64,
    scheme_bits: u64,
    allow_fragments_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let allow_fragments = is_truthy(_py, obj_from_bits(allow_fragments_bits));
        let split = urllib_urlsplit_impl(&url, &scheme, allow_fragments);
        alloc_string_tuple(_py, &split)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_urlsplit(
    url_bits: u64,
    scheme_bits: u64,
    allow_fragments_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let allow_fragments = is_truthy(_py, obj_from_bits(allow_fragments_bits));
        let split = urllib_urlsplit_impl(&url, &scheme, allow_fragments);
        alloc_string_tuple(_py, &split)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlparse(
    url_bits: u64,
    scheme_bits: u64,
    allow_fragments_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let allow_fragments = is_truthy(_py, obj_from_bits(allow_fragments_bits));
        let parsed = urllib_urlparse_impl(&url, &scheme, allow_fragments);
        alloc_string_tuple(_py, &parsed)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlunsplit(
    scheme_bits: u64,
    netloc_bits: u64,
    path_bits: u64,
    query_bits: u64,
    fragment_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let Some(netloc) = string_obj_to_owned(obj_from_bits(netloc_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "netloc must be str");
        };
        let Some(path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "path must be str");
        };
        let Some(query) = string_obj_to_owned(obj_from_bits(query_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "query must be str");
        };
        let Some(fragment) = string_obj_to_owned(obj_from_bits(fragment_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fragment must be str");
        };
        let out = urllib_unsplit_impl(&scheme, &netloc, &path, &query, &fragment);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlunparse(
    scheme_bits: u64,
    netloc_bits: u64,
    path_bits: u64,
    params_bits: u64,
    query_bits: u64,
    fragment_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let Some(netloc) = string_obj_to_owned(obj_from_bits(netloc_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "netloc must be str");
        };
        let Some(mut path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "path must be str");
        };
        let Some(params) = string_obj_to_owned(obj_from_bits(params_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "params must be str");
        };
        let Some(query) = string_obj_to_owned(obj_from_bits(query_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "query must be str");
        };
        let Some(fragment) = string_obj_to_owned(obj_from_bits(fragment_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fragment must be str");
        };
        if !params.is_empty() {
            path.push(';');
            path.push_str(&params);
        }
        let out = urllib_unsplit_impl(&scheme, &netloc, &path, &query, &fragment);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urldefrag(url_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let (base, fragment) = if let Some((base, fragment)) = url.split_once('#') {
            (base.to_string(), fragment.to_string())
        } else {
            (url, String::new())
        };
        alloc_string_tuple(_py, &[base, fragment])
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urljoin(base_bits: u64, url_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(base) = string_obj_to_owned(obj_from_bits(base_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "base must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        if base.is_empty() {
            let out_ptr = alloc_string(_py, url.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        let target = urllib_urlsplit_impl(&url, "", true);
        let out = if !target[0].is_empty() {
            url
        } else {
            let base_parts = urllib_urlparse_impl(&base, "", true);
            if url.starts_with("//") {
                format!("{}:{url}", base_parts[0])
            } else if !target[1].is_empty() {
                urllib_unsplit_impl(
                    &base_parts[0],
                    &target[1],
                    &target[2],
                    &target[3],
                    &target[4],
                )
            } else {
                let mut path = target[2].clone();
                if path.is_empty() {
                    path = base_parts[2].clone();
                } else if !path.starts_with('/') {
                    let base_path = &base_parts[2];
                    let base_dir = match base_path.rsplit_once('/') {
                        Some((dir, _)) => dir.to_string(),
                        None => String::new(),
                    };
                    if base_dir.is_empty() {
                        path = format!("/{path}");
                    } else {
                        path = format!("{base_dir}/{path}");
                    }
                }
                urllib_unsplit_impl(
                    &base_parts[0],
                    &base_parts[1],
                    &path,
                    &target[3],
                    &target[4],
                )
            }
        };
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = cookiejar_store_new() else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar allocation failed");
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(size) = cookiejar_with(handle, |jar| jar.cookies.len()) else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar handle is invalid");
        };
        MoltObject::from_int(i64::try_from(size).unwrap_or(i64::MAX)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_clear(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(()) = cookiejar_with_mut(handle, |jar| {
            jar.cookies.clear();
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar handle is invalid");
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_extract(
    handle_bits: u64,
    request_url_bits: u64,
    headers_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(request_url) = string_obj_to_owned(obj_from_bits(request_url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "request url must be str");
        };
        let headers = match urllib_http_extract_headers_mapping(_py, headers_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        urllib_cookiejar_store_from_headers(handle, &request_url, &headers);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_header_for_url(
    handle_bits: u64,
    request_url_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(request_url) = string_obj_to_owned(obj_from_bits(request_url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "request url must be str");
        };
        let Some(header) = urllib_cookiejar_header_for_url(handle, &request_url) else {
            return MoltObject::none().bits();
        };
        let Some(bits) = alloc_string_bits(_py, &header) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookies_parse(cookie_header_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(cookie_header) = string_obj_to_owned(obj_from_bits(cookie_header_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie header must be str");
        };
        let pairs = http_cookies_parse_pairs(&cookie_header);
        match urllib_http_headers_to_list(_py, &pairs) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookies_render_morsel(
    name_bits: u64,
    value_bits: u64,
    path_bits: u64,
    secure_bits: u64,
    httponly_bits: u64,
    max_age_bits: u64,
    expires_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let out = http_cookies_render_morsel_impl(
            _py,
            HttpCookieMorselInput {
                name_bits,
                value_bits,
                path_bits,
                secure_bits,
                httponly_bits,
                max_age_bits,
                expires_bits,
            },
        );
        let Some(bits) = alloc_string_bits(_py, &out) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_urlerror_init(
    self_bits: u64,
    reason_bits: u64,
    filename_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !urllib_error_init_args(_py, self_bits, &[reason_bits]) {
            return MoltObject::none().bits();
        }
        if !urllib_error_set_attr(_py, self_bits, "reason", reason_bits) {
            return MoltObject::none().bits();
        }
        if !obj_from_bits(filename_bits).is_none()
            && !urllib_error_set_attr(_py, self_bits, "filename", filename_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_urlerror_str(reason_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reason_text = crate::format_obj_str(_py, obj_from_bits(reason_bits));
        let out = format!("<urlopen error {reason_text}>");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_httperror_init(
    self_bits: u64,
    url_bits: u64,
    code_bits: u64,
    msg_bits: u64,
    hdrs_bits: u64,
    fp_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        // CPython's urllib.error.HTTPError does not populate BaseException.args
        // with constructor values in this path; normalize args to ().
        if !urllib_error_init_args(_py, self_bits, &[]) {
            return MoltObject::none().bits();
        }
        if !urllib_error_set_attr(_py, self_bits, "reason", msg_bits)
            || !urllib_error_set_attr(_py, self_bits, "code", code_bits)
            || !urllib_error_set_attr(_py, self_bits, "msg", msg_bits)
            || !urllib_error_set_attr(_py, self_bits, "hdrs", hdrs_bits)
            || !urllib_error_set_attr(_py, self_bits, "filename", url_bits)
            || !urllib_error_set_attr(_py, self_bits, "fp", fp_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_httperror_str(code_bits: u64, msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let code_text = crate::format_obj_str(_py, obj_from_bits(code_bits));
        let msg_text = crate::format_obj_str(_py, obj_from_bits(msg_bits));
        let out = format!("HTTP Error {code_text}: {msg_text}");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_content_too_short_init(
    self_bits: u64,
    msg_bits: u64,
    content_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !urllib_error_init_args(_py, self_bits, &[msg_bits]) {
            return MoltObject::none().bits();
        }
        if !urllib_error_set_attr(_py, self_bits, "reason", msg_bits)
            || !urllib_error_set_attr(_py, self_bits, "content", content_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_register(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        runtime.pending_by_server.entry(server_bits).or_default();
        runtime.closed_servers.remove(&server_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_unregister(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        runtime.closed_servers.insert(server_bits);
        if let Some(mut ids) = runtime.pending_by_server.remove(&server_bits) {
            while let Some(request_id) = ids.pop_front() {
                runtime.pending_requests.remove(&request_id);
                runtime.request_server.remove(&request_id);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_begin(server_bits: u64, request_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let request = match socketserver_extract_bytes(_py, request_bits, "request payload") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        if runtime.closed_servers.contains(&server_bits) {
            return raise_exception::<_>(_py, "OSError", "server closed");
        }
        let request_id = runtime.next_request_id;
        runtime.next_request_id = runtime.next_request_id.saturating_add(1);
        runtime.pending_requests.insert(
            request_id,
            MoltSocketServerPending {
                request,
                response: None,
            },
        );
        runtime.request_server.insert(request_id, server_bits);
        runtime
            .pending_by_server
            .entry(server_bits)
            .or_default()
            .push_back(request_id);
        MoltObject::from_int(request_id as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_poll(server_bits: u64, request_id_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let request_id = match socketserver_extract_request_id(_py, request_id_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        let Some(owner) = runtime.request_server.get(&request_id).copied() else {
            return MoltObject::none().bits();
        };
        if owner != server_bits {
            return raise_exception::<_>(_py, "RuntimeError", "request id owner mismatch");
        }
        let Some(pending) = runtime.pending_requests.get_mut(&request_id) else {
            runtime.request_server.remove(&request_id);
            return MoltObject::none().bits();
        };
        let Some(response) = pending.response.take() else {
            return MoltObject::none().bits();
        };
        runtime.pending_requests.remove(&request_id);
        runtime.request_server.remove(&request_id);
        let ptr = crate::alloc_bytes(_py, &response);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_cancel(server_bits: u64, request_id_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let request_id = match socketserver_extract_request_id(_py, request_id_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        if let Some(queue) = runtime.pending_by_server.get_mut(&server_bits) {
            queue.retain(|candidate| *candidate != request_id);
        }
        runtime.pending_requests.remove(&request_id);
        runtime.request_server.remove(&request_id);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_get_request_poll(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        if runtime.closed_servers.contains(&server_bits) {
            return raise_exception::<_>(_py, "OSError", "server closed");
        }
        let Some(queue) = runtime.pending_by_server.get_mut(&server_bits) else {
            return MoltObject::none().bits();
        };
        let Some(request_id) = queue.pop_front() else {
            return MoltObject::none().bits();
        };
        let Some(pending) = runtime.pending_requests.get(&request_id) else {
            runtime.request_server.remove(&request_id);
            return MoltObject::none().bits();
        };
        let request_ptr = crate::alloc_bytes(_py, &pending.request);
        if request_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let request_bits = MoltObject::from_ptr(request_ptr).bits();
        let request_id_bits = MoltObject::from_int(request_id as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[request_id_bits, request_bits]);
        dec_ref_bits(_py, request_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_set_response(
    server_bits: u64,
    request_id_bits: u64,
    response_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let request_id = match socketserver_extract_request_id(_py, request_id_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let response = match socketserver_extract_bytes(_py, response_bits, "response payload") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        let Some(owner) = runtime.request_server.get(&request_id).copied() else {
            return MoltObject::none().bits();
        };
        if owner != server_bits {
            return raise_exception::<_>(_py, "RuntimeError", "request id owner mismatch");
        }
        let Some(pending) = runtime.pending_requests.get_mut(&request_id) else {
            runtime.request_server.remove(&request_id);
            return MoltObject::none().bits();
        };
        pending.response = Some(response);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_serve_forever(
    server_bits: u64,
    poll_interval_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let poll_interval = to_f64(obj_from_bits(poll_interval_bits))
            .unwrap_or(0.5)
            .max(0.0);
        loop {
            let shutdown_requested =
                match urllib_attr_truthy(_py, server_bits, b"_molt_shutdown_request") {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
            if shutdown_requested {
                break;
            }
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"handle_request") else {
                return MoltObject::none().bits();
            };
            let missing = missing_bits(_py);
            let handle_request_bits = molt_getattr_builtin(server_bits, name_bits, missing);
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if handle_request_bits == missing {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "socketserver server is missing handle_request",
                );
            }
            let _ = unsafe { call_callable0(_py, handle_request_bits) };
            dec_ref_bits(_py, handle_request_bits);
            if !exception_pending(_py) {
                if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
                    return bits;
                }
                continue;
            }
            let kind = urllib_request_pending_exception_kind_name(_py).unwrap_or_default();
            if kind == "TimeoutError" {
                clear_exception(_py);
                if poll_interval > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(poll_interval.min(0.05)));
                }
                if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
                    return bits;
                }
                continue;
            }
            if kind == "OSError" {
                let shutdown_now =
                    match urllib_attr_truthy(_py, server_bits, b"_molt_shutdown_request") {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                let closed_now = match urllib_attr_truthy(_py, server_bits, b"_closed") {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if shutdown_now || closed_now {
                    clear_exception(_py);
                    break;
                }
            }
            let handled_kind = !kind.is_empty()
                && kind != "SystemExit"
                && kind != "KeyboardInterrupt"
                && kind != "GeneratorExit"
                && kind != "BaseExceptionGroup";
            if handled_kind {
                clear_exception(_py);
                let Some(name_bits) = attr_name_bits_from_bytes(_py, b"handle_error") else {
                    return MoltObject::none().bits();
                };
                let missing = missing_bits(_py);
                let handle_error_bits = molt_getattr_builtin(server_bits, name_bits, missing);
                dec_ref_bits(_py, name_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if handle_error_bits != missing {
                    let none_bits = MoltObject::none().bits();
                    let _ = unsafe { call_callable2(_py, handle_error_bits, none_bits, none_bits) };
                    dec_ref_bits(_py, handle_error_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
                    return bits;
                }
                continue;
            }
            return MoltObject::none().bits();
        }
        if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_handle_request(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(get_request_name_bits) = attr_name_bits_from_bytes(_py, b"get_request") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let get_request_bits = molt_getattr_builtin(server_bits, get_request_name_bits, missing);
        dec_ref_bits(_py, get_request_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if get_request_bits == missing
            || !is_truthy(_py, obj_from_bits(molt_is_callable(get_request_bits)))
        {
            if get_request_bits != missing {
                dec_ref_bits(_py, get_request_bits);
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "socketserver server is missing get_request",
            );
        }
        let request_tuple_bits = unsafe { call_callable0(_py, get_request_bits) };
        dec_ref_bits(_py, get_request_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let (request_bits, client_address_bits, request_id) =
            match socketserver_extract_handle_request_tuple(_py, request_tuple_bits) {
                Ok(parts) => parts,
                Err(bits) => {
                    dec_ref_bits(_py, request_tuple_bits);
                    return bits;
                }
            };

        let mut deferred_exception_bits: Option<u64> = None;
        let mut should_process = true;

        if let Some(verify_request_bits) =
            match urllib_request_attr_optional(_py, server_bits, b"verify_request") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, request_tuple_bits);
                    return bits;
                }
            }
        {
            if !is_truthy(_py, obj_from_bits(molt_is_callable(verify_request_bits))) {
                dec_ref_bits(_py, verify_request_bits);
                dec_ref_bits(_py, request_tuple_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "socketserver server verify_request must be callable",
                );
            }
            let verify_bits = unsafe {
                call_callable2(_py, verify_request_bits, request_bits, client_address_bits)
            };
            dec_ref_bits(_py, verify_request_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
            should_process = is_truthy(_py, obj_from_bits(verify_bits));
            dec_ref_bits(_py, verify_bits);
        }

        if should_process {
            let Some(process_request_name_bits) =
                attr_name_bits_from_bytes(_py, b"process_request")
            else {
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            };
            let process_request_bits =
                molt_getattr_builtin(server_bits, process_request_name_bits, missing);
            dec_ref_bits(_py, process_request_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
            if process_request_bits == missing
                || !is_truthy(_py, obj_from_bits(molt_is_callable(process_request_bits)))
            {
                if process_request_bits != missing {
                    dec_ref_bits(_py, process_request_bits);
                }
                dec_ref_bits(_py, request_tuple_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "socketserver server is missing process_request",
                );
            }
            let _ = unsafe {
                call_callable2(_py, process_request_bits, request_bits, client_address_bits)
            };
            dec_ref_bits(_py, process_request_bits);
            if exception_pending(_py) {
                let kind = urllib_request_pending_exception_kind_name(_py).unwrap_or_default();
                let handled_kind = !kind.is_empty()
                    && kind != "SystemExit"
                    && kind != "KeyboardInterrupt"
                    && kind != "GeneratorExit"
                    && kind != "BaseExceptionGroup";
                if handled_kind {
                    clear_exception(_py);
                    let Some(handle_error_name_bits) =
                        attr_name_bits_from_bytes(_py, b"handle_error")
                    else {
                        dec_ref_bits(_py, request_tuple_bits);
                        return MoltObject::none().bits();
                    };
                    let handle_error_bits =
                        molt_getattr_builtin(server_bits, handle_error_name_bits, missing);
                    dec_ref_bits(_py, handle_error_name_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, request_tuple_bits);
                        return MoltObject::none().bits();
                    }
                    if handle_error_bits != missing {
                        let _ = unsafe {
                            call_callable2(
                                _py,
                                handle_error_bits,
                                request_bits,
                                client_address_bits,
                            )
                        };
                        dec_ref_bits(_py, handle_error_bits);
                        if exception_pending(_py) {
                            let exc_bits = molt_exception_last();
                            clear_exception(_py);
                            deferred_exception_bits = Some(exc_bits);
                        }
                    }
                } else {
                    let exc_bits = molt_exception_last();
                    clear_exception(_py);
                    deferred_exception_bits = Some(exc_bits);
                }
            }
        }

        let Some(close_request_name_bits) = attr_name_bits_from_bytes(_py, b"close_request") else {
            if let Some(exc_bits) = deferred_exception_bits.take() {
                dec_ref_bits(_py, exc_bits);
            }
            dec_ref_bits(_py, request_tuple_bits);
            return MoltObject::none().bits();
        };
        let close_request_bits =
            molt_getattr_builtin(server_bits, close_request_name_bits, missing);
        dec_ref_bits(_py, close_request_name_bits);
        if exception_pending(_py) {
            if let Some(exc_bits) = deferred_exception_bits.take() {
                dec_ref_bits(_py, exc_bits);
            }
            dec_ref_bits(_py, request_tuple_bits);
            return MoltObject::none().bits();
        }
        if close_request_bits != missing
            && is_truthy(_py, obj_from_bits(molt_is_callable(close_request_bits)))
        {
            let _ = unsafe { call_callable1(_py, close_request_bits, request_bits) };
            dec_ref_bits(_py, close_request_bits);
            if exception_pending(_py) {
                if let Some(exc_bits) = deferred_exception_bits.take() {
                    dec_ref_bits(_py, exc_bits);
                }
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
        } else if close_request_bits != missing {
            dec_ref_bits(_py, close_request_bits);
        }

        if request_id >= 0 {
            let Some(response_bytes_name_bits) = attr_name_bits_from_bytes(_py, b"response_bytes")
            else {
                if let Some(exc_bits) = deferred_exception_bits.take() {
                    let out = crate::molt_raise(exc_bits);
                    dec_ref_bits(_py, request_tuple_bits);
                    return out;
                }
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            };
            let response_bytes_bits =
                molt_getattr_builtin(request_bits, response_bytes_name_bits, missing);
            dec_ref_bits(_py, response_bytes_name_bits);
            if exception_pending(_py) {
                if let Some(exc_bits) = deferred_exception_bits.take() {
                    dec_ref_bits(_py, exc_bits);
                }
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
            if response_bytes_bits != missing
                && is_truthy(_py, obj_from_bits(molt_is_callable(response_bytes_bits)))
            {
                let response_bits = unsafe { call_callable0(_py, response_bytes_bits) };
                dec_ref_bits(_py, response_bytes_bits);
                if exception_pending(_py) {
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        dec_ref_bits(_py, exc_bits);
                    }
                    dec_ref_bits(_py, request_tuple_bits);
                    return MoltObject::none().bits();
                }
                let response =
                    match socketserver_extract_bytes(_py, response_bits, "response payload") {
                        Ok(value) => value,
                        Err(bits) => {
                            dec_ref_bits(_py, response_bits);
                            if let Some(exc_bits) = deferred_exception_bits.take() {
                                dec_ref_bits(_py, exc_bits);
                            }
                            dec_ref_bits(_py, request_tuple_bits);
                            return bits;
                        }
                    };
                dec_ref_bits(_py, response_bits);
                let mut runtime = socketserver_runtime()
                    .lock()
                    .expect("socketserver runtime poisoned");
                let request_id_u64 = request_id as u64;
                let Some(owner) = runtime.request_server.get(&request_id_u64).copied() else {
                    dec_ref_bits(_py, request_tuple_bits);
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        return crate::molt_raise(exc_bits);
                    }
                    return MoltObject::none().bits();
                };
                if owner != server_bits {
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        dec_ref_bits(_py, exc_bits);
                    }
                    dec_ref_bits(_py, request_tuple_bits);
                    return raise_exception::<_>(_py, "RuntimeError", "request id owner mismatch");
                }
                let Some(pending) = runtime.pending_requests.get_mut(&request_id_u64) else {
                    runtime.request_server.remove(&request_id_u64);
                    dec_ref_bits(_py, request_tuple_bits);
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        return crate::molt_raise(exc_bits);
                    }
                    return MoltObject::none().bits();
                };
                pending.response = Some(response);
            } else if response_bytes_bits != missing {
                dec_ref_bits(_py, response_bytes_bits);
            }
        }

        dec_ref_bits(_py, request_tuple_bits);
        if let Some(exc_bits) = deferred_exception_bits.take() {
            return crate::molt_raise(exc_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_shutdown(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !urllib_request_set_attr(
            _py,
            server_bits,
            b"_molt_shutdown_request",
            MoltObject::from_bool(true).bits(),
        ) || !urllib_request_set_attr(
            _py,
            server_bits,
            b"_closed",
            MoltObject::from_bool(true).bits(),
        ) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_read_request(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_read_request_impl(_py, handler_bits) {
            Ok(state) => MoltObject::from_int(state).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_compute_close_connection(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_compute_close_connection_impl(_py, handler_bits) {
            Ok(close) => MoltObject::from_bool(close).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_handle_one_request(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_handle_one_request_impl(_py, handler_bits) {
            Ok(keep_running) => MoltObject::from_bool(keep_running).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_response(
    handler_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_response_only(
    handler_bits: u64,
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
        match http_server_send_response_only_impl(_py, handler_bits, code, message) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_header(
    handler_bits: u64,
    keyword_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let keyword = crate::format_obj_str(_py, obj_from_bits(keyword_bits));
        let value = crate::format_obj_str(_py, obj_from_bits(value_bits));
        match http_server_send_header_impl(_py, handler_bits, &keyword, &value) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_end_headers(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_end_headers_impl(_py, handler_bits) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_error(
    handler_bits: u64,
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
        match http_server_send_error_impl(_py, handler_bits, code, message) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_version_string(
    server_version_bits: u64,
    sys_version_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let server_version = crate::format_obj_str(_py, obj_from_bits(server_version_bits));
        let sys_version = if obj_from_bits(sys_version_bits).is_none() {
            String::new()
        } else {
            crate::format_obj_str(_py, obj_from_bits(sys_version_bits))
        };
        let out = http_server_version_string_impl(&server_version, &sys_version);
        let Some(bits) = alloc_string_bits(_py, &out) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_date_time_string(timestamp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let out = match http_server_date_time_string_from_bits(_py, timestamp_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(bits) = alloc_string_bits(_py, &out) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_status_reason(code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "status code must be int");
        };
        let phrase = http_server_reason_phrase(code);
        let ptr = alloc_string(_py, phrase.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_status_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        let entries = http_status_constants();
        let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        for (name, code) in entries.iter().copied() {
            let key_ptr = alloc_string(_py, name.as_bytes());
            if key_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(code).bits();
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
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_status_responses() -> u64 {
    crate::with_gil_entry!(_py, {
        let entries = http_status_constants();
        let mut seen_codes: HashSet<i64> = HashSet::new();
        let mut pairs: Vec<u64> = Vec::new();
        let mut owned_bits: Vec<u64> = Vec::new();
        for (_, code) in entries.iter().copied() {
            if !seen_codes.insert(code) {
                continue;
            }
            let key_bits = MoltObject::from_int(code).bits();
            let value_ptr = alloc_string(_py, http_server_reason_phrase(code).as_bytes());
            if value_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
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
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_request_init(
    self_bits: u64,
    url_bits: u64,
    data_bits: u64,
    headers_bits: u64,
    method_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let url_text = crate::format_obj_str(_py, obj_from_bits(url_bits));
        let url_ptr = alloc_string(_py, url_text.as_bytes());
        if url_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let full_url_bits = MoltObject::from_ptr(url_ptr).bits();
        let mut headers_value = headers_bits;
        if obj_from_bits(headers_bits).is_none() {
            let dict_bits = crate::molt_dict_new(0);
            if obj_from_bits(dict_bits).is_none() {
                return MoltObject::none().bits();
            }
            headers_value = dict_bits;
        }
        if !urllib_request_set_attr(_py, self_bits, b"full_url", full_url_bits)
            || !urllib_request_set_attr(_py, self_bits, b"data", data_bits)
            || !urllib_request_set_attr(_py, self_bits, b"headers", headers_value)
            || !urllib_request_set_attr(_py, self_bits, b"method", method_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_opener_init(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_ptr = alloc_list_with_capacity(_py, &[], 0);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let handlers_bits = MoltObject::from_ptr(list_ptr).bits();
        if !urllib_request_set_attr(_py, self_bits, b"_molt_handlers", handlers_bits)
            || !urllib_request_set_cursor(_py, self_bits, 0)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_add_handler(opener_bits: u64, handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_bits = match urllib_request_ensure_handlers_list(_py, opener_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        if !urllib_request_set_attr(_py, handler_bits, b"parent", opener_bits) {
            return MoltObject::none().bits();
        }
        let new_order = match urllib_request_handler_order(_py, handler_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "opener handler registry is invalid");
        };
        let existing: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
        let mut insert_at = existing.len();
        for (idx, existing_bits) in existing.iter().copied().enumerate() {
            let existing_order = match urllib_request_handler_order(_py, existing_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if new_order < existing_order {
                insert_at = idx;
                break;
            }
        }
        let index_bits = MoltObject::from_int(insert_at as i64).bits();
        let _ = molt_list_insert(list_bits, index_bits, handler_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_open(opener_bits: u64, request_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut owned_request_refs: Vec<u64> = Vec::new();
        let out_bits = (|| -> u64 {
            let mut active_request_bits = request_bits;
            let mut full_url = {
                let Some(full_url_bits) =
                    (match urllib_request_attr_optional(_py, active_request_bits, b"full_url") {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "request object is missing full_url",
                    );
                };
                let Some(text) = string_obj_to_owned(obj_from_bits(full_url_bits)) else {
                    dec_ref_bits(_py, full_url_bits);
                    return raise_exception::<_>(_py, "TypeError", "request.full_url must be str");
                };
                dec_ref_bits(_py, full_url_bits);
                text
            };
            let mut scheme = urllib_split_scheme(&full_url, "").0;

            let previous_cursor = match urllib_request_get_cursor(_py, opener_bits) {
                Ok(value) => value.max(0),
                Err(bits) => return bits,
            };
            let list_bits = match urllib_request_ensure_handlers_list(_py, opener_bits) {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
            let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "opener handler registry is invalid",
                );
            };
            let handlers: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
            let start_idx = (previous_cursor as usize).min(handlers.len());

            let request_method_name = format!("{}_request", scheme);
            for (idx, handler_bits) in handlers.iter().copied().enumerate().skip(start_idx) {
                let Some(method_bits) = (match urllib_request_attr_optional(
                    _py,
                    handler_bits,
                    request_method_name.as_bytes(),
                ) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                }) else {
                    continue;
                };
                if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
                    dec_ref_bits(_py, method_bits);
                    continue;
                }
                if !urllib_request_set_cursor(_py, opener_bits, (idx + 1) as i64) {
                    dec_ref_bits(_py, method_bits);
                    return MoltObject::none().bits();
                }
                let out_bits = unsafe { call_callable1(_py, method_bits, active_request_bits) };
                dec_ref_bits(_py, method_bits);
                if !urllib_request_set_cursor(_py, opener_bits, previous_cursor) {
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                    return MoltObject::none().bits();
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    active_request_bits = out_bits;
                    owned_request_refs.push(out_bits);
                } else {
                    dec_ref_bits(_py, out_bits);
                }
            }

            full_url = {
                let Some(full_url_bits) =
                    (match urllib_request_attr_optional(_py, active_request_bits, b"full_url") {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "request object is missing full_url",
                    );
                };
                let Some(text) = string_obj_to_owned(obj_from_bits(full_url_bits)) else {
                    dec_ref_bits(_py, full_url_bits);
                    return raise_exception::<_>(_py, "TypeError", "request.full_url must be str");
                };
                dec_ref_bits(_py, full_url_bits);
                text
            };
            scheme = urllib_split_scheme(&full_url, "").0;

            let method_name = format!("{}_open", scheme);
            for (idx, handler_bits) in handlers.iter().copied().enumerate().skip(start_idx) {
                let Some(method_bits) = (match urllib_request_attr_optional(
                    _py,
                    handler_bits,
                    method_name.as_bytes(),
                ) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                }) else {
                    continue;
                };
                if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
                    dec_ref_bits(_py, method_bits);
                    continue;
                }
                if !urllib_request_set_cursor(_py, opener_bits, (idx + 1) as i64) {
                    dec_ref_bits(_py, method_bits);
                    return MoltObject::none().bits();
                }
                let out_bits = unsafe { call_callable1(_py, method_bits, active_request_bits) };
                dec_ref_bits(_py, method_bits);
                if !urllib_request_set_cursor(_py, opener_bits, previous_cursor) {
                    return MoltObject::none().bits();
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    return out_bits;
                }
            }
            if !urllib_request_set_cursor(_py, opener_bits, previous_cursor) {
                return MoltObject::none().bits();
            }

            let allow_data_fallback = match urllib_request_attr_optional(
                _py,
                opener_bits,
                b"_molt_allow_data_fallback",
            ) {
                Ok(Some(bits)) => {
                    let value = is_truthy(_py, obj_from_bits(bits));
                    dec_ref_bits(_py, bits);
                    value
                }
                Ok(None) => false,
                Err(bits) => return bits,
            };

            let mut response_bits = if scheme == "data" && allow_data_fallback {
                let payload = match urllib_request_decode_data_url(&full_url) {
                    Ok(value) => value,
                    Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
                };
                let Some(handle) = urllib_response_store(urllib_response_from_parts(
                    payload,
                    full_url.clone(),
                    // CPython's data: handler returns an addinfourl without HTTP status metadata.
                    -1,
                    String::new(),
                    vec![("Content-Type".to_string(), "text/plain".to_string())],
                )) else {
                    return MoltObject::none().bits();
                };
                urllib_http_make_response_bits(_py, handle)
            } else if scheme == "http" || scheme == "https" {
                let split = urllib_urlsplit_impl(&full_url, "", true);
                let netloc = split[1].clone();
                if netloc.is_empty() {
                    return urllib_raise_url_error(_py, "no host given");
                }
                let default_port = if scheme == "https" { 443 } else { 80 };
                let (target_host, _target_port) =
                    urllib_http_parse_host_port(&netloc, default_port);
                if target_host.is_empty() {
                    return urllib_raise_url_error(_py, "no host given");
                }
                let timeout = match urllib_http_request_timeout(_py, active_request_bits) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let (mut method, mut body) =
                    match urllib_http_extract_method_and_body(_py, active_request_bits) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                let mut base_headers =
                    match urllib_http_extract_request_headers(_py, active_request_bits) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                let proxy = match urllib_http_find_proxy_for_scheme(
                    _py,
                    opener_bits,
                    &scheme,
                    &target_host,
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let mut proxy_auth_header: Option<String> = None;
                let mut proxy_auth_attempted = false;
                let mut current_url = full_url.clone();
                let mut redirects = 0usize;
                let cookiejar_handles = match urllib_cookiejar_handles_from_handlers(_py, &handlers)
                {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                loop {
                    let parts = urllib_urlsplit_impl(&current_url, "", true);
                    let netloc_now = parts[1].clone();
                    let mut path = parts[2].clone();
                    if path.is_empty() {
                        path = "/".to_string();
                    }
                    if !parts[3].is_empty() {
                        path.push('?');
                        path.push_str(&parts[3]);
                    }
                    let (host_now, port_now) =
                        urllib_http_parse_host_port(&netloc_now, default_port);
                    if host_now.is_empty() {
                        return urllib_raise_url_error(_py, "no host given");
                    }
                    let mut effective_headers = base_headers.clone();
                    urllib_cookiejar_apply_header_for_url(
                        _py,
                        &cookiejar_handles,
                        &current_url,
                        &mut effective_headers,
                    );
                    if let Some(proxy_auth_value) = proxy_auth_header.as_ref() {
                        let mut replaced = false;
                        for (name, value) in &mut effective_headers {
                            if name.eq_ignore_ascii_case("Proxy-Authorization") {
                                *value = proxy_auth_value.clone();
                                replaced = true;
                                break;
                            }
                        }
                        if !replaced {
                            effective_headers.push((
                                "Proxy-Authorization".to_string(),
                                proxy_auth_value.clone(),
                            ));
                        }
                    }
                    let req = if let Some(proxy_url) = proxy.as_deref() {
                        let proxy_parts = urllib_urlsplit_impl(proxy_url, "", true);
                        let proxy_netloc = proxy_parts[1].clone();
                        let (proxy_host, proxy_port) =
                            urllib_http_parse_host_port(&proxy_netloc, 80);
                        if proxy_host.is_empty() {
                            return urllib_raise_url_error(_py, "proxy URL is invalid");
                        }
                        if scheme == "https" {
                            return urllib_raise_url_error(_py, "https proxies are not supported");
                        }
                        UrllibHttpRequest {
                            host: proxy_host,
                            port: proxy_port,
                            path: current_url.clone(),
                            method: method.clone(),
                            headers: {
                                let mut out = Vec::new();
                                out.append(&mut effective_headers);
                                out
                            },
                            body: body.clone(),
                            timeout,
                        }
                    } else {
                        UrllibHttpRequest {
                            host: host_now.clone(),
                            port: port_now,
                            path: path.clone(),
                            method: method.clone(),
                            headers: {
                                let mut out = Vec::new();
                                out.append(&mut effective_headers);
                                out
                            },
                            body: body.clone(),
                            timeout,
                        }
                    };
                    let host_header = if port_now == default_port {
                        host_now.clone()
                    } else {
                        format!("{host_now}:{port_now}")
                    };
                    let (code, reason, resp_headers, resp_body) =
                        match urllib_http_try_inmemory_dispatch(_py, &req, &req.path, &host_header)
                        {
                            Ok(Some(value)) => value,
                            Ok(None) => {
                                match urllib_http_send_request(&req, &req.path, &host_header) {
                                    Ok(value) => value,
                                    Err(err) => {
                                        if err.kind() == ErrorKind::TimedOut
                                            || err.kind() == ErrorKind::WouldBlock
                                        {
                                            return urllib_http_timeout_error(_py);
                                        }
                                        return urllib_raise_url_error(_py, &err.to_string());
                                    }
                                }
                            }
                            Err(bits) => return bits,
                        };
                    urllib_cookiejar_store_headers_for_url(
                        &cookiejar_handles,
                        &current_url,
                        &resp_headers,
                    );
                    if code == 407 && proxy.is_some() {
                        if proxy_auth_attempted {
                            return urllib_raise_url_error(_py, "proxy authentication required");
                        }
                        let proxy_url = proxy.as_deref().unwrap_or_default();
                        let challenge =
                            urllib_http_find_header(&resp_headers, "Proxy-Authenticate")
                                .unwrap_or_default();
                        let realm = urllib_http_parse_basic_realm(challenge);
                        let creds = match urllib_proxy_find_basic_credentials(
                            _py,
                            &handlers,
                            proxy_url,
                            realm.as_deref(),
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let Some((username, password)) = creds else {
                            return urllib_raise_url_error(_py, "proxy authentication required");
                        };
                        let token =
                            urllib_base64_encode(format!("{username}:{password}").as_bytes());
                        proxy_auth_header = Some(format!("Basic {token}"));
                        proxy_auth_attempted = true;
                        continue;
                    }
                    let location =
                        urllib_http_find_header(&resp_headers, "Location").map(str::to_string);
                    if (code == 301 || code == 302 || code == 303 || code == 307 || code == 308)
                        && location.is_some()
                    {
                        if redirects >= 10 {
                            return urllib_raise_url_error(_py, "redirect loop");
                        }
                        redirects += 1;
                        let next =
                            urllib_http_join_url(&current_url, location.as_deref().unwrap_or(""));
                        current_url = next;
                        if code == 303 || ((code == 301 || code == 302) && method != "HEAD") {
                            method = "GET".to_string();
                            body.clear();
                            base_headers.retain(|(name, _)| {
                                !name.eq_ignore_ascii_case("Content-Length")
                                    && !name.eq_ignore_ascii_case("Content-Type")
                            });
                        }
                        continue;
                    }
                    let Some(handle) = urllib_response_store(urllib_response_from_parts(
                        resp_body,
                        current_url.clone(),
                        code,
                        reason,
                        resp_headers,
                    )) else {
                        return MoltObject::none().bits();
                    };
                    break urllib_http_make_response_bits(_py, handle);
                }
            } else {
                return MoltObject::none().bits();
            };

            let response_method_name = format!("{}_response", scheme);
            for handler_bits in handlers {
                let Some(method_bits) = (match urllib_request_attr_optional(
                    _py,
                    handler_bits,
                    response_method_name.as_bytes(),
                ) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                }) else {
                    continue;
                };
                if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
                    dec_ref_bits(_py, method_bits);
                    continue;
                }
                let out_bits =
                    unsafe { call_callable2(_py, method_bits, active_request_bits, response_bits) };
                dec_ref_bits(_py, method_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    if out_bits == response_bits {
                        // A handler may return the same response object it was passed.
                        // Keep the existing owned `response_bits` reference intact.
                    } else {
                        dec_ref_bits(_py, response_bits);
                        response_bits = out_bits;
                    }
                } else {
                    dec_ref_bits(_py, out_bits);
                }
            }
            response_bits
        })();
        for bits in owned_request_refs {
            dec_ref_bits(_py, bits);
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_process_http_error(
    request_bits: u64,
    response_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match urllib_request_response_handle_from_bits(_py, response_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some((code, reason, headers, url)) = urllib_response_with(handle, |resp| {
            (
                resp.code,
                resp.reason.clone(),
                resp.headers.clone(),
                resp.url.clone(),
            )
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        if code >= 400 {
            return urllib_raise_http_error(_py, &url, code, &reason, &headers, response_bits);
        }
        let _ = request_bits;
        response_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_read(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let size_opt = if obj_from_bits(size_bits).is_none() {
            None
        } else {
            to_i64(obj_from_bits(size_bits))
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_read_vec(resp, size_opt))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(data) => {
                let ptr = crate::alloc_bytes(_py, data.as_slice());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readinto(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let mut export = crate::BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if unsafe { crate::molt_buffer_export(buffer_bits, &mut export) } != 0
            || export.readonly != 0
            || export.itemsize != 1
            || export.stride != 1
        {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "readinto() argument must be a writable bytes-like object",
            );
        }
        let out_len = export.len as usize;
        if out_len == 0 {
            return MoltObject::from_int(0).bits();
        }
        let out_buf = unsafe { std::slice::from_raw_parts_mut(export.ptr as *mut u8, out_len) };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_readinto_len(resp, out_buf))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(read_len) => MoltObject::from_int(read_len as i64).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_read1(handle_bits: u64, size_bits: u64) -> u64 {
    molt_urllib_request_response_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readinto1(
    handle_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readline(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let size_opt = if obj_from_bits(size_bits).is_none() {
            None
        } else {
            to_i64(obj_from_bits(size_bits))
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_readline_vec(resp, size_opt))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(data) => {
                let ptr = crate::alloc_bytes(_py, data.as_slice());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readlines(handle_bits: u64, hint_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let hint_obj = obj_from_bits(hint_bits);
        let hint = if hint_obj.is_none() {
            None
        } else {
            match to_i64(hint_obj) {
                Some(value) if value <= 0 => None,
                Some(value) => Some(usize::try_from(value).unwrap_or(usize::MAX)),
                None => return raise_exception::<_>(_py, "TypeError", "hint must be int or None"),
            }
        };
        let Some(out) = urllib_response_with_mut(handle, |resp| {
            if resp.closed {
                if urllib_response_is_data(resp) {
                    return Err(raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "I/O operation on closed file.",
                    ));
                }
                let list_ptr = alloc_list_with_capacity(_py, &[], 0);
                if list_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                return Ok(MoltObject::from_ptr(list_ptr).bits());
            }
            let mut lines: Vec<u64> = Vec::new();
            let mut total = 0usize;
            loop {
                let line = match urllib_response_readline_vec(resp, None) {
                    Ok(data) => data,
                    Err(msg) => return Err(raise_exception::<u64>(_py, "ValueError", &msg)),
                };
                if line.is_empty() {
                    break;
                }
                total = total.saturating_add(line.len());
                let line_ptr = alloc_bytes(_py, line.as_slice());
                if line_ptr.is_null() {
                    for bits in lines {
                        dec_ref_bits(_py, bits);
                    }
                    return Err(MoltObject::none().bits());
                }
                lines.push(MoltObject::from_ptr(line_ptr).bits());
                if let Some(limit) = hint
                    && total >= limit
                {
                    break;
                }
            }
            let list_ptr = alloc_list_with_capacity(_py, lines.as_slice(), lines.len());
            if list_ptr.is_null() {
                for bits in lines {
                    dec_ref_bits(_py, bits);
                }
                return Err(MoltObject::none().bits());
            }
            for bits in lines {
                dec_ref_bits(_py, bits);
            }
            Ok(MoltObject::from_ptr(list_ptr).bits())
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.closed && urllib_response_is_data(resp) {
                return Err("I/O operation on closed file.".to_string());
            }
            Ok(true)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_writable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.closed && urllib_response_is_data(resp) {
                return Err("I/O operation on closed file.".to_string());
            }
            Ok(urllib_response_is_data(resp))
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_seekable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.closed && urllib_response_is_data(resp) {
                return Err("I/O operation on closed file.".to_string());
            }
            Ok(urllib_response_is_data(resp))
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_tell(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
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
            Ok(resp.pos)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(pos) => MoltObject::from_int(pos as i64).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_seek(
    handle_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(()) = urllib_response_with_mut(handle, |resp| {
            resp.closed = true;
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_drop(_py, handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_geturl(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            let ptr = alloc_string(_py, resp.url.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_getcode(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(code) = urllib_response_with(handle, |resp| resp.code) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        if code < 0 {
            MoltObject::none().bits()
        } else {
            MoltObject::from_int(code).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_getreason(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.reason.is_empty() {
                return Ok(MoltObject::none().bits());
            }
            let ptr = alloc_string(_py, resp.reason.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_getheader(
    handle_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let name = crate::format_obj_str(_py, obj_from_bits(name_bits));
        let Some(out) = urllib_response_with(handle, |resp| {
            let Some(joined) = urllib_response_joined_header(resp, name.as_str()) else {
                return Ok(None);
            };
            let ptr = alloc_string(_py, joined.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(Some(MoltObject::from_ptr(ptr).bits()))
            }
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_getheaders(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_headers_dict_bits(_py, resp))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_getheaders_list(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_headers_list_bits(_py, resp))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_message(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_message_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_parse_header_pairs(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match socketserver_extract_bytes(_py, data_bits, "header data") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let headers = http_parse_header_pairs(raw.as_slice());
        match urllib_http_headers_to_list(_py, &headers) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = http_message_store_new() else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_parse(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw = match socketserver_extract_bytes(_py, data_bits, "header data") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let headers = http_parse_header_pairs(raw.as_slice());
        let Some(handle) = http_message_store(headers) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_set_raw(
    handle_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_get(
    handle_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle =
            http_message_header_key(crate::format_obj_str(_py, obj_from_bits(name_bits)).as_str());
        let Some(out) = http_message_with(handle, |message| {
            let Some(idx) = message
                .index
                .get(&needle)
                .and_then(|positions| positions.last())
            else {
                return Ok(None);
            };
            let ptr = alloc_string(_py, message.headers[*idx].1.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(Some(MoltObject::from_ptr(ptr).bits()))
            }
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        match out {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_get_all(handle_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle =
            http_message_header_key(crate::format_obj_str(_py, obj_from_bits(name_bits)).as_str());
        let Some(out) = http_message_with(handle, |message| {
            let indices = message
                .index
                .get(&needle)
                .map(Vec::as_slice)
                .unwrap_or_default();
            http_message_values_to_list_from_indices(_py, message, indices)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_items(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out) = http_message_with_mut(handle, |message| {
            if let Some(cached_bits) = message.items_list_cache
                && !obj_from_bits(cached_bits).is_none()
            {
                inc_ref_bits(_py, cached_bits);
                return Ok(cached_bits);
            }
            let out_bits = urllib_http_headers_to_list(_py, &message.headers)?;
            inc_ref_bits(_py, out_bits);
            message.items_list_cache = Some(out_bits);
            Ok(out_bits)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_contains(handle_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle =
            http_message_header_key(crate::format_obj_str(_py, obj_from_bits(name_bits)).as_str());
        let Some(found) = http_message_with(handle, |message| message.index.contains_key(&needle))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(len_value) = http_message_with(handle, |message| message.headers.len()) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        let len_i64 = i64::try_from(len_value).unwrap_or(i64::MAX);
        MoltObject::from_int(len_i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        http_message_drop(_py, handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_new(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(host) = string_obj_to_owned(obj_from_bits(host_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "host must be str");
        };
        let Some(port_value) = to_i64(obj_from_bits(port_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "port must be int");
        };
        if !(0..=u16::MAX as i64).contains(&port_value) {
            return raise_exception::<_>(_py, "ValueError", "port out of range");
        }
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits))
                .or_else(|| to_i64(obj_from_bits(timeout_bits)).map(|v| v as f64))
            else {
                return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
            };
            Some(value)
        };
        let Some(handle) = http_client_connection_store(host, port_value as u16, timeout) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_putrequest(
    handle_bits: u64,
    method_bits: u64,
    url_bits: u64,
    skip_host_bits: u64,
    skip_accept_encoding_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let skip_host = is_truthy(_py, obj_from_bits(skip_host_bits));
        let skip_accept_encoding = is_truthy(_py, obj_from_bits(skip_accept_encoding_bits));
        let Some(buffer) = http_client_connection_with_mut(handle, |conn| {
            conn.method = Some(method.clone());
            conn.url = Some(url.clone());
            conn.headers.clear();
            conn.body.clear();
            conn.buffer.clear();
            conn.skip_host = skip_host;
            conn.skip_accept_encoding = skip_accept_encoding;
            conn.buffer
                .push(format!("{method} {url} HTTP/1.1\r\n").into_bytes());
            conn.buffer.clone()
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
        };
        http_client_alloc_buffer_list(_py, &buffer)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_putheader(
    handle_bits: u64,
    header_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(header) = string_obj_to_owned(obj_from_bits(header_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header value must be str");
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            if conn.method.is_none() || conn.url.is_none() {
                return Err("request not started");
            }
            conn.headers.push((header, value));
            Ok(conn.buffer.clone())
        });
        match state {
            Some(Ok(buffer)) => http_client_alloc_buffer_list(_py, &buffer),
            Some(Err(msg)) => raise_exception::<_>(_py, "OSError", msg),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_endheaders(handle_bits: u64, body_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let body = if obj_from_bits(body_bits).is_none() {
            None
        } else {
            match socketserver_extract_bytes(_py, body_bits, "message_body") {
                Ok(value) => Some(value),
                Err(bits) => return bits,
            }
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            if conn.method.is_none() || conn.url.is_none() {
                return Err("request not started");
            }
            if conn
                .buffer
                .last()
                .is_none_or(|line| line.as_slice() != b"\r\n")
            {
                http_client_apply_default_headers(
                    &mut conn.headers,
                    conn.host.as_str(),
                    conn.port,
                    conn.skip_host,
                    conn.skip_accept_encoding,
                );
                for (name, value) in &conn.headers {
                    conn.buffer
                        .push(format!("{name}: {value}\r\n").into_bytes());
                }
                conn.buffer.push(b"\r\n".to_vec());
            }
            if let Some(chunk) = body.as_ref() {
                conn.body.extend_from_slice(chunk.as_slice());
                conn.buffer.push(chunk.clone());
            }
            Ok(conn.buffer.clone())
        });
        match state {
            Some(Ok(buffer)) => http_client_alloc_buffer_list(_py, &buffer),
            Some(Err(msg)) => raise_exception::<_>(_py, "OSError", msg),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_send(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data = match socketserver_extract_bytes(_py, data_bits, "data") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            if conn.method.is_none() || conn.url.is_none() {
                return Err("request not started");
            }
            conn.body.extend_from_slice(data.as_slice());
            conn.buffer.push(data);
            Ok(conn.buffer.clone())
        });
        match state {
            Some(Ok(buffer)) => http_client_alloc_buffer_list(_py, &buffer),
            Some(Err(msg)) => raise_exception::<_>(_py, "OSError", msg),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_request(
    handle_bits: u64,
    method_bits: u64,
    url_bits: u64,
    body_bits: u64,
    headers_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let mut headers = if obj_from_bits(headers_bits).is_none() {
            Vec::new()
        } else {
            match urllib_http_extract_headers_mapping(_py, headers_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        let body = if obj_from_bits(body_bits).is_none() {
            None
        } else {
            match socketserver_extract_bytes(_py, body_bits, "body") {
                Ok(value) => Some(value),
                Err(bits) => return bits,
            }
        };
        if let Some(payload) = body.as_ref()
            && !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        {
            headers.push(("Content-Length".to_string(), payload.len().to_string()));
        }
        let state = http_client_connection_with_mut(handle, |conn| {
            conn.method = Some(method.clone());
            conn.url = Some(url.clone());
            conn.headers = headers;
            conn.body = body.unwrap_or_default();
            conn.skip_host = false;
            conn.skip_accept_encoding = true;
            conn.buffer.clear();
            conn.buffer
                .push(format!("{method} {url} HTTP/1.1\r\n").into_bytes());
            conn.buffer.clone()
        });
        match state {
            Some(buffer) => http_client_alloc_buffer_list(_py, &buffer),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_getresponse(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            let Some(method) = conn.method.clone() else {
                return Err("no request pending");
            };
            let Some(url) = conn.url.clone() else {
                return Err("no request pending");
            };
            http_client_apply_default_headers(
                &mut conn.headers,
                conn.host.as_str(),
                conn.port,
                conn.skip_host,
                conn.skip_accept_encoding,
            );
            Ok((
                conn.host.clone(),
                conn.port,
                conn.timeout,
                method,
                url,
                conn.headers.clone(),
                conn.body.clone(),
            ))
        });
        let (host, port, timeout, method, url, headers, body) = match state {
            Some(Ok(value)) => value,
            Some(Err(msg)) => return raise_exception::<_>(_py, "OSError", msg),
            None => {
                return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
            }
        };
        let response_handle = match http_client_execute_request(
            _py,
            HttpClientExecuteInput {
                host,
                port,
                timeout,
                method,
                url,
                headers,
                body,
                skip_host: true,
                skip_accept_encoding: true,
            },
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _ = http_client_connection_with_mut(handle, http_client_connection_reset_pending);
        MoltObject::from_int(response_handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(()) =
            http_client_connection_with_mut(handle, http_client_connection_reset_pending)
        else {
            return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        http_client_connection_drop(handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_get_buffer(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(buffer) = http_client_connection_with(handle, |conn| conn.buffer.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
        };
        http_client_alloc_buffer_list(_py, &buffer)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_execute(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
    method_bits: u64,
    url_bits: u64,
    headers_bits: u64,
    body_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(host) = string_obj_to_owned(obj_from_bits(host_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "host must be str");
        };
        let Some(port_value) = to_i64(obj_from_bits(port_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "port must be int");
        };
        if !(0..=u16::MAX as i64).contains(&port_value) {
            return raise_exception::<_>(_py, "ValueError", "port out of range");
        }
        let port = port_value as u16;
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
            };
            Some(value)
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let headers = match http_client_extract_headers(_py, headers_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let body = if obj_from_bits(body_bits).is_none() {
            Vec::new()
        } else {
            match socketserver_extract_bytes(_py, body_bits, "body") {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        match http_client_execute_request(
            _py,
            HttpClientExecuteInput {
                host,
                port,
                timeout,
                method,
                url,
                headers,
                body,
                skip_host: true,
                skip_accept_encoding: true,
            },
        ) {
            Ok(handle) => MoltObject::from_int(handle).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_urllib_request_response_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_close(handle_bits: u64) -> u64 {
    molt_urllib_request_response_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_drop(handle_bits: u64) -> u64 {
    molt_urllib_request_response_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getstatus(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(code) = urllib_response_with(handle, |resp| resp.code) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        let status = if code < 0 { 0 } else { code };
        MoltObject::from_int(status).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getreason(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            let ptr = alloc_string(_py, resp.reason.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getheader(
    handle_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            let Some(joined) = urllib_response_joined_header(resp, name.as_str()) else {
                return Ok(None);
            };
            let ptr = alloc_string(_py, joined.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(Some(MoltObject::from_ptr(ptr).bits()))
            }
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getheaders(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_headers_list_bits(_py, resp))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_message(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        urllib_response_message_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pkgutil_iter_modules(path_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let paths = if obj_from_bits(path_bits).is_none() {
            Vec::new()
        } else {
            match iterable_to_string_vec(_py, path_bits) {
                Ok(paths) => paths,
                Err(bits) => return bits,
            }
        };
        let out = pkgutil_iter_modules_impl(&paths, &prefix);
        alloc_pkgutil_module_info_list(_py, &out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pkgutil_walk_packages(path_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let paths = if obj_from_bits(path_bits).is_none() {
            Vec::new()
        } else {
            match iterable_to_string_vec(_py, path_bits) {
                Ok(paths) => paths,
                Err(bits) => return bits,
            }
        };
        let out = pkgutil_walk_packages_impl(&paths, &prefix);
        alloc_pkgutil_module_info_list(_py, &out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_copyfile(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") || !crate::has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let Some(src) = string_obj_to_owned(obj_from_bits(src_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "src must be str");
        };
        let Some(dst) = string_obj_to_owned(obj_from_bits(dst_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "dst must be str");
        };
        if let Err(err) = fs::copy(&src, &dst) {
            return raise_os_error_from_io(_py, err);
        }
        let out_ptr = alloc_string(_py, dst.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_which(cmd_bits: u64, path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") || !crate::has_capability(_py, "env.read") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/env.read capability",
            );
        }
        let Some(cmd) = string_obj_to_owned(obj_from_bits(cmd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cmd must be str");
        };
        if cmd.is_empty() {
            return MoltObject::none().bits();
        }
        let path = if obj_from_bits(path_bits).is_none() {
            env_state_get("PATH")
                .or_else(|| std::env::var("PATH").ok())
                .unwrap_or_default()
        } else {
            let Some(path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "path must be str or None");
            };
            path
        };

        #[cfg(windows)]
        let pathexts: Vec<String> = {
            let raw = env_state_get("PATHEXT")
                .or_else(|| std::env::var("PATHEXT").ok())
                .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
            raw.split(';')
                .filter_map(|entry| {
                    let trimmed = entry.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .collect()
        };

        let cmd_path = Path::new(&cmd);
        let has_path_sep =
            cmd.contains(std::path::MAIN_SEPARATOR) || (cfg!(windows) && cmd.contains('/'));

        let check_candidate = |candidate: PathBuf| -> Option<u64> {
            #[cfg(windows)]
            {
                if path_is_executable(&candidate) {
                    return Some(alloc_optional_path(_py, &candidate));
                }
                let ext_present = candidate
                    .extension()
                    .map(|ext| !ext.is_empty())
                    .unwrap_or(false);
                if !ext_present {
                    for ext in &pathexts {
                        let ext_clean = ext.trim_start_matches('.');
                        let with_ext = candidate.with_extension(ext_clean);
                        if path_is_executable(&with_ext) {
                            return Some(alloc_optional_path(_py, &with_ext));
                        }
                    }
                }
                None
            }
            #[cfg(not(windows))]
            {
                if path_is_executable(&candidate) {
                    Some(alloc_optional_path(_py, &candidate))
                } else {
                    None
                }
            }
        };

        if cmd_path.is_absolute() || has_path_sep {
            if let Some(bits) = check_candidate(PathBuf::from(&cmd)) {
                return bits;
            }
            return MoltObject::none().bits();
        }

        #[cfg(windows)]
        let path_sep = ';';
        #[cfg(not(windows))]
        let path_sep = ':';
        for entry in path.split(path_sep) {
            let dir = if entry.is_empty() { "." } else { entry };
            let candidate = Path::new(dir).join(&cmd);
            if let Some(bits) = check_candidate(candidate) {
                return bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_py_compile_compile(file_bits: u64, cfile_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") || !crate::has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let Some(file) = string_obj_to_owned(obj_from_bits(file_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "file must be str");
        };
        let cfile = if obj_from_bits(cfile_bits).is_none() {
            format!("{file}c")
        } else {
            let Some(cfile) = string_obj_to_owned(obj_from_bits(cfile_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "cfile must be str or None");
            };
            cfile
        };

        let mut in_file = match fs::File::open(&file) {
            Ok(handle) => handle,
            Err(err) => return raise_os_error_from_io(_py, err),
        };
        let mut one = [0u8; 1];
        if let Err(err) = in_file.read(&mut one) {
            return raise_os_error_from_io(_py, err);
        }
        if let Err(err) = fs::File::create(&cfile) {
            return raise_os_error_from_io(_py, err);
        }
        let abs = absolutize_path(&cfile);
        let out_ptr = alloc_string(_py, abs.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_file(fullname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(fullname) = string_obj_to_owned(obj_from_bits(fullname_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fullname must be str");
        };
        MoltObject::from_bool(compileall_compile_file_impl(&fullname)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_dir(dir_bits: u64, maxlevels_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(dir) = string_obj_to_owned(obj_from_bits(dir_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "dir must be str");
        };
        let Some(maxlevels) = to_i64(obj_from_bits(maxlevels_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxlevels must be int");
        };
        MoltObject::from_bool(compileall_compile_dir_impl(&dir, maxlevels)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_path(
    paths_bits: u64,
    skip_curdir_bits: u64,
    maxlevels_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let paths = match iterable_to_string_vec(_py, paths_bits) {
            Ok(paths) => paths,
            Err(bits) => return bits,
        };
        let skip_curdir = is_truthy(_py, obj_from_bits(skip_curdir_bits));
        let Some(maxlevels) = to_i64(obj_from_bits(maxlevels_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxlevels must be int");
        };

        let mut success = true;
        for entry in paths {
            if skip_curdir && (entry.is_empty() || entry == ".") {
                continue;
            }
            if !compileall_compile_dir_impl(&entry, maxlevels) {
                success = false;
            }
        }
        MoltObject::from_bool(success).bits()
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

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_compile_builtin(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    dont_inherit_bits: u64,
    optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        if mode != "exec" && mode != "eval" && mode != "single" {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'",
            );
        }
        if to_i64(obj_from_bits(flags_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        }
        if to_i64(obj_from_bits(dont_inherit_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 5 must be int");
        }
        if to_i64(obj_from_bits(optimize_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 6 must be int");
        }
        if let Err((error_type, message)) = compile_validate_source(&source, &filename, &mode) {
            return raise_exception::<_>(_py, error_type, &message);
        }
        codeobj_from_filename_bits(_py, filename_bits)
    })
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_codeop_compile(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    incomplete_input_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        };
        let incomplete_input = is_truthy(_py, obj_from_bits(incomplete_input_bits));
        match codeop_compile_status(&source, &filename, &mode, flags, incomplete_input) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(result_ptr).bits()
            }
            CodeopCompileStatus::Incomplete => {
                raise_exception::<_>(_py, "SyntaxError", "incomplete input")
            }
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => raise_exception::<_>(_py, error_type, &message),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_codeop_compile_command(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        };

        let mut only_blank_or_comment = true;
        for line in source.split('\n') {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                only_blank_or_comment = false;
                break;
            }
        }
        if only_blank_or_comment && mode != "eval" {
            source = "pass".to_string();
        }
        if codeop_source_has_missing_indented_suite(&source) {
            return raise_exception::<_>(_py, "SyntaxError", "expected an indented block");
        }

        match codeop_compile_status(&source, &filename, &mode, flags, true) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(result_ptr).bits();
            }
            CodeopCompileStatus::Incomplete => {}
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => {
                if error_type != "SyntaxError" {
                    return raise_exception::<_>(_py, error_type, &message);
                }
            }
        }

        let source_newline = format!("{source}\n");
        match codeop_compile_status(&source_newline, &filename, &mode, flags, true) {
            CodeopCompileStatus::Compiled { .. } | CodeopCompileStatus::Incomplete => {
                let flags_out_bits = MoltObject::from_int(flags).bits();
                let result_ptr = alloc_tuple(_py, &[MoltObject::none().bits(), flags_out_bits]);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(result_ptr).bits();
            }
            CodeopCompileStatus::Error {
                error_type: "SyntaxError",
                ..
            } => {}
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => return raise_exception::<_>(_py, error_type, &message),
        }

        match codeop_compile_status(&source, &filename, &mode, flags, false) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(result_ptr).bits()
            }
            CodeopCompileStatus::Incomplete => {
                raise_exception::<_>(_py, "SyntaxError", "incomplete input")
            }
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => raise_exception::<_>(_py, error_type, &message),
        }
    })
}

// --- Stubs when stdlib_ast is disabled ---

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_compile_builtin(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
    _dont_inherit_bits: u64,
    _optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "NotImplementedError", "compile() requires the stdlib_ast feature")
    })
}

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_codeop_compile(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
    _incomplete_input_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "NotImplementedError", "compile() requires the stdlib_ast feature")
    })
}

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_codeop_compile_command(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "NotImplementedError", "compile() requires the stdlib_ast feature")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                function_set_trampoline_ptr(ptr, trampoline_ptr);
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_builtin(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = matches!(
            std::env::var("MOLT_TRACE_BUILTIN_FUNC").ok().as_deref(),
            Some("1")
        );
        let trace_enter_ptr = fn_addr!(molt_trace_enter_slot);
        if trace {
            eprintln!(
                "molt builtin_func new: fn_ptr=0x{fn_ptr:x} tramp_ptr=0x{trampoline_ptr:x} arity={arity}"
            );
        }
        if fn_ptr == 0 || trampoline_ptr == 0 {
            let msg = format!(
                "builtin func pointer missing: fn=0x{fn_ptr:x} tramp=0x{trampoline_ptr:x} arity={arity}"
            );
            return raise_exception::<_>(_py, "RuntimeError", &msg);
        }
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "RuntimeError", "builtin func alloc failed");
        }
        unsafe {
            function_set_trampoline_ptr(ptr, trampoline_ptr);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            object_set_class_bits(_py, ptr, builtin_bits);
            inc_ref_bits(_py, builtin_bits);
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        if trace && fn_ptr == trace_enter_ptr {
            eprintln!(
                "molt builtin_func trace_enter_slot bits=0x{bits:x} ptr=0x{:x}",
                ptr as usize
            );
        }
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_closure(
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
            let cell_bits = cell_class(_py);
            if cell_bits != 0 && !obj_from_bits(cell_bits).is_none() {
                let closure_obj = obj_from_bits(closure_bits);
                if let Some(closure_ptr) = closure_obj.as_ptr() {
                    unsafe {
                        if object_type_id(closure_ptr) == TYPE_ID_TUPLE {
                            for &entry_bits in seq_vec_ref(closure_ptr).iter() {
                                let entry_obj = obj_from_bits(entry_bits);
                                let Some(entry_ptr) = entry_obj.as_ptr() else {
                                    continue;
                                };
                                if object_type_id(entry_ptr) != TYPE_ID_LIST {
                                    continue;
                                }
                                if seq_vec_ref(entry_ptr).len() != 1 {
                                    continue;
                                }
                                let old_class_bits = object_class_bits(entry_ptr);
                                if old_class_bits == cell_bits {
                                    continue;
                                }
                                if old_class_bits != 0 {
                                    dec_ref_bits(_py, old_class_bits);
                                }
                                object_set_class_bits(_py, entry_ptr, cell_bits);
                                inc_ref_bits(_py, cell_bits);
                            }
                        }
                    }
                }
            }
        }
        unsafe {
            function_set_closure_bits(_py, ptr, closure_bits);
            function_set_trampoline_ptr(ptr, trampoline_ptr);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_set_builtin(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, func_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_get_code(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let code_bits = ensure_function_code_bits(_py, func_ptr);
            if obj_from_bits(code_bits).is_none() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, code_bits);
            code_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_get_globals(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let dict_bits = function_dict_bits(func_ptr);
            if dict_bits == 0 {
                return MoltObject::none().bits();
            }
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
            let Some(module_name_bits) = attr_name_bits_from_bytes(_py, b"__module__") else {
                return MoltObject::none().bits();
            };
            let Some(name_bits) = dict_get_in_place(_py, dict_ptr, module_name_bits) else {
                return MoltObject::none().bits();
            };
            let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            let Some(module_bits) = guard.get(&name) else {
                return MoltObject::none().bits();
            };
            let module_bits = *module_bits;
            inc_ref_bits(_py, module_bits);
            drop(guard);
            let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
                dec_ref_bits(_py, module_bits);
                return MoltObject::none().bits();
            };
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                dec_ref_bits(_py, module_bits);
                return MoltObject::none().bits();
            }
            let globals_bits = module_dict_bits(module_ptr);
            if obj_from_bits(globals_bits).is_none() {
                dec_ref_bits(_py, module_bits);
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, globals_bits);
            dec_ref_bits(_py, module_bits);
            globals_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_new(
    filename_bits: u64,
    name_bits: u64,
    firstlineno_bits: u64,
    linetable_bits: u64,
    varnames_bits: u64,
    argcount_bits: u64,
    posonlyargcount_bits: u64,
    kwonlyargcount_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let filename_obj = obj_from_bits(filename_bits);
        let Some(filename_ptr) = filename_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code filename must be str");
        };
        unsafe {
            if object_type_id(filename_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code filename must be str");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code name must be str");
            }
        }
        if !obj_from_bits(linetable_bits).is_none() {
            let Some(table_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code linetable must be tuple or None",
                );
            };
            unsafe {
                if object_type_id(table_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "code linetable must be tuple or None",
                    );
                }
            }
        }
        let Some(argcount) = to_i64(obj_from_bits(argcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code argcount must be int");
        };
        let Some(posonlyargcount) = to_i64(obj_from_bits(posonlyargcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code posonlyargcount must be int");
        };
        let Some(kwonlyargcount) = to_i64(obj_from_bits(kwonlyargcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code kwonlyargcount must be int");
        };
        if argcount < 0 || posonlyargcount < 0 || kwonlyargcount < 0 {
            return raise_exception::<_>(_py, "ValueError", "code arg counts must be >= 0");
        }
        let mut varnames_bits = varnames_bits;
        let mut varnames_owned = false;
        if obj_from_bits(varnames_bits).is_none() {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            varnames_bits = MoltObject::from_ptr(tuple_ptr).bits();
            varnames_owned = true;
        } else {
            let Some(varnames_ptr) = obj_from_bits(varnames_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code varnames must be tuple or None",
                );
            };
            unsafe {
                if object_type_id(varnames_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "code varnames must be tuple or None",
                    );
                }
            }
        }
        let firstlineno = to_i64(obj_from_bits(firstlineno_bits)).unwrap_or(0);
        let ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            firstlineno,
            linetable_bits,
            varnames_bits,
            argcount as u64,
            posonlyargcount as u64,
            kwonlyargcount as u64,
        );
        if varnames_owned {
            dec_ref_bits(_py, varnames_bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let debug_bound = std::env::var_os("MOLT_DEBUG_BOUND_METHOD").is_some();
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            if debug_bound {
                let self_obj = obj_from_bits(self_bits);
                let self_label = self_obj
                    .as_ptr()
                    .map(|_| type_name(_py, self_obj).into_owned())
                    .unwrap_or_else(|| format!("immediate:{:#x}", self_bits));
                let self_type_id = self_obj
                    .as_ptr()
                    .map(|ptr| unsafe { object_type_id(ptr) })
                    .unwrap_or(0);
                eprintln!(
                    "molt_bound_method_new: non-object func_bits={:#x} self={} self_type_id={}",
                    func_bits, self_label, self_type_id
                );
                if let Some(name) = crate::builtins::attr::debug_last_attr_name() {
                    eprintln!("molt_bound_method_new last_attr={}", name);
                }
            }
            return raise_exception::<_>(_py, "TypeError", "bound method expects function object");
        };
        unsafe {
            // If func_bits is already a BOUND_METHOD, unwrap to its inner function
            // so we don't fail the TYPE_ID_FUNCTION check below. This happens when
            // inline int/float/bool attribute fallback passes a bound method through
            // the builtin_class_method_bits path.
            if object_type_id(func_ptr) == TYPE_ID_BOUND_METHOD {
                let inner_func_bits = bound_method_func_bits(func_ptr);
                return molt_bound_method_new(inner_func_bits, self_bits);
            }
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                if debug_bound {
                    let type_label = type_name(_py, func_obj).into_owned();
                    let self_label = obj_from_bits(self_bits)
                        .as_ptr()
                        .map(|_| type_name(_py, obj_from_bits(self_bits)).into_owned())
                        .unwrap_or_else(|| format!("immediate:{:#x}", self_bits));
                    eprintln!(
                        "molt_bound_method_new: expected function got type_id={} type={} self={}",
                        object_type_id(func_ptr),
                        type_label,
                        self_label
                    );
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bound method expects function object",
                );
            }
        }
        let ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            let method_bits = {
                let func_class_bits = unsafe { object_class_bits(func_ptr) };
                if func_class_bits == builtin_classes(_py).builtin_function_or_method {
                    func_class_bits
                } else {
                    crate::builtins::types::method_class(_py)
                }
            };
            if method_bits != 0 {
                unsafe {
                    let old_bits = object_class_bits(ptr);
                    if old_bits != method_bits {
                        if old_bits != 0 {
                            dec_ref_bits(_py, old_bits);
                        }
                        object_set_class_bits(_py, ptr, method_bits);
                        inc_ref_bits(_py, method_bits);
                    }
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

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

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_dict(config_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let version_bits = match logging_config_dict_lookup(_py, config_bits, "version") {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                return raise_exception::<_>(_py, "ValueError", "logging config missing version");
            }
            Err(bits) => return bits,
        };
        let Some(version) = to_i64(obj_from_bits(version_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging config version must be int");
        };
        if version != 1 {
            return raise_exception::<_>(_py, "ValueError", "unsupported logging config version");
        }

        let formatter_class_bits = match pickle_resolve_global_bits(_py, "logging", "Formatter") {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let stream_handler_class_bits =
            match pickle_resolve_global_bits(_py, "logging", "StreamHandler") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
        let file_handler_class_bits =
            match pickle_resolve_global_bits(_py, "logging", "FileHandler") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
        let get_logger_bits = match pickle_resolve_global_bits(_py, "logging", "getLogger") {
            Ok(bits) => bits,
            Err(bits) => {
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return bits;
            }
        };

        let mut formatter_map: HashMap<String, u64> = HashMap::new();
        let mut handler_map: HashMap<String, u64> = HashMap::new();

        if let Ok(Some(formatters_bits)) =
            logging_config_dict_lookup(_py, config_bits, "formatters")
        {
            let pairs = match logging_config_dict_items(_py, formatters_bits) {
                Ok(items) => items,
                Err(bits) => {
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            let Some(formatter_class_ptr) = obj_from_bits(formatter_class_bits).as_ptr() else {
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.Formatter is invalid");
            };
            for (name_bits, cfg_bits) in pairs {
                let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging formatter name must be str",
                        );
                    }
                };
                let fmt_bits = match logging_config_dict_lookup(_py, cfg_bits, "format") {
                    Ok(Some(bits)) => bits,
                    Ok(None) => MoltObject::none().bits(),
                    Err(bits) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return bits;
                    }
                };
                let formatter_bits =
                    unsafe { call_class_init_with_args(_py, formatter_class_ptr, &[fmt_bits]) };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                formatter_map.insert(name, formatter_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(handlers_bits)) = logging_config_dict_lookup(_py, config_bits, "handlers") {
            let pairs = match logging_config_dict_items(_py, handlers_bits) {
                Ok(items) => items,
                Err(bits) => {
                    for (_, formatter_bits) in formatter_map {
                        dec_ref_bits(_py, formatter_bits);
                    }
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            let Some(stream_handler_class_ptr) = obj_from_bits(stream_handler_class_bits).as_ptr()
            else {
                for (_, formatter_bits) in formatter_map {
                    dec_ref_bits(_py, formatter_bits);
                }
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.StreamHandler is invalid");
            };
            let Some(file_handler_class_ptr) = obj_from_bits(file_handler_class_bits).as_ptr()
            else {
                for (_, formatter_bits) in formatter_map {
                    dec_ref_bits(_py, formatter_bits);
                }
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.FileHandler is invalid");
            };
            for (name_bits, cfg_bits) in pairs {
                let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging handler name must be str",
                        );
                    }
                };
                let class_bits = match logging_config_dict_lookup(_py, cfg_bits, "class") {
                    Ok(Some(bits)) => bits,
                    Ok(None) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "logging handler config missing class",
                        );
                    }
                    Err(bits) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return bits;
                    }
                };
                let class_name = match string_obj_to_owned(obj_from_bits(class_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging handler class must be str",
                        );
                    }
                };
                let handler_bits = if class_name == "logging.StreamHandler" {
                    let stream_arg_bits = match logging_config_dict_lookup(_py, cfg_bits, "stream")
                    {
                        Ok(Some(bits)) => match logging_config_resolve_ext_stream(_py, bits) {
                            Ok(resolved_bits) => resolved_bits,
                            Err(err_bits) => {
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return err_bits;
                            }
                        },
                        Ok(None) => MoltObject::none().bits(),
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    unsafe {
                        call_class_init_with_args(_py, stream_handler_class_ptr, &[stream_arg_bits])
                    }
                } else if class_name == "logging.FileHandler" {
                    let filename_bits = match logging_config_dict_lookup(_py, cfg_bits, "filename")
                    {
                        Ok(Some(bits)) => bits,
                        Ok(None) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "logging FileHandler config missing filename",
                            );
                        }
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    let mode_bits = match logging_config_dict_lookup(_py, cfg_bits, "mode") {
                        Ok(Some(bits)) => bits,
                        Ok(None) => match alloc_string_bits(_py, "a") {
                            Some(bits) => bits,
                            None => {
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return MoltObject::none().bits();
                            }
                        },
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    let out_bits = unsafe {
                        call_class_init_with_args(
                            _py,
                            file_handler_class_ptr,
                            &[filename_bits, mode_bits],
                        )
                    };
                    if let Ok(None) = logging_config_dict_lookup(_py, cfg_bits, "mode") {
                        dec_ref_bits(_py, mode_bits);
                    }
                    out_bits
                } else {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "unsupported logging handler class for intrinsic dictConfig",
                    );
                };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, cfg_bits, "level") {
                    let out_bits = match logging_config_call_method1(
                        _py,
                        handler_bits,
                        b"setLevel",
                        level_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            dec_ref_bits(_py, handler_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                if let Ok(Some(formatter_name_bits)) =
                    logging_config_dict_lookup(_py, cfg_bits, "formatter")
                {
                    let Some(formatter_name) =
                        string_obj_to_owned(obj_from_bits(formatter_name_bits))
                    else {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        dec_ref_bits(_py, handler_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging formatter reference must be str",
                        );
                    };
                    let Some(formatter_bits) = formatter_map.get(&formatter_name).copied() else {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        dec_ref_bits(_py, handler_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unknown formatter in logging handler config",
                        );
                    };
                    let out_bits = match logging_config_call_method1(
                        _py,
                        handler_bits,
                        b"setFormatter",
                        formatter_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            dec_ref_bits(_py, handler_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                handler_map.insert(name, handler_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            for (_, formatter_bits) in formatter_map {
                dec_ref_bits(_py, formatter_bits);
            }
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(loggers_bits)) = logging_config_dict_lookup(_py, config_bits, "loggers") {
            let pairs = match logging_config_dict_items(_py, loggers_bits) {
                Ok(items) => items,
                Err(bits) => {
                    for (_, handler_bits) in handler_map {
                        dec_ref_bits(_py, handler_bits);
                    }
                    for (_, formatter_bits) in formatter_map {
                        dec_ref_bits(_py, formatter_bits);
                    }
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            for (name_bits, cfg_bits) in pairs {
                let logger_bits = unsafe { call_callable1(_py, get_logger_bits, name_bits) };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                if let Err(bits) = logging_config_clear_logger_handlers(_py, logger_bits) {
                    dec_ref_bits(_py, logger_bits);
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return bits;
                }
                if let Ok(Some(handler_list_bits)) =
                    logging_config_dict_lookup(_py, cfg_bits, "handlers")
                {
                    let handler_names = match logging_config_name_list(_py, handler_list_bits) {
                        Ok(value) => value,
                        Err(bits) => {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    for handler_name in handler_names {
                        let Some(handler_bits) = handler_map.get(&handler_name).copied() else {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "unknown handler in logger config",
                            );
                        };
                        let out_bits = match logging_config_call_method1(
                            _py,
                            logger_bits,
                            b"addHandler",
                            handler_bits,
                        ) {
                            Ok(bits) => bits,
                            Err(bits) => {
                                dec_ref_bits(_py, logger_bits);
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return bits;
                            }
                        };
                        if !obj_from_bits(out_bits).is_none() {
                            dec_ref_bits(_py, out_bits);
                        }
                    }
                }
                if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, cfg_bits, "level") {
                    let out_bits = match logging_config_call_method1(
                        _py,
                        logger_bits,
                        b"setLevel",
                        level_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                dec_ref_bits(_py, logger_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            for (_, handler_bits) in handler_map {
                dec_ref_bits(_py, handler_bits);
            }
            for (_, formatter_bits) in formatter_map {
                dec_ref_bits(_py, formatter_bits);
            }
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(root_bits)) = logging_config_dict_lookup(_py, config_bits, "root") {
            let root_logger_bits = unsafe { call_callable0(_py, get_logger_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Err(bits) = logging_config_clear_logger_handlers(_py, root_logger_bits) {
                dec_ref_bits(_py, root_logger_bits);
                return bits;
            }
            if let Ok(Some(handler_list_bits)) =
                logging_config_dict_lookup(_py, root_bits, "handlers")
            {
                let handler_names = match logging_config_name_list(_py, handler_list_bits) {
                    Ok(value) => value,
                    Err(bits) => {
                        dec_ref_bits(_py, root_logger_bits);
                        return bits;
                    }
                };
                for handler_name in handler_names {
                    let Some(handler_bits) = handler_map.get(&handler_name).copied() else {
                        dec_ref_bits(_py, root_logger_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unknown handler in root logger config",
                        );
                    };
                    let out_bits = match logging_config_call_method1(
                        _py,
                        root_logger_bits,
                        b"addHandler",
                        handler_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, root_logger_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
            }
            if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, root_bits, "level") {
                let out_bits = match logging_config_call_method1(
                    _py,
                    root_logger_bits,
                    b"setLevel",
                    level_bits,
                ) {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(_py, root_logger_bits);
                        return bits;
                    }
                };
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
            }
            dec_ref_bits(_py, root_logger_bits);
        } else if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        for (_, handler_bits) in handler_map {
            dec_ref_bits(_py, handler_bits);
        }
        for (_, formatter_bits) in formatter_map {
            dec_ref_bits(_py, formatter_bits);
        }
        dec_ref_bits(_py, get_logger_bits);
        dec_ref_bits(_py, file_handler_class_bits);
        dec_ref_bits(_py, stream_handler_class_bits);
        dec_ref_bits(_py, formatter_class_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_valid_ident(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "logging.config.valid_ident expects str",
            );
        };
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return MoltObject::from_bool(false).bits();
        };
        let first_ok = first == '_' || first.is_ascii_alphabetic();
        if !first_ok {
            return MoltObject::from_bool(false).bits();
        }
        for ch in chars {
            if ch != '_' && !ch.is_ascii_alphanumeric() {
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_file_config(
    config_file_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_listen(port_bits: u64, verify_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = (port_bits, verify_bits);
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "logging.config.listen is not implemented in Molt yet",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_stop_listening() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_percent_style_format(fmt_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fmt) = string_obj_to_owned(obj_from_bits(fmt_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging format string must be str");
        };
        let Some(mapping_ptr) = obj_from_bits(mapping_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        };
        if unsafe { object_type_id(mapping_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        }

        let chars: Vec<char> = fmt.chars().collect();
        let mut out = String::with_capacity(fmt.len());
        let mut idx = 0usize;

        while idx < chars.len() {
            let ch = chars[idx];
            if ch != '%' {
                out.push(ch);
                idx += 1;
                continue;
            }
            if idx + 1 >= chars.len() {
                out.push('%');
                break;
            }
            if chars[idx + 1] == '%' {
                out.push('%');
                idx += 2;
                continue;
            }
            if chars[idx + 1] != '(' {
                out.push('%');
                idx += 1;
                continue;
            }
            let mut close = idx + 2;
            while close < chars.len() && chars[close] != ')' {
                close += 1;
            }
            if close >= chars.len() || close + 1 >= chars.len() {
                for ch in &chars[idx..] {
                    out.push(*ch);
                }
                break;
            }

            let spec = chars[close + 1];
            let token: String = chars[idx..=close + 1].iter().collect();
            if !matches!(spec, 's' | 'd' | 'r' | 'f') {
                out.push_str(token.as_str());
                idx = close + 2;
                continue;
            }

            let key: String = chars[idx + 2..close].iter().collect();
            let Some(value_bits) =
                logging_percent_lookup_mapping_value(_py, mapping_ptr, key.as_str())
            else {
                out.push_str(token.as_str());
                idx = close + 2;
                continue;
            };

            let Some(rendered) = logging_percent_render_value(_py, spec, value_bits) else {
                return MoltObject::none().bits();
            };
            out.push_str(rendered.as_str());
            idx = close + 2;
        }

        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_crc32(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "zipfile crc32 expects bytes-like");
        };
        let Some(bytes) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile crc32 expects bytes-like");
        };

        let mut crc = 0xFFFF_FFFFu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                if (crc & 1) != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc ^= 0xFFFF_FFFF;
        MoltObject::from_int(i64::from(crc)).bits()
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_detect(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(kind) = imghdr_detect_kind(header) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, kind.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_parse_central_directory(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "zipfile central directory input must be bytes-like",
            );
        };
        let Some(data) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "zipfile central directory input must be bytes-like",
            );
        };
        let entries = match zipfile_parse_central_directory_impl(data) {
            Ok(value) => value,
            Err(message) => return raise_exception::<_>(_py, "ValueError", message),
        };

        let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        for (name, fields) in entries {
            let Some(name_bits) = alloc_string_bits(_py, name.as_str()) else {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };

            let mut item_bits: [u64; 5] = [0; 5];
            for (idx, field) in fields.iter().enumerate() {
                let Ok(value) = i64::try_from(*field) else {
                    dec_ref_bits(_py, name_bits);
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "zipfile central directory value overflow",
                    );
                };
                item_bits[idx] = MoltObject::from_int(value).bits();
            }
            let tuple_ptr = alloc_tuple(_py, &item_bits);
            if tuple_ptr.is_null() {
                dec_ref_bits(_py, name_bits);
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            pairs.push(name_bits);
            pairs.push(tuple_bits);
            owned_bits.push(name_bits);
            owned_bits.push(tuple_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let out = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_build_zip64_extra(
    size_bits: u64,
    comp_size_bits: u64,
    offset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(size) = to_i64(obj_from_bits(size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile zip64 size must be int");
        };
        if size < 0 {
            return raise_exception::<_>(_py, "ValueError", "zipfile zip64 size must be >= 0");
        }
        let Some(comp_size) = to_i64(obj_from_bits(comp_size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile zip64 comp_size must be int");
        };
        if comp_size < 0 {
            return raise_exception::<_>(_py, "ValueError", "zipfile zip64 comp_size must be >= 0");
        }
        let offset = if obj_from_bits(offset_bits).is_none() {
            None
        } else {
            let Some(value) = to_i64(obj_from_bits(offset_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "zipfile zip64 offset must be int");
            };
            if value < 0 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "zipfile zip64 offset must be >= 0",
                );
            }
            Some(value as u64)
        };

        let out = zipfile_build_zip64_extra_impl(size as u64, comp_size as u64, offset);
        let ptr = alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_implied_dirs(names_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let names = match iterable_to_string_vec(_py, names_bits) {
            Ok(items) => items,
            Err(err) => return err,
        };
        let names_set: HashSet<String> = names.iter().cloned().collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();

        for name in &names {
            let ancestry = zipfile_ancestry(name.as_str());
            for parent in ancestry.into_iter().skip(1) {
                let candidate = format!("{parent}/");
                if names_set.contains(candidate.as_str()) {
                    continue;
                }
                if seen.insert(candidate.clone()) {
                    out.push(candidate);
                }
            }
        }
        alloc_string_list(_py, out.as_slice())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_resolve_dir(name_bits: u64, names_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path name must be str");
        };
        let names = match iterable_to_string_vec(_py, names_bits) {
            Ok(items) => items,
            Err(err) => return err,
        };
        let names_set: HashSet<String> = names.into_iter().collect();
        let mut resolved = name.clone();
        let dirname = format!("{name}/");
        if !names_set.contains(name.as_str()) && names_set.contains(dirname.as_str()) {
            resolved = dirname;
        }
        let ptr = alloc_string(_py, resolved.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_is_child(path_at_bits: u64, parent_at_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(path_at) = string_obj_to_owned(obj_from_bits(path_at_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path candidate must be str");
        };
        let Some(parent_at) = string_obj_to_owned(obj_from_bits(parent_at_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path parent must be str");
        };

        let candidate_parent = zipfile_parent_of(path_at.as_str());
        let parent_norm = zipfile_trim_trailing_slashes(parent_at.as_str());
        if candidate_parent == parent_norm {
            MoltObject::from_bool(true).bits()
        } else {
            MoltObject::from_bool(false).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_translate_glob(
    pattern_bits: u64,
    seps_bits: u64,
    py313_plus_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile glob pattern must be str");
        };
        let Some(seps) = string_obj_to_owned(obj_from_bits(seps_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile glob separators must be str");
        };
        let py313_plus = is_truthy(_py, obj_from_bits(py313_plus_bits));

        let translated =
            match zipfile_translate_glob_impl(pattern.as_str(), seps.as_str(), py313_plus) {
                Ok(value) => value,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            };
        let ptr = alloc_string(_py, translated.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_normalize_member_path(member_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(member) = string_obj_to_owned(obj_from_bits(member_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile member path must be str");
        };
        let Some(normalized) = zipfile_normalize_member_path_impl(member.as_str()) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, normalized.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_what(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(kind) = imghdr_detect_kind(header) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, kind.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_test(kind_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr kind must be str");
        };
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let matches = imghdr_detect_kind(header)
            .map(|detected| detected == kind.as_str())
            .unwrap_or(false);
        MoltObject::from_bool(matches).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wsgiref_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipapp_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xmlrpc_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

/// Tokenize a UTF-8 source string into a list of (type, string, start, end, line) tuples.
/// Token types: 0=ENDMARKER, 1=NAME, 2=NUMBER, 4=NEWLINE, 54=OP, 64=COMMENT, 65=NL, 67=ENCODING
#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_scan(source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let source_obj = crate::obj_from_bits(source_bits);
        let Some(source) = crate::string_obj_to_owned(source_obj) else {
            return crate::raise_exception::<_>(_py, "TypeError", "source must be str");
        };

        const ENDMARKER: i64 = 0;
        const NAME: i64 = 1;
        const NUMBER: i64 = 2;
        const NEWLINE: i64 = 4;
        const OP: i64 = 54;
        const COMMENT: i64 = 64;
        const NL: i64 = 65;

        fn is_name_start(ch: u8) -> bool {
            ch == b'_' || ch.is_ascii_alphabetic()
        }
        fn is_name_char(ch: u8) -> bool {
            is_name_start(ch) || ch.is_ascii_digit()
        }

        let mut tokens: Vec<u64> = Vec::new();
        let source_bytes = source.as_bytes();
        let mut line_no: i64 = 1;

        if !source_bytes.is_empty() {
            let mut start = 0usize;
            while start < source_bytes.len() {
                let line_end = memchr(b'\n', &source_bytes[start..])
                    .map(|rel| start + rel + 1)
                    .unwrap_or(source_bytes.len());
                let line = &source[start..line_end];
                let line_bytes = line.as_bytes();
                let line_len = line_bytes.len();
                let line_bits =
                    alloc_string_bits(_py, line).unwrap_or_else(|| MoltObject::none().bits());

                // Full-line comment check
                let trimmed_start = line_bytes.iter().position(|&b| b != b' ' && b != b'\t');
                if let Some(ts) = trimmed_start
                    && line_bytes[ts] == b'#'
                {
                    let comment = line.trim();
                    let tok = make_token_tuple(
                        _py,
                        COMMENT,
                        comment,
                        (line_no, 0),
                        (line_no, comment.len() as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                    if line.ends_with('\n') {
                        let tok = make_token_tuple(
                            _py,
                            NL,
                            "\n",
                            (line_no, (line_len - 1) as i64),
                            (line_no, line_len as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                    }
                    if line_bits != MoltObject::none().bits() {
                        dec_ref_bits(_py, line_bits);
                    }
                    line_no += 1;
                    start = line_end;
                    continue;
                }

                let mut col: usize = 0;
                while col < line_len {
                    let ch = line_bytes[col];
                    if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                        col += 1;
                        continue;
                    }
                    if ch == b'#' {
                        let comment = line[col..].trim_end_matches(['\r', '\n']);
                        let tok = make_token_tuple(
                            _py,
                            COMMENT,
                            comment,
                            (line_no, col as i64),
                            (line_no, (col + comment.len()) as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        break;
                    }
                    if is_name_start(ch) {
                        let start_col = col;
                        col += 1;
                        while col < line_len && is_name_char(line_bytes[col]) {
                            col += 1;
                        }
                        let text = &line[start_col..col];
                        let tok = make_token_tuple(
                            _py,
                            NAME,
                            text,
                            (line_no, start_col as i64),
                            (line_no, col as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        continue;
                    }
                    if ch.is_ascii_digit() {
                        let start_col = col;
                        col += 1;
                        while col < line_len && line_bytes[col].is_ascii_digit() {
                            col += 1;
                        }
                        let text = &line[start_col..col];
                        let tok = make_token_tuple(
                            _py,
                            NUMBER,
                            text,
                            (line_no, start_col as i64),
                            (line_no, col as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        continue;
                    }
                    // OP
                    let ch_str = &line[col..col + 1];
                    let tok = make_token_tuple(
                        _py,
                        OP,
                        ch_str,
                        (line_no, col as i64),
                        (line_no, (col + 1) as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                    col += 1;
                }

                if line.ends_with('\n') {
                    let stripped = line.trim();
                    let has_content = !stripped.is_empty() && !stripped.starts_with('#');
                    let tok_type = if has_content { NEWLINE } else { NL };
                    let tok = make_token_tuple(
                        _py,
                        tok_type,
                        "\n",
                        (line_no, (line_len - 1) as i64),
                        (line_no, line_len as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                }
                if line_bits != MoltObject::none().bits() {
                    dec_ref_bits(_py, line_bits);
                }
                line_no += 1;
                if line_end == source_bytes.len() {
                    break;
                }
                start = line_end;
            }
        }

        // ENDMARKER
        let endmarker_line_bits =
            alloc_string_bits(_py, "").unwrap_or_else(|| MoltObject::none().bits());
        let tok = make_token_tuple(
            _py,
            ENDMARKER,
            "",
            (line_no, 0),
            (line_no, 0),
            endmarker_line_bits,
        );
        tokens.push(tok);
        if endmarker_line_bits != MoltObject::none().bits() {
            dec_ref_bits(_py, endmarker_line_bits);
        }

        let list_ptr = crate::alloc_list(_py, &tokens);
        for bits in &tokens {
            crate::dec_ref_bits(_py, *bits);
        }
        if list_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

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
#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_detect_encoding(first_bits: u64, second_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let first_obj = crate::obj_from_bits(first_bits);
        let second_obj = crate::obj_from_bits(second_bits);

        let first_bytes = if let Some(ptr) = first_obj.as_ptr() {
            unsafe { crate::bytes_like_slice(ptr) }.unwrap_or(&[])
        } else {
            &[]
        };

        let second_bytes = if let Some(ptr) = second_obj.as_ptr() {
            unsafe { crate::bytes_like_slice(ptr) }.unwrap_or(&[])
        } else {
            &[]
        };

        let bom_utf8: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut bom_found = false;
        let mut effective_first = first_bytes;
        let mut default_enc = "utf-8";

        if effective_first.starts_with(bom_utf8) {
            bom_found = true;
            effective_first = &effective_first[3..];
            default_enc = "utf-8-sig";
        }

        if effective_first.is_empty() && second_bytes.is_empty() {
            let enc_ptr = crate::alloc_string(_py, default_enc.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Check first line
        if let Some(encoding) = find_encoding_cookie(effective_first) {
            let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
                "utf-8-sig"
            } else {
                encoding
            };
            let enc_ptr = crate::alloc_string(_py, encoding.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Check second line
        if !second_bytes.is_empty()
            && let Some(encoding) = find_encoding_cookie(second_bytes)
        {
            let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
                "utf-8-sig"
            } else {
                encoding
            };
            let enc_ptr = crate::alloc_string(_py, encoding.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Default encoding
        let enc_ptr = crate::alloc_string(_py, default_enc.as_bytes());
        let bom_bits = MoltObject::from_bool(bom_found).bits();
        if enc_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
        let tuple_ptr = crate::alloc_tuple(_py, &elems);
        if tuple_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tomllib_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_symtable_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_import_smoke_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}
