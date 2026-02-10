use molt_obj_model::MoltObject;
use rustpython_parser::{ast as pyast, parse as parse_python, Mode as ParseMode, ParseErrorType};
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
use crate::builtins::platform::env_state_get;
use crate::{
    alloc_bound_method_obj, alloc_code_obj, alloc_dict_with_pairs, alloc_function_obj,
    alloc_list_with_capacity, alloc_string, alloc_tuple, attr_name_bits_from_bytes, bits_from_ptr,
    builtin_classes, bytes_like_slice, call_callable0, call_callable1, call_callable2,
    call_class_init_with_args, clear_exception, dec_ref_bits, dict_get_in_place,
    ensure_function_code_bits, exception_kind_bits, exception_pending, function_dict_bits,
    function_set_closure_bits, function_set_trampoline_ptr, inc_ref_bits, is_truthy,
    maybe_ptr_from_bits, missing_bits, module_dict_bits, molt_exception_last, molt_getattr_builtin,
    molt_is_callable, molt_iter, molt_iter_next, molt_list_insert, molt_trace_enter_slot,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, ptr_from_bits,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_f64, to_i64, type_name, TYPE_ID_DICT,
    TYPE_ID_FUNCTION, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE,
};

struct MoltEmailMessage {
    headers: Vec<(String, String)>,
}

#[derive(Clone)]
struct MoltUrllibResponse {
    body: Vec<u8>,
    pos: usize,
    closed: bool,
    url: String,
    code: i64,
    reason: String,
    headers: Vec<(String, String)>,
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
static COOKIEJAR_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltCookieJar>>> = OnceLock::new();
static COOKIEJAR_NEXT: AtomicU64 = AtomicU64::new(1);
static SOCKETSERVER_RUNTIME: OnceLock<Mutex<MoltSocketServerRuntime>> = OnceLock::new();

#[no_mangle]
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
        const RE_IGNORECASE: i64 = 2;
        let matched = if flags & RE_IGNORECASE != 0 {
            segment.to_lowercase() == literal.to_lowercase()
        } else {
            segment == literal
        };
        MoltObject::from_bool(matched).bits()
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_pickle_encode_protocol0(parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts_obj = obj_from_bits(parts_bits);
        let Some(parts_ptr) = parts_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pickle opcode chunks must be a sequence",
            );
        };
        let parts_type = unsafe { object_type_id(parts_ptr) };
        if parts_type != TYPE_ID_LIST && parts_type != TYPE_ID_TUPLE {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pickle opcode chunks must be a sequence",
            );
        }
        let elems = unsafe { seq_vec_ref(parts_ptr) };
        let mut joined = String::new();
        for &elem_bits in elems.iter() {
            let Some(chunk) = string_obj_to_owned(obj_from_bits(elem_bits)) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "pickle opcode chunks must contain str values",
                );
            };
            joined.push_str(&chunk);
        }
        let bytes_ptr = crate::alloc_bytes(_py, joined.as_bytes());
        if bytes_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(bytes_ptr).bits()
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

