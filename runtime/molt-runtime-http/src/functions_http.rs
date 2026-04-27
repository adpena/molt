use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bridge::{
    alloc_bytes, alloc_dict_with_pairs, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bytes_like_slice, call_callable0, call_callable1, call_callable2,
    call_class_init_with_args, clear_exception, dec_ref_bits, env_state_get, exception_kind_bits,
    exception_pending, inc_ref_bits, is_truthy, maybe_ptr_from_bits, missing_bits,
    molt_exception_last, molt_getattr_builtin, molt_is_callable, molt_iter, molt_iter_next,
    molt_list_insert, object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned, to_f64,
    to_i64,
};

// ---------------------------------------------------------------------------
// Helper functions (originally in builtins/functions.rs)
// ---------------------------------------------------------------------------

pub(crate) fn alloc_string_bits(_py: &CoreGilToken, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

fn alloc_string_tuple(_py: &CoreGilToken, values: &[String]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let Some(bits) = alloc_string_bits(_py, value) else {
            for bit in &item_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        item_bits.push(bits);
    }
    let tuple_ptr = alloc_tuple(_py, &item_bits);
    if tuple_ptr.is_null() {
        for bit in &item_bits {
            dec_ref_bits(_py, *bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(tuple_ptr).bits();
    for bit in &item_bits {
        dec_ref_bits(_py, *bit);
    }
    out
}

fn alloc_qsl_list(_py: &CoreGilToken, items: &[(String, String)]) -> u64 {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(items.len());
    for (key, value) in items {
        let Some(key_bits) = alloc_string_bits(_py, key) else {
            for bit in &tuple_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        let Some(value_bits) = alloc_string_bits(_py, value) else {
            dec_ref_bits(_py, key_bits);
            for bit in &tuple_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(_py, &[key_bits, value_bits]);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, value_bits);
        if tuple_ptr.is_null() {
            for bit in &tuple_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, &tuple_bits, tuple_bits.len());
    if list_ptr.is_null() {
        for bit in &tuple_bits {
            dec_ref_bits(_py, *bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(list_ptr).bits();
    for bit in &tuple_bits {
        dec_ref_bits(_py, *bit);
    }
    out
}

fn alloc_qs_dict(
    _py: &CoreGilToken,
    order: &[String],
    values: &HashMap<String, Vec<String>>,
) -> u64 {
    let mut pairs: Vec<u64> = Vec::with_capacity(order.len() * 2);
    let mut owned_bits: Vec<u64> = Vec::with_capacity(order.len() * 2);
    for key in order {
        let Some(key_bits) = alloc_string_bits(_py, key) else {
            for bit in &owned_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        let mut value_bits: Vec<u64> = Vec::new();
        for value in values.get(key).into_iter().flatten() {
            let Some(bits) = alloc_string_bits(_py, value) else {
                dec_ref_bits(_py, key_bits);
                for bit in &value_bits {
                    dec_ref_bits(_py, *bit);
                }
                for bit in &owned_bits {
                    dec_ref_bits(_py, *bit);
                }
                return MoltObject::none().bits();
            };
            value_bits.push(bits);
        }
        let list_ptr = alloc_list_with_capacity(_py, &value_bits, value_bits.len());
        for bit in &value_bits {
            dec_ref_bits(_py, *bit);
        }
        if list_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            for bit in &owned_bits {
                dec_ref_bits(_py, *bit);
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
    for bit in &owned_bits {
        dec_ref_bits(_py, *bit);
    }
    if dict_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(dict_ptr).bits()
}

fn iter_next_pair(_py: &CoreGilToken, iter_bits: u64) -> Result<(u64, bool), u64> {
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        return Err(MoltObject::none().bits());
    };
    unsafe {
        if object_type_id(pair_ptr) != crate::bridge::type_id_tuple() {
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

// ---------------------------------------------------------------------------

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
    /// When `Some(server_name)`, the request is sent over TLS using rustls
    /// with the given SNI server name. Required for `https://` URLs.
    tls_server_name: Option<String>,
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
    /// Set when the connection was created via `molt_http_client_connection_new_https`
    /// (i.e. backing an `http.client.HTTPSConnection`). Causes request execution
    /// to negotiate TLS via rustls.
    use_tls: bool,
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

static URLLIB_RESPONSE_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltUrllibResponse>>> =
    OnceLock::new();
static URLLIB_RESPONSE_NEXT: AtomicU64 = AtomicU64::new(1);
static HTTP_CLIENT_CONNECTION_RUNTIME: OnceLock<Mutex<MoltHttpClientConnectionRuntime>> =
    OnceLock::new();
static HTTP_MESSAGE_RUNTIME: OnceLock<Mutex<MoltHttpMessageRuntime>> = OnceLock::new();
static COOKIEJAR_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltCookieJar>>> = OnceLock::new();
static COOKIEJAR_NEXT: AtomicU64 = AtomicU64::new(1);
static SOCKETSERVER_RUNTIME: OnceLock<Mutex<MoltSocketServerRuntime>> = OnceLock::new();

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
    _py: &molt_runtime_core::CoreGilToken,
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
        if item_type != crate::bridge::type_id_list() && item_type != crate::bridge::type_id_tuple()
        {
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
        let key_text = crate::bridge::format_obj_str(_py, obj_from_bits(item_fields[0]));
        let key_enc = urllib_quote_plus_impl(&key_text, safe);
        let value_obj = obj_from_bits(item_fields[1]);
        let mut wrote_pair = false;
        if doseq && let Some(value_ptr) = value_obj.as_ptr() {
            let value_type = unsafe { object_type_id(value_ptr) };
            if value_type == crate::bridge::type_id_list()
                || value_type == crate::bridge::type_id_tuple()
            {
                let seq = unsafe { seq_vec_ref(value_ptr) };
                for value_bits in seq.iter().copied() {
                    let value_text = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
                    let value_enc = urllib_quote_plus_impl(&value_text, safe);
                    out_pairs.push(format!("{key_enc}={value_enc}"));
                }
                wrote_pair = true;
            }
        }
        if !wrote_pair {
            let value_text = crate::bridge::format_obj_str(_py, value_obj);
            let value_enc = urllib_quote_plus_impl(&value_text, safe);
            out_pairs.push(format!("{key_enc}={value_enc}"));
        }
    }
    Ok(out_pairs.join("&"))
}

fn urllib_error_set_attr(
    _py: &molt_runtime_core::CoreGilToken,
    self_bits: u64,
    name: &str,
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
        return false;
    };
    crate::bridge::molt_object_setattr(self_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn urllib_error_init_args(
    _py: &molt_runtime_core::CoreGilToken,
    self_bits: u64,
    args: &[u64],
) -> bool {
    let args_ptr = alloc_tuple(_py, args);
    if args_ptr.is_null() {
        return false;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let _ = crate::bridge::molt_exception_init(self_bits, args_bits);
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

pub(crate) fn urllib_request_attr_optional(
    _py: &molt_runtime_core::CoreGilToken,
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

fn urllib_request_pending_exception_kind_name(
    _py: &molt_runtime_core::CoreGilToken,
) -> Option<String> {
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

fn ctypes_attr_present(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<bool, u64> {
    match urllib_request_attr_optional(_py, obj_bits, name)? {
        Some(bits) => {
            dec_ref_bits(_py, bits);
            Ok(true)
        }
        None => Ok(false),
    }
}

fn ctypes_is_scalar_ctype(
    _py: &molt_runtime_core::CoreGilToken,
    ctype_bits: u64,
) -> Result<bool, u64> {
    let has_size = ctypes_attr_present(_py, ctype_bits, b"_size")?;
    if !has_size {
        return Ok(false);
    }
    let has_fields = ctypes_attr_present(_py, ctype_bits, b"_fields_")?;
    let has_length = ctypes_attr_present(_py, ctype_bits, b"_length")?;
    Ok(!has_fields && !has_length)
}

fn ctypes_sizeof_bits(
    _py: &molt_runtime_core::CoreGilToken,
    obj_or_type_bits: u64,
) -> Result<u64, u64> {
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

fn urllib_attr_truthy(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<bool, u64> {
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
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
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
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
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
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
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
    molt_runtime_core::with_core_gil!(_py, {
        if !crate::bridge::has_capability(_py, "ffi.unsafe") {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    let Some(bytes) = (unsafe { bytes_like_slice(MoltObject::from_ptr(ptr).bits()) }) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be bytes-like"),
        ));
    };
    Ok(bytes.to_vec())
}

fn socketserver_extract_request_id(
    _py: &molt_runtime_core::CoreGilToken,
    bits: u64,
) -> Result<u64, u64> {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if ty != crate::bridge::type_id_tuple() && ty != crate::bridge::type_id_list() {
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
    _py: &molt_runtime_core::CoreGilToken,
    server_bits: u64,
) -> Result<(), u64> {
    let Some(method_bits) = urllib_request_attr_optional(_py, server_bits, b"service_actions")?
    else {
        return Ok(());
    };
    if !molt_is_callable(method_bits) {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    let out = crate::bridge::format_obj_str(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Ok(Some(out))
}

fn http_server_write_bytes(
    _py: &molt_runtime_core::CoreGilToken,
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
    if write_bits == missing || !molt_is_callable(write_bits) {
        if write_bits != missing {
            dec_ref_bits(_py, write_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http handler wfile.write is unavailable",
        ));
    }
    let data_ptr = crate::bridge::alloc_bytes(_py, payload);
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

fn http_server_flush(_py: &molt_runtime_core::CoreGilToken, handler_bits: u64) -> Result<(), u64> {
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
    if flush_bits == missing || !molt_is_callable(flush_bits) {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if readline_bits == missing || !molt_is_callable(readline_bits) {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    keyword: &str,
    value: &str,
) -> Result<(), u64> {
    let line = format!("{keyword}: {value}\r\n");
    http_server_write_bytes(_py, handler_bits, line.as_bytes())
}

fn http_server_end_headers_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<(), u64> {
    http_server_write_bytes(_py, handler_bits, b"\r\n")?;
    http_server_flush(_py, handler_bits)
}

fn http_server_send_error_impl(
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if prepare_headers_bits != missing && molt_is_callable(prepare_headers_bits) {
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
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return false;
    };
    crate::bridge::molt_object_setattr(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn urllib_request_handler_order(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<i64, u64> {
    let Some(order_bits) = urllib_request_attr_optional(_py, handler_bits, b"handler_order")?
    else {
        return Ok(500);
    };
    let out = to_i64(obj_from_bits(order_bits)).unwrap_or(500);
    dec_ref_bits(_py, order_bits);
    Ok(out)
}

fn urllib_request_ensure_handlers_list(
    _py: &molt_runtime_core::CoreGilToken,
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
            if object_type_id(list_ptr) != crate::bridge::type_id_list() {
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

fn urllib_request_set_cursor(
    _py: &molt_runtime_core::CoreGilToken,
    opener_bits: u64,
    cursor: i64,
) -> bool {
    urllib_request_set_attr(
        _py,
        opener_bits,
        b"_molt_open_cursor",
        MoltObject::from_int(cursor).bits(),
    )
}

fn urllib_request_get_cursor(
    _py: &molt_runtime_core::CoreGilToken,
    opener_bits: u64,
) -> Result<i64, u64> {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if unsafe { object_type_id(cached_ptr) } != crate::bridge::type_id_list() {
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

fn urllib_response_drop(_py: &molt_runtime_core::CoreGilToken, handle: i64) {
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

fn http_client_connection_store(
    host: String,
    port: u16,
    timeout: Option<f64>,
    use_tls: bool,
) -> Option<i64> {
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
            use_tls,
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

fn http_client_alloc_buffer_list(_py: &molt_runtime_core::CoreGilToken, buffer: &[Vec<u8>]) -> u64 {
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
    _py: &molt_runtime_core::CoreGilToken,
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

fn http_message_drop(_py: &molt_runtime_core::CoreGilToken, handle: i64) {
    if let Ok(mut guard) = http_message_runtime().lock()
        && let Some(message) = guard.messages.remove(&(handle as u64))
        && let Some(cache_bits) = message.items_list_cache
        && !obj_from_bits(cache_bits).is_none()
    {
        dec_ref_bits(_py, cache_bits);
    }
}

fn http_message_handle_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
    handle_bits: u64,
) -> Result<i64, u64> {
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
    _py: &molt_runtime_core::CoreGilToken,
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

fn http_cookies_attr_text(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
) -> Option<String> {
    if obj_from_bits(value_bits).is_none() {
        return None;
    }
    let text = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
    if text.is_empty() { None } else { Some(text) }
}

fn http_cookies_expires_text(
    _py: &molt_runtime_core::CoreGilToken,
    expires_bits: u64,
) -> Option<String> {
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
    _py: &molt_runtime_core::CoreGilToken,
    input: HttpCookieMorselInput,
) -> String {
    let name = crate::bridge::format_obj_str(_py, obj_from_bits(input.name_bits));
    let value = crate::bridge::format_obj_str(_py, obj_from_bits(input.value_bits));
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
    /// When true, the request is sent over TLS (HTTPS). The SNI server name is
    /// taken from `host`.
    use_tls: bool,
}

fn urllib_http_extract_headers_mapping(
    _py: &molt_runtime_core::CoreGilToken,
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
    if items_method_bits == missing || !molt_is_callable(items_method_bits) {
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
        if item_type != crate::bridge::type_id_list() && item_type != crate::bridge::type_id_tuple()
        {
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
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[0])),
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[1])),
        ));
        dec_ref_bits(_py, item_bits);
    }
    Ok(out)
}

fn urllib_cookiejar_handles_from_handlers(
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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

fn urllib_http_timeout_error(_py: &molt_runtime_core::CoreGilToken) -> u64 {
    raise_exception::<_>(_py, "TimeoutError", "timed out")
}

fn urllib_http_request_timeout(
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
            ty == crate::bridge::type_id_tuple() || ty == crate::bridge::type_id_list()
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
        let value = crate::bridge::format_obj_str(_py, obj_from_bits(pair[1]));
        dec_ref_bits(_py, item_bits);
        out.push((name, value));
    }
    Ok(out)
}

fn http_client_response_handle_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    _py: &molt_runtime_core::CoreGilToken,
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
    let default_port: u16 = if input.use_tls { 443 } else { 80 };
    let host_header = if input.port == default_port {
        input.host.clone()
    } else {
        format!("{}:{}", input.host, input.port)
    };
    let tls_server_name = if input.use_tls {
        Some(input.host.clone())
    } else {
        None
    };
    let req = UrllibHttpRequest {
        host: input.host.clone(),
        port: input.port,
        path: request_target.clone(),
        method: input.method,
        headers: input.headers,
        body: input.body,
        timeout: input.timeout,
        tls_server_name,
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
    let scheme_prefix = if input.use_tls { "https" } else { "http" };
    let response_url = if input.url.starts_with("http://") || input.url.starts_with("https://") {
        input.url
    } else if request_target.starts_with('/') {
        format!("{scheme_prefix}://{host_header}{request_target}")
    } else {
        format!("{scheme_prefix}://{host_header}/{request_target}")
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if items_method_bits == missing || !molt_is_callable(items_method_bits) {
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
        if item_type != crate::bridge::type_id_list() && item_type != crate::bridge::type_id_tuple()
        {
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
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[0])),
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[1])),
        ));
        dec_ref_bits(_py, item_bits);
    }
    Ok(out)
}

fn urllib_http_extract_method_and_body(
    _py: &molt_runtime_core::CoreGilToken,
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
            let Some(bytes) = (unsafe { bytes_like_slice(MoltObject::from_ptr(ptr).bits()) })
            else {
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
            let value = crate::bridge::format_obj_str(_py, obj_from_bits(bits));
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
    _py: &molt_runtime_core::CoreGilToken,
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
        if get_method_bits == missing || !molt_is_callable(get_method_bits) {
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
            proxy = Some(crate::bridge::format_obj_str(_py, obj_from_bits(out_bits)));
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
    _py: &molt_runtime_core::CoreGilToken,
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
        if find_bits == missing || !molt_is_callable(find_bits) {
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
        if ty != crate::bridge::type_id_tuple() && ty != crate::bridge::type_id_list() {
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
        let user = crate::bridge::format_obj_str(_py, obj_from_bits(fields[0]));
        let pass = crate::bridge::format_obj_str(_py, obj_from_bits(fields[1]));
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
    _py: &molt_runtime_core::CoreGilToken,
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Result<Option<HttpResponseParts>, u64> {
    let module_name_ptr = alloc_string(_py, b"socketserver");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::bridge::molt_module_import(module_name_bits);
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
    if unsafe { object_type_id(module_ptr) } != crate::bridge::type_id_module() {
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
    if !molt_is_callable(dispatch_bits) {
        dec_ref_bits(_py, dispatch_bits);
        dec_ref_bits(_py, server_bits);
        return Ok(None);
    }

    let request = urllib_http_build_request_bytes(req, request_target, host_header);
    let request_ptr = crate::bridge::alloc_bytes(_py, &request);
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
    let Some(raw_bytes) = (unsafe { bytes_like_slice(MoltObject::from_ptr(response_ptr).bits()) })
    else {
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
        let _release = crate::bridge::GilReleaseGuard::new();
        let mut stream = TcpStream::connect((req.host.as_str(), req.port))?;
        if let Some(timeout) = req.timeout {
            let timeout = Duration::from_secs_f64(timeout);
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
        }
        if let Some(server_name) = req.tls_server_name.as_deref() {
            #[cfg(feature = "tls")]
            {
                urllib_https_send_over_tls(stream, server_name, &request, &mut raw)?;
            }
            #[cfg(not(feature = "tls"))]
            {
                let _ = (stream, server_name);
                return Err(std::io::Error::new(
                    ErrorKind::Unsupported,
                    "https requires the molt-runtime-http `tls` feature (rustls)",
                ));
            }
        } else {
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
    }
    match urllib_http_parse_response_bytes(&raw) {
        Ok(parsed) => Ok(parsed),
        Err(msg) => Err(std::io::Error::new(ErrorKind::InvalidData, msg)),
    }
}

/// Send an HTTP request over TLS using rustls and read the full response into `out`.
///
/// Uses `webpki-roots` for trust anchors and the supplied `server_name` for SNI
/// and certificate hostname verification (default-secure rustls config).
#[cfg(feature = "tls")]
fn urllib_https_send_over_tls(
    tcp: TcpStream,
    server_name: &str,
    request: &[u8],
    out: &mut Vec<u8>,
) -> std::io::Result<()> {
    use std::sync::Arc;

    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

    fn shared_client_config() -> Arc<ClientConfig> {
        use std::sync::OnceLock;
        static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
        CONFIG
            .get_or_init(|| {
                let mut roots = RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                let cfg = ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                Arc::new(cfg)
            })
            .clone()
    }

    let server_name_owned = ServerName::try_from(server_name.to_string())
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, format!("{e}")))?;
    let conn = ClientConnection::new(shared_client_config(), server_name_owned)
        .map_err(|e| std::io::Error::other(format!("TLS init failed: {e}")))?;
    let mut tls = StreamOwned::new(conn, tcp);

    tls.write_all(request)?;
    if let Err(err) = tls.read_to_end(out) {
        if (err.kind() == ErrorKind::UnexpectedEof
            || err.kind() == ErrorKind::TimedOut
            || err.kind() == ErrorKind::WouldBlock
            || err.kind() == ErrorKind::ConnectionAborted
            || err.kind() == ErrorKind::ConnectionReset)
            && !out.is_empty()
            && urllib_http_parse_response_bytes(out).is_ok()
        {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

fn urllib_http_make_response_bits(_py: &molt_runtime_core::CoreGilToken, handle: i64) -> u64 {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    if ty != crate::bridge::type_id_tuple() {
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

fn urllib_error_class_bits(
    _py: &molt_runtime_core::CoreGilToken,
    class_name: &[u8],
) -> Result<u64, u64> {
    let module_name_ptr = alloc_string(_py, b"urllib.error");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::bridge::molt_module_import(module_name_bits);
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

fn urllib_raise_url_error(_py: &molt_runtime_core::CoreGilToken, reason: &str) -> u64 {
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
    let exc_bits = unsafe {
        call_class_init_with_args(_py, MoltObject::from_ptr(class_ptr).bits(), &[reason_bits])
    };
    dec_ref_bits(_py, reason_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::bridge::molt_raise(exc_bits)
}

fn urllib_raise_http_error(
    _py: &molt_runtime_core::CoreGilToken,
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
            MoltObject::from_ptr(class_ptr).bits(),
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
    crate::bridge::molt_raise(exc_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_quote(string_bits: u64, safe_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = cookiejar_store_new() else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar allocation failed");
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let reason_text = crate::bridge::format_obj_str(_py, obj_from_bits(reason_bits));
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let code_text = crate::bridge::format_obj_str(_py, obj_from_bits(code_bits));
        let msg_text = crate::bridge::format_obj_str(_py, obj_from_bits(msg_bits));
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
        let ptr = crate::bridge::alloc_bytes(_py, &response);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_cancel(server_bits: u64, request_id_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
        let request_ptr = crate::bridge::alloc_bytes(_py, &pending.request);
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(get_request_name_bits) = attr_name_bits_from_bytes(_py, b"get_request") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let get_request_bits = molt_getattr_builtin(server_bits, get_request_name_bits, missing);
        dec_ref_bits(_py, get_request_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if get_request_bits == missing || !molt_is_callable(get_request_bits) {
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
            if !molt_is_callable(verify_request_bits) {
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
            if process_request_bits == missing || !molt_is_callable(process_request_bits) {
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
        if close_request_bits != missing && molt_is_callable(close_request_bits) {
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
                    let out = crate::bridge::molt_raise(exc_bits);
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
            if response_bytes_bits != missing && molt_is_callable(response_bytes_bits) {
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
                        return crate::bridge::molt_raise(exc_bits);
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
                        return crate::bridge::molt_raise(exc_bits);
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
            return crate::bridge::molt_raise(exc_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_shutdown(server_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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

fn http_server_read_request_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<i64, u64> {
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
    _py: &molt_runtime_core::CoreGilToken,
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
    molt_runtime_core::with_core_gil!(_py, {
        match http_server_read_request_impl(_py, handler_bits) {
            Ok(state) => MoltObject::from_int(state).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_compute_close_connection(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match http_server_compute_close_connection_impl(_py, handler_bits) {
            Ok(close) => MoltObject::from_bool(close).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_handle_one_request(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::bridge::format_obj_str(
                _py,
                obj_from_bits(message_bits),
            ))
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::bridge::format_obj_str(
                _py,
                obj_from_bits(message_bits),
            ))
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
    molt_runtime_core::with_core_gil!(_py, {
        let keyword = crate::bridge::format_obj_str(_py, obj_from_bits(keyword_bits));
        let value = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
        match http_server_send_header_impl(_py, handler_bits, &keyword, &value) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_end_headers(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::bridge::format_obj_str(
                _py,
                obj_from_bits(message_bits),
            ))
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
    molt_runtime_core::with_core_gil!(_py, {
        let server_version = crate::bridge::format_obj_str(_py, obj_from_bits(server_version_bits));
        let sys_version = if obj_from_bits(sys_version_bits).is_none() {
            String::new()
        } else {
            crate::bridge::format_obj_str(_py, obj_from_bits(sys_version_bits))
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let url_text = crate::bridge::format_obj_str(_py, obj_from_bits(url_bits));
        let url_ptr = alloc_string(_py, url_text.as_bytes());
        if url_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let full_url_bits = MoltObject::from_ptr(url_ptr).bits();
        let mut headers_value = headers_bits;
        if obj_from_bits(headers_bits).is_none() {
            let dict_bits = crate::bridge::molt_dict_new(0);
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
                if !molt_is_callable(method_bits) {
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
                if !molt_is_callable(method_bits) {
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
                            // Proxies always speak plain HTTP to the proxy peer
                            // (CONNECT tunneling for https proxies is rejected
                            // above), so no TLS termination at this hop.
                            tls_server_name: None,
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
                            tls_server_name: if scheme == "https" {
                                Some(host_now.clone())
                            } else {
                                None
                            },
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
                if !molt_is_callable(method_bits) {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
                let ptr = crate::bridge::alloc_bytes(_py, data.as_slice());
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let mut export = crate::bridge::BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if crate::bridge::molt_buffer_export(buffer_bits, &mut export)
            || export.readonly != 0
            || export.itemsize != 1
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
    molt_runtime_core::with_core_gil!(_py, {
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
                let ptr = crate::bridge::alloc_bytes(_py, data.as_slice());
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_drop(_py, handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_geturl(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let name = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits));
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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

fn urllib_response_message_bits(_py: &molt_runtime_core::CoreGilToken, handle: i64) -> u64 {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_message_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_parse_header_pairs(data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = http_message_store_new() else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_parse(data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits));
        let value = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
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
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle = http_message_header_key(
            crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)).as_str(),
        );
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
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle = http_message_header_key(
            crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)).as_str(),
        );
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle = http_message_header_key(
            crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)).as_str(),
        );
        let Some(found) = http_message_with(handle, |message| message.index.contains_key(&needle))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    http_client_connection_new_impl(host_bits, port_bits, timeout_bits, false)
}

/// Constructor for `http.client.HTTPSConnection` — same shape as
/// `molt_http_client_connection_new` but marks the connection as TLS so that
/// request execution dispatches over rustls.
#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_new_https(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
) -> u64 {
    http_client_connection_new_impl(host_bits, port_bits, timeout_bits, true)
}

fn http_client_connection_new_impl(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
    use_tls: bool,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
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
        let Some(handle) =
            http_client_connection_store(host, port_value as u16, timeout, use_tls)
        else {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
                conn.use_tls,
            ))
        });
        let (host, port, timeout, method, url, headers, body, use_tls) = match state {
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
                use_tls,
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
                use_tls: false,
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
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
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        urllib_response_message_bits(_py, handle)
    })
}
