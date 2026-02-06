use crate::PyToken;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::OnceLock;

use molt_obj_model::MoltObject;

use crate::builtins::annotations::pep649_enabled;
use crate::{
    alloc_dict_with_pairs, alloc_function_obj, alloc_property_obj, alloc_string, attr_lookup_ptr,
    builtin_class_method_bits, builtin_classes, builtin_func_bits, call_callable1, call_callable3,
    call_function_obj1, class_bases_bits, class_bases_vec, class_dict_bits,
    class_layout_version_bits, class_mro_ref, class_mro_vec, class_name_bits, class_name_for_error,
    classmethod_func_bits, clear_exception, dataclass_desc_ptr, dataclass_dict_bits,
    dataclass_fields_ref, dataclass_set_dict_bits, dec_ref_bits, dict_get_in_place, dict_order,
    dict_set_in_place, exception_class_bits, exception_dict_bits, exception_kind_bits,
    exception_pending, exception_type_bits_from_name, header_from_obj_ptr, inc_ref_bits,
    init_atomic_bits, instance_dict_bits, instance_set_dict_bits, intern_static_name,
    is_builtin_class_bits, is_missing_bits, is_truthy, issubclass_bits, maybe_ptr_from_bits,
    module_dict_bits, molt_awaitable_await, molt_bound_method_new, molt_exception_last,
    molt_function_get_code, molt_function_get_globals, molt_iter, molt_iter_next, obj_eq,
    obj_from_bits, object_class_bits, object_field_get_ptr_raw, object_set_class_bits,
    object_type_id, property_get_bits, raise_exception, runtime_state, seq_vec_ref,
    staticmethod_func_bits, string_obj_to_owned, type_name, type_of_bits, TYPE_ID_CALL_ITER,
    TYPE_ID_CLASSMETHOD, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_DICT_ITEMS_VIEW,
    TYPE_ID_DICT_KEYS_VIEW, TYPE_ID_DICT_VALUES_VIEW, TYPE_ID_ENUMERATE, TYPE_ID_EXCEPTION,
    TYPE_ID_FILE_HANDLE, TYPE_ID_FILTER, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_ITER,
    TYPE_ID_LIST, TYPE_ID_MAP, TYPE_ID_OBJECT, TYPE_ID_PROPERTY, TYPE_ID_REVERSED,
    TYPE_ID_STATICMETHOD, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE, TYPE_ID_ZIP,
};

struct AttrNameCacheEntry {
    bytes: Vec<u8>,
    bits: u64,
}

fn debug_class_layout_filter() -> Option<&'static str> {
    static FILTER: OnceLock<Option<String>> = OnceLock::new();
    FILTER
        .get_or_init(|| {
            std::env::var("MOLT_DEBUG_CLASS_LAYOUT")
                .ok()
                .map(|raw| raw.trim().to_string())
                .filter(|val| !val.is_empty())
        })
        .as_deref()
}

fn debug_class_layout_match(class_name: &str) -> bool {
    match debug_class_layout_filter() {
        Some("1") => true,
        Some(filter) => class_name.contains(filter),
        None => false,
    }
}

#[derive(Clone)]
pub(crate) struct DescriptorCacheEntry {
    pub(crate) class_bits: u64,
    pub(crate) attr_name: Vec<u8>,
    pub(crate) version: u64,
    pub(crate) data_desc_bits: Option<u64>,
    pub(crate) class_attr_bits: Option<u64>,
}

thread_local! {
    static ATTR_NAME_TLS: RefCell<Option<AttrNameCacheEntry>> = const { RefCell::new(None) };
    static DESCRIPTOR_CACHE_TLS: RefCell<Option<DescriptorCacheEntry>> = const { RefCell::new(None) };
}

pub(crate) fn clear_attr_tls_caches(_py: &PyToken<'_>) {
    crate::gil_assert();
    let _ = ATTR_NAME_TLS.try_with(|cell| {
        let mut entry = cell.borrow_mut();
        if let Some(prev) = entry.take() {
            dec_ref_bits(_py, prev.bits);
        }
    });
    let _ = DESCRIPTOR_CACHE_TLS.try_with(|cell| {
        cell.borrow_mut().take();
    });
}

pub(crate) fn debug_last_attr_name() -> Option<String> {
    ATTR_NAME_TLS
        .try_with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|entry| String::from_utf8_lossy(&entry.bytes).into_owned())
        })
        .ok()
        .flatten()
}

pub(crate) fn attr_error(_py: &PyToken<'_>, type_label: impl AsRef<str>, attr_name: &str) -> i64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    let res = raise_exception(_py, "AttributeError", &msg);
    let exc_bits = molt_exception_last();
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_defaults(_py, exc_bits);
    }
    dec_ref_bits(_py, exc_bits);
    res
}

