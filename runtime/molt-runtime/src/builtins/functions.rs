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
use crate::audit::{AuditArgs, audit_capability_decision};
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

#[cfg(target_arch = "wasm32")]
use super::exceptions::{molt_exception_init, molt_exception_new_bound, molt_exceptiongroup_init};
#[cfg(target_arch = "wasm32")]
use crate::builtins::types::{
    molt_object_new_bound, molt_type_init, molt_type_new, molt_types_capsule_new,
    molt_types_cell_new, molt_types_coroutine, molt_types_dynamic_class_attr_init,
    molt_types_get_original_bases, molt_types_mappingproxy_init, molt_types_mappingproxy_new,
    molt_types_method_init, molt_types_method_new, molt_types_new_class, molt_types_prepare_class,
    molt_types_resolve_bases, molt_types_simplenamespace_init,
};
#[cfg(target_arch = "wasm32")]
use crate::object::ops_builtins::{molt_object_init, molt_object_init_subclass, molt_type_call};

#[cfg(target_arch = "wasm32")]
const RESERVED_WASM_RUNTIME_CALLABLE_BASE: u64 = 33;

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_info(
    fn_ptr: u64,
) -> Option<(u64, &'static str, &'static str, usize)> {
    macro_rules! entry_list {
        ($(($idx:expr, $sym:ident, $import:literal, $arity:expr))+) => {
            {
                $(
                    if fn_ptr == fn_addr!($sym) {
                        return Some(($idx as u64, stringify!($sym), $import, $arity));
                    }
                )+
            }
        };
    }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wasm_runtime_callables.inc"
    ));
    None
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_ptr(fn_ptr: u64) -> Option<u64> {
    let base = crate::wasm_table_base();
    reserved_wasm_runtime_callable_info(fn_ptr)
        .map(|(idx, _sym, _import, _arity)| base + RESERVED_WASM_RUNTIME_CALLABLE_BASE + idx)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn reserved_wasm_runtime_callable_arity(fn_ptr: u64) -> Option<usize> {
    reserved_wasm_runtime_callable_info(fn_ptr).map(|(_idx, _sym, _import, arity)| arity)
}

#[cfg(target_arch = "wasm32")]
const RESERVED_WASM_RUNTIME_CALLABLE_COUNT: u64 = {
    macro_rules! entry_list {
        ($(($idx:expr, $sym:ident, $import:literal, $arity:expr))+) => {
            [$( { let _ = ($idx, stringify!($sym), $import, $arity); () }, )+].len() as u64
        };
    }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wasm_runtime_callables.inc"
    ))
};

#[cfg(target_arch = "wasm32")]
const RESERVED_WASM_RUNTIME_TRAMPOLINE_BASE: u64 =
    RESERVED_WASM_RUNTIME_CALLABLE_BASE + RESERVED_WASM_RUNTIME_CALLABLE_COUNT;

#[cfg(target_arch = "wasm32")]
fn reserved_wasm_runtime_trampoline_ptr(fn_ptr: u64) -> Option<u64> {
    let base = crate::wasm_table_base();
    macro_rules! entry_list {
        ($(($idx:expr, $sym:ident, $import:literal, $arity:expr))+) => {
            {
                $(
                    if fn_ptr == fn_addr!($sym) {
                        return Some(base + RESERVED_WASM_RUNTIME_TRAMPOLINE_BASE + ($idx as u64));
                    }
                )+
            }
        };
    }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wasm_runtime_callables.inc"
    ));
    None
}

#[inline]
pub(crate) fn runtime_callable_represents_symbol(
    fn_ptr: u64,
    _tramp_ptr: u64,
    symbol_fn_ptr: u64,
) -> bool {
    if fn_ptr == symbol_fn_ptr {
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if reserved_wasm_runtime_callable_ptr(symbol_fn_ptr) == Some(fn_ptr) {
            return true;
        }
        if reserved_wasm_runtime_trampoline_ptr(symbol_fn_ptr) == Some(_tramp_ptr) {
            return true;
        }
    }
    false
}

pub(crate) fn alloc_runtime_function_obj(
    _py: &crate::PyToken<'_>,
    fn_ptr: u64,
    arity: u64,
) -> *mut u8 {
    let ptr = alloc_function_obj(_py, fn_ptr, arity);
    if ptr.is_null() {
        return ptr;
    }
    #[cfg(target_arch = "wasm32")]
    unsafe {
        if let Some(tramp_ptr) = reserved_wasm_runtime_trampoline_ptr(fn_ptr) {
            function_set_trampoline_ptr(ptr, tramp_ptr);
        }
    }
    ptr
}


