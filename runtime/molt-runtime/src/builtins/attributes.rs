use crate::PyToken;
use crate::object::{HEADER_FLAG_COROUTINE, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF, NEWLINE_KIND_LF};
use molt_obj_model::MoltObject;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, OnceLock};

use crate::async_rt::generators::{generator_locals_dict, generator_yieldfrom_bits};
use crate::builtins::annotations::pep649_enabled;
use crate::builtins::attr::{
    awaitable_await_func_bits, class_slots_info, exception_is_attribute_error,
    object_attr_lookup_raw,
};
use crate::builtins::methods::{
    asyncgen_method_bits, complex_method_bits, coroutine_method_bits, generator_method_bits,
    int_method_bits, object_method_bits, property_method_bits,
};
use crate::*;

static PROPERTY_DOCS: OnceLock<Mutex<HashMap<PtrSlot, u64>>> = OnceLock::new();
static PROPERTY_DOC_NAME: AtomicU64 = AtomicU64::new(0);
static ATTR_SITE_NAME_CACHE: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();

fn is_task_trampoline_attr_name(attr_name: &str) -> bool {
    matches!(
        attr_name,
        "__molt_is_generator__" | "__molt_is_coroutine__" | "__molt_is_async_generator__"
    )
}

fn property_docs() -> &'static Mutex<HashMap<PtrSlot, u64>> {
    PROPERTY_DOCS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn attr_site_name_cache() -> &'static Mutex<HashMap<u64, u64>> {
    ATTR_SITE_NAME_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn clear_attr_site_name_cache(_py: &PyToken<'_>) {
    let mut cache = attr_site_name_cache().lock().unwrap();
    for (_site, bits) in cache.drain() {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
}

fn ic_site_from_bits(site_bits: u64) -> Option<u64> {
    let site = obj_from_bits(site_bits);
    if let Some(i) = site.as_int() {
        return u64::try_from(i).ok();
    }
    if site.is_bool() {
        return Some(if site.as_bool().unwrap_or(false) {
            1
        } else {
            0
        });
    }
    if site.is_ptr() || site.is_none() || site.is_pending() {
        return None;
    }
    Some(site_bits)
}

unsafe fn attr_name_bits_for_site(_py: &PyToken<'_>, site_id: u64, slice: &[u8]) -> Option<u64> {
    unsafe {
        let mut cache = attr_site_name_cache().lock().unwrap();
        if let Some(bits) = cache.get(&site_id).copied() {
            if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    let cached = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    if cached == slice {
                        profile_hit_unchecked(&ATTR_SITE_NAME_CACHE_HIT_COUNT);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                }
            }
            dec_ref_bits(_py, bits);
            cache.remove(&site_id);
        }
        profile_hit_unchecked(&ATTR_SITE_NAME_CACHE_MISS_COUNT);
        let bits = attr_name_bits_from_bytes(_py, slice)?;
        inc_ref_bits(_py, bits);
        cache.insert(site_id, bits);
        Some(bits)
    }
}

fn property_doc_bits(_py: &PyToken<'_>, prop_ptr: *mut u8) -> u64 {
    if let Some(bits) = property_docs()
        .lock()
        .unwrap()
        .get(&PtrSlot(prop_ptr))
        .copied()
    {
        inc_ref_bits(_py, bits);
        return bits;
    }
    let get_bits = unsafe { property_get_bits(prop_ptr) };
    if obj_from_bits(get_bits).is_none() {
        return MoltObject::none().bits();
    }
    if let Some(get_ptr) = obj_from_bits(get_bits).as_ptr() {
        if unsafe { object_type_id(get_ptr) } == TYPE_ID_FUNCTION {
            let doc_bits = intern_static_name(_py, &PROPERTY_DOC_NAME, b"__doc__");
            if let Some(bits) = unsafe { function_attr_bits(_py, get_ptr, doc_bits) } {
                inc_ref_bits(_py, bits);
                return bits;
            }
        }
    }
    MoltObject::none().bits()
}

fn property_doc_set(_py: &PyToken<'_>, prop_ptr: *mut u8, val_bits: u64) {
    let mut guard = property_docs().lock().unwrap();
    let key = PtrSlot(prop_ptr);
    if obj_from_bits(val_bits).is_none() {
        if let Some(old_bits) = guard.remove(&key) {
            dec_ref_bits(_py, old_bits);
        }
        return;
    }
    inc_ref_bits(_py, val_bits);
    if let Some(old_bits) = guard.insert(key, val_bits) {
        dec_ref_bits(_py, old_bits);
    }
}

fn is_typing_param(_py: &PyToken<'_>, bits: u64) -> bool {
    if obj_from_bits(bits).is_none() {
        return false;
    }
    let class_bits = type_of_bits(_py, bits);
    let name = class_name_for_error(class_bits);
    matches!(name.as_str(), "_TypeVar" | "_ParamSpec" | "_TypeVarTuple")
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_positions(code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let code_obj = obj_from_bits(code_bits);
        let Some(code_ptr) = code_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code.co_positions() requires code");
        };
        unsafe {
            if object_type_id(code_ptr) != TYPE_ID_CODE {
                return raise_exception::<_>(_py, "TypeError", "code.co_positions() requires code");
            }
        }

        let mut owned_table = false;
        let mut table_bits = unsafe { code_linetable_bits(code_ptr) };
        let needs_fallback = if let Some(table_ptr) = obj_from_bits(table_bits).as_ptr() {
            unsafe {
                object_type_id(table_ptr) != TYPE_ID_TUPLE || seq_vec_ref(table_ptr).is_empty()
            }
        } else {
            true
        };

        if needs_fallback {
            let mut line = unsafe { code_firstlineno(code_ptr) };
            let mut start_col = 0i64;
            let mut end_col = 0i64;
            if let Some(filename) =
                string_obj_to_owned(obj_from_bits(unsafe { code_filename_bits(code_ptr) }))
            {
                if let Ok(contents) = std::fs::read_to_string(&filename) {
                    let lines: Vec<&str> = contents.lines().collect();
                    let mut line_index = if line > 0 {
                        (line as usize).saturating_sub(1)
                    } else {
                        0
                    };
                    if let Some(raw_line) = lines.get(line_index).copied() {
                        let mut trimmed = raw_line.trim_end_matches(['\r', '\n']);
                        let starts_def = {
                            let lead = trimmed.trim_start();
                            lead.starts_with("def ") || lead.starts_with("async def ")
                        };
                        if starts_def {
                            let next_index = line_index.saturating_add(1);
                            if let Some(next_line) = lines.get(next_index).copied() {
                                line_index = next_index;
                                line = (line_index + 1) as i64;
                                trimmed = next_line.trim_end_matches(['\r', '\n']);
                            }
                        }
                        end_col = trimmed.chars().count() as i64;
                        if let Some(pos) = trimmed.find("return ") {
                            start_col = (pos + "return ".len()) as i64;
                        } else if let Some(pos) = trimmed.chars().position(|ch| !ch.is_whitespace())
                        {
                            start_col = pos as i64;
                        }
                        if line <= 0 {
                            line = (line_index + 1) as i64;
                        }
                    }
                }
            }
            let line_bits = MoltObject::from_int(line).bits();
            let start_col_bits = MoltObject::from_int(start_col).bits();
            let end_col_bits = MoltObject::from_int(end_col).bits();
            let pos_ptr = alloc_tuple(_py, &[line_bits, line_bits, start_col_bits, end_col_bits]);
            if pos_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let pos_bits = MoltObject::from_ptr(pos_ptr).bits();
            let table_ptr = alloc_tuple(_py, &[pos_bits]);
            dec_ref_bits(_py, pos_bits);
            if table_ptr.is_null() {
                return MoltObject::none().bits();
            }
            table_bits = MoltObject::from_ptr(table_ptr).bits();
            owned_table = true;
        }

        let iter_bits = molt_iter(table_bits);
        if owned_table {
            dec_ref_bits(_py, table_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        iter_bits
    })
}

unsafe fn classed_attr_lookup_without_dict_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_bits: u64,
    attr_bits: u64,
    allow_custom_getattribute: bool,
) -> Option<u64> {
    unsafe {
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let getattribute_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattribute_name,
            b"__getattribute__",
        );
        let getattribute_raw = class_attr_lookup_raw_mro(_py, class_ptr, getattribute_bits);
        let default_getattribute_bits = object_method_bits(_py, "__getattribute__");
        let use_custom_getattribute = allow_custom_getattribute
            && match (getattribute_raw, default_getattribute_bits) {
                (Some(raw_bits), Some(default_bits)) => {
                    !obj_eq(_py, obj_from_bits(raw_bits), obj_from_bits(default_bits))
                }
                (Some(_), None) => true,
                (None, _) => false,
            };
        if use_custom_getattribute
            && !obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(getattribute_bits),
            )
        {
            if let Some(call_bits) =
                class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), getattribute_bits)
            {
                let getattr_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.getattr_name,
                    b"__getattr__",
                );
                let getattr_candidate =
                    !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits))
                        && class_attr_lookup_raw_mro(_py, class_ptr, getattr_bits).is_some();
                if getattr_candidate {
                    traceback_suppress_enter();
                }
                exception_stack_push();
                let res_bits = call_callable1(_py, call_bits, attr_bits);
                if getattr_candidate {
                    traceback_suppress_exit();
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    let kind_bits = molt_exception_kind(exc_bits);
                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                    dec_ref_bits(_py, kind_bits);
                    if kind.as_deref() == Some("AttributeError") && getattr_candidate {
                        molt_exception_clear();
                        dec_ref_bits(_py, exc_bits);
                        exception_stack_pop(_py);
                        if let Some(getattr_call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            getattr_bits,
                        ) {
                            let getattr_res = call_callable1(_py, getattr_call_bits, attr_bits);
                            if exception_pending(_py) {
                                return None;
                            }
                            return Some(getattr_res);
                        }
                    }
                    dec_ref_bits(_py, exc_bits);
                    exception_stack_pop(_py);
                    return None;
                }
                exception_stack_pop(_py);
                return Some(res_bits);
            }
        }
        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
            if descriptor_is_data(_py, val_bits) {
                if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                    return Some(bound);
                }
                if exception_pending(_py) {
                    return None;
                }
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            inc_ref_bits(_py, class_bits);
            return Some(class_bits);
        }
        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
            if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        let getattr_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattr_name,
            b"__getattr__",
        );
        if !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
            if let Some(call_bits) =
                class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), getattr_bits)
            {
                let res_bits = call_callable1(_py, call_bits, attr_bits);
                if exception_pending(_py) {
                    return None;
                }
                return Some(res_bits);
            }
        }
        None
    }
}

unsafe fn classed_attr_lookup_without_dict(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_bits: u64,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { classed_attr_lookup_without_dict_inner(_py, obj_ptr, class_bits, attr_bits, true) }
}