fn set_attribute_error_defaults(_py: &PyToken<'_>, exc_bits: u64) {
    crate::gil_assert();
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return;
    };
    let name_key = intern_static_name(_py, &runtime_state(_py).interned.name_name, b"name");
    let obj_key = intern_static_name(_py, &runtime_state(_py).interned.obj_name, b"obj");
    let none_bits = MoltObject::none().bits();
    let mut dict_bits = unsafe { exception_dict_bits(exc_ptr) };
    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        unsafe {
            let slot = exc_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *slot;
            if old_bits != dict_bits {
                dec_ref_bits(_py, old_bits);
                *slot = dict_bits;
            }
        }
    }
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        unsafe {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(_py, dict_ptr, name_key, none_bits);
                dict_set_in_place(_py, dict_ptr, obj_key, none_bits);
            }
        }
    }
}

fn set_attribute_error_attrs(_py: &PyToken<'_>, exc_bits: u64, attr_name: &str, obj_bits: u64) {
    crate::gil_assert();
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return;
    };
    let Some(name_bits) = attr_name_bits_from_bytes(_py, attr_name.as_bytes()) else {
        return;
    };
    let name_key = intern_static_name(_py, &runtime_state(_py).interned.name_name, b"name");
    let obj_key = intern_static_name(_py, &runtime_state(_py).interned.obj_name, b"obj");
    let mut dict_bits = unsafe { exception_dict_bits(exc_ptr) };
    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            return;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        unsafe {
            let slot = exc_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *slot;
            if old_bits != dict_bits {
                dec_ref_bits(_py, old_bits);
                *slot = dict_bits;
            }
        }
    }
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
        unsafe {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_set_in_place(_py, dict_ptr, name_key, name_bits);
                dict_set_in_place(_py, dict_ptr, obj_key, obj_bits);
            }
        }
    }
    dec_ref_bits(_py, name_bits);
}

pub(crate) fn attr_error_with_obj(
    _py: &PyToken<'_>,
    type_label: impl AsRef<str>,
    attr_name: &str,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    let res = raise_exception(_py, "AttributeError", &msg);
    let exc_bits = molt_exception_last();
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_attrs(_py, exc_bits, attr_name, obj_bits);
    }
    dec_ref_bits(_py, exc_bits);
    res
}

pub(crate) fn attr_error_with_message(_py: &PyToken<'_>, msg: &str) -> i64 {
    crate::gil_assert();
    let res = raise_exception(_py, "AttributeError", msg);
    let exc_bits = molt_exception_last();
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_defaults(_py, exc_bits);
    }
    dec_ref_bits(_py, exc_bits);
    res
}

pub(crate) fn attr_error_with_obj_message(
    _py: &PyToken<'_>,
    msg: &str,
    attr_name: &str,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let res = raise_exception(_py, "AttributeError", msg);
    let exc_bits = molt_exception_last();
    if !obj_from_bits(exc_bits).is_none() {
        set_attribute_error_attrs(_py, exc_bits, attr_name, obj_bits);
    }
    dec_ref_bits(_py, exc_bits);
    res
}

pub(crate) fn property_no_setter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no setter");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn property_no_deleter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no deleter");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn descriptor_no_setter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn descriptor_no_deleter(
    _py: &PyToken<'_>,
    attr_name: &str,
    class_ptr: *mut u8,
    obj_bits: u64,
) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    attr_error_with_obj_message(_py, &msg, attr_name, obj_bits)
}

pub(crate) fn attr_name_bits_from_bytes(_py: &PyToken<'_>, slice: &[u8]) -> Option<u64> {
    crate::gil_assert();
    if let Some(bits) = ATTR_NAME_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| entry.bytes == slice)
            .map(|entry| entry.bits)
    }) {
        inc_ref_bits(_py, bits);
        return Some(bits);
    }
    let ptr = alloc_string(_py, slice);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    ATTR_NAME_TLS.with(|cell| {
        let mut entry = cell.borrow_mut();
        if let Some(prev) = entry.take() {
            dec_ref_bits(_py, prev.bits);
        }
        inc_ref_bits(_py, bits);
        *entry = Some(AttrNameCacheEntry {
            bytes: slice.to_vec(),
            bits,
        });
    });
    Some(bits)
}

pub(crate) fn raise_attr_name_type_error(_py: &PyToken<'_>, name_bits: u64) -> u64 {
    crate::gil_assert();
    let name_obj = obj_from_bits(name_bits);
    let msg = format!(
        "attribute name must be string, not '{}'",
        type_name(_py, name_obj)
    );
    raise_exception(_py, "TypeError", &msg)
}

pub(crate) fn exception_is_attribute_error(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    crate::gil_assert();
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return false;
        }
        let class_bits = exception_class_bits(exc_ptr);
        if class_bits != 0 {
            let attr_error_bits = exception_type_bits_from_name(_py, "AttributeError");
            if attr_error_bits != 0 && issubclass_bits(class_bits, attr_error_bits) {
                return true;
            }
        }
        let kind_bits = exception_kind_bits(exc_ptr);
        let kind = string_obj_to_owned(obj_from_bits(kind_bits));
        kind.as_deref() == Some("AttributeError")
    }
}