#[inline]
// Regex engine extracted to functions_re.rs.
use super::functions_re::*;


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
// shlex extracted to functions_shlex.rs
// shlex extracted to functions_shlex.rs


// Fnmatch implementation extracted to functions_fnmatch.rs.
use super::functions_fnmatch::*;


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
// shlex extracted to functions_shlex.rs
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
// shlex extracted to functions_shlex.rs

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

// Textwrap implementation extracted to functions_textwrap.rs.
use super::functions_textwrap::*;


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
// pkgutil extracted to functions_pkgutil.rs
// pkgutil extracted to functions_pkgutil.rs
// pkgutil extracted to functions_pkgutil.rs
// pkgutil extracted to functions_pkgutil.rs

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
// compileall extracted to functions_compileall.rs
// compileall extracted to functions_compileall.rs
// shlex extracted to functions_shlex.rs
// shlex extracted to functions_shlex.rs
// shlex extracted to functions_shlex.rs
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
// shlex extracted to functions_shlex.rs

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
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs

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
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs
// opcode extracted to functions_opcode.rs

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
// argparse extracted to functions_argparse.rs
// argparse extracted to functions_argparse.rs
// argparse extracted to functions_argparse.rs
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
// argparse extracted to functions_argparse.rs

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
// bisect extracted to functions_bisect.rs
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
// bisect extracted to functions_bisect.rs
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
// bisect extracted to functions_bisect.rs
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
// bisect extracted to functions_bisect.rs
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
// bisect extracted to functions_bisect.rs
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
// bisect extracted to functions_bisect.rs
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
// bisect extracted to functions_bisect.rs
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
// pkgutil extracted to functions_pkgutil.rs
// pkgutil extracted to functions_pkgutil.rs

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_copyfile(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed_1 = crate::has_capability(_py, "fs.read");
        audit_capability_decision("shutil.copyfile", "fs.read", AuditArgs::None, allowed_1);
        let allowed_2 = crate::has_capability(_py, "fs.write");
        audit_capability_decision("shutil.copyfile", "fs.write", AuditArgs::None, allowed_2);
        if !allowed_1 || !allowed_2 {
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
        let allowed_1 = crate::has_capability(_py, "fs.read");
        audit_capability_decision("shutil.which", "fs.read", AuditArgs::None, allowed_1);
        let allowed_2 = crate::has_capability(_py, "env.read");
        audit_capability_decision("shutil.which", "env.read", AuditArgs::None, allowed_2);
        if !allowed_1 || !allowed_2 {
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
        let allowed_1 = crate::has_capability(_py, "fs.read");
        audit_capability_decision("py.compile.compile", "fs.read", AuditArgs::None, allowed_1);
        let allowed_2 = crate::has_capability(_py, "fs.write");
        audit_capability_decision("py.compile.compile", "fs.write", AuditArgs::None, allowed_2);
        if !allowed_1 || !allowed_2 {
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
// compileall extracted to functions_compileall.rs
// compileall extracted to functions_compileall.rs
// compileall extracted to functions_compileall.rs
    skip_curdir_bits: u64,
    maxlevels_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision(
            "compileall.compile.path",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs
// codeop extracted to functions_codeop.rs

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
// codeop extracted to functions_codeop.rs
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
// codeop extracted to functions_codeop.rs
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
// codeop extracted to functions_codeop.rs
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
// codeop extracted to functions_codeop.rs
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
// codeop extracted to functions_codeop.rs
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
// codeop extracted to functions_codeop.rs
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
// codeop extracted to functions_codeop.rs
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
        let trace = matches!(
            std::env::var("MOLT_TRACE_FUNC_NEW").ok().as_deref(),
            Some("1")
        );
        if trace {
            eprintln!("molt func new: fn_ptr={fn_ptr} tramp_ptr={trampoline_ptr} arity={arity}");
        }
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
        let trace = matches!(
            std::env::var("MOLT_TRACE_FUNC_NEW").ok().as_deref(),
            Some("1")
        );
        if trace {
            eprintln!(
                "molt func new closure: fn_ptr={fn_ptr} tramp_ptr={trampoline_ptr} arity={arity} closure_bits={closure_bits}"
            );
        }
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