pub(crate) unsafe fn type_attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        if is_builtin_class_bits(_py, class_bits) {
            let getattribute_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.getattribute_name,
                b"__getattribute__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(getattribute_bits),
            ) {
                if let Some(func_bits) =
                    builtin_class_method_bits(_py, class_bits, "__getattribute__")
                {
                    return descriptor_bind(_py, func_bits, obj_ptr, None);
                }
            }
            let setattr_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.setattr_name,
                b"__setattr__",
            );
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(setattr_bits)) {
                if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__setattr__") {
                    return descriptor_bind(_py, func_bits, obj_ptr, None);
                }
            }
            let delattr_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.delattr_name,
                b"__delattr__",
            );
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(delattr_bits)) {
                if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__delattr__") {
                    return descriptor_bind(_py, func_bits, obj_ptr, None);
                }
            }
        }
        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            if name == "__init_subclass__"
                && matches!(
                    std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                    Some("1")
                )
            {
                let builtins = builtin_classes(_py);
                eprintln!(
                    "molt init_subclass lookup class_bits=0x{:x} builtins.object=0x{:x} is_builtin={}",
                    class_bits,
                    builtins.object,
                    is_builtin_class_bits(_py, class_bits),
                );
            }
            if name == "__class__" {
                let builtins = builtin_classes(_py);
                let class_bits = object_class_bits(obj_ptr);
                let res_bits = if class_bits != 0 {
                    class_bits
                } else {
                    builtins.type_obj
                };
                inc_ref_bits(_py, res_bits);
                return Some(res_bits);
            }

            // Builtin-type class surfaces that are implemented as Rust intrinsics rather than
            // being materialized in the class dict.
            //
            // CPython: bytes/bytearray expose `fromhex` as a classmethod and `maketrans` as a
            // staticmethod on the type object.
            let builtins = builtin_classes(_py);
            if class_bits == builtins.bytes {
                if name == "fromhex" {
                    static BYTES_FROMHEX: AtomicU64 = AtomicU64::new(0);
                    let func_bits =
                        builtin_func_bits(_py, &BYTES_FROMHEX, fn_addr!(molt_bytes_fromhex), 2);
                    let bound = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound);
                }
                if name == "maketrans" {
                    if let Some(func_bits) = bytes_method_bits(_py, "maketrans") {
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                }
            }
            if class_bits == builtins.bytearray {
                if name == "fromhex" {
                    static BYTEARRAY_FROMHEX: AtomicU64 = AtomicU64::new(0);
                    let func_bits = builtin_func_bits(
                        _py,
                        &BYTEARRAY_FROMHEX,
                        fn_addr!(molt_bytearray_fromhex),
                        2,
                    );
                    let bound = molt_bound_method_new(func_bits, class_bits);
                    return Some(bound);
                }
                if name == "maketrans" {
                    if let Some(func_bits) = bytearray_method_bits(_py, "maketrans") {
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                }
            }

            if name == "__name__" {
                let name_bits = class_name_bits(obj_ptr);
                inc_ref_bits(_py, name_bits);
                return Some(name_bits);
            }
            if name == "__qualname__" {
                let qualname_bits = class_qualname_bits(obj_ptr);
                let bits = if qualname_bits == 0 {
                    class_name_bits(obj_ptr)
                } else {
                    qualname_bits
                };
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            if name == "__dict__" {
                let dict_bits = class_dict_bits(obj_ptr);
                let mappingproxy_bits = crate::builtins::types::mappingproxy_class_bits(_py);
                if !obj_from_bits(mappingproxy_bits).is_none() {
                    let res_bits = call_callable1(_py, mappingproxy_bits, dict_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    return Some(res_bits);
                }
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            if name == "__annotate__" && pep649_enabled() {
                let mut annotate_bits = class_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    let annotate_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotate_name,
                        b"__annotate__",
                    );
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            if let Some(val_bits) =
                                dict_get_in_place(_py, dict_ptr, annotate_name_bits)
                            {
                                annotate_bits = val_bits;
                                class_set_annotate_bits(_py, obj_ptr, annotate_bits);
                            }
                        }
                    }
                    if annotate_bits == 0 {
                        annotate_bits = MoltObject::none().bits();
                    }
                }
                inc_ref_bits(_py, annotate_bits);
                return Some(annotate_bits);
            }
            if name == "__annotations__" {
                let annotations_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotations_name,
                    b"__annotations__",
                );
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotations_bits) {
                            inc_ref_bits(_py, val_bits);
                            class_set_annotations_bits(_py, obj_ptr, val_bits);
                            return Some(val_bits);
                        }
                    }
                }
                let cached = class_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(_py, cached);
                    return Some(cached);
                }
                let annotate_bits = class_annotate_bits(obj_ptr);
                let res_bits = if pep649_enabled()
                    && annotate_bits != 0
                    && !obj_from_bits(annotate_bits).is_none()
                {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(_py, annotate_bits, format_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                };
                class_set_annotations_bits(_py, obj_ptr, res_bits);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, annotations_bits, res_bits);
                    }
                }
                return Some(res_bits);
            }
            if name == "__name__" {
                let bits = class_name_bits(obj_ptr);
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            if name == "__base__" {
                let bases_bits = class_bases_bits(obj_ptr);
                let bases = class_bases_vec(bases_bits);
                if bases.is_empty() {
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(_py, none_bits);
                    return Some(none_bits);
                }
                let base_bits = bases[0];
                inc_ref_bits(_py, base_bits);
                return Some(base_bits);
            }
            if name == "__bases__" {
                let bases_bits = class_bases_bits(obj_ptr);
                let bases_obj = obj_from_bits(bases_bits);
                if bases_obj.is_none() || bases_bits == 0 {
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return None;
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                if let Some(bases_ptr) = bases_obj.as_ptr() {
                    let bases_type = object_type_id(bases_ptr);
                    if bases_type == TYPE_ID_TUPLE {
                        inc_ref_bits(_py, bases_bits);
                        return Some(bases_bits);
                    }
                    if bases_type == TYPE_ID_TYPE {
                        let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                        if tuple_ptr.is_null() {
                            return None;
                        }
                        return Some(MoltObject::from_ptr(tuple_ptr).bits());
                    }
                }
                return None;
            }
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if name == "fromkeys" {
                let builtins = builtin_classes(_py);
                if issubclass_bits(class_bits, builtins.dict) {
                    if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                        let bound_bits = molt_bound_method_new(func_bits, class_bits);
                        return Some(bound_bits);
                    }
                }
            }
            if is_builtin_class_bits(_py, class_bits) {
                if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, name.as_str()) {
                    if name == "__init_subclass__"
                        && matches!(
                            std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                            Some("1")
                        )
                    {
                        eprintln!("molt init_subclass builtin bits=0x{:x}", func_bits);
                    }
                    return descriptor_bind(_py, func_bits, obj_ptr, None);
                } else if name == "__init_subclass__"
                    && matches!(
                        std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                        Some("1")
                    )
                {
                    eprintln!("molt init_subclass builtin missing");
                }
            }
        }
        let meta_bits = object_class_bits(obj_ptr);
        let meta_ptr = if meta_bits != 0 {
            obj_from_bits(meta_bits).as_ptr()
        } else {
            obj_from_bits(builtin_classes(_py).type_obj).as_ptr()
        };
        let meta_ptr = match meta_ptr {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TYPE => Some(ptr),
            _ => None,
        };
        if let Some(meta_ptr) = meta_ptr {
            if let Some(meta_bits) = class_attr_lookup_raw_mro(_py, meta_ptr, attr_bits) {
                if descriptor_is_data(_py, meta_bits) {
                    return descriptor_bind(_py, meta_bits, meta_ptr, Some(obj_ptr));
                }
            }
        }
        if let Some(class_bits) = class_attr_lookup(_py, obj_ptr, obj_ptr, None, attr_bits) {
            return Some(class_bits);
        }
        if let Some(meta_ptr) = meta_ptr {
            if let Some(meta_bits) = class_attr_lookup_raw_mro(_py, meta_ptr, attr_bits) {
                return descriptor_bind(_py, meta_bits, meta_ptr, Some(obj_ptr));
            }
        }
        None
    }
}