pub(crate) fn clear_attribute_error_if_pending(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last();
    let is_attr = exception_is_attribute_error(_py, exc_bits);
    if is_attr {
        clear_exception(_py);
        dec_ref_bits(_py, exc_bits);
        return true;
    }
    dec_ref_bits(_py, exc_bits);
    false
}

pub(crate) unsafe fn module_attr_lookup(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let dict_bits = module_dict_bits(ptr);
    let dict_obj = obj_from_bits(dict_bits);
    let dict_ptr = dict_obj.as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    let dict_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
    if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
        inc_ref_bits(_py, dict_bits);
        return Some(dict_bits);
    }
    let annotations_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.annotations_name,
        b"__annotations__",
    );
    if obj_eq(
        _py,
        obj_from_bits(attr_bits),
        obj_from_bits(annotations_name_bits),
    ) {
        if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotations_name_bits) {
            inc_ref_bits(_py, val_bits);
            return Some(val_bits);
        }
        let res_bits = if pep649_enabled() {
            let annotate_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotate_name,
                b"__annotate__",
            );
            let annotate_bits = dict_get_in_place(_py, dict_ptr, annotate_name_bits)
                .unwrap_or_else(|| MoltObject::none().bits());
            if !obj_from_bits(annotate_bits).is_none() {
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
                    return raise_exception(_py, "TypeError", &msg);
                };
                if object_type_id(res_ptr) != TYPE_ID_DICT {
                    let msg = format!(
                        "__annotate__ returned non-dict of type '{}'",
                        type_name(_py, res_obj)
                    );
                    dec_ref_bits(_py, res_bits);
                    return raise_exception(_py, "TypeError", &msg);
                }
                res_bits
            } else {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    return None;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            }
        } else {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return None;
            }
            MoltObject::from_ptr(dict_ptr).bits()
        };
        let complete_name_bits =
            attr_name_bits_from_bytes(_py, b"__molt_module_complete__").unwrap_or(0);
        let mut cache = false;
        if complete_name_bits != 0 {
            if let Some(complete_bits) = dict_get_in_place(_py, dict_ptr, complete_name_bits) {
                cache = is_truthy(_py, obj_from_bits(complete_bits));
            }
            dec_ref_bits(_py, complete_name_bits);
        }
        if cache {
            dict_set_in_place(_py, dict_ptr, annotations_name_bits, res_bits);
        }
        return Some(res_bits);
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
    ) {
        if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotate_name_bits) {
            inc_ref_bits(_py, val_bits);
            return Some(val_bits);
        }
        let none_bits = MoltObject::none().bits();
        inc_ref_bits(_py, none_bits);
        return Some(none_bits);
    }
    dict_get_in_place(_py, dict_ptr, attr_bits).inspect(|val| inc_ref_bits(_py, *val))
}

pub(crate) unsafe fn dir_collect_from_dict_ptr(
    dict_ptr: *mut u8,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    crate::gil_assert();
    let order = dict_order(dict_ptr);
    for pair in order.chunks_exact(2) {
        let key_bits = pair[0];
        if let Some(name) = string_obj_to_owned(obj_from_bits(key_bits)) {
            if seen.insert(name) {
                out.push(key_bits);
            }
        }
    }
}

pub(crate) unsafe fn dir_collect_from_class_bits(
    class_bits: u64,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    crate::gil_assert();
    for base_bits in class_mro_vec(class_bits) {
        let class_obj = obj_from_bits(base_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            continue;
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            continue;
        }
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            continue;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            continue;
        }
        dir_collect_from_dict_ptr(dict_ptr, seen, out);
    }
}

pub(crate) unsafe fn dir_collect_from_instance(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    crate::gil_assert();
    let dict_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
    let Some(dict_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, dict_name_bits) else {
        return;
    };
    let dict_obj = obj_from_bits(dict_bits);
    if let Some(dict_ptr) = dict_obj.as_ptr() {
        if object_type_id(dict_ptr) == TYPE_ID_DICT {
            dir_collect_from_dict_ptr(dict_ptr, seen, out);
        }
    }
    dec_ref_bits(_py, dict_bits);
}

pub(crate) unsafe fn instance_bits_for_call(ptr: *mut u8) -> u64 {
    MoltObject::from_ptr(ptr).bits()
}

fn function_code_descriptor_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.function_code_descriptor,
        || {
            let getter_ptr = alloc_function_obj(_py, fn_addr!(molt_function_get_code), 1);
            if getter_ptr.is_null() {
                return 0;
            }
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                object_set_class_bits(_py, getter_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
            let getter_bits = MoltObject::from_ptr(getter_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let prop_ptr = alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            dec_ref_bits(_py, getter_bits);
            if prop_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(prop_ptr).bits()
        },
    )
}

fn function_globals_descriptor_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.function_globals_descriptor,
        || {
            let getter_ptr = alloc_function_obj(_py, fn_addr!(molt_function_get_globals), 1);
            if getter_ptr.is_null() {
                return 0;
            }
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                object_set_class_bits(_py, getter_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
            let getter_bits = MoltObject::from_ptr(getter_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let prop_ptr = alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            dec_ref_bits(_py, getter_bits);
            if prop_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(prop_ptr).bits()
        },
    )
}

