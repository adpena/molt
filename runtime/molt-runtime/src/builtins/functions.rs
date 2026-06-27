const THIS_ENCODED: &str = concat!(
    "NoErapgvbaf sbe perngvat naq znavchyngvat P-glcr pbqrf.",
    " Gur P zbqhyr cebivqrf sbezhynf sbe qrpbqvat, rapbqvat,",
    " naq genafyngvat fcrpvsvp punenpgre rapbqvatf.",
);

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::builtins::callable::is_callable_impl;
use crate::builtins::exceptions::{
    molt_exception_init, molt_exception_new_bound, molt_exceptiongroup_init,
};
use crate::builtins::functions_re::this_build_rot13_text;
use crate::builtins::types::{
    molt_object_new_bound, molt_type_init, molt_type_new, molt_types_capsule_new,
    molt_types_cell_new, molt_types_coroutine, molt_types_dynamic_class_attr_init,
    molt_types_get_original_bases, molt_types_mappingproxy_init, molt_types_mappingproxy_new,
    molt_types_method_init, molt_types_method_new, molt_types_new_class, molt_types_prepare_class,
    molt_types_resolve_bases, molt_types_simplenamespace_init,
};
use crate::object::layout::{
    code_arg_names_bits, code_callable_arity, code_callable_fn_ptr, code_callable_trampoline_ptr,
    code_kwonly_names_bits, code_name_bits, code_set_signature_bits, code_signature_posonly_bits,
    code_vararg_bits, code_varkw_bits, function_defaults_version, function_set_call_target_ptr,
    function_set_code_bits,
};
use crate::object::ops_builtins::{molt_object_init, molt_object_init_subclass, molt_type_call};
use molt_obj_model::MoltObject;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::types::cell_class;
use crate::builtins::numbers::index_i64_with_overflow;
use crate::builtins::platform::env_state_get;
use crate::{
    PyToken, TYPE_ID_BOUND_METHOD, TYPE_ID_CODE, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_LIST,
    TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_bound_method_obj, alloc_code_obj,
    alloc_dict_with_pairs, alloc_function_obj, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bound_method_func_bits, builtin_classes, call_callable1,
    call_callable2, dec_ref_bits, dict_get_in_place, ensure_function_code_bits, exception_pending,
    function_globals_bits, function_set_closure_bits, function_set_globals_bits,
    function_set_globals_override_enabled, function_set_trampoline_ptr, inc_ref_bits, is_truthy,
    missing_bits, module_dict_bits, molt_getattr_builtin, molt_getitem_method, molt_iter,
    molt_iter_next, molt_trace_enter_slot, obj_from_bits, object_class_bits, object_set_class_bits,
    object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned, to_i64, type_name,
};
mod compile_codeop;
mod function_abi;
mod opcode_payload;
mod tokenize;
mod wasm_callables_generated;

pub use compile_codeop::*;
pub use function_abi::*;
pub use opcode_payload::*;
pub use tokenize::*;

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm_callables_generated::{
    WASM_POLL_SLOT_MAX_OFFSET, wasm_poll_table_slot_from_symbol_name,
};

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
    crate::with_gil_entry_nopanic!(_py, {
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

// ─── logging filter intrinsics ──────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_filter_check(filter_name_bits: u64, record_name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
pub extern "C" fn molt_pkgutil_iter_modules(path_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision("pkgutil.iter.modules", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision("pkgutil.walk.packages", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_file(fullname_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision(
            "compileall.compile.file",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision(
            "compileall.compile.dir",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wsgiref_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipapp_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xmlrpc_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tomllib_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_symtable_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_import_smoke_runtime_ready() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
}