#[unsafe(no_mangle)]
pub(crate) unsafe fn attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        profile_hit(_py, &ATTR_LOOKUP_COUNT);
        let type_id = object_type_id(obj_ptr);
        if type_id == TYPE_ID_MODULE {
            return module_attr_lookup(_py, obj_ptr, attr_bits);
        }
        if type_id == TYPE_ID_BIGINT {
            let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
                return None;
            };
            if let Some(func_bits) = int_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                return Some(molt_bound_method_new(func_bits, self_bits));
            }
        }
        if type_id == TYPE_ID_BOUND_METHOD {
            let name = string_obj_to_owned(obj_from_bits(attr_bits));
            if let Some(name) = name.as_deref() {
                match name {
                    "__func__" => {
                        let func_bits = bound_method_func_bits(obj_ptr);
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                    "__self__" => {
                        let self_bits = bound_method_self_bits(obj_ptr);
                        inc_ref_bits(_py, self_bits);
                        return Some(self_bits);
                    }
                    "__name__" | "__qualname__" | "__doc__" => {
                        let func_bits = bound_method_func_bits(obj_ptr);
                        if let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() {
                            if object_type_id(func_ptr) == TYPE_ID_FUNCTION {
                                if let Some(bits) = function_attr_bits(_py, func_ptr, attr_bits) {
                                    inc_ref_bits(_py, bits);
                                    return Some(bits);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if type_id == TYPE_ID_PROPERTY {
            let name = string_obj_to_owned(obj_from_bits(attr_bits));
            let attr_name = name.as_deref()?;
            match attr_name {
                "fget" => {
                    let bits = property_get_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "fset" => {
                    let bits = property_set_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "fdel" => {
                    let bits = property_del_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "getter" | "setter" | "deleter" => {
                    if let Some(func_bits) = property_method_bits(_py, attr_name) {
                        let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                        return Some(molt_bound_method_new(func_bits, self_bits));
                    }
                }
                "__doc__" => {
                    let bits = property_doc_bits(_py, obj_ptr);
                    return Some(bits);
                }
                _ => {}
            }
        }
        if type_id == TYPE_ID_EXCEPTION {
            let name = string_obj_to_owned(obj_from_bits(attr_bits));
            let attr_name = name.as_deref()?;
            match attr_name {
                "name" | "obj" => {
                    let kind_bits = exception_kind_bits(obj_ptr);
                    if let Some(kind_ptr) = obj_from_bits(kind_bits).as_ptr() {
                        if object_type_id(kind_ptr) == TYPE_ID_STRING {
                            let kind_len = string_len(kind_ptr);
                            let kind_bytes =
                                std::slice::from_raw_parts(string_bytes(kind_ptr), kind_len);
                            if kind_bytes == b"AttributeError" {
                                let members_bits = exception_value_bits(obj_ptr);
                                if obj_from_bits(members_bits).is_none() || members_bits == 0 {
                                    return Some(MoltObject::none().bits());
                                }
                                if let Some(members_ptr) = obj_from_bits(members_bits).as_ptr() {
                                    if object_type_id(members_ptr) == TYPE_ID_TUPLE {
                                        let elems = seq_vec_ref(members_ptr);
                                        let bits = if attr_name == "name" {
                                            elems.get(0).copied().unwrap_or_else(|| MoltObject::none().bits())
                                        } else {
                                            elems.get(1).copied().unwrap_or_else(|| MoltObject::none().bits())
                                        };
                                        inc_ref_bits(_py, bits);
                                        return Some(bits);
                                    }
                                }
                                return Some(MoltObject::none().bits());
                            }
                        }
                    }
                }
                "__cause__" => {
                    let bits = exception_cause_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__context__" => {
                    let bits = exception_context_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__suppress_context__" => {
                    let bits = exception_suppress_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__traceback__" => {
                    let bits = exception_trace_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "__class__" => {
                    let mut class_bits = exception_class_bits(obj_ptr);
                    if obj_from_bits(class_bits).is_none() || class_bits == 0 {
                        let new_bits = exception_type_bits(_py, exception_kind_bits(obj_ptr));
                        let slot = obj_ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != new_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, new_bits);
                            *slot = new_bits;
                        }
                        class_bits = new_bits;
                    }
                    inc_ref_bits(_py, class_bits);
                    return Some(class_bits);
                }
                "__dict__" => {
                    let mut dict_bits = exception_dict_bits(obj_ptr);
                    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                        if dict_ptr.is_null() {
                            return None;
                        }
                        let new_bits = MoltObject::from_ptr(dict_ptr).bits();
                        let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != new_bits {
                            dec_ref_bits(_py, old_bits);
                            *slot = new_bits;
                        }
                        dict_bits = new_bits;
                    }
                    inc_ref_bits(_py, dict_bits);
                    return Some(dict_bits);
                }
                "args" => {
                    let mut args_bits = exception_args_bits(obj_ptr);
                    if obj_from_bits(args_bits).is_none() || args_bits == 0 {
                        let ptr = alloc_tuple(_py, &[]);
                        if ptr.is_null() {
                            return None;
                        }
                        let new_bits = MoltObject::from_ptr(ptr).bits();
                        let slot = obj_ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != new_bits {
                            dec_ref_bits(_py, old_bits);
                            *slot = new_bits;
                        }
                        args_bits = new_bits;
                    }
                    inc_ref_bits(_py, args_bits);
                    return Some(args_bits);
                }
                "msg" => {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)));
                    if matches!(
                        kind.as_deref(),
                        Some("SyntaxError" | "IndentationError" | "TabError")
                    ) {
                        let bits = exception_msg_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                }
                "value" => {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)));
                    if kind.as_deref() == Some("StopIteration") {
                        let bits = exception_value_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                }
                "code" => {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)));
                    if kind.as_deref() == Some("SystemExit") {
                        let bits = exception_value_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                }
                _ => {}
            }
            let dict_bits = exception_dict_bits(obj_ptr);
            if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                            inc_ref_bits(_py, bits);
                            return Some(bits);
                        }
                    }
                }
            }
            let mut class_bits = exception_class_bits(obj_ptr);
            if obj_from_bits(class_bits).is_none() || class_bits == 0 {
                class_bits = exception_type_bits(_py, exception_kind_bits(obj_ptr));
            }
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(val_bits) =
                        class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), attr_bits)
                    {
                        return Some(val_bits);
                    }
                    if exception_pending(_py) {
                        return None;
                    }
                }
            }
        }
        if type_id == TYPE_ID_GENERATOR {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                match name.as_str() {
                    "gi_running" => {
                        return Some(MoltObject::from_bool(generator_running(obj_ptr)).bits());
                    }
                    "gi_code" => {
                        let header = header_from_obj_ptr(obj_ptr);
                        let code_bits = fn_ptr_code_get(_py, (*header).poll_fn);
                        if code_bits != 0 {
                            inc_ref_bits(_py, code_bits);
                            return Some(code_bits);
                        }
                        return Some(MoltObject::none().bits());
                    }
                    "gi_frame" => {
                        if generator_closed(obj_ptr) {
                            return Some(MoltObject::none().bits());
                        }
                        let lasti = if generator_started(obj_ptr) { 0 } else { -1 };
                        let frame_bits = molt_object_new();
                        let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                            return Some(MoltObject::none().bits());
                        };
                        let f_code_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.f_code_name,
                            b"f_code",
                        );
                        let f_lasti_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.f_lasti_name,
                            b"f_lasti",
                        );
                        let f_locals_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.f_locals_name,
                            b"f_locals",
                        );
                        let val_bits = MoltObject::from_int(lasti).bits();
                        let header = header_from_obj_ptr(obj_ptr);
                        let mut code_bits = fn_ptr_code_get(_py, (*header).poll_fn);
                        if code_bits == 0 {
                            code_bits = MoltObject::none().bits();
                        } else {
                            inc_ref_bits(_py, code_bits);
                        }
                        let locals_bits = generator_locals_dict(_py, obj_ptr);
                        let dict_ptr = alloc_dict_with_pairs(
                            _py,
                            &[
                                f_code_bits,
                                code_bits,
                                f_lasti_bits,
                                val_bits,
                                f_locals_bits,
                                locals_bits,
                            ],
                        );
                        if !dict_ptr.is_null() {
                            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                            instance_set_dict_bits(_py, frame_ptr, dict_bits);
                            object_mark_has_ptrs(_py, frame_ptr);
                        }
                        return Some(frame_bits);
                    }
                    "gi_yieldfrom" => {
                        if generator_closed(obj_ptr) {
                            return Some(MoltObject::none().bits());
                        }
                        let bits = generator_yieldfrom_bits(obj_ptr);
                        if !obj_from_bits(bits).is_none() {
                            inc_ref_bits(_py, bits);
                        }
                        return Some(bits);
                    }
                    _ => {}
                }
                if let Some(func_bits) = generator_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0
            && type_id != TYPE_ID_OBJECT
            && type_id != TYPE_ID_DATACLASS
            && type_id != TYPE_ID_EXCEPTION
            && type_id != TYPE_ID_FUNCTION
        {
            if let Some(val_bits) =
                classed_attr_lookup_without_dict(_py, obj_ptr, class_bits, attr_bits)
            {
                return Some(val_bits);
            }
        }
        if type_id == TYPE_ID_ASYNC_GENERATOR {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                match name.as_str() {
                    "ag_running" => {
                        let gen_bits = asyncgen_gen_bits(obj_ptr);
                        let gen_running = if let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) {
                            object_type_id(gen_ptr) == TYPE_ID_GENERATOR
                                && generator_running(gen_ptr)
                        } else {
                            false
                        };
                        let running = asyncgen_running(obj_ptr) || gen_running;
                        return Some(MoltObject::from_bool(running).bits());
                    }
                    "ag_await" => {
                        let await_bits = asyncgen_await_bits(_py, obj_ptr);
                        return Some(await_bits);
                    }
                    "ag_code" => {
                        let code_bits = asyncgen_code_bits(_py, obj_ptr);
                        return Some(code_bits);
                    }
                    "ag_frame" => {
                        let gen_bits = asyncgen_gen_bits(obj_ptr);
                        let Some(gen_ptr) = maybe_ptr_from_bits(gen_bits) else {
                            return Some(MoltObject::none().bits());
                        };
                        if object_type_id(gen_ptr) != TYPE_ID_GENERATOR {
                            return Some(MoltObject::none().bits());
                        }
                        if generator_closed(gen_ptr) {
                            return Some(MoltObject::none().bits());
                        }
                        let lasti = if generator_started(gen_ptr) { 0 } else { -1 };
                        let frame_bits = molt_object_new();
                        let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                            return Some(MoltObject::none().bits());
                        };
                        let name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.f_lasti_name,
                            b"f_lasti",
                        );
                        let val_bits = MoltObject::from_int(lasti).bits();
                        let dict_ptr = alloc_dict_with_pairs(_py, &[name_bits, val_bits]);
                        if !dict_ptr.is_null() {
                            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                            instance_set_dict_bits(_py, frame_ptr, dict_bits);
                            object_mark_has_ptrs(_py, frame_ptr);
                        }
                        return Some(frame_bits);
                    }
                    _ => {}
                }
                if let Some(func_bits) = asyncgen_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_MEMORYVIEW {
            let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
            match name.as_str() {
                "format" => {
                    let bits = memoryview_format_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "itemsize" => {
                    return Some(MoltObject::from_int(memoryview_itemsize(obj_ptr) as i64).bits());
                }
                "ndim" => {
                    return Some(MoltObject::from_int(memoryview_ndim(obj_ptr) as i64).bits());
                }
                "shape" => {
                    let shape = memoryview_shape(obj_ptr).unwrap_or(&[]);
                    return Some(tuple_from_isize_slice(_py, shape));
                }
                "strides" => {
                    let strides = memoryview_strides(obj_ptr).unwrap_or(&[]);
                    return Some(tuple_from_isize_slice(_py, strides));
                }
                "readonly" => {
                    return Some(MoltObject::from_bool(memoryview_readonly(obj_ptr)).bits());
                }
                "nbytes" => {
                    return Some(MoltObject::from_int(memoryview_nbytes(obj_ptr) as i64).bits());
                }
                _ => {}
            }
            if let Some(func_bits) = memoryview_method_bits(_py, name.as_str()) {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let bound_bits = molt_bound_method_new(func_bits, self_bits);
                return Some(bound_bits);
            }
        }
        if type_id == TYPE_ID_SLICE {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                match name.as_str() {
                    "start" => {
                        let bits = slice_start_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    "stop" => {
                        let bits = slice_stop_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    "step" => {
                        let bits = slice_step_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    _ => {}
                }
                if let Some(func_bits) = slice_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_GENERIC_ALIAS {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                match name.as_str() {
                    "__origin__" => {
                        let bits = generic_alias_origin_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    "__args__" => {
                        let bits = generic_alias_args_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    "__parameters__" => {
                        let args_bits = generic_alias_args_bits(obj_ptr);
                        let mut params: Vec<u64> = Vec::new();
                        if let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() {
                            if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                                for &arg_bits in seq_vec_ref(args_ptr).iter() {
                                    if !is_typing_param(_py, arg_bits) {
                                        continue;
                                    }
                                    if params.contains(&arg_bits) {
                                        continue;
                                    }
                                    params.push(arg_bits);
                                }
                            }
                        }
                        let tuple_ptr = alloc_tuple(_py, &params);
                        if tuple_ptr.is_null() {
                            return Some(MoltObject::none().bits());
                        }
                        return Some(MoltObject::from_ptr(tuple_ptr).bits());
                    }
                    "__unpacked__" => {
                        return Some(MoltObject::from_bool(false).bits());
                    }
                    _ => {}
                }
            }
        }
        if type_id == TYPE_ID_UNION {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                match name.as_str() {
                    "__origin__" => {
                        let bits = builtin_classes(_py).union_type;
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    "__args__" => {
                        let bits = union_type_args_bits(obj_ptr);
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    _ => {}
                }
            }
        }
        if type_id == TYPE_ID_FILE_HANDLE {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                let handle_ptr = file_handle_ptr(obj_ptr);
                if handle_ptr.is_null() {
                    return None;
                }
                let handle = &*handle_ptr;
                match name.as_str() {
                    "__class__" => {
                        let class_bits = if handle.class_bits != 0 {
                            handle.class_bits
                        } else {
                            builtin_classes(_py).file
                        };
                        inc_ref_bits(_py, class_bits);
                        return Some(class_bits);
                    }
                    "closed" => {
                        if handle.detached {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                file_handle_detached_message(handle),
                            );
                        }
                        return Some(MoltObject::from_bool(file_handle_is_closed(handle)).bits());
                    }
                    "name" => {
                        if handle.detached {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                file_handle_detached_message(handle),
                            );
                        }
                        if handle.name_bits != 0 {
                            inc_ref_bits(_py, handle.name_bits);
                            return Some(handle.name_bits);
                        }
                        return Some(MoltObject::none().bits());
                    }
                    "mode" => {
                        if handle.detached && !handle.text {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                file_handle_detached_message(handle),
                            );
                        }
                        let ptr = alloc_string(_py, handle.mode.as_bytes());
                        if ptr.is_null() {
                            return Some(MoltObject::none().bits());
                        }
                        return Some(MoltObject::from_ptr(ptr).bits());
                    }
                    "encoding" => {
                        if !handle.text {
                            return None;
                        }
                        let Some(encoding) = handle.encoding.as_deref() else {
                            return Some(MoltObject::none().bits());
                        };
                        let ptr = alloc_string(_py, encoding.as_bytes());
                        if ptr.is_null() {
                            return Some(MoltObject::none().bits());
                        }
                        return Some(MoltObject::from_ptr(ptr).bits());
                    }
                    "errors" => {
                        if !handle.text {
                            return None;
                        }
                        let Some(errors) = handle.errors.as_deref() else {
                            return Some(MoltObject::none().bits());
                        };
                        let ptr = alloc_string(_py, errors.as_bytes());
                        if ptr.is_null() {
                            return Some(MoltObject::none().bits());
                        }
                        return Some(MoltObject::from_ptr(ptr).bits());
                    }
                    "newline" => {
                        if !handle.text {
                            return None;
                        }
                        let Some(newline) = handle.newline.as_deref() else {
                            return Some(MoltObject::none().bits());
                        };
                        let ptr = alloc_string(_py, newline.as_bytes());
                        if ptr.is_null() {
                            return Some(MoltObject::none().bits());
                        }
                        return Some(MoltObject::from_ptr(ptr).bits());
                    }
                    "newlines" => {
                        if !handle.text {
                            return None;
                        }
                        if handle.newlines_len == 0 {
                            return Some(MoltObject::none().bits());
                        }
                        let mut out_bits: Vec<u64> = Vec::new();
                        for idx in 0..handle.newlines_len {
                            let kind = handle.newlines_seen[idx as usize];
                            let text = match kind {
                                NEWLINE_KIND_LF => "\n",
                                NEWLINE_KIND_CR => "\r",
                                NEWLINE_KIND_CRLF => "\r\n",
                                _ => "\n",
                            };
                            let ptr = alloc_string(_py, text.as_bytes());
                            if ptr.is_null() {
                                for bits in out_bits {
                                    dec_ref_bits(_py, bits);
                                }
                                return Some(MoltObject::none().bits());
                            }
                            out_bits.push(MoltObject::from_ptr(ptr).bits());
                        }
                        if out_bits.len() == 1 {
                            return Some(out_bits[0]);
                        }
                        let tuple_ptr = alloc_tuple(_py, out_bits.as_slice());
                        if tuple_ptr.is_null() {
                            for bits in out_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return Some(MoltObject::none().bits());
                        }
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return Some(MoltObject::from_ptr(tuple_ptr).bits());
                    }
                    "line_buffering" => {
                        return Some(MoltObject::from_bool(handle.line_buffering).bits());
                    }
                    "write_through" => {
                        if !handle.text {
                            return None;
                        }
                        return Some(MoltObject::from_bool(handle.write_through).bits());
                    }
                    "buffer" => {
                        if !handle.text {
                            return None;
                        }
                        if handle.detached {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                file_handle_detached_message(handle),
                            );
                        }
                        let buffer_bits = handle.buffer_bits;
                        if buffer_bits == 0 || buffer_bits == MoltObject::none().bits() {
                            return Some(MoltObject::none().bits());
                        }
                        inc_ref_bits(_py, buffer_bits);
                        return Some(buffer_bits);
                    }
                    "closefd" => {
                        let builtins = builtin_classes(_py);
                        if handle.class_bits != builtins.file_io {
                            return None;
                        }
                        if handle.detached {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                file_handle_detached_message(handle),
                            );
                        }
                        return Some(MoltObject::from_bool(handle.closefd).bits());
                    }
                    _ => {}
                }
                if handle.text && (name == "readinto" || name == "readinto1") {
                    return None;
                }
                if !handle.text && name == "reconfigure" {
                    return None;
                }
                if let Some(func_bits) = file_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_DICT {
            let class_bits = object_class_bits(obj_ptr);
            let builtins = builtin_classes(_py);
            if class_bits != 0 && class_bits != builtins.dict {
                return unsafe { object_attr_lookup_raw(_py, obj_ptr, attr_bits) };
            }
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "fromkeys" {
                    if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                        let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                        let class_bits = type_of_bits(_py, self_bits);
                        let bound_bits = molt_bound_method_new(func_bits, class_bits);
                        return Some(bound_bits);
                    }
                }
                if let Some(func_bits) = dict_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_SET {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if let Some(func_bits) = set_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_FROZENSET {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if let Some(func_bits) = frozenset_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_LIST {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if let Some(func_bits) = list_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_STRING {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if let Some(func_bits) = string_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_BYTES {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "fromhex" {
                    static BYTES_FROMHEX: AtomicU64 = AtomicU64::new(0);
                    let builtins = builtin_classes(_py);
                    let func_bits =
                        builtin_func_bits(_py, &BYTES_FROMHEX, fn_addr!(molt_bytes_fromhex), 2);
                    let bound = molt_bound_method_new(func_bits, builtins.bytes);
                    return Some(bound);
                }
                if name == "maketrans" {
                    if let Some(func_bits) = bytes_method_bits(_py, name.as_str()) {
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                }
                if let Some(func_bits) = bytes_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_BYTEARRAY {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "fromhex" {
                    static BYTEARRAY_FROMHEX: AtomicU64 = AtomicU64::new(0);
                    let builtins = builtin_classes(_py);
                    let func_bits = builtin_func_bits(
                        _py,
                        &BYTEARRAY_FROMHEX,
                        fn_addr!(molt_bytearray_fromhex),
                        2,
                    );
                    let bound = molt_bound_method_new(func_bits, builtins.bytearray);
                    return Some(bound);
                }
                if name == "maketrans" {
                    if let Some(func_bits) = bytearray_method_bits(_py, name.as_str()) {
                        inc_ref_bits(_py, func_bits);
                        return Some(func_bits);
                    }
                }
                if let Some(func_bits) = bytearray_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_COMPLEX {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "real" {
                    let value = unsafe { *complex_ref(obj_ptr) };
                    return Some(MoltObject::from_float(value.re).bits());
                }
                if name == "imag" {
                    let value = unsafe { *complex_ref(obj_ptr) };
                    return Some(MoltObject::from_float(value.im).bits());
                }
                if let Some(func_bits) = complex_method_bits(_py, name.as_str()) {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_bits = molt_bound_method_new(func_bits, self_bits);
                    return Some(bound_bits);
                }
            }
        }
        if type_id == TYPE_ID_TYPE {
            return type_attr_lookup_ptr(_py, obj_ptr, attr_bits);
        }
        if type_id == TYPE_ID_SUPER {
            let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
            let start_bits = super_type_bits(obj_ptr);
            let target_bits = super_obj_bits(obj_ptr);
            let target_ptr = maybe_ptr_from_bits(target_bits);
            let obj_type_bits = if let Some(raw_ptr) = target_ptr {
                if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                    if issubclass_bits(target_bits, start_bits) {
                        target_bits
                    } else {
                        type_of_bits(_py, target_bits)
                    }
                } else {
                    type_of_bits(_py, target_bits)
                }
            } else {
                type_of_bits(_py, target_bits)
            };
            let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
            if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
                return None;
            }
            let mro_storage: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(obj_type_ptr) {
                Cow::Borrowed(mro.as_slice())
            } else {
                Cow::Owned(class_mro_vec(obj_type_bits))
            };
            let mut instance_ptr = None;
            let mut owner_ptr = obj_type_ptr;
            if let Some(raw_ptr) = target_ptr {
                if object_type_id(raw_ptr) == TYPE_ID_TYPE {
                    owner_ptr = raw_ptr;
                    instance_ptr = Some(raw_ptr);
                } else {
                    instance_ptr = Some(raw_ptr);
                }
            }
            let mut found_start = false;
            for class_bits in mro_storage.iter() {
                if !found_start {
                    if *class_bits == start_bits {
                        found_start = true;
                    }
                    continue;
                }
                let class_obj = obj_from_bits(*class_bits);
                let Some(class_ptr) = class_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    continue;
                }
                let dict_bits = class_dict_bits(class_ptr);
                let dict_obj = obj_from_bits(dict_bits);
                let Some(dict_ptr) = dict_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                    if attr_name.as_deref() == Some("__new__") {
                        if let Some(val_ptr) = obj_from_bits(val_bits).as_ptr() {
                            if object_type_id(val_ptr) == TYPE_ID_FUNCTION {
                                inc_ref_bits(_py, val_bits);
                                return Some(val_bits);
                            }
                        }
                    }
                    return descriptor_bind(_py, val_bits, owner_ptr, instance_ptr);
                }
                if let Some(name) = attr_name.as_deref() {
                    if is_builtin_class_bits(_py, *class_bits) {
                        if let Some(func_bits) = builtin_class_method_bits(_py, *class_bits, name) {
                            if name == "__new__" {
                                inc_ref_bits(_py, func_bits);
                                return Some(func_bits);
                            }
                            return descriptor_bind(_py, func_bits, owner_ptr, instance_ptr);
                        }
                    }
                }
            }
            return None;
        }
        if type_id == TYPE_ID_FUNCTION {
            if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                if name == "__code__" {
                    // CPython parity: builtin_function_or_method objects do not expose __code__.
                    let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                    if object_class_bits(obj_ptr) == builtin_bits {
                        return None;
                    }
                    let code_bits = ensure_function_code_bits(_py, obj_ptr);
                    if !obj_from_bits(code_bits).is_none() {
                        inc_ref_bits(_py, code_bits);
                        return Some(code_bits);
                    }
                    return None;
                }
                if name == "__text_signature__" {
                    // CPython parity: builtin_function_or_method objects expose a read-only
                    // `__text_signature__` string used by `inspect.signature`.
                    let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                    if object_class_bits(obj_ptr) == builtin_bits {
                        let fn_ptr = function_fn_ptr(obj_ptr);
                        let text_sig = match fn_ptr {
                            v if v == fn_addr!(molt_abs_builtin) => Some("(x, /)"),
                            v if v == fn_addr!(molt_aiter) => Some("(async_iterable, /)"),
                            v if v == fn_addr!(molt_all_builtin) => Some("(iterable, /)"),
                            v if v == fn_addr!(molt_any_builtin) => Some("(iterable, /)"),
                            v if v == fn_addr!(molt_ascii_from_obj) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_bin_builtin) => Some("(number, /)"),
                            v if v == fn_addr!(molt_callable_builtin) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_chr) => Some("(i, /)"),
                            v if v == fn_addr!(molt_del_attr_name) => Some("(obj, name, /)"),
                            v if v == fn_addr!(molt_divmod_builtin) => Some("(x, y, /)"),
                            v if v == fn_addr!(molt_format_builtin) => {
                                Some("(value, format_spec='', /)")
                            }
                            v if v == fn_addr!(molt_has_attr_name) => Some("(obj, name, /)"),
                            v if v == fn_addr!(molt_hash_builtin) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_hex_builtin) => Some("(number, /)"),
                            v if v == fn_addr!(molt_id) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_isinstance) => Some("(obj, class_or_tuple, /)"),
                            v if v == fn_addr!(molt_issubclass) => Some("(cls, class_or_tuple, /)"),
                            v if v == fn_addr!(molt_len) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_open_builtin) => Some(
                                "(file, mode='r', buffering=-1, encoding=None, errors=None, newline=None, closefd=True, opener=None)",
                            ),
                            v if v == fn_addr!(molt_oct_builtin) => Some("(number, /)"),
                            v if v == fn_addr!(molt_ord) => Some("(c, /)"),
                            v if v == fn_addr!(molt_pow) => Some("(base, exp, mod=None)"),
                            v if v == fn_addr!(molt_print_builtin) => {
                                Some("(*args, sep=' ', end='\\n', file=None, flush=False)")
                            }
                            v if v == fn_addr!(molt_repr_builtin) => Some("(obj, /)"),
                            v if v == fn_addr!(molt_round_builtin) => {
                                Some("(number, ndigits=None)")
                            }
                            v if v == fn_addr!(molt_set_attr_name) => Some("(obj, name, value, /)"),
                            v if v == fn_addr!(molt_sorted_builtin) => {
                                Some("(iterable, /, *, key=None, reverse=False)")
                            }
                            v if v == fn_addr!(molt_sum_builtin) => Some("(iterable, /, start=0)"),
                            _ => None,
                        };
                        if let Some(text_sig) = text_sig {
                            let ptr = alloc_string(_py, text_sig.as_bytes());
                            if ptr.is_null() {
                                return None;
                            }
                            return Some(MoltObject::from_ptr(ptr).bits());
                        }
                    }
                }
                if name == "__closure__" {
                    let closure_bits = function_closure_bits(obj_ptr);
                    if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
                        inc_ref_bits(_py, closure_bits);
                        return Some(closure_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                if name == "__module__" {
                    // `__module__` is writable on CPython builtin_function_or_method objects.
                    // Ensure attribute reads consult the per-function dict rather than falling
                    // back to the type's own `__module__` (which is always "builtins").
                    let dict_bits = function_dict_bits(obj_ptr);
                    if dict_bits != 0 {
                        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                                if let Some(module_key_bits) =
                                    attr_name_bits_from_bytes(_py, b"__module__")
                                {
                                    let value = unsafe {
                                        dict_get_in_place(_py, dict_ptr, module_key_bits)
                                    };
                                    dec_ref_bits(_py, module_key_bits);
                                    if let Some(bits) = value {
                                        inc_ref_bits(_py, bits);
                                        return Some(bits);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let annotate_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotate_name,
                b"__annotate__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(annotate_name_bits),
            ) && pep649_enabled()
            {
                let mut annotate_bits = function_annotate_bits(obj_ptr);
                if annotate_bits == 0 {
                    annotate_bits = MoltObject::none().bits();
                }
                inc_ref_bits(_py, annotate_bits);
                return Some(annotate_bits);
            }
            let annotations_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotations_name,
                b"__annotations__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(annotations_bits),
            ) {
                let cached = function_annotations_bits(obj_ptr);
                if cached != 0 {
                    inc_ref_bits(_py, cached);
                    return Some(cached);
                }
                let annotate_bits = function_annotate_bits(obj_ptr);
                let res_bits = if pep649_enabled()
                    && annotate_bits != 0
                    && !obj_from_bits(annotate_bits).is_none()
                {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(_py, annotate_bits, format_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                };
                function_set_annotations_bits(_py, obj_ptr, res_bits);
                return Some(res_bits);
            }
            let dict_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
                let mut dict_bits = function_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    function_set_dict_bits(obj_ptr, dict_bits);
                }
                inc_ref_bits(_py, dict_bits);
                return Some(dict_bits);
            }
            let dict_bits = function_dict_bits(obj_ptr);
            if dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                            inc_ref_bits(_py, val);
                            return Some(val);
                        }
                    }
                }
            }
            // Fall through to the function type for descriptor-backed attributes
            // such as function.__get__ and function.__repr__.
            let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if let Some(bound) =
                                descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                            {
                                return Some(bound);
                            }
                            if exception_pending(_py) {
                                return None;
                            }
                        }
                    }
                }
            }
            return None;
        }
        if type_id == TYPE_ID_CODE {
            // Keep basic CPython-compatible code metadata available for inspect/types consumers.
            let name = string_obj_to_owned(obj_from_bits(attr_bits))?;
            match name.as_str() {
                "co_filename" => {
                    let bits = code_filename_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "co_name" => {
                    let bits = code_name_bits(obj_ptr);
                    inc_ref_bits(_py, bits);
                    return Some(bits);
                }
                "co_firstlineno" => {
                    return Some(MoltObject::from_int(code_firstlineno(obj_ptr)).bits());
                }
                "co_linetable" => {
                    let bits = code_linetable_bits(obj_ptr);
                    if bits != 0 {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                "co_varnames" => {
                    let bits = code_varnames_bits(obj_ptr);
                    if bits != 0 {
                        inc_ref_bits(_py, bits);
                        return Some(bits);
                    }
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "co_freevars" | "co_cellvars" => {
                    let tuple_ptr = alloc_tuple(_py, &[]);
                    if tuple_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(tuple_ptr).bits());
                }
                "co_argcount" => {
                    return Some(MoltObject::from_int(code_argcount(obj_ptr) as i64).bits());
                }
                "co_posonlyargcount" => {
                    return Some(MoltObject::from_int(code_posonlyargcount(obj_ptr) as i64).bits());
                }
                "co_kwonlyargcount" => {
                    return Some(MoltObject::from_int(code_kwonlyargcount(obj_ptr) as i64).bits());
                }
                "co_nlocals" => {
                    let bits = code_varnames_bits(obj_ptr);
                    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                        if object_type_id(ptr) == TYPE_ID_TUPLE {
                            return Some(MoltObject::from_int(tuple_len(ptr) as i64).bits());
                        }
                    }
                    return Some(MoltObject::from_int(0).bits());
                }
                "co_flags" => {
                    // CPython parity:
                    // - module/compile() code objects report flags=0
                    // - function code objects report CO_OPTIMIZED | CO_NEWLOCALS
                    let name_bits = code_name_bits(obj_ptr);
                    if string_obj_to_owned(obj_from_bits(name_bits))
                        .is_some_and(|value| value == "<module>")
                    {
                        return Some(MoltObject::from_int(0).bits());
                    }
                    return Some(MoltObject::from_int(0x01 | 0x02).bits());
                }
                "co_consts" => {
                    let name_bits = code_name_bits(obj_ptr);
                    let is_module = string_obj_to_owned(obj_from_bits(name_bits))
                        .is_some_and(|value| value == "<module>");
                    let elems: [u64; 2] =
                        [MoltObject::none().bits(), MoltObject::from_int(0).bits()];
                    let ptr = if is_module {
                        alloc_tuple(_py, &elems)
                    } else {
                        alloc_tuple(_py, &[])
                    };
                    if ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(ptr).bits());
                }
                "co_positions" => {
                    let func_ptr = alloc_function_obj(
                        _py,
                        crate::molt_code_positions as *const () as usize as u64,
                        1,
                    );
                    if func_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    let func_bits = MoltObject::from_ptr(func_ptr).bits();
                    let _ = crate::molt_function_set_builtin(func_bits);
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let bound_ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
                    dec_ref_bits(_py, func_bits);
                    if bound_ptr.is_null() {
                        return Some(MoltObject::none().bits());
                    }
                    return Some(MoltObject::from_ptr(bound_ptr).bits());
                }
                _ => {}
            }
            return None;
        }
        if type_id == TYPE_ID_DATACLASS {
            let desc_ptr = dataclass_desc_ptr(obj_ptr);
            if !desc_ptr.is_null() {
                let slots = (*desc_ptr).slots;
                let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
                let class_bits = (*desc_ptr).class_bits;
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            let getattribute_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.getattribute_name,
                                b"__getattribute__",
                            );
                            if !obj_eq(
                                _py,
                                obj_from_bits(attr_bits),
                                obj_from_bits(getattribute_bits),
                            ) {
                                if let Some(call_bits) = class_attr_lookup(
                                    _py,
                                    class_ptr,
                                    class_ptr,
                                    Some(obj_ptr),
                                    getattribute_bits,
                                ) {
                                    exception_stack_push();
                                    let res_bits = call_callable1(_py, call_bits, attr_bits);
                                    if exception_pending(_py) {
                                        let exc_bits = molt_exception_last();
                                        let kind_bits = molt_exception_kind(exc_bits);
                                        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                        dec_ref_bits(_py, kind_bits);
                                        if kind.as_deref() == Some("AttributeError") {
                                            let getattr_bits = intern_static_name(
                                                _py,
                                                &runtime_state(_py).interned.getattr_name,
                                                b"__getattr__",
                                            );
                                            if !obj_eq(
                                                _py,
                                                obj_from_bits(attr_bits),
                                                obj_from_bits(getattr_bits),
                                            ) && class_attr_lookup_raw_mro(
                                                _py,
                                                class_ptr,
                                                getattr_bits,
                                            )
                                            .is_some()
                                            {
                                                molt_exception_clear();
                                                dec_ref_bits(_py, exc_bits);
                                                exception_stack_pop(_py);
                                                if let Some(getattr_call_bits) = class_attr_lookup(
                                                    _py,
                                                    class_ptr,
                                                    class_ptr,
                                                    Some(obj_ptr),
                                                    getattr_bits,
                                                ) {
                                                    let getattr_res = call_callable1(
                                                        _py,
                                                        getattr_call_bits,
                                                        attr_bits,
                                                    );
                                                    if exception_pending(_py) {
                                                        return None;
                                                    }
                                                    return Some(getattr_res);
                                                }
                                            }
                                        }
                                        dec_ref_bits(_py, exc_bits);
                                        exception_stack_pop(_py);
                                        return None;
                                    }
                                    exception_stack_pop(_py);
                                    return Some(res_bits);
                                }
                            }
                            if let Some(val_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                if descriptor_is_data(_py, val_bits) {
                                    if let Some(bound) =
                                        descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                                    {
                                        return Some(bound);
                                    }
                                    if exception_pending(_py) {
                                        return None;
                                    }
                                }
                            }
                        }
                    }
                }
                let class_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
                if obj_eq(
                    _py,
                    obj_from_bits(attr_bits),
                    obj_from_bits(class_name_bits),
                ) {
                    if class_bits != 0 {
                        inc_ref_bits(_py, class_bits);
                        return Some(class_bits);
                    }
                    return None;
                }
                let dict_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
                if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
                    if !slots {
                        let mut dict_bits = dataclass_dict_bits(obj_ptr);
                        if dict_bits == 0 {
                            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                            if !dict_ptr.is_null() {
                                dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                                dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
                            }
                        }
                        if dict_bits != 0 {
                            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                                    let fields = dataclass_fields_ref(obj_ptr);
                                    let names = &(*desc_ptr).field_names;
                                    let limit = std::cmp::min(fields.len(), names.len());
                                    for idx in 0..limit {
                                        let Some(key_bits) =
                                            attr_name_bits_from_bytes(_py, names[idx].as_bytes())
                                        else {
                                            continue;
                                        };
                                        if dict_get_in_place(_py, dict_ptr, key_bits).is_none() {
                                            let val_bits = fields[idx];
                                            if !is_missing_bits(_py, val_bits) {
                                                dict_set_in_place(
                                                    _py, dict_ptr, key_bits, val_bits,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            inc_ref_bits(_py, dict_bits);
                            return Some(dict_bits);
                        }
                    }
                    return None;
                }
                if !slots {
                    let dict_bits = dataclass_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                                inc_ref_bits(_py, val);
                                return Some(val);
                            }
                        }
                    }
                }
                if let Some(name) = attr_name {
                    let fields = dataclass_fields_ref(obj_ptr);
                    let names = &(*desc_ptr).field_names;
                    let limit = std::cmp::min(fields.len(), names.len());
                    for idx in 0..limit {
                        if names[idx] == name {
                            let val_bits = fields[idx];
                            if is_missing_bits(_py, val_bits) {
                                return None;
                            }
                            inc_ref_bits(_py, val_bits);
                            return Some(val_bits);
                        }
                    }
                }
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            if std::env::var_os("MOLT_DEBUG_BOUND_METHOD").is_some() {
                                let class_name_bits_val = crate::class_name_bits(class_ptr);
                                let class_name =
                                    string_obj_to_owned(obj_from_bits(class_name_bits_val))
                                        .unwrap_or_default();
                                if class_name == "ThreadPoolExecutor" {
                                    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))
                                        .unwrap_or_default();
                                    eprintln!("attr_lookup ThreadPoolExecutor attr={}", attr_name);
                                }
                            }
                            if let Some(val_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                if let Some(bound) =
                                    descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                                {
                                    return Some(bound);
                                }
                                if exception_pending(_py) {
                                    return None;
                                }
                            }
                        }
                    }
                }
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            let getattr_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.getattr_name,
                                b"__getattr__",
                            );
                            if !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                                if let Some(call_bits) = class_attr_lookup(
                                    _py,
                                    class_ptr,
                                    class_ptr,
                                    Some(obj_ptr),
                                    getattr_bits,
                                ) {
                                    let res_bits = call_callable1(_py, call_bits, attr_bits);
                                    if exception_pending(_py) {
                                        return None;
                                    }
                                    return Some(res_bits);
                                }
                            }
                        }
                    }
                }
            }
            return None;
        }
        if type_id == TYPE_ID_OBJECT {
            let header = header_from_obj_ptr(obj_ptr);
            if (*header).flags & HEADER_FLAG_COROUTINE != 0 {
                if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
                    match name.as_str() {
                        "cr_running" => {
                            let running = ((*header).flags & HEADER_FLAG_TASK_RUNNING) != 0;
                            return Some(MoltObject::from_bool(running).bits());
                        }
                        "cr_frame" => {
                            if (*header).poll_fn == 0
                                || ((*header).flags & HEADER_FLAG_TASK_DONE) != 0
                            {
                                return Some(MoltObject::none().bits());
                            }
                            let lasti = if (*header).state == 0 { -1 } else { 0 };
                            let frame_bits = molt_object_new();
                            let Some(frame_ptr) = maybe_ptr_from_bits(frame_bits) else {
                                return Some(MoltObject::none().bits());
                            };
                            let name_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.f_lasti_name,
                                b"f_lasti",
                            );
                            let val_bits = MoltObject::from_int(lasti).bits();
                            let dict_ptr = alloc_dict_with_pairs(_py, &[name_bits, val_bits]);
                            if !dict_ptr.is_null() {
                                let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                                instance_set_dict_bits(_py, frame_ptr, dict_bits);
                                object_mark_has_ptrs(_py, frame_ptr);
                            }
                            return Some(frame_bits);
                        }
                        "cr_code" => {
                            let code_bits = fn_ptr_code_get(_py, (*header).poll_fn);
                            if code_bits != 0 {
                                inc_ref_bits(_py, code_bits);
                                return Some(code_bits);
                            }
                            return Some(MoltObject::none().bits());
                        }
                        "cr_await" => {
                            let awaited = {
                                let guard = task_waiting_on(_py).lock().unwrap();
                                guard.get(&PtrSlot(obj_ptr)).copied()
                            };
                            if let Some(waiting_on) = awaited {
                                let bits = MoltObject::from_ptr(waiting_on.0).bits();
                                inc_ref_bits(_py, bits);
                                return Some(bits);
                            }
                            return Some(MoltObject::none().bits());
                        }
                        _ => {}
                    }
                    if let Some(func_bits) = coroutine_method_bits(_py, name.as_str()) {
                        let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                        let bound_bits = molt_bound_method_new(func_bits, self_bits);
                        return Some(bound_bits);
                    }
                }
            }
            let class_bits = object_class_bits(obj_ptr);
            let mut cached_attr_bits: Option<u64> = None;
            if class_bits == 0 {
                let await_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.await_name, b"__await__");
                if obj_eq(
                    _py,
                    obj_from_bits(attr_bits),
                    obj_from_bits(await_name_bits),
                ) && (*header_from_obj_ptr(obj_ptr)).poll_fn != 0
                {
                    let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                    let func_bits = awaitable_await_func_bits(_py);
                    return Some(molt_bound_method_new(func_bits, self_bits));
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattribute_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.getattribute_name,
                            b"__getattribute__",
                        );
                        let getattribute_raw =
                            class_attr_lookup_raw_mro(_py, class_ptr, getattribute_bits);
                        let default_getattribute_bits = object_method_bits(_py, "__getattribute__");
                        let use_custom_getattribute =
                            match (getattribute_raw, default_getattribute_bits) {
                                (Some(raw_bits), Some(default_bits)) => !obj_eq(
                                    _py,
                                    obj_from_bits(raw_bits),
                                    obj_from_bits(default_bits),
                                ),
                                (Some(_), None) => true,
                                (None, _) => false,
                            };
                        if use_custom_getattribute
                            && !obj_eq(
                                _py,
                                obj_from_bits(attr_bits),
                                obj_from_bits(getattribute_bits),
                            )
                        {
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                getattribute_bits,
                            ) {
                                let getattr_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.getattr_name,
                                    b"__getattr__",
                                );
                                let getattr_candidate =
                                    !obj_eq(
                                        _py,
                                        obj_from_bits(attr_bits),
                                        obj_from_bits(getattr_bits),
                                    ) && class_attr_lookup_raw_mro(_py, class_ptr, getattr_bits)
                                        .is_some();
                                if getattr_candidate {
                                    traceback_suppress_enter();
                                }
                                exception_stack_push();
                                let res_bits = call_callable1(_py, call_bits, attr_bits);
                                if getattr_candidate {
                                    traceback_suppress_exit();
                                }
                                if exception_pending(_py) {
                                    let exc_bits = molt_exception_last();
                                    let kind_bits = molt_exception_kind(exc_bits);
                                    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                    dec_ref_bits(_py, kind_bits);
                                    if kind.as_deref() == Some("AttributeError")
                                        && !obj_eq(
                                            _py,
                                            obj_from_bits(attr_bits),
                                            obj_from_bits(getattr_bits),
                                        )
                                        && class_attr_lookup_raw_mro(_py, class_ptr, getattr_bits)
                                            .is_some()
                                    {
                                        molt_exception_clear();
                                        dec_ref_bits(_py, exc_bits);
                                        exception_stack_pop(_py);
                                        if let Some(getattr_call_bits) = class_attr_lookup(
                                            _py,
                                            class_ptr,
                                            class_ptr,
                                            Some(obj_ptr),
                                            getattr_bits,
                                        ) {
                                            exception_stack_push();
                                            let getattr_res =
                                                call_callable1(_py, getattr_call_bits, attr_bits);
                                            if exception_pending(_py) {
                                                exception_stack_pop(_py);
                                                return None;
                                            }
                                            exception_stack_pop(_py);
                                            return Some(getattr_res);
                                        }
                                    }
                                    dec_ref_bits(_py, exc_bits);
                                    exception_stack_pop(_py);
                                    return None;
                                }
                                exception_stack_pop(_py);
                                return Some(res_bits);
                            }
                        }
                        let class_version = class_layout_version_bits(class_ptr);
                        if let Some(entry) =
                            descriptor_cache_lookup(class_bits, attr_bits, class_version)
                        {
                            if let Some(bits) = entry.data_desc_bits {
                                if let Some(bound) =
                                    descriptor_bind(_py, bits, class_ptr, Some(obj_ptr))
                                {
                                    return Some(bound);
                                }
                                if exception_pending(_py) {
                                    return None;
                                }
                            }
                            cached_attr_bits = entry.class_attr_bits;
                        }
                        if cached_attr_bits.is_none() {
                            if let Some(val_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                if descriptor_is_data(_py, val_bits) {
                                    descriptor_cache_store(
                                        class_bits,
                                        attr_bits,
                                        class_version,
                                        Some(val_bits),
                                        None,
                                    );
                                    if let Some(bound) =
                                        descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                                    {
                                        return Some(bound);
                                    }
                                    if exception_pending(_py) {
                                        return None;
                                    }
                                }
                                cached_attr_bits = Some(val_bits);
                                descriptor_cache_store(
                                    class_bits,
                                    attr_bits,
                                    class_version,
                                    None,
                                    Some(val_bits),
                                );
                            } else {
                                descriptor_cache_store(
                                    class_bits,
                                    attr_bits,
                                    class_version,
                                    None,
                                    None,
                                );
                            }
                        }
                        if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                            let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                            if !is_missing_bits(_py, bits) {
                                return Some(bits);
                            }
                            dec_ref_bits(_py, bits);
                        }
                    }
                }
            }
            let class_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(class_name_bits),
            ) {
                if class_bits != 0 {
                    inc_ref_bits(_py, class_bits);
                    return Some(class_bits);
                }
                return None;
            }
            let weakref_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.weakref_name,
                b"__weakref__",
            );
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(weakref_name_bits),
            ) {
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            if let Some(info) = class_slots_info(_py, class_ptr, attr_bits) {
                                if !info.allows_attr {
                                    return None;
                                }
                            }
                        }
                    }
                }
                return Some(MoltObject::none().bits());
            }
            let dict_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
            if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            if let Some(info) = class_slots_info(_py, class_ptr, attr_bits) {
                                if !info.allows_dict {
                                    return None;
                                }
                            }
                        }
                    }
                }
                let mut dict_bits = instance_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        instance_set_dict_bits(_py, obj_ptr, dict_bits);
                    }
                }
                if dict_bits != 0 {
                    inc_ref_bits(_py, dict_bits);
                    return Some(dict_bits);
                }
                return None;
            }
            let dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                            inc_ref_bits(_py, val);
                            return Some(val);
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if cached_attr_bits.is_none() {
                            if let Some(val_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                cached_attr_bits = Some(val_bits);
                                let class_version = class_layout_version_bits(class_ptr);
                                descriptor_cache_store(
                                    class_bits,
                                    attr_bits,
                                    class_version,
                                    None,
                                    Some(val_bits),
                                );
                            }
                        }
                        if let Some(val_bits) = cached_attr_bits {
                            if let Some(bound) =
                                descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr))
                            {
                                return Some(bound);
                            }
                            if exception_pending(_py) {
                                return None;
                            }
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let getattr_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.getattr_name,
                            b"__getattr__",
                        );
                        if !obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(getattr_bits)) {
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                getattr_bits,
                            ) {
                                let res_bits = call_callable1(_py, call_bits, attr_bits);
                                if exception_pending(_py) {
                                    return None;
                                }
                                return Some(res_bits);
                            }
                        }
                    }
                }
            }
            return None;
        }
        None
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_FUNCTION {
                if attr_name == "__closure__" {
                    let closure_bits = function_closure_bits(obj_ptr);
                    if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
                        inc_ref_bits(_py, closure_bits);
                        return closure_bits as i64;
                    }
                    return MoltObject::none().bits() as i64;
                }
                if attr_name == "__code__" {
                    let code_bits = ensure_function_code_bits(_py, obj_ptr);
                    if !obj_from_bits(code_bits).is_none() {
                        inc_ref_bits(_py, code_bits);
                        return code_bits as i64;
                    }
                }
            }
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let found = attr_lookup_ptr(_py, obj_ptr, attr_bits);
            dec_ref_bits(_py, attr_bits);
            if let Some(val) = found {
                return val as i64;
            }
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                molt_exception_clear();
                let _ = molt_raise(exc_bits);
                dec_ref_bits(_py, exc_bits);
                return MoltObject::none().bits() as i64;
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(obj_ptr);
                if !desc_ptr.is_null() && (*desc_ptr).slots {
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error_with_obj(
                        _py,
                        type_label,
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let type_label = if !desc_ptr.is_null() {
                    let name = &(*desc_ptr).name;
                    if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    }
                } else {
                    "dataclass"
                };
                return attr_error(_py, type_label, attr_name);
            }
            if type_id == TYPE_ID_TYPE {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                let res = attr_error_with_message(_py, &msg);
                let exc_bits = molt_exception_last();
                if !obj_from_bits(exc_bits).is_none() {
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                }
                return res;
            }
            attr_error_with_obj(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            )
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_MODULE {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let module_bits = MoltObject::from_ptr(obj_ptr).bits();
                let res = molt_module_set_attr(module_bits, attr_bits, val_bits);
                dec_ref_bits(_py, attr_bits);
                return res as i64;
            }
            if type_id == TYPE_ID_PROPERTY {
                if attr_name == "__doc__" {
                    property_doc_set(_py, obj_ptr, val_bits);
                    return MoltObject::none().bits() as i64;
                }
                return attr_error_with_obj(
                    _py,
                    "property",
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if type_id == TYPE_ID_TYPE {
                let class_bits = MoltObject::from_ptr(obj_ptr).bits();
                if is_builtin_class_bits(_py, class_bits) {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot set attributes on builtin type",
                    );
                }
                if attr_name == "__name__" || attr_name == "__qualname__" {
                    let val_obj = obj_from_bits(val_bits);
                    let is_str = if let Some(val_ptr) = val_obj.as_ptr() {
                        object_type_id(val_ptr) == TYPE_ID_STRING
                    } else {
                        false
                    };
                    if !is_str {
                        let class_label = class_name_for_error(class_bits);
                        let type_label = type_name(_py, val_obj);
                        let msg = format!(
                            "can only assign string to {class_label}.{attr_name}, not '{}'",
                            type_label
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if attr_name == "__name__" {
                        class_set_name_bits(_py, obj_ptr, val_bits);
                    } else {
                        class_set_qualname_bits(_py, obj_ptr, val_bits);
                    }
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits() as i64;
                }
                if attr_name == "__annotate__" && pep649_enabled() {
                    let val_obj = obj_from_bits(val_bits);
                    if !val_obj.is_none() {
                        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                        if !callable_ok {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotate__ must be callable or None",
                            );
                        }
                        class_set_annotations_bits(_py, obj_ptr, 0u64);
                    }
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            let annotate_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotate_name,
                                b"__annotate__",
                            );
                            dict_set_in_place(_py, dict_ptr, annotate_bits, val_bits);
                            if !val_obj.is_none() {
                                let annotations_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.annotations_name,
                                    b"__annotations__",
                                );
                                dict_del_in_place(_py, dict_ptr, annotations_bits);
                            }
                        }
                    }
                    class_set_annotate_bits(_py, obj_ptr, val_bits);
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits() as i64;
                }
                if attr_name == "__annotations__" {
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            let annotations_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotations_name,
                                b"__annotations__",
                            );
                            dict_set_in_place(_py, dict_ptr, annotations_bits, val_bits);
                            let annotate_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotate_name,
                                b"__annotate__",
                            );
                            let none_bits = MoltObject::none().bits();
                            if pep649_enabled() {
                                dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                            }
                        }
                    }
                    class_set_annotations_bits(_py, obj_ptr, val_bits);
                    if pep649_enabled() {
                        class_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                    }
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits() as i64;
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                        class_bump_layout_version(obj_ptr);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "type", attr_name);
            }
            if type_id == TYPE_ID_EXCEPTION {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
                if name == "name" || name == "obj" {
                    let kind_bits = exception_kind_bits(obj_ptr);
                    let mut is_attrerr = false;
                    if let Some(kind_ptr) = obj_from_bits(kind_bits).as_ptr() {
                        if object_type_id(kind_ptr) == TYPE_ID_STRING {
                            let kind_len = string_len(kind_ptr);
                            let kind_bytes =
                                std::slice::from_raw_parts(string_bytes(kind_ptr), kind_len);
                            is_attrerr = kind_bytes == b"AttributeError";
                        }
                    }
                    if is_attrerr {
                        let members_bits = exception_value_bits(obj_ptr);
                        let (old_name_bits, old_obj_bits) = if let Some(members_ptr) =
                            obj_from_bits(members_bits).as_ptr()
                        {
                            if object_type_id(members_ptr) == TYPE_ID_TUPLE {
                                let elems = seq_vec_ref(members_ptr);
                                (
                                    elems.get(0)
                                        .copied()
                                        .unwrap_or_else(|| MoltObject::none().bits()),
                                    elems.get(1)
                                        .copied()
                                        .unwrap_or_else(|| MoltObject::none().bits()),
                                )
                            } else {
                                (MoltObject::none().bits(), MoltObject::none().bits())
                            }
                        } else {
                            (MoltObject::none().bits(), MoltObject::none().bits())
                        };
                        let new_name_bits = if name == "name" {
                            val_bits
                        } else {
                            old_name_bits
                        };
                        let new_obj_bits = if name == "obj" {
                            val_bits
                        } else {
                            old_obj_bits
                        };
                        let tuple_ptr = alloc_tuple(_py, &[new_name_bits, new_obj_bits]);
                        if tuple_ptr.is_null() {
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                        unsafe {
                            let slot = obj_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
                            let old_bits = *slot;
                            if old_bits != tuple_bits {
                                dec_ref_bits(_py, old_bits);
                                inc_ref_bits(_py, tuple_bits);
                                *slot = tuple_bits;
                            }
                        }
                        dec_ref_bits(_py, tuple_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                }
                if name == "__cause__" || name == "__context__" {
                    let val_obj = obj_from_bits(val_bits);
                    if !val_obj.is_none() {
                        let Some(val_ptr) = val_obj.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                if name == "__cause__" {
                                    "exception cause must be an exception or None"
                                } else {
                                    "exception context must be an exception or None"
                                },
                            );
                        };
                        unsafe {
                            if object_type_id(val_ptr) != TYPE_ID_EXCEPTION {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    if name == "__cause__" {
                                        "exception cause must be an exception or None"
                                    } else {
                                        "exception context must be an exception or None"
                                    },
                                );
                            }
                        }
                    }
                    unsafe {
                        let slot = if name == "__cause__" {
                            obj_ptr.add(2 * std::mem::size_of::<u64>())
                        } else {
                            obj_ptr.add(3 * std::mem::size_of::<u64>())
                        } as *mut u64;
                        let old_bits = *slot;
                        if old_bits != val_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, val_bits);
                            *slot = val_bits;
                        }
                        if name == "__cause__" {
                            let suppress_bits = MoltObject::from_bool(true).bits();
                            let suppress_slot =
                                obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                            let old_bits = *suppress_slot;
                            if old_bits != suppress_bits {
                                dec_ref_bits(_py, old_bits);
                                inc_ref_bits(_py, suppress_bits);
                                *suppress_slot = suppress_bits;
                            }
                        }
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "args" {
                    let args_bits = exception_args_from_iterable(_py, val_bits);
                    if obj_from_bits(args_bits).is_none() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    let msg_bits = exception_message_from_args(_py, args_bits);
                    if obj_from_bits(msg_bits).is_none() {
                        dec_ref_bits(_py, args_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    unsafe {
                        exception_store_args_and_message(_py, obj_ptr, args_bits, msg_bits);
                        exception_set_stop_iteration_value(_py, obj_ptr, args_bits);
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "__suppress_context__" {
                    let suppress = is_truthy(_py, obj_from_bits(val_bits));
                    let suppress_bits = MoltObject::from_bool(suppress).bits();
                    unsafe {
                        let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != suppress_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, suppress_bits);
                            *slot = suppress_bits;
                        }
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "__dict__" {
                    let val_obj = obj_from_bits(val_bits);
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        let msg = format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            type_name(_py, val_obj)
                        );
                        dec_ref_bits(_py, attr_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    };
                    if object_type_id(val_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            type_name(_py, val_obj)
                        );
                        dec_ref_bits(_py, attr_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    unsafe {
                        let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != val_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, val_bits);
                            *slot = val_bits;
                        }
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "value" {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)))
                        .unwrap_or_default();
                    if kind != "StopIteration" {
                        dec_ref_bits(_py, attr_bits);
                        return attr_error(_py, "exception", attr_name);
                    }
                    unsafe {
                        let slot = obj_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != val_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, val_bits);
                            *slot = val_bits;
                        }
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                let mut dict_bits = exception_dict_bits(obj_ptr);
                if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != dict_bits {
                            dec_ref_bits(_py, old_bits);
                            *slot = dict_bits;
                        }
                    }
                }
                if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "exception", attr_name);
            }
            if type_id == TYPE_ID_FUNCTION {
                if attr_name == "__code__" {
                    let val_obj = obj_from_bits(val_bits);
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "function __code__ must be a code object",
                        );
                    };
                    unsafe {
                        if object_type_id(val_ptr) != TYPE_ID_CODE {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "function __code__ must be a code object",
                            );
                        }
                        function_set_code_bits(_py, obj_ptr, val_bits);
                    }
                    return MoltObject::none().bits() as i64;
                }
                if attr_name == "__closure__" {
                    return raise_exception::<_>(_py, "AttributeError", "readonly attribute");
                }
                if attr_name == "__annotate__" && pep649_enabled() {
                    let val_obj = obj_from_bits(val_bits);
                    if !val_obj.is_none() {
                        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                        if !callable_ok {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotate__ must be callable or None",
                            );
                        }
                        function_set_annotations_bits(_py, obj_ptr, 0);
                    }
                    function_set_annotate_bits(_py, obj_ptr, val_bits);
                    return MoltObject::none().bits() as i64;
                }
                if attr_name == "__annotations__" {
                    let val_obj = obj_from_bits(val_bits);
                    let ann_bits = if val_obj.is_none() {
                        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                        if dict_ptr.is_null() {
                            return MoltObject::none().bits() as i64;
                        }
                        MoltObject::from_ptr(dict_ptr).bits()
                    } else {
                        let Some(val_ptr) = val_obj.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotations__ must be set to a dict object",
                            );
                        };
                        if object_type_id(val_ptr) != TYPE_ID_DICT {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotations__ must be set to a dict object",
                            );
                        }
                        val_bits
                    };
                    function_set_annotations_bits(_py, obj_ptr, ann_bits);
                    if pep649_enabled() {
                        function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                    }
                    return MoltObject::none().bits() as i64;
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let mut dict_bits = function_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    function_set_dict_bits(obj_ptr, dict_bits);
                }
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                        if is_task_trampoline_attr_name(attr_name) {
                            refresh_function_task_trampoline_cache(_py, obj_ptr);
                        }
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "function", attr_name);
            }
            if type_id == TYPE_ID_CODE {
                return attr_error(_py, "code", attr_name);
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(obj_ptr);
                if !desc_ptr.is_null() && (*desc_ptr).frozen {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot assign to frozen dataclass field",
                    );
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0 {
                        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                                let setattr_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.setattr_name,
                                    b"__setattr__",
                                );
                                if let Some(call_bits) = class_attr_lookup(
                                    _py,
                                    class_ptr,
                                    class_ptr,
                                    Some(obj_ptr),
                                    setattr_bits,
                                ) {
                                    let _ = call_callable2(_py, call_bits, attr_bits, val_bits);
                                    dec_ref_bits(_py, attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                                if let Some(desc_bits) =
                                    class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                                {
                                    if descriptor_is_data(_py, desc_bits) {
                                        let desc_obj = obj_from_bits(desc_bits);
                                        if let Some(desc_ptr) = desc_obj.as_ptr() {
                                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                                let set_bits = property_set_bits(desc_ptr);
                                                if obj_from_bits(set_bits).is_none() {
                                                    dec_ref_bits(_py, attr_bits);
                                                    return property_no_setter(
                                                        _py,
                                                        attr_name,
                                                        class_ptr,
                                                        MoltObject::from_ptr(obj_ptr).bits(),
                                                    );
                                                }
                                                let inst_bits = instance_bits_for_call(obj_ptr);
                                                let _ = call_function_obj2(
                                                    _py, set_bits, inst_bits, val_bits,
                                                );
                                                dec_ref_bits(_py, attr_bits);
                                                return MoltObject::none().bits() as i64;
                                            }
                                        }
                                        let set_bits = intern_static_name(
                                            _py,
                                            &runtime_state(_py).interned.set_name,
                                            b"__set__",
                                        );
                                        if let Some(method_bits) =
                                            descriptor_method_bits(_py, desc_bits, set_bits)
                                        {
                                            let self_bits = desc_bits;
                                            let inst_bits = instance_bits_for_call(obj_ptr);
                                            let method_obj = obj_from_bits(method_bits);
                                            if let Some(method_ptr) = method_obj.as_ptr() {
                                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                                    let _ = call_function_obj3(
                                                        _py,
                                                        method_bits,
                                                        self_bits,
                                                        inst_bits,
                                                        val_bits,
                                                    );
                                                } else {
                                                    let _ = call_callable2(
                                                        _py,
                                                        method_bits,
                                                        inst_bits,
                                                        val_bits,
                                                    );
                                                }
                                            } else {
                                                let _ = call_callable2(
                                                    _py,
                                                    method_bits,
                                                    inst_bits,
                                                    val_bits,
                                                );
                                            }
                                            dec_ref_bits(_py, attr_bits);
                                            return MoltObject::none().bits() as i64;
                                        }
                                        dec_ref_bits(_py, attr_bits);
                                        return descriptor_no_setter(
                                            _py,
                                            attr_name,
                                            class_ptr,
                                            MoltObject::from_ptr(obj_ptr).bits(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if (*desc_ptr).slots {
                        dec_ref_bits(_py, attr_bits);
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        return attr_error_with_obj(
                            _py,
                            type_label,
                            attr_name,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                }
                let mut dict_bits = dataclass_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
                }
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                let type_label = if !desc_ptr.is_null() {
                    let name = &(*desc_ptr).name;
                    if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    }
                } else {
                    "dataclass"
                };
                return attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if type_id == TYPE_ID_OBJECT {
                let header = header_from_obj_ptr(obj_ptr);
                if (*header).poll_fn != 0 {
                    return attr_error_with_obj(
                        _py,
                        "object",
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let payload = object_payload_size(obj_ptr);
                if payload < std::mem::size_of::<u64>() {
                    return attr_error_with_obj(
                        _py,
                        "object",
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let class_bits = object_class_bits(obj_ptr);
                let mut slots_info = None;
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            slots_info = class_slots_info(_py, class_ptr, attr_bits);
                            let setattr_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.setattr_name,
                                b"__setattr__",
                            );
                            let mut use_custom_setattr = false;
                            if let Some(raw_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, setattr_bits)
                            {
                                if let Some(default_bits) = object_method_bits(_py, "__setattr__") {
                                    if !obj_eq(
                                        _py,
                                        obj_from_bits(raw_bits),
                                        obj_from_bits(default_bits),
                                    ) {
                                        use_custom_setattr = true;
                                    }
                                } else {
                                    use_custom_setattr = true;
                                }
                            }
                            if use_custom_setattr {
                                if let Some(call_bits) = class_attr_lookup(
                                    _py,
                                    class_ptr,
                                    class_ptr,
                                    Some(obj_ptr),
                                    setattr_bits,
                                ) {
                                    let _ = call_callable2(_py, call_bits, attr_bits, val_bits);
                                    dec_ref_bits(_py, attr_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            if let Some(desc_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                if descriptor_is_data(_py, desc_bits) {
                                    let desc_obj = obj_from_bits(desc_bits);
                                    if let Some(desc_ptr) = desc_obj.as_ptr() {
                                        if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                            let set_bits = property_set_bits(desc_ptr);
                                            if obj_from_bits(set_bits).is_none() {
                                                dec_ref_bits(_py, attr_bits);
                                                return property_no_setter(
                                                    _py,
                                                    attr_name,
                                                    class_ptr,
                                                    MoltObject::from_ptr(obj_ptr).bits(),
                                                );
                                            }
                                            let inst_bits = instance_bits_for_call(obj_ptr);
                                            let _ = call_function_obj2(
                                                _py, set_bits, inst_bits, val_bits,
                                            );
                                            dec_ref_bits(_py, attr_bits);
                                            return MoltObject::none().bits() as i64;
                                        }
                                    }
                                    let set_bits = intern_static_name(
                                        _py,
                                        &runtime_state(_py).interned.set_name,
                                        b"__set__",
                                    );
                                    if let Some(method_bits) =
                                        descriptor_method_bits(_py, desc_bits, set_bits)
                                    {
                                        let self_bits = desc_bits;
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let method_obj = obj_from_bits(method_bits);
                                        if let Some(method_ptr) = method_obj.as_ptr() {
                                            if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                                let _ = call_function_obj3(
                                                    _py,
                                                    method_bits,
                                                    self_bits,
                                                    inst_bits,
                                                    val_bits,
                                                );
                                            } else {
                                                let _ = call_callable2(
                                                    _py,
                                                    method_bits,
                                                    inst_bits,
                                                    val_bits,
                                                );
                                            }
                                        } else {
                                            let _ = call_callable2(
                                                _py,
                                                method_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        }
                                        dec_ref_bits(_py, attr_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                    dec_ref_bits(_py, attr_bits);
                                    return descriptor_no_setter(
                                        _py,
                                        attr_name,
                                        class_ptr,
                                        MoltObject::from_ptr(obj_ptr).bits(),
                                    );
                                }
                            }
                            if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                                dec_ref_bits(_py, attr_bits);
                                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits)
                                    as i64;
                            }
                        }
                    }
                }
                if let Some(info) = slots_info {
                    if !info.allows_dict {
                        dec_ref_bits(_py, attr_bits);
                        let type_label = class_name_for_error(class_bits);
                        if !info.allows_attr {
                            return attr_error_with_obj(
                                _py,
                                type_label,
                                attr_name,
                                MoltObject::from_ptr(obj_ptr).bits(),
                            );
                        }
                        return attr_error_with_obj(
                            _py,
                            type_label,
                            attr_name,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                }
                let mut dict_bits = instance_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    instance_set_dict_bits(_py, obj_ptr, dict_bits);
                }
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error_with_obj(
                    _py,
                    "object",
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            attr_error_with_obj(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            )
        })
    }
}

pub(crate) unsafe fn del_attr_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if type_id == TYPE_ID_MODULE {
            let dict_bits = module_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let annotations_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotations_name,
                        b"__annotations__",
                    );
                    if obj_eq(
                        _py,
                        obj_from_bits(attr_bits),
                        obj_from_bits(annotations_bits),
                    ) {
                        if dict_del_in_place(_py, dict_ptr, annotations_bits) {
                            if pep649_enabled() {
                                let annotate_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.annotate_name,
                                    b"__annotate__",
                                );
                                let none_bits = MoltObject::none().bits();
                                dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let module_name =
                            string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr)))
                                .unwrap_or_default();
                        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
                        return raise_exception::<_>(_py, "AttributeError", &msg);
                    }
                    let annotate_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotate_name,
                        b"__annotate__",
                    );
                    if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(annotate_bits))
                        && pep649_enabled()
                    {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot delete __annotate__ attribute",
                        );
                    }
                    if dict_del_in_place(_py, dict_ptr, attr_bits) {
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            let module_name =
                string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
            return attr_error_with_message(_py, &msg);
        }
        if type_id == TYPE_ID_TYPE {
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if is_builtin_class_bits(_py, class_bits) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete attributes on builtin type",
                );
            }
            if attr_name == "__annotate__" && pep649_enabled() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete __annotate__ attribute",
                );
            }
            if attr_name == "__annotations__" {
                let dict_bits = class_dict_bits(obj_ptr);
                let mut removed = false;
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let annotations_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotations_name,
                            b"__annotations__",
                        );
                        if dict_del_in_place(_py, dict_ptr, annotations_bits) {
                            removed = true;
                        }
                        if removed && pep649_enabled() {
                            let annotate_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotate_name,
                                b"__annotate__",
                            );
                            let none_bits = MoltObject::none().bits();
                            dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                        }
                    }
                }
                if !removed && class_annotations_bits(obj_ptr) != 0 {
                    removed = true;
                }
                if removed {
                    class_set_annotations_bits(_py, obj_ptr, 0u64);
                    if pep649_enabled() {
                        class_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                    }
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits() as i64;
                }
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return raise_exception::<_>(_py, "AttributeError", &msg);
            }
            let dict_bits = class_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits() as i64;
                }
            }
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            return attr_error_with_message(_py, &msg);
        }
        if type_id == TYPE_ID_EXCEPTION {
            if attr_name == "__cause__" || attr_name == "__context__" {
                unsafe {
                    let slot = if attr_name == "__cause__" {
                        obj_ptr.add(2 * std::mem::size_of::<u64>())
                    } else {
                        obj_ptr.add(3 * std::mem::size_of::<u64>())
                    } as *mut u64;
                    let old_bits = *slot;
                    if !obj_from_bits(old_bits).is_none() {
                        dec_ref_bits(_py, old_bits);
                        let none_bits = MoltObject::none().bits();
                        inc_ref_bits(_py, none_bits);
                        *slot = none_bits;
                    }
                    if attr_name == "__cause__" {
                        let suppress_bits = MoltObject::from_bool(false).bits();
                        let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *suppress_slot;
                        if old_bits != suppress_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, suppress_bits);
                            *suppress_slot = suppress_bits;
                        }
                    }
                }
                return MoltObject::none().bits() as i64;
            }
            if attr_name == "__suppress_context__" {
                unsafe {
                    let suppress_bits = MoltObject::from_bool(false).bits();
                    let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, suppress_bits);
                        *slot = suppress_bits;
                    }
                }
                return MoltObject::none().bits() as i64;
            }
            let dict_bits = exception_dict_bits(obj_ptr);
            if !obj_from_bits(dict_bits).is_none() && dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT
                        && dict_del_in_place(_py, dict_ptr, attr_bits)
                    {
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            return attr_error(_py, "exception", attr_name);
        }
        if type_id == TYPE_ID_FUNCTION {
            if attr_name == "__annotate__" && pep649_enabled() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete __annotate__ attribute",
                );
            }
            if attr_name == "__annotations__" {
                function_set_annotations_bits(_py, obj_ptr, 0);
                if pep649_enabled() {
                    function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                }
                return MoltObject::none().bits() as i64;
            }
            let dict_bits = function_dict_bits(obj_ptr);
            if dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT
                        && dict_del_in_place(_py, dict_ptr, attr_bits)
                    {
                        if is_task_trampoline_attr_name(attr_name) {
                            refresh_function_task_trampoline_cache(_py, obj_ptr);
                        }
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            return attr_error(_py, "function", attr_name);
        }
        if type_id == TYPE_ID_DATACLASS {
            let desc_ptr = dataclass_desc_ptr(obj_ptr);
            if !desc_ptr.is_null() {
                let class_bits = (*desc_ptr).class_bits;
                if class_bits != 0 {
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                        if object_type_id(class_ptr) == TYPE_ID_TYPE {
                            let delattr_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.delattr_name,
                                b"__delattr__",
                            );
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(obj_ptr),
                                delattr_bits,
                            ) {
                                let _ = call_callable1(_py, call_bits, attr_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            if let Some(desc_bits) =
                                class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            {
                                if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                                    if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                        let del_bits = property_del_bits(desc_ptr);
                                        if obj_from_bits(del_bits).is_none() {
                                            return property_no_deleter(
                                                _py,
                                                attr_name,
                                                class_ptr,
                                                MoltObject::from_ptr(obj_ptr).bits(),
                                            );
                                        }
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let _ = call_function_obj1(_py, del_bits, inst_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                }
                                let del_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.delete_name,
                                    b"__delete__",
                                );
                                if let Some(method_bits) =
                                    descriptor_method_bits(_py, desc_bits, del_bits)
                                {
                                    let self_bits = desc_bits;
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let method_obj = obj_from_bits(method_bits);
                                    if let Some(method_ptr) = method_obj.as_ptr() {
                                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                            let _ = call_function_obj2(
                                                _py,
                                                method_bits,
                                                self_bits,
                                                inst_bits,
                                            );
                                        } else {
                                            let _ = call_callable1(_py, method_bits, inst_bits);
                                        }
                                    } else {
                                        let _ = call_callable1(_py, method_bits, inst_bits);
                                    }
                                    return MoltObject::none().bits() as i64;
                                }
                                let set_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.set_name,
                                    b"__set__",
                                );
                                if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                                    return descriptor_no_deleter(
                                        _py,
                                        attr_name,
                                        class_ptr,
                                        MoltObject::from_ptr(obj_ptr).bits(),
                                    );
                                }
                            }
                        }
                    }
                }
                if (*desc_ptr).frozen {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot delete frozen dataclass field",
                    );
                }
                if (*desc_ptr).slots {
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error(_py, type_label, attr_name);
                }
            }
            let dict_bits = dataclass_dict_bits(obj_ptr);
            if dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT
                        && dict_del_in_place(_py, dict_ptr, attr_bits)
                    {
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            let type_label = if !desc_ptr.is_null() {
                let name = &(*desc_ptr).name;
                if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                }
            } else {
                "dataclass"
            };
            return attr_error(_py, type_label, attr_name);
        }
        if type_id == TYPE_ID_OBJECT {
            let header = header_from_obj_ptr(obj_ptr);
            if (*header).poll_fn != 0 {
                return attr_error(_py, "object", attr_name);
            }
            let payload = object_payload_size(obj_ptr);
            if payload < std::mem::size_of::<u64>() {
                return attr_error(_py, "object", attr_name);
            }
            let class_bits = object_class_bits(obj_ptr);
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        let delattr_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.delattr_name,
                            b"__delattr__",
                        );
                        if let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            delattr_bits,
                        ) {
                            let _ = call_callable1(_py, call_bits, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        if let Some(desc_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let del_bits = property_del_bits(desc_ptr);
                                    if obj_from_bits(del_bits).is_none() {
                                        return property_no_deleter(
                                            _py,
                                            attr_name,
                                            class_ptr,
                                            MoltObject::from_ptr(obj_ptr).bits(),
                                        );
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj1(_py, del_bits, inst_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let del_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.delete_name,
                                b"__delete__",
                            );
                            if let Some(method_bits) =
                                descriptor_method_bits(_py, desc_bits, del_bits)
                            {
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj2(
                                            _py,
                                            method_bits,
                                            self_bits,
                                            inst_bits,
                                        );
                                    } else {
                                        let _ = call_callable1(_py, method_bits, inst_bits);
                                    }
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            let set_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.set_name,
                                b"__set__",
                            );
                            if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                                return descriptor_no_deleter(
                                    _py,
                                    attr_name,
                                    class_ptr,
                                    MoltObject::from_ptr(obj_ptr).bits(),
                                );
                            }
                        }
                    }
                }
            }
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                            let slot = obj_ptr.add(offset) as *const u64;
                            if is_missing_bits(_py, *slot) {
                                return attr_error(_py, "object", attr_name);
                            }
                            let missing = missing_bits(_py);
                            let _ = object_field_set_ptr_raw(_py, obj_ptr, offset, missing);
                            return MoltObject::none().bits() as i64;
                        }
                    }
                }
            }
            let dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits != 0 {
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT
                        && dict_del_in_place(_py, dict_ptr, attr_bits)
                    {
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
            return attr_error(_py, "object", attr_name);
        }
        attr_error(
            _py,
            type_name(_py, MoltObject::from_ptr(obj_ptr)),
            attr_name,
        )
    }
}

