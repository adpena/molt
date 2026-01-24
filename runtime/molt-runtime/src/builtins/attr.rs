use crate::PyToken;
use std::cell::RefCell;
use std::collections::HashSet;

use molt_obj_model::MoltObject;

use crate::{
    alloc_dict_with_pairs, alloc_string, attr_lookup_ptr, builtin_class_method_bits,
    call_callable1, call_callable3, call_function_obj1, class_bases_bits, class_bases_vec,
    class_dict_bits, class_layout_version_bits, class_mro_ref, class_mro_vec, class_name_bits,
    class_name_for_error, classmethod_func_bits, dataclass_desc_ptr, dataclass_dict_bits,
    dataclass_set_dict_bits, dec_ref_bits, dict_get_in_place, dict_order, dict_set_in_place,
    exception_pending, inc_ref_bits, instance_dict_bits, instance_set_dict_bits,
    intern_static_name, is_builtin_class_bits, is_truthy, maybe_ptr_from_bits, module_dict_bits,
    molt_bound_method_new, molt_exception_clear, molt_exception_kind, molt_exception_last, obj_eq,
    obj_from_bits, object_class_bits, object_field_get_ptr_raw, object_type_id, property_get_bits,
    raise_exception, runtime_state, staticmethod_func_bits, string_obj_to_owned, type_name,
    type_of_bits, TYPE_ID_CALL_ITER, TYPE_ID_CLASSMETHOD, TYPE_ID_DATACLASS, TYPE_ID_DICT,
    TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW, TYPE_ID_DICT_VALUES_VIEW, TYPE_ID_ENUMERATE,
    TYPE_ID_FILE_HANDLE, TYPE_ID_FILTER, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_ITER,
    TYPE_ID_MAP, TYPE_ID_OBJECT, TYPE_ID_PROPERTY, TYPE_ID_REVERSED, TYPE_ID_STATICMETHOD,
    TYPE_ID_TYPE, TYPE_ID_ZIP,
};

struct AttrNameCacheEntry {
    bytes: Vec<u8>,
    bits: u64,
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

pub(crate) fn attr_error(_py: &PyToken<'_>, type_label: impl AsRef<str>, attr_name: &str) -> i64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    raise_exception(_py, "AttributeError", &msg)
}

pub(crate) fn property_no_setter(_py: &PyToken<'_>, attr_name: &str, class_ptr: *mut u8) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no setter");
    raise_exception(_py, "AttributeError", &msg)
}

pub(crate) fn property_no_deleter(_py: &PyToken<'_>, attr_name: &str, class_ptr: *mut u8) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() || unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE
    {
        "object".to_string()
    } else {
        string_obj_to_owned(obj_from_bits(unsafe { class_name_bits(class_ptr) }))
            .unwrap_or_else(|| "object".to_string())
    };
    let msg = format!("property '{attr_name}' of '{class_name}' object has no deleter");
    raise_exception(_py, "AttributeError", &msg)
}

pub(crate) fn descriptor_no_setter(_py: &PyToken<'_>, attr_name: &str, class_ptr: *mut u8) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    raise_exception(_py, "AttributeError", &msg)
}

pub(crate) fn descriptor_no_deleter(_py: &PyToken<'_>, attr_name: &str, class_ptr: *mut u8) -> i64 {
    crate::gil_assert();
    let class_name = if class_ptr.is_null() {
        "object".to_string()
    } else {
        class_name_for_error(MoltObject::from_ptr(class_ptr).bits())
    };
    let msg = format!("attribute '{attr_name}' of '{class_name}' object is read-only");
    raise_exception(_py, "AttributeError", &msg)
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

pub(crate) fn clear_attribute_error_if_pending(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last();
    let kind_bits = molt_exception_kind(exc_bits);
    let kind = string_obj_to_owned(obj_from_bits(kind_bits));
    dec_ref_bits(_py, kind_bits);
    if kind.as_deref() == Some("AttributeError") {
        molt_exception_clear();
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
        let annotate_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotate_name,
            b"__annotate__",
        );
        let annotate_bits = dict_get_in_place(_py, dict_ptr, annotate_name_bits)
            .unwrap_or_else(|| MoltObject::none().bits());
        let res_bits = if !obj_from_bits(annotate_bits).is_none() {
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
        };
        let complete_name_bits =
            attr_name_bits_from_bytes(_py, b"__molt_module_complete__").unwrap_or(0);
        let mut cache = false;
        if complete_name_bits != 0 {
            if let Some(complete_bits) = dict_get_in_place(_py, dict_ptr, complete_name_bits) {
                cache = is_truthy(obj_from_bits(complete_bits));
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

pub(crate) unsafe fn class_attr_lookup_raw_mro(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
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
    let dict_bits = class_dict_bits(class_ptr);
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    let fields_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.field_offsets_name,
        b"__molt_field_offsets__",
    );
    let offsets_bits = dict_get_in_place(_py, dict_ptr, fields_bits)?;
    let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
    if object_type_id(offsets_ptr) != TYPE_ID_DICT {
        return None;
    }
    let offset_bits = dict_get_in_place(_py, offsets_ptr, attr_bits)?;
    obj_from_bits(offset_bits).as_int().and_then(
        |val| {
            if val >= 0 {
                Some(val as usize)
            } else {
                None
            }
        },
    )
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

pub(crate) unsafe fn object_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    crate::gil_assert();
    let class_bits = object_class_bits(obj_ptr);
    let mut cached_attr_bits: Option<u64> = None;
    let mut class_ptr_opt: Option<*mut u8> = None;
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
        return None;
    }
    let dict_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
    if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
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