pub(crate) unsafe fn class_attr_lookup_raw_mro(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
    if let Some(name) = attr_name.as_deref() {
        if name == "__code__" || name == "__globals__" {
            let builtins = builtin_classes(_py);
            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            if class_bits == builtins.function {
                let bits = if name == "__code__" {
                    function_code_descriptor_bits(_py)
                } else {
                    function_globals_descriptor_bits(_py)
                };
                if bits != 0 {
                    return Some(bits);
                }
            }
        }
    }
    let debug_bound = std::env::var_os("MOLT_DEBUG_BOUND_METHOD").is_some();
    if let Some(mro) = class_mro_ref(class_ptr) {
        for class_bits in mro.iter() {
            let class_obj = obj_from_bits(*class_bits);
            let Some(ptr) = class_obj.as_ptr() else {
                continue;
            };
            if object_type_id(ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let Some(dict_ptr) = dict_obj.as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                if debug_bound {
                    if let Some(name) = attr_name.as_deref() {
                        let class_name_bits = class_name_bits(ptr);
                        let class_name = string_obj_to_owned(obj_from_bits(class_name_bits))
                            .unwrap_or_else(|| "<unknown>".to_string());
                        let val_obj = obj_from_bits(val_bits);
                        let (val_type_id, val_type_name) = match val_obj.as_ptr() {
                            Some(val_ptr) => (
                                object_type_id(val_ptr),
                                type_name(_py, val_obj).into_owned(),
                            ),
                            None => (0, format!("immediate:{:#x}", val_bits)),
                        };
                        if class_name == "ThreadPoolExecutor" || class_name == "Executor" {
                            eprintln!(
                                "class_attr_lookup_raw_mro: attr={} class={} val_bits={:#x} val_type_id={} val_type={}",
                                name, class_name, val_bits, val_type_id, val_type_name
                            );
                        }
                    }
                }
                return Some(val_bits);
            }
            if let Some(name) = attr_name.as_deref() {
                if is_builtin_class_bits(_py, *class_bits) {
                    if let Some(func_bits) = builtin_class_method_bits(_py, *class_bits, name) {
                        return Some(func_bits);
                    }
                }
            }
        }
        return None;
    }
    let mut current_ptr = class_ptr;
    let mut depth = 0usize;
    loop {
        let dict_bits = class_dict_bits(current_ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = dict_obj.as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
            return Some(val_bits);
        }
        if let Some(name) = attr_name.as_deref() {
            let current_bits = MoltObject::from_ptr(current_ptr).bits();
            if is_builtin_class_bits(_py, current_bits) {
                if let Some(func_bits) = builtin_class_method_bits(_py, current_bits, name) {
                    return Some(func_bits);
                }
            }
        }
        let bases_bits = class_bases_bits(current_ptr);
        let bases = class_bases_vec(bases_bits);
        let next_bits = bases.first().copied()?;
        let next_obj = obj_from_bits(next_bits);
        let next_ptr = next_obj.as_ptr()?;
        if object_type_id(next_ptr) != TYPE_ID_TYPE {
            return None;
        }
        if next_ptr == current_ptr {
            return None;
        }
        current_ptr = next_ptr;
        depth += 1;
        if depth > 64 {
            return None;
        }
    }
}

pub(crate) unsafe fn class_field_offset(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<usize> {
    crate::gil_assert();
    let fields_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.field_offsets_name,
        b"__molt_field_offsets__",
    );
    let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
        Cow::Borrowed(mro.as_slice())
    } else {
        Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
    };
    for class_bits in mro.iter().copied() {
        let Some(current_ptr) = obj_from_bits(class_bits).as_ptr() else {
            continue;
        };
        if object_type_id(current_ptr) != TYPE_ID_TYPE {
            continue;
        }
        let dict_bits = class_dict_bits(current_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            continue;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            continue;
        }
        let Some(offsets_bits) = dict_get_in_place(_py, dict_ptr, fields_bits) else {
            continue;
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            continue;
        };
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            continue;
        }
        let Some(offset_bits) = dict_get_in_place(_py, offsets_ptr, attr_bits) else {
            continue;
        };
        return obj_from_bits(offset_bits).as_int().and_then(|val| {
            if val >= 0 {
                Some(val as usize)
            } else {
                None
            }
        });
    }
    None
}