pub(crate) unsafe fn object_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    unsafe {
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error_with_obj(
                _py,
                "object",
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error_with_obj(
                _py,
                "object",
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
        }
        let class_bits = object_class_bits(obj_ptr);
        let mut slots_info = None;
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    slots_info = class_slots_info(_py, class_ptr, attr_bits);
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                        if descriptor_is_data(_py, desc_bits) {
                            let desc_obj = obj_from_bits(desc_bits);
                            if let Some(desc_ptr) = desc_obj.as_ptr() {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let set_bits = property_set_bits(desc_ptr);
                                    if obj_from_bits(set_bits).is_none() {
                                        return property_no_setter(
                                            _py,
                                            attr_name,
                                            class_ptr,
                                            MoltObject::from_ptr(obj_ptr).bits(),
                                        );
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let set_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.set_name,
                                b"__set__",
                            );
                            if let Some(method_bits) =
                                descriptor_method_bits(_py, desc_bits, set_bits)
                            {
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj3(
                                            _py,
                                            method_bits,
                                            desc_bits,
                                            inst_bits,
                                            val_bits,
                                        );
                                    } else {
                                        let _ =
                                            call_callable2(_py, method_bits, inst_bits, val_bits);
                                    }
                                } else {
                                    let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            return descriptor_no_setter(
                                _py,
                                attr_name,
                                class_ptr,
                                MoltObject::from_ptr(obj_ptr).bits(),
                            );
                        }
                    }
                    if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                        return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits) as i64;
                    }
                }
            }
        }
        if let Some(info) = slots_info {
            if !info.allows_dict {
                let type_label = class_name_for_error(class_bits);
                if !info.allows_attr {
                    return attr_error_with_obj(
                        _py,
                        type_label,
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                return attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
        }
        let mut dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            let valid = obj_from_bits(dict_bits)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT);
            if !valid {
                dict_bits = 0;
                instance_set_dict_bits(_py, obj_ptr, 0);
            }
        }
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            instance_set_dict_bits(_py, obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        attr_error_with_obj(
            _py,
            "object",
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        )
    }
}