fn fnmatch_parse_char_class(
    pat: &[char],
    mut idx: usize,
) -> Option<(Vec<char>, Vec<(char, char)>, bool, usize)> {
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
    if negate {
        !hit
    } else {
        hit
    }
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
        if pi < pat_chars.len() && pat_chars[pi] == '[' {
            if let Some((singles, ranges, negate, next_idx)) =
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

fn fnmatch_escape_regex_char(out: &mut String, ch: char) {
    if matches!(
        ch,
        '.' | '^' | '$' | '+' | '{' | '}' | '(' | ')' | '|' | '\\' | '[' | ']'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn fnmatch_translate_impl(pat: &str) -> String {
    let chars: Vec<char> = pat.chars().collect();
    let mut out = String::from("(?s:");
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                if let Some((singles, ranges, negate, next_idx)) =
                    fnmatch_parse_char_class(&chars, i)
                {
                    out.push('[');
                    if negate {
                        out.push('^');
                    }
                    for ch in singles {
                        if matches!(ch, '\\' | '^' | '-' | ']') {
                            out.push('\\');
                        }
                        out.push(ch);
                    }
                    for (start, end) in ranges {
                        if matches!(start, '\\' | '^' | '-' | ']') {
                            out.push('\\');
                        }
                        out.push(start);
                        out.push('-');
                        if matches!(end, '\\' | '^' | '-' | ']') {
                            out.push('\\');
                        }
                        out.push(end);
                    }
                    out.push(']');
                    i = next_idx;
                    continue;
                } else {
                    out.push_str("\\[");
                }
            }
            ch => fnmatch_escape_regex_char(&mut out, ch),
        }
        i += 1;
    }
    out.push_str(")\\Z");
    out
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
    if allow_fragments {
        if let Some(idx) = working.find('#') {
            fragment = working[idx + 1..].to_string();
            working.truncate(idx);
        }
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
        if doseq {
            if let Some(value_ptr) = value_obj.as_ptr() {
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
        if let Some(exc_name) = urllib_request_pending_exception_kind_name(_py) {
            if exc_name == "AttributeError" {
                clear_exception(_py);
                return Ok(None);
            }
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
        101 => "Switching Protocols",
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "",
    }
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
        let _ = timestamp;
        return "Thu, 01 Jan 1970 00:00:00 GMT".to_string();
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
        if !value.is_finite() {
            0
        } else if value < 0.0 {
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

    let command =
        http_server_get_optional_attr_string(_py, handler_bits, b"command")?.unwrap_or_default();
    let method_name = format!("do_{command}");
    let Some(method_name_bits) = alloc_string_bits(_py, &method_name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
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
    if compact.len() % 4 != 0 {
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

fn urllib_response_drop(handle: i64) {
    if let Ok(mut guard) = urllib_response_registry().lock() {
        guard.remove(&(handle as u64));
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
                if let Some(value) = value_opt {
                    if !value.is_empty() {
                        path = if value.starts_with('/') {
                            value.to_string()
                        } else {
                            format!("/{value}")
                        };
                    }
                }
            }
            "max-age" => {
                if let Some(value) = value_opt {
                    if value == "0" {
                        delete_cookie = true;
                    }
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
    if without_user.starts_with('[') {
        if let Some(end) = without_user.find(']') {
            let host = without_user[1..end].to_string();
            if let Some(port_part) = without_user[end + 1..].strip_prefix(':') {
                if let Ok(port) = port_part.parse::<u16>() {
                    return (host, port);
                }
            }
            return (host, default_port);
        }
    }
    if let Some((host, port_part)) = without_user.rsplit_once(':') {
        if !host.is_empty() && !port_part.is_empty() && !host.contains(':') {
            if let Ok(port) = port_part.parse::<u16>() {
                return (host.to_string(), port);
            }
        }
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
    if let (Some(rule), Some(_proxy_url)) = (no_proxy.as_deref(), proxy.as_ref()) {
        if urllib_http_host_matches_no_proxy(host, rule) {
            proxy = None;
        }
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

fn urllib_http_parse_response_bytes(
    raw: &[u8],
) -> Result<(i64, String, Vec<(String, String)>, Vec<u8>), String> {
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
) -> Result<Option<(i64, String, Vec<(String, String)>, Vec<u8>)>, u64> {
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
) -> Result<(i64, String, Vec<(String, String)>, Vec<u8>), std::io::Error> {
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
            {
                if let Ok(parsed) = urllib_http_parse_response_bytes(&raw) {
                    return Ok(parsed);
                }
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

fn textwrap_wrap_impl(text: &str, width: i64) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = words[0].to_string();
    for word in words.iter().skip(1) {
        let candidate_width = current
            .chars()
            .count()
            .saturating_add(1)
            .saturating_add(word.chars().count()) as i64;
        if candidate_width <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = (*word).to_string();
        }
    }
    lines.push(current);
    lines
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

#[no_mangle]
pub extern "C" fn molt_email_message_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let msg = Box::new(MoltEmailMessage {
            headers: Vec::new(),
        });
        bits_from_ptr(Box::into_raw(msg) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_email_message_set(
    message_bits: u64,
    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let message_ptr = ptr_from_bits(message_bits);
        if message_ptr.is_null() {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header value must be str");
        };
        let message = unsafe { &mut *(message_ptr as *mut MoltEmailMessage) };
        message.headers.push((name, value));
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_email_message_items(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let message_ptr = ptr_from_bits(message_bits);
        if message_ptr.is_null() {
            return raise_exception::<_>(_py, "TypeError", "email message handle is invalid");
        }
        let message = unsafe { &*(message_ptr as *mut MoltEmailMessage) };
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

#[no_mangle]
pub extern "C" fn molt_email_message_drop(message_bits: u64) {
    crate::with_gil_entry!(_py, {
        let message_ptr = ptr_from_bits(message_bits);
        if message_ptr.is_null() {
            return;
        }
        unsafe {
            drop(Box::from_raw(message_ptr as *mut MoltEmailMessage));
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_fnmatchcase(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "name must be str");
        };
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pat must be str");
        };
        MoltObject::from_bool(fnmatch_match_impl(&name, &pat)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_fnmatch(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "name must be str");
        };
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pat must be str");
        };
        MoltObject::from_bool(fnmatch_match_impl(&name, &pat)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_fnmatch_filter(names_bits: u64, pat_bits: u64, invert_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pat must be str");
        };
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
            let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "TypeError", "expected str item");
            };
            let matched = fnmatch_match_impl(&name, &pat);
            if matched != invert {
                inc_ref_bits(_py, item_bits);
                out_bits.push(item_bits);
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

#[no_mangle]
pub extern "C" fn molt_fnmatch_translate(pat_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pat must be str");
        };
        let out = fnmatch_translate_impl(&pat);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        const S_IFMT_MASK: i64 = 0o170000;
        const S_IFSOCK: i64 = 0o140000;
        const S_IFLNK: i64 = 0o120000;
        const S_IFREG: i64 = 0o100000;
        const S_IFBLK: i64 = 0o060000;
        const S_IFDIR: i64 = 0o040000;
        const S_IFCHR: i64 = 0o020000;
        const S_IFIFO: i64 = 0o010000;
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
        let payload = [
            MoltObject::from_int(S_IFMT_MASK).bits(),
            MoltObject::from_int(S_IFSOCK).bits(),
            MoltObject::from_int(S_IFLNK).bits(),
            MoltObject::from_int(S_IFREG).bits(),
            MoltObject::from_int(S_IFBLK).bits(),
            MoltObject::from_int(S_IFDIR).bits(),
            MoltObject::from_int(S_IFCHR).bits(),
            MoltObject::from_int(S_IFIFO).bits(),
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_stat_isdir(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o040000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_isreg(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o100000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_ischr(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o020000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_isblk(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o060000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_isfifo(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o010000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_islnk(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o120000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stat_issock(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool((mode & 0o170000) == 0o140000).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_textwrap_wrap(text_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let lines = textwrap_wrap_impl(&text, width);
        alloc_string_list(_py, &lines)
    })
}

#[no_mangle]
pub extern "C" fn molt_textwrap_fill(text_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let out = textwrap_wrap_impl(&text, width).join("\n");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_textwrap_indent(text_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let out = text
            .split('\n')
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<String>>()
            .join("\n");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_http_cookiejar_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = cookiejar_store_new() else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar allocation failed");
        };
        MoltObject::from_int(handle).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
    let headers_bits = urllib_http_headers_to_dict(_py, &headers)?;
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

#[no_mangle]
pub extern "C" fn molt_http_server_read_request(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_read_request_impl(_py, handler_bits) {
            Ok(state) => MoltObject::from_int(state).bits(),
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_http_server_compute_close_connection(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_compute_close_connection_impl(_py, handler_bits) {
            Ok(close) => MoltObject::from_bool(close).bits(),
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_http_server_handle_one_request(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_handle_one_request_impl(_py, handler_bits) {
            Ok(keep_running) => MoltObject::from_bool(keep_running).bits(),
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_http_server_end_headers(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_end_headers_impl(_py, handler_bits) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
                let Some(handle) = urllib_response_store(MoltUrllibResponse {
                    body: payload,
                    pos: 0,
                    closed: false,
                    url: full_url.clone(),
                    // CPython's data: handler returns an addinfourl without HTTP status metadata.
                    code: -1,
                    reason: String::new(),
                    headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
                }) else {
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
                    let Some(handle) = urllib_response_store(MoltUrllibResponse {
                        body: resp_body,
                        pos: 0,
                        closed: false,
                        url: current_url.clone(),
                        code,
                        reason,
                        headers: resp_headers,
                    }) else {
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
                    dec_ref_bits(_py, response_bits);
                    response_bits = out_bits;
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

#[no_mangle]
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

#[no_mangle]
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
        let Some(out) = urllib_response_with_mut(handle, |resp| {
            if resp.closed {
                return Err("I/O operation on closed file.".to_string());
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
        }) else {
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_urllib_request_response_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_drop(handle);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_urllib_request_response_geturl(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(url) = urllib_response_with(handle, |resp| resp.url.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        let ptr = alloc_string(_py, url.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_urllib_request_response_getreason(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(reason) = urllib_response_with(handle, |resp| resp.reason.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        if reason.is_empty() {
            return MoltObject::none().bits();
        }
        let ptr = alloc_string(_py, reason.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_urllib_request_response_getheaders(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(headers) = urllib_response_with(handle, |resp| resp.headers.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match urllib_http_headers_to_dict(_py, &headers) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
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
        let request_target = if url.is_empty() {
            "/".to_string()
        } else {
            url.clone()
        };
        let host_header = if port == 80 {
            host.clone()
        } else {
            format!("{host}:{port}")
        };
        let req = UrllibHttpRequest {
            host: host.clone(),
            port,
            path: request_target.clone(),
            method,
            headers,
            body,
            timeout,
        };
        let (code, reason, resp_headers, resp_body) =
            match urllib_http_try_inmemory_dispatch(_py, &req, &request_target, &host_header) {
                Ok(Some(value)) => value,
                Ok(None) => match urllib_http_send_request(&req, &request_target, &host_header) {
                    Ok(value) => value,
                    Err(err) => {
                        if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock
                        {
                            return raise_exception::<_>(_py, "TimeoutError", "timed out");
                        }
                        return raise_exception::<_>(_py, "OSError", &err.to_string());
                    }
                },
                Err(bits) => return bits,
            };
        let response_url = if url.starts_with("http://") || url.starts_with("https://") {
            url
        } else if request_target.starts_with('/') {
            format!("http://{host_header}{request_target}")
        } else {
            format!("http://{host_header}/{request_target}")
        };
        let Some(handle) = urllib_response_store(MoltUrllibResponse {
            body: resp_body,
            pos: 0,
            closed: false,
            url: response_url,
            code,
            reason,
            headers: resp_headers,
        }) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_http_client_response_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_urllib_request_response_read(handle_bits, size_bits)
}

#[no_mangle]
pub extern "C" fn molt_http_client_response_close(handle_bits: u64) -> u64 {
    molt_urllib_request_response_close(handle_bits)
}

#[no_mangle]
pub extern "C" fn molt_http_client_response_drop(handle_bits: u64) -> u64 {
    molt_urllib_request_response_drop(handle_bits)
}

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_http_client_response_getreason(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(reason) = urllib_response_with(handle, |resp| resp.reason.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        let ptr = alloc_string(_py, reason.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
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
        let needle = name.to_lowercase();
        let Some(values) = urllib_response_with(handle, |resp| {
            resp.headers
                .iter()
                .filter_map(|(k, v)| {
                    if k.eq_ignore_ascii_case(&needle) {
                        Some(v.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<String>>()
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        if values.is_empty() {
            return default_bits;
        }
        let joined = if values.len() == 1 {
            values[0].clone()
        } else {
            values.join(", ")
        };
        let ptr = alloc_string(_py, joined.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_http_client_response_getheaders(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(headers) = urllib_response_with(handle, |resp| resp.headers.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match urllib_http_headers_to_list(_py, &headers) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

fn compile_error_type(error: &ParseErrorType) -> &'static str {
    if error.is_tab_error() {
        "TabError"
    } else if error.is_indentation_error() {
        "IndentationError"
    } else {
        "SyntaxError"
    }
}

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

fn compile_validate_nonlocal_semantics(parsed: &pyast::Mod) -> Result<(), String> {
    match parsed {
        pyast::Mod::Module(module) => walk_nested_function_scopes(&module.body, &[]),
        pyast::Mod::Interactive(module) => walk_nested_function_scopes(&module.body, &[]),
        _ => Ok(()),
    }
}

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

#[no_mangle]
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
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
#[no_mangle]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
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

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
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