pub(crate) unsafe fn is_iterator_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    crate::gil_assert();
    let Some(ptr) = maybe_ptr_from_bits(bits) else {
        return false;
    };
    match object_type_id(ptr) {
        TYPE_ID_ITER
        | TYPE_ID_GENERATOR
        | TYPE_ID_ENUMERATE
        | TYPE_ID_CALL_ITER
        | TYPE_ID_REVERSED
        | TYPE_ID_ZIP
        | TYPE_ID_MAP
        | TYPE_ID_FILTER
        | TYPE_ID_DICT_KEYS_VIEW
        | TYPE_ID_DICT_VALUES_VIEW
        | TYPE_ID_DICT_ITEMS_VIEW
        | TYPE_ID_FILE_HANDLE => return true,
        _ => {}
    }
    let class_bits = if object_type_id(ptr) == TYPE_ID_TYPE {
        type_of_bits(_py, MoltObject::from_ptr(ptr).bits())
    } else {
        object_class_bits(ptr)
    };
    if class_bits == 0 {
        return false;
    }
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return false;
    };
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return false;
    }
    let Some(next_bits) = attr_name_bits_from_bytes(_py, b"__next__") else {
        return false;
    };
    let has_next = class_attr_lookup_raw_mro(_py, class_ptr, next_bits).is_some();
    dec_ref_bits(_py, next_bits);
    has_next
}

pub(crate) fn descriptor_cache_lookup(
    class_bits: u64,
    attr_bits: u64,
    version: u64,
) -> Option<DescriptorCacheEntry> {
    crate::gil_assert();
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))?;
    let attr_bytes = attr_name.as_bytes();
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| {
                entry.class_bits == class_bits
                    && entry.version == version
                    && entry.attr_name == attr_bytes
            })
            .cloned()
    })
}

pub(crate) fn descriptor_cache_store(
    class_bits: u64,
    attr_bits: u64,
    version: u64,
    data_desc_bits: Option<u64>,
    class_attr_bits: Option<u64>,
) {
    crate::gil_assert();
    let Some(attr_name) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
        return;
    };
    let entry = DescriptorCacheEntry {
        class_bits,
        attr_name: attr_name.into_bytes(),
        version,
        data_desc_bits,
        class_attr_bits,
    };
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        *cell.borrow_mut() = Some(entry);
    });
}

pub(crate) unsafe fn descriptor_method_bits(
    _py: &PyToken<'_>,
    val_bits: u64,
    name_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let class_bits = if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => MoltObject::from_ptr(ptr).bits(),
                TYPE_ID_OBJECT => object_class_bits(ptr),
                _ => type_of_bits(_py, val_bits),
            }
        }
    } else {
        type_of_bits(_py, val_bits)
    };
    let class_obj = obj_from_bits(class_bits);
    let class_ptr = class_obj.as_ptr()?;
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return None;
    }
    class_attr_lookup_raw_mro(_py, class_ptr, name_bits)
}

pub(crate) unsafe fn descriptor_has_method(
    _py: &PyToken<'_>,
    val_bits: u64,
    name_bits: u64,
) -> bool {
    crate::gil_assert();
    descriptor_method_bits(_py, val_bits, name_bits).is_some()
}

pub(crate) unsafe fn descriptor_is_data(_py: &PyToken<'_>, val_bits: u64) -> bool {
    crate::gil_assert();
    let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
        return false;
    };
    if object_type_id(val_ptr) == TYPE_ID_PROPERTY {
        return true;
    }
    let set_bits = intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
    let del_bits = intern_static_name(_py, &runtime_state(_py).interned.delete_name, b"__delete__");
    descriptor_has_method(_py, val_bits, set_bits) || descriptor_has_method(_py, val_bits, del_bits)
}

pub(crate) unsafe fn attr_lookup_ptr_any(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    match object_type_id(obj_ptr) {
        TYPE_ID_OBJECT => object_attr_lookup_raw(_py, obj_ptr, attr_bits),
        TYPE_ID_DATACLASS => dataclass_attr_lookup_raw(_py, obj_ptr, attr_bits),
        _ => attr_lookup_ptr(_py, obj_ptr, attr_bits),
    }
}

pub(crate) unsafe fn attr_lookup_ptr_allow_missing(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let res = attr_lookup_ptr_any(_py, obj_ptr, attr_bits);
    if matches!(
        std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
        Some("1")
    ) && string_obj_to_owned(obj_from_bits(attr_bits)).as_deref() == Some("__init_subclass__")
    {
        match res {
            Some(bits) => {
                let obj = obj_from_bits(bits);
                eprintln!(
                    "molt init_subclass allow_missing res_bits=0x{:x} none={} ptr={}",
                    bits,
                    obj.is_none(),
                    obj.as_ptr().is_some(),
                );
            }
            None => {
                eprintln!("molt init_subclass allow_missing res=None");
            }
        }
    }
    if res.is_some() {
        return res;
    }
    if exception_pending(_py) {
        let _ = clear_attribute_error_if_pending(_py);
    }
    None
}