unsafe fn dataclass_setattr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
    enforce_frozen: bool,
) -> i64 {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if enforce_frozen && !desc_ptr.is_null() && (*desc_ptr).frozen {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cannot assign to frozen dataclass field",
            );
        }
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(desc_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if descriptor_is_data(_py, desc_bits) {
                                let desc_obj = obj_from_bits(desc_bits);
                                if let Some(desc_ptr) = desc_obj.as_ptr() {
                                    if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                        let set_bits = property_set_bits(desc_ptr);
                                        if obj_from_bits(set_bits).is_none() {
                                            return property_no_setter(
                                                _py,
                                                attr_name,
                                                class_ptr,
                                                MoltObject::from_ptr(obj_ptr).bits(),
                                            );
                                        }
                                        let inst_bits = instance_bits_for_call(obj_ptr);
                                        let _ =
                                            call_function_obj2(_py, set_bits, inst_bits, val_bits);
                                        return MoltObject::none().bits() as i64;
                                    }
                                }
                                let set_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.set_name,
                                    b"__set__",
                                );
                                if let Some(method_bits) =
                                    descriptor_method_bits(_py, desc_bits, set_bits)
                                {
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let method_obj = obj_from_bits(method_bits);
                                    if let Some(method_ptr) = method_obj.as_ptr() {
                                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                            let _ = call_function_obj3(
                                                _py,
                                                method_bits,
                                                desc_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        } else {
                                            let _ = call_callable2(
                                                _py,
                                                method_bits,
                                                inst_bits,
                                                val_bits,
                                            );
                                        }
                                    } else {
                                        let _ =
                                            call_callable2(_py, method_bits, inst_bits, val_bits);
                                    }
                                    return MoltObject::none().bits() as i64;
                                }
                                return descriptor_no_setter(
                                    _py,
                                    attr_name,
                                    class_ptr,
                                    MoltObject::from_ptr(obj_ptr).bits(),
                                );
                            }
                        }
                    }
                }
            }
            let field_names = &(*desc_ptr).field_names;
            if let Some(index) = field_names.iter().position(|name| name == attr_name) {
                let fields = dataclass_fields_mut(obj_ptr);
                if index < fields.len() {
                    let old_bits = fields[index];
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        fields[index] = val_bits;
                    }
                }
                if !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                        }
                    }
                }
                return MoltObject::none().bits() as i64;
            }
            if (*desc_ptr).slots {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
        }
        let mut dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                return MoltObject::none().bits() as i64;
            }
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        attr_error_with_obj(
            _py,
            type_label,
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        )
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn dataclass_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    unsafe { dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, true) }
}

