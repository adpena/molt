use molt_obj_model::MoltObject;
use rustpython_parser::{Mode as ParseMode, ParseErrorType, ast as pyast, parse as parse_python};
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
    TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_LIST, TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE,
    alloc_bound_method_obj, alloc_code_obj, alloc_dict_with_pairs, alloc_function_obj,
    alloc_list_with_capacity, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    builtin_classes, bytes_like_slice, call_callable0, call_callable1, call_callable2,
    call_callable3, call_class_init_with_args, clear_exception, dec_ref_bits, dict_get_in_place,
    ensure_function_code_bits, exception_kind_bits, exception_pending, format_obj,
    function_dict_bits, function_set_closure_bits, function_set_trampoline_ptr, inc_ref_bits,
    is_truthy, maybe_ptr_from_bits, missing_bits, module_dict_bits, molt_exception_last,
    molt_getattr_builtin, molt_is_callable, molt_iter, molt_iter_next, molt_list_insert,
    molt_trace_enter_slot, obj_from_bits, object_class_bits, object_set_class_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_f64, to_i64, type_name, type_of_bits,
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
    if let Some(text) = string_obj_to_owned(obj_from_bits(message_bits)) {
        if let Some(raw) = text.strip_prefix("molt-email-message-") {
            if let Ok(id) = raw.parse::<u64>() {
                if id > 0 {
                    return Ok(id);
                }
            }
        }
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

#[unsafe(no_mangle)]
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
    let trimmed = text.trim_end_matches(|c| c == 'L' || c == 'l');
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_dumps_protocol01(obj_bits: u64, protocol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(protocol) = to_i64(obj_from_bits(protocol_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle protocol must be int");
        };
        if protocol != 0 && protocol != 1 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "only pickle protocols 0 and 1 are supported",
            );
        }
        let mut out = String::new();
        if let Err(err_bits) = pickle_dump_obj(_py, obj_bits, protocol, &mut out) {
            return err_bits;
        }
        out.push('.');
        let out_ptr = crate::alloc_bytes(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_loads_protocol01(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle data must be str");
        };
        let bytes = text.as_bytes();
        let mut idx: usize = 0;
        let mut stack: Vec<PickleStackItem> = Vec::new();
        let mut memo: HashMap<i64, PickleStackItem> = HashMap::new();
        while idx < bytes.len() {
            let op = bytes[idx] as char;
            idx += 1;
            match op {
                '.' => break,
                'N' => stack.push(PickleStackItem::Value(MoltObject::none().bits())),
                'I' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if line == "01" {
                        stack.push(PickleStackItem::Value(MoltObject::from_bool(true).bits()));
                    } else if line == "00" {
                        stack.push(PickleStackItem::Value(MoltObject::from_bool(false).bits()));
                    } else {
                        let int_bits = match pickle_parse_int_bits(_py, line) {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                        stack.push(PickleStackItem::Value(int_bits));
                    }
                }
                'L' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let int_bits = match pickle_parse_long_line_bits(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(int_bits));
                }
                'F' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let float_bits = match pickle_parse_float_bits(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(float_bits));
                }
                'S' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let parsed = match pickle_parse_string_literal(line) {
                        Ok(value) => value,
                        Err(message) => return pickle_raise(_py, message),
                    };
                    let out_ptr = alloc_string(_py, parsed.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(MoltObject::from_ptr(out_ptr).bits()));
                }
                'V' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let out_ptr = alloc_string(_py, line.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(MoltObject::from_ptr(out_ptr).bits()));
                }
                '(' => stack.push(PickleStackItem::Mark),
                't' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let tuple_ptr = alloc_tuple(_py, values.as_slice());
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(tuple_ptr).bits(),
                    ));
                }
                'l' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let list_ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(list_ptr).bits(),
                    ));
                }
                'd' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if values.len() % 2 != 0 {
                        return pickle_raise(_py, "pickle.loads: dict has odd number of items");
                    }
                    let dict_ptr = alloc_dict_with_pairs(_py, values.as_slice());
                    if dict_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(dict_ptr).bits(),
                    ));
                }
                'a' => {
                    let item_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let target_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: append target is not list");
                    };
                    if unsafe { object_type_id(target_ptr) } != TYPE_ID_LIST {
                        return pickle_raise(_py, "pickle.loads: append target is not list");
                    }
                    let _ = crate::molt_list_append(target_bits, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(target_bits));
                }
                's' => {
                    let value_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let key_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let target_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    };
                    if unsafe { object_type_id(target_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    }
                    unsafe {
                        crate::dict_set_in_place(_py, target_ptr, key_bits, value_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(target_bits));
                }
                'c' => {
                    let module = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let name = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(global) = pickle_resolve_global(module, name) else {
                        let message =
                            format!("pickle.loads: unsupported global {}.{}", module, name);
                        return pickle_raise(_py, &message);
                    };
                    stack.push(PickleStackItem::Global(global));
                }
                'R' => {
                    let args_item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let func_item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let args_bits = match pickle_stack_item_to_value(_py, &args_item) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let out_bits = match pickle_apply_reduce(_py, func_item, args_bits) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(out_bits));
                }
                'p' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let key = match pickle_parse_memo_key(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    memo.insert(key, item.clone());
                    stack.push(item);
                }
                'g' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let key = match pickle_parse_memo_key(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(item) = memo.get(&key).cloned() else {
                        let message = format!("pickle.loads: memo key {} missing", key);
                        return pickle_raise(_py, &message);
                    };
                    stack.push(item);
                }
                _ => {
                    let message = format!("pickle.loads: unsupported opcode {:?}", op);
                    return pickle_raise(_py, &message);
                }
            }
        }
        let Some(item) = stack.last() else {
            return pickle_raise(_py, "pickle.loads: pickle stack empty");
        };
        match pickle_stack_item_to_value(_py, item) {
            Ok(value) => value,
            Err(err_bits) => err_bits,
        }
    })
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
    if let Some(ptr) = obj_from_bits(data_bits).as_ptr() {
        if let Some(raw) = unsafe { bytes_like_slice(ptr) } {
            return Ok(raw.to_vec());
        }
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
    let registry_bits = match pickle_resolve_global_bits(_py, "copyreg", "_extension_registry") {
        Ok(bits) => bits,
        Err(err_bits) => return Err(err_bits),
    };
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
    if state.protocol >= 2 {
        if let Some(code) = pickle_lookup_extension_code(_py, &module_name, &attr_name)? {
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
    if state.protocol == 0 {
        if let Some(pid_text) = string_obj_to_owned(obj_from_bits(pid_bits)) {
            state.push(PICKLE_OP_PERSID);
            state.extend(pid_text.as_bytes());
            state.push(b'\n');
            return Ok(true);
        }
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
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        if let Some(raw) = unsafe { bytes_like_slice(ptr) } {
            let out_ptr = crate::alloc_bytes(_py, raw);
            if out_ptr.is_null() {
                return Err(MoltObject::none().bits());
            }
            return Ok(MoltObject::from_ptr(out_ptr).bits());
        }
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
    let _ = pickle_memo_store_if_absent(state, obj_bits);
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
    if let Some(dispatch_bits) = state.dispatch_table_bits {
        if let Some(reduced) = pickle_dispatch_reducer_from_table(_py, dispatch_bits, obj_bits)? {
            return Ok(Some(reduced));
        }
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
    if let Some(state_bits) = state_bits {
        if !obj_from_bits(state_bits).is_none() {
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
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT {
                    if !unsafe { crate::dict_order(dict_ptr).is_empty() } {
                        inc_ref_bits(_py, dict_bits);
                        return Ok(Some(dict_bits));
                    }
                }
            }
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
    if extra_bits != 0 {
        if let Some(extra_ptr) = obj_from_bits(extra_bits).as_ptr() {
            if unsafe { object_type_id(extra_ptr) } == TYPE_ID_DICT {
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

fn pickle_object_state_bits(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Result<Option<u64>, u64> {
    let mut dict_state_bits: Option<u64> = None;
    let dict_bits = unsafe { crate::instance_dict_bits(ptr) };
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
                && !unsafe { crate::dict_order(dict_ptr).is_empty() }
            {
                inc_ref_bits(_py, dict_bits);
                dict_state_bits = Some(dict_bits);
            }
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
        return pickle_object_state_bits(_py, ptr);
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
    if let Some(bits) = kwargs_bits {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
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
    let Some(dict_name_bits) = attr_name_bits_from_bytes(_py, b"__dict__") else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let inst_dict_bits = molt_getattr_builtin(inst_bits, dict_name_bits, missing);
    dec_ref_bits(_py, dict_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if inst_dict_bits == missing {
        return Err(pickle_raise(
            _py,
            "pickle.loads: BUILD state requires __dict__",
        ));
    }
    let Some(inst_dict_ptr) = obj_from_bits(inst_dict_bits).as_ptr() else {
        if !obj_from_bits(inst_dict_bits).is_none() {
            dec_ref_bits(_py, inst_dict_bits);
        }
        return Err(pickle_raise(
            _py,
            "pickle.loads: BUILD state requires __dict__",
        ));
    };
    if unsafe { object_type_id(inst_dict_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, inst_dict_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: BUILD state requires __dict__",
        ));
    }
    let pairs = unsafe { crate::dict_order(state_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        unsafe {
            crate::dict_set_in_place(_py, inst_dict_ptr, pairs[idx], pairs[idx + 1]);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, inst_dict_bits);
            return Err(MoltObject::none().bits());
        }
        idx += 2;
    }
    dec_ref_bits(_py, inst_dict_bits);
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
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
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
    if let Some(state_ptr) = obj_from_bits(state_bits).as_ptr() {
        if unsafe { object_type_id(state_ptr) } == TYPE_ID_TUPLE {
            let fields = unsafe { seq_vec_ref(state_ptr) };
            if fields.len() == 2 {
                dict_state_bits = fields[0];
                slot_state_bits = Some(fields[1]);
            }
        }
    }
    pickle_apply_dict_state(_py, inst_bits, dict_state_bits)?;
    if let Some(slot_bits) = slot_state_bits {
        if !obj_from_bits(slot_bits).is_none() {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_dumps_core(
    obj_bits: u64,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_loads_core(
    data_bits: u64,
    _fix_imports_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    persistent_load_bits: u64,
    find_class_bits: u64,
    buffers_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let encoding = if let Some(text) = string_obj_to_owned(obj_from_bits(encoding_bits)) {
            text
        } else {
            return raise_exception::<_>(_py, "TypeError", "pickle encoding must be str");
        };
        let errors = if let Some(text) = string_obj_to_owned(obj_from_bits(errors_bits)) {
            text
        } else {
            return raise_exception::<_>(_py, "TypeError", "pickle errors must be str");
        };
        let persistent_load =
            match pickle_option_callable_bits(_py, persistent_load_bits, "persistent_load") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let find_class = match pickle_option_callable_bits(_py, find_class_bits, "find_class") {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let data = match pickle_input_to_bytes(_py, data_bits) {
            Ok(bytes) => bytes,
            Err(err_bits) => return err_bits,
        };
        let buffers_iter = if obj_from_bits(buffers_bits).is_none() {
            None
        } else {
            let iter_bits = molt_iter(buffers_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            Some(iter_bits)
        };
        if data.first().is_none_or(|op| *op != PICKLE_OP_PROTO) {
            let text = match String::from_utf8(data) {
                Ok(value) => value,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "pickle.loads: protocol 0/1 payload must be UTF-8",
                    );
                }
            };
            let text_ptr = alloc_string(_py, text.as_bytes());
            if text_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let text_bits = MoltObject::from_ptr(text_ptr).bits();
            let out_bits = molt_pickle_loads_protocol01(text_bits);
            dec_ref_bits(_py, text_bits);
            return out_bits;
        }

        let mut idx: usize = 0;
        let mut stack: Vec<PickleVmItem> = Vec::new();
        let mut memo: Vec<Option<PickleVmItem>> = Vec::new();
        while idx < data.len() {
            let op = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                Ok(value) => value,
                Err(err_bits) => return err_bits,
            };
            match op {
                PICKLE_OP_STOP => break,
                PICKLE_OP_POP => {
                    if stack.pop().is_none() {
                        return pickle_raise(_py, "pickle.loads: stack underflow");
                    }
                }
                PICKLE_OP_POP_MARK => {
                    let mut found_mark = false;
                    while let Some(item) = stack.pop() {
                        if matches!(item, PickleVmItem::Mark) {
                            found_mark = true;
                            break;
                        }
                    }
                    if !found_mark {
                        return pickle_raise(_py, "pickle.loads: mark not found");
                    }
                }
                PICKLE_OP_PROTO => {
                    let version = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if version > PICKLE_PROTO_5 as u8 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unsupported pickle protocol",
                        );
                    }
                }
                PICKLE_OP_FRAME => {
                    if pickle_read_u64_le(data.as_slice(), &mut idx, _py).is_err() {
                        return MoltObject::none().bits();
                    }
                }
                PICKLE_OP_NEXT_BUFFER => {
                    let bits = match pickle_next_external_buffer_bits(_py, buffers_iter) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_READONLY_BUFFER => {
                    let value_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let view_bits =
                        match pickle_buffer_value_to_memoryview(_py, value_bits, "READONLY_BUFFER")
                        {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                    let readonly_bits = if let Some(toreadonly_bits) =
                        match pickle_attr_optional(_py, view_bits, b"toreadonly") {
                            Ok(bits) => bits,
                            Err(err_bits) => return err_bits,
                        } {
                        let out_bits = unsafe { call_callable0(_py, toreadonly_bits) };
                        dec_ref_bits(_py, toreadonly_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        out_bits
                    } else {
                        view_bits
                    };
                    stack.push(PickleVmItem::Value(readonly_bits));
                }
                PICKLE_OP_MARK => stack.push(PickleVmItem::Mark),
                PICKLE_OP_NONE => stack.push(PickleVmItem::Value(MoltObject::none().bits())),
                PICKLE_OP_NEWTRUE => {
                    stack.push(PickleVmItem::Value(MoltObject::from_bool(true).bits()))
                }
                PICKLE_OP_NEWFALSE => {
                    stack.push(PickleVmItem::Value(MoltObject::from_bool(false).bits()))
                }
                PICKLE_OP_INT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid INT payload"),
                    };
                    let bits = match pickle_parse_int_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_BININT => {
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, 4, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let value = i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                    stack.push(PickleVmItem::Value(
                        MoltObject::from_int(value as i64).bits(),
                    ));
                }
                PICKLE_OP_BININT1 => {
                    let value = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as i64,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_int(value).bits()));
                }
                PICKLE_OP_BININT2 => {
                    let value = match pickle_read_u16_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as i64,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_int(value).bits()));
                }
                PICKLE_OP_LONG => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid LONG payload"),
                    };
                    let bits = match pickle_parse_long_line_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_LONG1 | PICKLE_OP_LONG4 => {
                    let size = if op == PICKLE_OP_LONG1 {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_parse_long_bytes_bits(_py, raw) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_FLOAT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid FLOAT payload"),
                    };
                    let bits = match pickle_parse_float_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_BINFLOAT => {
                    let value = match pickle_read_f64_be(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_float(value).bits()));
                }
                PICKLE_OP_STRING => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid STRING payload"),
                    };
                    let parsed = match pickle_parse_string_literal(text) {
                        Ok(v) => v,
                        Err(message) => return pickle_raise(_py, message),
                    };
                    let ptr = alloc_string(_py, parsed.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_UNICODE => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match pickle_decode_utf8(_py, line, "UNICODE payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_BINUNICODE | PICKLE_OP_SHORT_BINUNICODE => {
                    let size = if op == PICKLE_OP_SHORT_BINUNICODE {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match pickle_decode_utf8(_py, raw, "BINUNICODE payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_SHORT_BINBYTES | PICKLE_OP_BINBYTES | PICKLE_OP_BINBYTES8 => {
                    let size = match op {
                        PICKLE_OP_SHORT_BINBYTES => {
                            match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                                Ok(v) => v as usize,
                                Err(err_bits) => return err_bits,
                            }
                        }
                        PICKLE_OP_BINBYTES => {
                            match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                                Ok(v) => v as usize,
                                Err(err_bits) => return err_bits,
                            }
                        }
                        _ => match pickle_read_u64_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        },
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = crate::alloc_bytes(_py, raw);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_BYTEARRAY8 => {
                    let size = match pickle_read_u64_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bytes_ptr = crate::alloc_bytes(_py, raw);
                    if bytes_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
                    let out_bits =
                        pickle_call_with_args(_py, builtin_classes(_py).bytearray, &[bytes_bits]);
                    dec_ref_bits(_py, bytes_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(out_bits));
                }
                PICKLE_OP_EMPTY_TUPLE => {
                    let ptr = alloc_tuple(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_TUPLE1 | PICKLE_OP_TUPLE2 | PICKLE_OP_TUPLE3 => {
                    let needed = if op == PICKLE_OP_TUPLE1 {
                        1
                    } else if op == PICKLE_OP_TUPLE2 {
                        2
                    } else {
                        3
                    };
                    let mut items: Vec<u64> = Vec::with_capacity(needed);
                    for _ in 0..needed {
                        let bits = match pickle_vm_pop_value(_py, &mut stack) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        items.push(bits);
                    }
                    items.reverse();
                    let ptr = alloc_tuple(_py, items.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_TUPLE => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let ptr = alloc_tuple(_py, values.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_EMPTY_LIST => {
                    let ptr = alloc_list_with_capacity(_py, &[], 0);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_LIST => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_APPEND => {
                    let item_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let list_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let _ = crate::molt_list_append(list_bits, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(list_bits));
                }
                PICKLE_OP_APPENDS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let list_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        let _ = crate::molt_list_append(list_bits, bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    stack.push(PickleVmItem::Value(list_bits));
                }
                PICKLE_OP_EMPTY_DICT => {
                    let ptr = alloc_dict_with_pairs(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_DICT => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    if values.len() % 2 != 0 {
                        return pickle_raise(_py, "pickle.loads: dict has odd number of items");
                    }
                    let ptr = alloc_dict_with_pairs(_py, values.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_SETITEM => {
                    let value_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let key_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let dict_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    };
                    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    }
                    unsafe {
                        crate::dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(dict_bits));
                }
                PICKLE_OP_SETITEMS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let dict_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitems target is not dict");
                    };
                    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitems target is not dict");
                    }
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    if values.len() % 2 != 0 {
                        return pickle_raise(
                            _py,
                            "pickle.loads: setitems has odd number of values",
                        );
                    }
                    let mut pair_idx = 0usize;
                    while pair_idx + 1 < values.len() {
                        unsafe {
                            crate::dict_set_in_place(
                                _py,
                                dict_ptr,
                                values[pair_idx],
                                values[pair_idx + 1],
                            );
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        pair_idx += 2;
                    }
                    stack.push(PickleVmItem::Value(dict_bits));
                }
                PICKLE_OP_EMPTY_SET => {
                    let bits = crate::molt_set_new(0);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_ADDITEMS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let set_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(set_ptr) = obj_from_bits(set_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: additems target is not set");
                    };
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        unsafe {
                            crate::set_add_in_place(_py, set_ptr, bits);
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    stack.push(PickleVmItem::Value(set_bits));
                }
                PICKLE_OP_FROZENSET => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let list_ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let list_bits = MoltObject::from_ptr(list_ptr).bits();
                    let tuple_ptr = alloc_tuple(_py, &[list_bits]);
                    dec_ref_bits(_py, list_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    let out_bits =
                        pickle_apply_reduce_bits(_py, builtin_classes(_py).frozenset, args_bits);
                    dec_ref_bits(_py, args_bits);
                    match out_bits {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_GLOBAL => {
                    let module = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let name = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let module_text = match pickle_decode_utf8(_py, module, "GLOBAL module") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let name_text = match pickle_decode_utf8(_py, name, "GLOBAL name") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    if let Some(global) = pickle_resolve_global(&module_text, &name_text) {
                        stack.push(PickleVmItem::Global(global));
                    } else {
                        match pickle_resolve_global_with_hook(
                            _py,
                            &module_text,
                            &name_text,
                            find_class,
                        ) {
                            Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                            Err(err_bits) => return err_bits,
                        }
                    }
                }
                PICKLE_OP_STACK_GLOBAL => {
                    let name_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let module_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(module) = string_obj_to_owned(obj_from_bits(module_bits)) else {
                        return pickle_raise(_py, "pickle.loads: STACK_GLOBAL module must be str");
                    };
                    let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
                        return pickle_raise(_py, "pickle.loads: STACK_GLOBAL name must be str");
                    };
                    if let Some(global) = pickle_resolve_global(&module, &name) {
                        stack.push(PickleVmItem::Global(global));
                    } else {
                        match pickle_resolve_global_with_hook(_py, &module, &name, find_class) {
                            Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                            Err(err_bits) => return err_bits,
                        }
                    }
                }
                PICKLE_OP_REDUCE => {
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let callable_item = match stack.pop() {
                        Some(v) => v,
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    match pickle_apply_reduce_vm(_py, callable_item, args_bits) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_NEWOBJ => {
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let cls_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_newobj(_py, cls_bits, args_bits, None) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_NEWOBJ_EX => {
                    let kwargs_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let cls_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_newobj(_py, cls_bits, args_bits, Some(kwargs_bits)) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_BUILD => {
                    let state_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let inst_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_build(_py, inst_bits, state_bits) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_PUT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid PUT payload"),
                    };
                    let index = match text.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid memo key"),
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_BINPUT => {
                    let index = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_LONG_BINPUT => {
                    let index = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_MEMOIZE => {
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    memo.push(Some(item));
                }
                PICKLE_OP_GET => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid GET payload"),
                    };
                    let index = match text.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid memo key"),
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_BINGET => {
                    let index = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_LONG_BINGET => {
                    let index = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_PERSID => {
                    let pid_line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let pid_text = match pickle_decode_utf8(_py, pid_line, "PERSID payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(pid_bits) = alloc_string_bits(_py, pid_text.as_str()) else {
                        return MoltObject::none().bits();
                    };
                    let Some(persistent_load_bits) = persistent_load else {
                        dec_ref_bits(_py, pid_bits);
                        return pickle_raise(
                            _py,
                            "pickle.loads: persistent IDs require persistent_load",
                        );
                    };
                    let value_bits = unsafe { call_callable1(_py, persistent_load_bits, pid_bits) };
                    dec_ref_bits(_py, pid_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(value_bits));
                }
                PICKLE_OP_BINPERSID => {
                    let pid_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(persistent_load_bits) = persistent_load else {
                        return pickle_raise(
                            _py,
                            "pickle.loads: persistent IDs require persistent_load",
                        );
                    };
                    let value_bits = unsafe { call_callable1(_py, persistent_load_bits, pid_bits) };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(value_bits));
                }
                PICKLE_OP_EXT1 | PICKLE_OP_EXT2 | PICKLE_OP_EXT4 => {
                    let code = if op == PICKLE_OP_EXT1 {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    } else if op == PICKLE_OP_EXT2 {
                        match pickle_read_u16_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    match pickle_lookup_extension_bits(_py, code, find_class) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                // Python 2 string opcodes.
                b'U' => {
                    let size = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_decode_8bit_string(_py, raw, &encoding, &errors) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                b'T' => {
                    let size = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_decode_8bit_string(_py, raw, &encoding, &errors) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                _ => {
                    let msg = format!("pickle.loads: unsupported opcode 0x{op:02x}");
                    return pickle_raise(_py, msg.as_str());
                }
            }
        }
        let Some(item) = stack.last() else {
            return pickle_raise(_py, "pickle.loads: pickle stack empty");
        };
        match pickle_vm_item_to_bits(_py, item) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
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
            if let Some(idx) = last_header {
                if let Some((_, value)) = message.headers.get_mut(idx) {
                    value.push(' ');
                    value.push_str(line.trim());
                }
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

fn email_day_of_year0(year: i64, month: i64, day: i64) -> i64 {
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let month_lengths = [
        31i64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut sum = 0i64;
    let month_index = usize::try_from(month.saturating_sub(1)).unwrap_or(0);
    for len in month_lengths
        .iter()
        .take(month_index.min(month_lengths.len()))
    {
        sum += *len;
    }
    sum + day.saturating_sub(1)
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
            if let (Some(start), Some(end)) = (entry.rfind('<'), entry.rfind('>')) {
                if start < end {
                    let name = entry[..start].trim().trim_matches('"').to_string();
                    let addr = entry[start + 1..end].trim().to_string();
                    out.push((name, addr));
                    continue;
                }
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
        urllib_response_drop(handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