pub(crate) unsafe fn descriptor_bind(
    _py: &PyToken<'_>,
    val_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
) -> Option<u64> {
    crate::gil_assert();
    let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
        inc_ref_bits(_py, val_bits);
        return Some(val_bits);
    };
    match object_type_id(val_ptr) {
        TYPE_ID_FUNCTION => {
            if let Some(inst_ptr) = instance_ptr {
                let inst_bits = instance_bits_for_call(inst_ptr);
                let bound_bits = molt_bound_method_new(val_bits, inst_bits);
                Some(bound_bits)
            } else {
                inc_ref_bits(_py, val_bits);
                Some(val_bits)
            }
        }
        TYPE_ID_CLASSMETHOD => {
            let func_bits = classmethod_func_bits(val_ptr);
            if owner_ptr.is_null() {
                inc_ref_bits(_py, func_bits);
                return Some(func_bits);
            }
            let class_bits = MoltObject::from_ptr(owner_ptr).bits();
            Some(molt_bound_method_new(func_bits, class_bits))
        }
        TYPE_ID_STATICMETHOD => {
            let func_bits = staticmethod_func_bits(val_ptr);
            inc_ref_bits(_py, func_bits);
            Some(func_bits)
        }
        TYPE_ID_PROPERTY => {
            if let Some(inst_ptr) = instance_ptr {
                let get_bits = property_get_bits(val_ptr);
                if obj_from_bits(get_bits).is_none() {
                    return raise_exception(_py, "AttributeError", "unreadable property");
                }
                let inst_bits = instance_bits_for_call(inst_ptr);
                let value_bits = call_function_obj1(_py, get_bits, inst_bits);
                if exception_pending(_py) {
                    let _ = clear_attribute_error_if_pending(_py);
                    return None;
                }
                Some(value_bits)
            } else {
                inc_ref_bits(_py, val_bits);
                Some(val_bits)
            }
        }
        _ => {
            let get_bits =
                intern_static_name(_py, &runtime_state(_py).interned.get_name, b"__get__");
            if let Some(method_bits) = descriptor_method_bits(_py, val_bits, get_bits) {
                let self_bits = MoltObject::from_ptr(val_ptr).bits();
                let inst_bits = instance_ptr
                    .map(|ptr| instance_bits_for_call(ptr))
                    .unwrap_or_else(|| MoltObject::none().bits());
                let owner_bits = if owner_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(owner_ptr).bits()
                };
                let res = call_callable3(_py, method_bits, self_bits, inst_bits, owner_bits);
                if exception_pending(_py) {
                    let _ = clear_attribute_error_if_pending(_py);
                    return None;
                }
                return Some(res);
            }
            inc_ref_bits(_py, val_bits);
            Some(val_bits)
        }
    }
}

pub(crate) unsafe fn class_attr_lookup(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let val_bits = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?;
    descriptor_bind(_py, val_bits, owner_ptr, instance_ptr)
}

pub(crate) fn awaitable_await_func_bits(_py: &PyToken<'_>) -> u64 {
    builtin_func_bits(
        _py,
        &runtime_state(_py).special_cache.awaitable_await,
        fn_addr!(molt_awaitable_await),
        1,
    )
}

pub(crate) struct SlotsInfo {
    pub(crate) allows_attr: bool,
    pub(crate) allows_dict: bool,
}

