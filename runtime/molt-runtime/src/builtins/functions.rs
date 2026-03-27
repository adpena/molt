#[cfg(not(feature = "stdlib_serial"))]
use super::functions_email::email_quopri_alloc_str;
#[cfg(feature = "stdlib_serial")]
fn email_quopri_alloc_str(_py: &crate::PyToken<'_>, value: &str) -> u64 {
    let ptr = crate::alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}
use molt_obj_model::MoltObject;
#[cfg(feature = "stdlib_ast")]
use rustpython_parser::{Mode as ParseMode, ParseErrorType, ast as pyast, parse as parse_python};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::types::cell_class;
use crate::builtins::numbers::index_i64_with_overflow;
use crate::builtins::platform::env_state_get;
use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_LIST,
    TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_bound_method_obj, alloc_bytes,
    alloc_code_obj, alloc_dict_with_pairs, alloc_function_obj, alloc_list_with_capacity,
    alloc_string, alloc_tuple, attr_name_bits_from_bytes, bound_method_func_bits, builtin_classes,
    bytes_like_slice, call_callable1, call_callable2, dec_ref_bits, dict_get_in_place,
    ensure_function_code_bits, exception_pending, function_dict_bits, function_set_closure_bits,
    function_set_trampoline_ptr, inc_ref_bits, is_truthy, missing_bits, module_dict_bits,
    molt_getattr_builtin, molt_getitem_method, molt_iter, molt_iter_next, molt_trace_enter_slot,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, raise_exception,
    seq_vec_ref, string_obj_to_owned, to_i64, type_name,
};
use memchr::{memchr, memmem};

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

const OPCODE_PAYLOAD_312_JSON: &str = include_str!("../intrinsics/data/opcode_payload_312.json");
const OPCODE_METADATA_PAYLOAD_314_JSON: &str =
    include_str!("../intrinsics/data/opcode_metadata_payload_314.json");
const TOKEN_PAYLOAD_312_JSON: &str = include_str!("../intrinsics/data/token_payload_312.json");

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

pub(super) fn iter_next_pair(_py: &crate::PyToken<'_>, iter_bits: u64) -> Result<(u64, bool), u64> {
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

pub(super) fn iterable_to_string_vec(
    _py: &crate::PyToken<'_>,
    values_bits: u64,
) -> Result<Vec<String>, u64> {
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

pub(super) fn alloc_string_list(_py: &crate::PyToken<'_>, values: &[String]) -> u64 {
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

pub(super) fn alloc_string_bits(_py: &crate::PyToken<'_>, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

#[allow(dead_code)]
pub(super) fn alloc_string_tuple(_py: &crate::PyToken<'_>, values: &[String]) -> u64 {
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

#[allow(dead_code)]
pub(super) fn alloc_qsl_list(_py: &crate::PyToken<'_>, items: &[(String, String)]) -> u64 {
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

#[allow(dead_code)]
pub(super) fn alloc_qs_dict(
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
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "compile() requires the stdlib_ast feature",
        )
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
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "compile() requires the stdlib_ast feature",
        )
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
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "compile() requires the stdlib_ast feature",
        )
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