pub(crate) unsafe fn dataclass_setattr_raw_unchecked(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    unsafe { dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, false) }
}

pub(crate) unsafe fn object_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    unsafe {
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return attr_error_with_obj(
                _py,
                class_name_for_error(object_class_bits(obj_ptr)),
                attr_name,
                obj_bits,
            );
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error_with_obj(
                _py,
                class_name_for_error(object_class_bits(obj_ptr)),
                attr_name,
                obj_bits,
            );
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                            if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                let del_bits = property_del_bits(desc_ptr);
                                if obj_from_bits(del_bits).is_none() {
                                    return property_no_deleter(
                                        _py,
                                        attr_name,
                                        class_ptr,
                                        MoltObject::from_ptr(obj_ptr).bits(),
                                    );
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj1(_py, del_bits, inst_bits);
                                return MoltObject::none().bits() as i64;
                            }
                        }
                        let del_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.delete_name,
                            b"__delete__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits)
                        {
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ =
                                        call_function_obj2(_py, method_bits, desc_bits, inst_bits);
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                            } else {
                                let _ = call_callable1(_py, method_bits, inst_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.set_name,
                            b"__set__",
                        );
                        if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(
                                _py,
                                attr_name,
                                class_ptr,
                                MoltObject::from_ptr(obj_ptr).bits(),
                            );
                        }
                    }
                }
            }
        }
        if class_bits != 0 {
            if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                        let slot = obj_ptr.add(offset) as *const u64;
                        if is_missing_bits(_py, *slot) {
                            return attr_error(_py, class_name_for_error(class_bits), attr_name);
                        }
                        let missing = missing_bits(_py);
                        let _ = object_field_set_ptr_raw(_py, obj_ptr, offset, missing);
                        return MoltObject::none().bits() as i64;
                    }
                }
            }
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        attr_error(_py, class_name_for_error(class_bits), attr_name)
    }
}