pub(crate) unsafe fn class_slots_info(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<SlotsInfo> {
    crate::gil_assert();
    let slots_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.slots_name, b"__slots__");
    let dict_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
    let class_dict_bits_val = class_dict_bits(class_ptr);
    let class_dict_ptr = obj_from_bits(class_dict_bits_val).as_ptr()?;
    if object_type_id(class_dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    dict_get_in_place(_py, class_dict_ptr, slots_name_bits)?;
    let mut allows_attr = false;
    let mut allows_dict = false;
    let attr_obj = obj_from_bits(attr_bits);
    let dict_obj = obj_from_bits(dict_name_bits);
    let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
        Cow::Borrowed(mro.as_slice())
    } else {
        Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
    };
    for class_bits in mro.iter().copied() {
        let Some(ptr) = obj_from_bits(class_bits).as_ptr() else {
            continue;
        };
        if object_type_id(ptr) != TYPE_ID_TYPE {
            continue;
        }
        let dict_bits = class_dict_bits(ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            continue;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            continue;
        }
        let Some(slots_bits) = dict_get_in_place(_py, dict_ptr, slots_name_bits) else {
            continue;
        };
        let slots_obj = obj_from_bits(slots_bits);
        if let Some(slots_ptr) = slots_obj.as_ptr() {
            match object_type_id(slots_ptr) {
                TYPE_ID_STRING => {
                    if obj_eq(_py, attr_obj, slots_obj) {
                        allows_attr = true;
                    }
                    if obj_eq(_py, dict_obj, slots_obj) {
                        allows_dict = true;
                    }
                }
                TYPE_ID_TUPLE | TYPE_ID_LIST => {
                    for slot_bits in seq_vec_ref(slots_ptr).iter().copied() {
                        let slot_obj = obj_from_bits(slot_bits);
                        if obj_eq(_py, attr_obj, slot_obj) {
                            allows_attr = true;
                        }
                        if obj_eq(_py, dict_obj, slot_obj) {
                            allows_dict = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Some(SlotsInfo {
        allows_attr,
        allows_dict,
    })
}

pub(crate) unsafe fn apply_class_slots_layout(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    crate::gil_assert();
    if class_ptr.is_null() {
        return true;
    }
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        return true;
    }
    let dict_bits = class_dict_bits(class_ptr);
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return true;
    };
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return true;
    }
    let slots_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.slots_name, b"__slots__");
    let Some(slots_bits) = dict_get_in_place(_py, dict_ptr, slots_name_bits) else {
        return true;
    };
    let mut slot_names: Vec<u64> = Vec::new();
    let slots_obj = obj_from_bits(slots_bits);
    let Some(slots_ptr) = slots_obj.as_ptr() else {
        raise_exception::<()>(_py, "TypeError", "__slots__ must be a string or iterable");
        return false;
    };
    match object_type_id(slots_ptr) {
        TYPE_ID_STRING => slot_names.push(slots_bits),
        TYPE_ID_TUPLE | TYPE_ID_LIST => {
            for slot_bits in seq_vec_ref(slots_ptr).iter().copied() {
                let slot_obj = obj_from_bits(slot_bits);
                let Some(slot_ptr) = slot_obj.as_ptr() else {
                    raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                    return false;
                };
                if object_type_id(slot_ptr) != TYPE_ID_STRING {
                    raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                    return false;
                }
                slot_names.push(slot_bits);
            }
        }
        _ => {
            let iter_bits = molt_iter(slots_bits);
            if obj_from_bits(iter_bits).is_none() {
                raise_exception::<()>(_py, "TypeError", "__slots__ must be a string or iterable");
                return false;
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                if exception_pending(_py) {
                    return false;
                }
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "__slots__ must be a string or iterable",
                    );
                    return false;
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "__slots__ must be a string or iterable",
                    );
                    return false;
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "__slots__ must be a string or iterable",
                    );
                    return false;
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let slot_bits = elems[0];
                let slot_obj = obj_from_bits(slot_bits);
                let Some(slot_ptr) = slot_obj.as_ptr() else {
                    raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                    return false;
                };
                if object_type_id(slot_ptr) != TYPE_ID_STRING {
                    raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
                    return false;
                }
                slot_names.push(slot_bits);
            }
        }
    }

    let offsets_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.field_offsets_name,
        b"__molt_field_offsets__",
    );
    let layout_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    let dict_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
    let weakref_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.weakref_name,
        b"__weakref__",
    );

    let mut offsets_bits = dict_get_in_place(_py, dict_ptr, offsets_name_bits).unwrap_or(0);
    if obj_from_bits(offsets_bits).is_none() || offsets_bits == 0 {
        let new_ptr = alloc_dict_with_pairs(_py, &[]);
        if new_ptr.is_null() {
            return false;
        }
        offsets_bits = MoltObject::from_ptr(new_ptr).bits();
        dict_set_in_place(_py, dict_ptr, offsets_name_bits, offsets_bits);
    }
    let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
        raise_exception::<()>(_py, "TypeError", "__molt_field_offsets__ must be dict");
        return false;
    };
    if object_type_id(offsets_ptr) != TYPE_ID_DICT {
        raise_exception::<()>(_py, "TypeError", "__molt_field_offsets__ must be dict");
        return false;
    }

    let mut layout_size = 0usize;
    if let Some(size_bits) = dict_get_in_place(_py, dict_ptr, layout_name_bits) {
        if let Some(size) = obj_from_bits(size_bits).as_int() {
            if size > 0 {
                layout_size = size as usize;
            }
        }
    }
    if layout_size == 0 {
        if let Some(size_bits) = class_attr_lookup_raw_mro(_py, class_ptr, layout_name_bits) {
            if let Some(size) = obj_from_bits(size_bits).as_int() {
                if size > 0 {
                    layout_size = size as usize;
                }
            }
        }
    }
    if layout_size == 0 {
        layout_size = 8;
    }

    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let builtins = builtin_classes(_py);
    let reserved_tail = if issubclass_bits(class_bits, builtins.dict) {
        2 * std::mem::size_of::<u64>()
    } else {
        std::mem::size_of::<u64>()
    };
    if layout_size < reserved_tail {
        layout_size = reserved_tail;
    }
    layout_size = layout_size.saturating_sub(reserved_tail);

    let mut updated = false;
    let mro: Cow<'_, [u64]> = if let Some(mro) = class_mro_ref(class_ptr) {
        Cow::Borrowed(mro.as_slice())
    } else {
        Cow::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
    };
    for base_bits in mro.iter().copied().skip(1) {
        let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
            continue;
        };
        if object_type_id(base_ptr) != TYPE_ID_TYPE {
            continue;
        }
        let base_dict_bits = class_dict_bits(base_ptr);
        let Some(base_dict_ptr) = obj_from_bits(base_dict_bits).as_ptr() else {
            continue;
        };
        if object_type_id(base_dict_ptr) != TYPE_ID_DICT {
            continue;
        }
        let Some(base_offsets_bits) = dict_get_in_place(_py, base_dict_ptr, offsets_name_bits)
        else {
            continue;
        };
        let Some(base_offsets_ptr) = obj_from_bits(base_offsets_bits).as_ptr() else {
            continue;
        };
        if object_type_id(base_offsets_ptr) != TYPE_ID_DICT {
            continue;
        }
        let entries = dict_order(base_offsets_ptr).clone();
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let key_bits = pair[0];
            let val_bits = pair[1];
            if dict_get_in_place(_py, offsets_ptr, key_bits).is_some() {
                continue;
            }
            dict_set_in_place(_py, offsets_ptr, key_bits, val_bits);
            if let Some(offset) = obj_from_bits(val_bits).as_int() {
                let end = offset.saturating_add(8) as usize;
                if end > layout_size {
                    layout_size = end;
                }
            }
            updated = true;
        }
    }
    for slot_bits in slot_names {
        let slot_obj = obj_from_bits(slot_bits);
        if obj_eq(_py, slot_obj, obj_from_bits(dict_name_bits))
            || obj_eq(_py, slot_obj, obj_from_bits(weakref_name_bits))
        {
            continue;
        }
        if dict_get_in_place(_py, offsets_ptr, slot_bits).is_some() {
            continue;
        }
        let offset_bits = MoltObject::from_int(layout_size as i64).bits();
        dict_set_in_place(_py, offsets_ptr, slot_bits, offset_bits);
        layout_size += 8;
        updated = true;
    }
    layout_size = layout_size.saturating_add(reserved_tail);
    if updated {
        let size_bits = MoltObject::from_int(layout_size as i64).bits();
        dict_set_in_place(_py, dict_ptr, layout_name_bits, size_bits);
    }
    if let Some(filter) = debug_class_layout_filter() {
        let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
            .unwrap_or_else(|| "<unknown>".to_string());
        if debug_class_layout_match(&class_name) {
            let mut offsets_dump: Vec<String> = Vec::new();
            let entries = dict_order(offsets_ptr).clone();
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                let key_bits = pair[0];
                let val_bits = pair[1];
                let key = string_obj_to_owned(obj_from_bits(key_bits))
                    .unwrap_or_else(|| "<non-str>".to_string());
                let val = obj_from_bits(val_bits).as_int().unwrap_or(-1);
                offsets_dump.push(format!("{key}={val}"));
            }
            offsets_dump.sort();
            eprintln!(
                "molt debug class_layout: {class_name} layout_size={} slots_filter={} offsets=[{}]",
                layout_size,
                filter,
                offsets_dump.join(", ")
            );
        }
    }
    true
}

pub(crate) unsafe fn object_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let class_bits = object_class_bits(obj_ptr);
    let mut cached_attr_bits: Option<u64> = None;
    let mut class_ptr_opt: Option<*mut u8> = None;
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
                class_ptr_opt = Some(class_ptr);
                let class_version = class_layout_version_bits(class_ptr);
                if let Some(entry) = descriptor_cache_lookup(class_bits, attr_bits, class_version) {
                    if let Some(bits) = entry.data_desc_bits {
                        if let Some(bound) = descriptor_bind(_py, bits, class_ptr, Some(obj_ptr)) {
                            return Some(bound);
                        }
                        if exception_pending(_py) {
                            return None;
                        }
                    }
                    cached_attr_bits = entry.class_attr_bits;
                }
                if cached_attr_bits.is_none() {
                    if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
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
                        descriptor_cache_store(class_bits, attr_bits, class_version, None, None);
                    }
                }
                if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                    let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                    if is_missing_bits(_py, bits) {
                        dec_ref_bits(_py, bits);
                        return None;
                    }
                    return Some(bits);
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
        let fallback = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
        inc_ref_bits(_py, fallback);
        return Some(fallback);
    }
    let dict_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
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
        if let Some(class_ptr) = class_ptr_opt {
            if let Some(info) = class_slots_info(_py, class_ptr, attr_bits) {
                if !info.allows_attr {
                    return None;
                }
            }
        }
        return Some(MoltObject::none().bits());
    }
    if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
        if let Some(class_ptr) = class_ptr_opt {
            if let Some(info) = class_slots_info(_py, class_ptr, attr_bits) {
                if !info.allows_dict {
                    return None;
                }
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
    if let (Some(val_bits), Some(class_ptr)) = (cached_attr_bits, class_ptr_opt) {
        if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
            return Some(bound);
        }
        if exception_pending(_py) {
            return None;
        }
    }
    None
}

pub(crate) unsafe fn dataclass_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let desc_ptr = dataclass_desc_ptr(obj_ptr);
    if desc_ptr.is_null() {
        return None;
    }
    let slots = (*desc_ptr).slots;
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
    let class_bits = (*desc_ptr).class_bits;
    if class_bits != 0 {
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
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
                                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
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
                if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                    if let Some(bound) = descriptor_bind(_py, val_bits, class_ptr, Some(obj_ptr)) {
                        return Some(bound);
                    }
                    if exception_pending(_py) {
                        return None;
                    }
                }
            }
        }
    }
    None
}