unsafe fn dataclass_delattr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    enforce_frozen: bool,
) -> i64 {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0 {
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                    if object_type_id(class_ptr) == TYPE_ID_TYPE {
                        if let Some(desc_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        {
                            if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits) {
                                if object_type_id(desc_ptr) == TYPE_ID_PROPERTY {
                                    let del_bits = property_del_bits(desc_ptr);
                                    if obj_from_bits(del_bits).is_none() {
                                        return property_no_deleter(
                                            _py,
                                            attr_name,
                                            class_ptr,
                                            MoltObject::from_ptr(obj_ptr).bits(),
                                        );
                                    }
                                    let inst_bits = instance_bits_for_call(obj_ptr);
                                    let _ = call_function_obj1(_py, del_bits, inst_bits);
                                    return MoltObject::none().bits() as i64;
                                }
                            }
                            let del_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.delete_name,
                                b"__delete__",
                            );
                            if let Some(method_bits) =
                                descriptor_method_bits(_py, desc_bits, del_bits)
                            {
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj2(
                                            _py,
                                            method_bits,
                                            desc_bits,
                                            inst_bits,
                                        );
                                    } else {
                                        let _ = call_callable1(_py, method_bits, inst_bits);
                                    }
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                                return MoltObject::none().bits() as i64;
                            }
                            let set_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.set_name,
                                b"__set__",
                            );
                            if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                                return descriptor_no_deleter(
                                    _py,
                                    attr_name,
                                    class_ptr,
                                    MoltObject::from_ptr(obj_ptr).bits(),
                                );
                            }
                        }
                    }
                }
            }
            if enforce_frozen && (*desc_ptr).frozen {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete frozen dataclass field",
                );
            }
            let field_names = &(*desc_ptr).field_names;
            if let Some(index) = field_names.iter().position(|name| name == attr_name) {
                let fields = dataclass_fields_mut(obj_ptr);
                if index < fields.len() {
                    let old_bits = fields[index];
                    if !is_missing_bits(_py, old_bits) {
                        let missing = missing_bits(_py);
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, missing);
                        fields[index] = missing;
                    }
                }
                if !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
                            let _ = dict_del_in_place(_py, dict_ptr, attr_bits);
                        }
                    }
                }
                return MoltObject::none().bits() as i64;
            }
            if (*desc_ptr).slots {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
        }
        let dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits != 0 {
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT
                    && dict_del_in_place(_py, dict_ptr, attr_bits)
                {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        attr_error_with_obj(
            _py,
            type_label,
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        )
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn dataclass_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    unsafe { dataclass_delattr_inner(_py, obj_ptr, attr_bits, attr_name, true) }
}

pub(crate) unsafe fn dataclass_delattr_raw_unchecked(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    unsafe { dataclass_delattr_inner(_py, obj_ptr, attr_bits, attr_name, false) }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            molt_set_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let res = del_attr_ptr(_py, obj_ptr, attr_bits, attr_name);
            dec_ref_bits(_py, attr_bits);
            res
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            molt_del_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                if object_type_id(ptr) == TYPE_ID_TYPE {
                    let class_bits = MoltObject::from_ptr(ptr).bits();
                    if is_builtin_class_bits(_py, class_bits)
                        && matches!(
                            attr_name,
                            "__getattribute__" | "__setattr__" | "__delattr__"
                        )
                    {
                        if let Some(func_bits) =
                            builtin_class_method_bits(_py, class_bits, attr_name)
                        {
                            if let Some(bits) = descriptor_bind(_py, func_bits, ptr, None) {
                                return bits as i64;
                            }
                        }
                    }
                }
                return molt_get_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
            }
            if obj.is_int() || obj.is_bool() {
                if let Some(func_bits) = int_method_bits(_py, attr_name) {
                    let bound_bits = molt_bound_method_new(func_bits, obj_bits);
                    return bound_bits as i64;
                }
            }
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_object_ic(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    site_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let Some(site_id) = ic_site_from_bits(site_bits) else {
                return molt_get_attr_object(obj_bits, attr_name_ptr, attr_name_len_bits);
            };
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let Some(name_bits) = attr_name_bits_for_site(_py, site_id, slice) else {
                return MoltObject::none().bits() as i64;
            };
            let out = molt_get_attr_name(obj_bits, name_bits);
            dec_ref_bits(_py, name_bits);
            out as i64
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_get_attr_special(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
                return attr_error_with_obj(_py, type_name(_py, obj), attr_name, obj_bits);
            };
            let name_ptr = alloc_string(_py, slice);
            if name_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let class_bits = object_class_bits(obj_ptr);
            let class_ptr = obj_from_bits(class_bits).as_ptr();
            let res = if let Some(class_ptr) = class_ptr {
                if object_type_id(class_ptr) == TYPE_ID_TYPE {
                    class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), name_bits)
                } else {
                    None
                }
            } else {
                None
            };
            dec_ref_bits(_py, name_bits);
            if let Some(bits) = res {
                return bits as i64;
            }
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                return molt_set_attr_generic(ptr, attr_name_ptr, attr_name_len_bits, val_bits);
            }
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                return molt_del_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
            }
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            attr_error(_py, type_name(_py, obj), attr_name)
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        if exception_pending(_py) {
            // Preserve any pre-existing exception; callers should unwind.
            return MoltObject::none().bits();
        }
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if let Some(val) = attr_lookup_ptr(_py, obj_ptr, name_bits) {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_DATACLASS {
                    let desc_ptr = dataclass_desc_ptr(obj_ptr);
                    if !desc_ptr.is_null() && (*desc_ptr).slots {
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        return attr_error_with_obj(_py, type_label, &attr_name, obj_bits) as u64;
                    }
                    let type_label = if !desc_ptr.is_null() {
                        let name = &(*desc_ptr).name;
                        if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        }
                    } else {
                        "dataclass"
                    };
                    return attr_error_with_obj(_py, type_label, &attr_name, obj_bits) as u64;
                }
                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return attr_error_with_obj_message(_py, &msg, &attr_name, obj_bits) as u64;
                }
                return attr_error_with_obj(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    &attr_name,
                    obj_bits,
                ) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            if obj.is_int() || obj.is_bool() {
                if let Some(func_bits) = int_method_bits(_py, &attr_name) {
                    return molt_bound_method_new(func_bits, obj_bits);
                }
            }
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_get_attr_name_default(
    obj_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            if exception_is_attribute_error(_py, exc_bits) {
                clear_exception(_py);
                dec_ref_bits(_py, exc_bits);
                inc_ref_bits(_py, default_bits);
                return default_bits;
            }
            clear_exception(_py);
            let _ = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return MoltObject::none().bits();
        }
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if let Some(val) = attr_lookup_ptr(_py, obj_ptr, name_bits) {
                    if matches!(
                        std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                        Some("1")
                    ) && string_obj_to_owned(obj_from_bits(name_bits)).as_deref()
                        == Some("__init_subclass__")
                    {
                        let val_obj = obj_from_bits(val);
                        eprintln!(
                            "molt init_subclass found val_bits=0x{:x} none={} ptr={}",
                            val,
                            val_obj.is_none(),
                            val_obj.as_ptr().is_some(),
                        );
                    }
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    if exception_is_attribute_error(_py, exc_bits) {
                        clear_exception(_py);
                        dec_ref_bits(_py, exc_bits);
                        inc_ref_bits(_py, default_bits);
                        return default_bits;
                    }
                    clear_exception(_py);
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                if matches!(
                    std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
                    Some("1")
                ) && string_obj_to_owned(obj_from_bits(name_bits)).as_deref()
                    == Some("__init_subclass__")
                {
                    let type_id = object_type_id(obj_ptr);
                    let class_bits = if type_id == TYPE_ID_TYPE {
                        MoltObject::from_ptr(obj_ptr).bits()
                    } else {
                        object_class_bits(obj_ptr)
                    };
                    eprintln!(
                        "molt init_subclass default obj_bits=0x{:x} type_id={} class_bits=0x{:x} default_bits=0x{:x} default_is_none={}",
                        MoltObject::from_ptr(obj_ptr).bits(),
                        type_id,
                        class_bits,
                        default_bits,
                        obj_from_bits(default_bits).is_none(),
                    );
                }
                inc_ref_bits(_py, default_bits);
                return default_bits;
            }
            let obj = obj_from_bits(obj_bits);
            if obj.is_int() || obj.is_bool() {
                if let Some(func_bits) = int_method_bits(_py, &attr_name) {
                    return molt_bound_method_new(func_bits, obj_bits);
                }
            }
        }
        inc_ref_bits(_py, default_bits);
        default_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_has_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        if exception_pending(_py) {
            let exc_bits = molt_exception_last();
            if exception_is_attribute_error(_py, exc_bits) {
                clear_exception(_py);
                dec_ref_bits(_py, exc_bits);
                return MoltObject::from_bool(false).bits();
            }
            clear_exception(_py);
            let _ = molt_raise(exc_bits);
            dec_ref_bits(_py, exc_bits);
            return MoltObject::from_bool(false).bits();
        }
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                if attr_lookup_ptr(_py, obj_ptr, name_bits).is_some() {
                    return MoltObject::from_bool(true).bits();
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    if exception_is_attribute_error(_py, exc_bits) {
                        clear_exception(_py);
                        dec_ref_bits(_py, exc_bits);
                        return MoltObject::from_bool(false).bits();
                    }
                    clear_exception(_py);
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::from_bool(false).bits();
                }
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let bytes = string_bytes(name_ptr);
                let len = string_len(name_ptr);
                return molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits) as u64;
            }
        }
        let obj = obj_from_bits(obj_bits);
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        attr_error(_py, type_name(_py, obj), &name) as u64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                return del_attr_ptr(_py, obj_ptr, name_bits, &attr_name) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error(_py, type_name(_py, obj), &attr_name) as u64
        }
    })
}
