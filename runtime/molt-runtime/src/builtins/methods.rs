use crate::builtins::containers::tuple_method_bits;
use crate::PyToken;
use std::sync::atomic::AtomicU64;

use crate::*;

pub(crate) fn builtin_func_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    builtin_func_bits_with_default(_py, slot, fn_ptr, arity, 0)
}

pub(crate) fn builtin_func_bits_with_default(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    default_kind: i64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            if default_kind != 0 {
                let bits = MoltObject::from_int(default_kind).bits();
                unsafe {
                    function_set_dict_bits(ptr, bits);
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

pub(crate) fn builtin_classmethod_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let func_ptr = alloc_function_obj(_py, fn_ptr, arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let cm_ptr = alloc_classmethod_obj(_py, func_bits);
        if cm_ptr.is_null() {
            dec_ref_bits(_py, func_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, func_bits);
        MoltObject::from_ptr(cm_ptr).bits()
    })
}

pub(crate) fn builtin_classmethod_bits_with_default(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
    default_kind: i64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let func_ptr = alloc_function_obj(_py, fn_ptr, arity);
        if func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if default_kind != 0 {
            let bits = MoltObject::from_int(default_kind).bits();
            unsafe {
                function_set_dict_bits(func_ptr, bits);
            }
        }
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        let cm_ptr = alloc_classmethod_obj(_py, func_bits);
        if cm_ptr.is_null() {
            dec_ref_bits(_py, func_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, func_bits);
        MoltObject::from_ptr(cm_ptr).bits()
    })
}

pub(crate) fn missing_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(_py, &runtime_state(_py).special_cache.molt_missing, || {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(_py, total_size, TYPE_ID_OBJECT);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

pub(crate) fn is_missing_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    if bits == missing_bits(_py) {
        return true;
    }
    let Some(ptr) = maybe_ptr_from_bits(bits) else {
        return false;
    };
    unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && object_class_bits(ptr) == 0
            && object_payload_size(ptr) == 0
    }
}

pub(crate) fn not_implemented_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.molt_not_implemented,
        || {
            let total_size = std::mem::size_of::<MoltHeader>();
            let ptr = alloc_object(_py, total_size, TYPE_ID_NOT_IMPLEMENTED);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        },
    )
}

pub(crate) fn is_not_implemented_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    if let Some(ptr) = maybe_ptr_from_bits(bits) {
        unsafe { object_type_id(ptr) == TYPE_ID_NOT_IMPLEMENTED }
    } else {
        false
    }
}

pub(crate) fn ellipsis_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(_py, &runtime_state(_py).special_cache.molt_ellipsis, || {
        let total_size = std::mem::size_of::<MoltHeader>();
        let ptr = alloc_object(_py, total_size, TYPE_ID_ELLIPSIS);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

pub(crate) fn slice_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "indices" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_indices,
            fn_addr!(molt_slice_indices),
            2,
        )),
        "__hash__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_hash,
            fn_addr!(molt_slice_hash),
            1,
        )),
        "__eq__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_eq,
            fn_addr!(molt_slice_eq),
            2,
        )),
        "__reduce__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_reduce,
            fn_addr!(molt_slice_reduce),
            1,
        )),
        "__reduce_ex__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.slice_reduce_ex,
            fn_addr!(molt_slice_reduce_ex),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn string_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_count,
            fn_addr!(molt_string_count_slice),
            6,
        )),
        "startswith" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_startswith,
            fn_addr!(molt_string_startswith_slice),
            6,
        )),
        "endswith" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_endswith,
            fn_addr!(molt_string_endswith_slice),
            6,
        )),
        "find" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_find,
            fn_addr!(molt_string_find_slice),
            6,
        )),
        "rfind" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_rfind,
            fn_addr!(molt_string_rfind_slice),
            6,
        )),
        "format" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_format,
            fn_addr!(molt_string_format_method),
            3,
        )),
        "isidentifier" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isidentifier,
            fn_addr!(molt_string_isidentifier),
            1,
        )),
        "isdigit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_isdigit,
            fn_addr!(molt_string_isdigit),
            1,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_upper,
            fn_addr!(molt_string_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_lower,
            fn_addr!(molt_string_lower),
            1,
        )),
        "capitalize" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_capitalize,
            fn_addr!(molt_string_capitalize),
            1,
        )),
        "strip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.str_strip,
            fn_addr!(molt_string_strip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "lstrip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.str_lstrip,
            fn_addr!(molt_string_lstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "rstrip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.str_rstrip,
            fn_addr!(molt_string_rstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "split" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_split,
            fn_addr!(molt_string_split_max),
            3,
        )),
        "rsplit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_rsplit,
            fn_addr!(molt_string_rsplit_max),
            3,
        )),
        "splitlines" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.str_splitlines,
            fn_addr!(molt_string_splitlines),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "partition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_partition,
            fn_addr!(molt_string_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_rpartition,
            fn_addr!(molt_string_rpartition),
            2,
        )),
        "replace" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.str_replace,
            fn_addr!(molt_string_replace),
            4,
            FUNC_DEFAULT_REPLACE_COUNT,
        )),
        "join" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_join,
            fn_addr!(molt_string_join),
            2,
        )),
        "encode" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.str_encode,
            fn_addr!(molt_string_encode),
            3,
        )),
        _ => None,
    }
}

pub(crate) fn bytes_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "count" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_count,
            fn_addr!(molt_bytes_count_slice),
            6,
        )),
        "find" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_find,
            fn_addr!(molt_bytes_find_slice),
            6,
        )),
        "rfind" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_rfind,
            fn_addr!(molt_bytes_rfind_slice),
            6,
        )),
        "split" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_split,
            fn_addr!(molt_bytes_split_max),
            3,
        )),
        "rsplit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_rsplit,
            fn_addr!(molt_bytes_rsplit_max),
            3,
        )),
        "strip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytes_strip,
            fn_addr!(molt_bytes_strip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "lstrip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytes_lstrip,
            fn_addr!(molt_bytes_lstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "rstrip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytes_rstrip,
            fn_addr!(molt_bytes_rstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "startswith" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_startswith,
            fn_addr!(molt_bytes_startswith_slice),
            6,
        )),
        "endswith" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_endswith,
            fn_addr!(molt_bytes_endswith_slice),
            6,
        )),
        "__reversed__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "splitlines" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytes_splitlines,
            fn_addr!(molt_bytes_splitlines),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "partition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_partition,
            fn_addr!(molt_bytes_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_rpartition,
            fn_addr!(molt_bytes_rpartition),
            2,
        )),
        "replace" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytes_replace,
            fn_addr!(molt_bytes_replace),
            4,
            FUNC_DEFAULT_REPLACE_COUNT,
        )),
        "join" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_join,
            fn_addr!(molt_bytes_join),
            2,
        )),
        "upper" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_upper,
            fn_addr!(molt_bytes_upper),
            1,
        )),
        "lower" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_lower,
            fn_addr!(molt_bytes_lower),
            1,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_hex,
            fn_addr!(molt_bytes_hex),
            3,
        )),
        "decode" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytes_decode,
            fn_addr!(molt_bytes_decode),
            3,
        )),
        _ => None,
    }
}

pub(crate) fn bytearray_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "extend" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_extend,
            fn_addr!(molt_bytearray_extend),
            2,
        )),
        "append" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_append,
            fn_addr!(molt_bytearray_append),
            2,
        )),
        "hex" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_hex,
            fn_addr!(molt_bytearray_hex),
            3,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_clear,
            fn_addr!(molt_bytearray_clear),
            1,
        )),
        "count" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_count,
            fn_addr!(molt_bytearray_count_slice),
            6,
        )),
        "find" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_find,
            fn_addr!(molt_bytearray_find_slice),
            6,
        )),
        "rfind" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_rfind,
            fn_addr!(molt_bytearray_rfind_slice),
            6,
        )),
        "split" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_split,
            fn_addr!(molt_bytearray_split_max),
            3,
        )),
        "rsplit" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_rsplit,
            fn_addr!(molt_bytearray_rsplit_max),
            3,
        )),
        "strip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytearray_strip,
            fn_addr!(molt_bytearray_strip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "lstrip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytearray_lstrip,
            fn_addr!(molt_bytearray_lstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "rstrip" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytearray_rstrip,
            fn_addr!(molt_bytearray_rstrip),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "startswith" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_startswith,
            fn_addr!(molt_bytearray_startswith_slice),
            6,
        )),
        "endswith" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_endswith,
            fn_addr!(molt_bytearray_endswith_slice),
            6,
        )),
        "__reversed__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        "splitlines" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytearray_splitlines,
            fn_addr!(molt_bytearray_splitlines),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "partition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_partition,
            fn_addr!(molt_bytearray_partition),
            2,
        )),
        "rpartition" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_rpartition,
            fn_addr!(molt_bytearray_rpartition),
            2,
        )),
        "replace" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.bytearray_replace,
            fn_addr!(molt_bytearray_replace),
            4,
            FUNC_DEFAULT_REPLACE_COUNT,
        )),
        "decode" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.bytearray_decode,
            fn_addr!(molt_bytearray_decode),
            3,
        )),
        _ => None,
    }
}

pub(crate) fn int_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_new,
            fn_addr!(molt_int_new),
            3,
        )),
        "__int__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_int,
            fn_addr!(molt_int_int),
            1,
        )),
        "__index__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_index,
            fn_addr!(molt_int_index),
            1,
        )),
        "bit_length" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.int_bit_length,
            fn_addr!(molt_int_bit_length),
            1,
        )),
        "to_bytes" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.int_to_bytes,
            fn_addr!(molt_int_to_bytes),
            4,
            FUNC_DEFAULT_ZERO,
        )),
        _ => None,
    }
}

pub(crate) fn int_class_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "from_bytes" => Some(builtin_classmethod_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.int_from_bytes,
            fn_addr!(molt_int_from_bytes),
            4,
            FUNC_DEFAULT_ZERO,
        )),
        _ => None,
    }
}

pub(crate) fn builtin_class_method_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
    name: &str,
) -> Option<u64> {
    let builtins = builtin_classes(_py);
    if name == "__class_getitem__"
        && (class_bits == builtins.list
            || class_bits == builtins.dict
            || class_bits == builtins.tuple
            || class_bits == builtins.set
            || class_bits == builtins.frozenset
            || class_bits == builtins.type_obj)
    {
        return Some(builtin_classmethod_bits(
            _py,
            &runtime_state(_py).method_cache.generic_alias_class_getitem,
            fn_addr!(molt_generic_alias_new),
            2,
        ));
    }
    if class_bits == builtins.object {
        return object_method_bits(_py, name);
    }
    if class_bits == builtins.type_obj {
        return type_method_bits(_py, name);
    }
    if class_bits == builtins.int {
        if let Some(bits) = int_method_bits(_py, name) {
            return Some(bits);
        }
        if let Some(bits) = int_class_method_bits(_py, name) {
            return Some(bits);
        }
    }
    if class_bits == builtins.base_exception || class_bits == builtins.exception {
        return exception_method_bits(_py, name);
    }
    if class_bits == builtins.base_exception_group || class_bits == builtins.exception_group {
        return exception_group_method_bits(_py, name);
    }
    if class_bits == builtins.dict {
        return dict_method_bits(_py, name);
    }
    if class_bits == builtins.tuple {
        return tuple_method_bits(_py, name);
    }
    if class_bits == builtins.list {
        return list_method_bits(_py, name);
    }
    if class_bits == builtins.set {
        return set_method_bits(_py, name);
    }
    if class_bits == builtins.frozenset {
        return frozenset_method_bits(_py, name);
    }
    if class_bits == builtins.str {
        return string_method_bits(_py, name);
    }
    if class_bits == builtins.bytes {
        return bytes_method_bits(_py, name);
    }
    if class_bits == builtins.bytearray {
        return bytearray_method_bits(_py, name);
    }
    if class_bits == builtins.complex {
        return complex_method_bits(_py, name);
    }
    if class_bits == builtins.slice {
        return slice_method_bits(_py, name);
    }
    if class_bits == builtins.memoryview {
        return memoryview_method_bits(_py, name);
    }
    if class_bits == builtins.file
        || class_bits == builtins.file_io
        || class_bits == builtins.buffered_reader
        || class_bits == builtins.buffered_writer
        || class_bits == builtins.buffered_random
        || class_bits == builtins.text_io_wrapper
    {
        return file_method_bits(_py, name);
    }
    None
}

pub(crate) fn complex_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "conjugate" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.complex_conjugate,
            fn_addr!(molt_complex_conjugate),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn type_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__getattribute__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_getattribute,
            fn_addr!(molt_type_getattribute),
            2,
        )),
        "__call__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_call,
            fn_addr!(molt_type_call),
            1,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_new,
            fn_addr!(molt_type_new),
            5,
        )),
        "__init__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_init,
            fn_addr!(molt_type_init),
            5,
        )),
        "__instancecheck__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_instancecheck,
            fn_addr!(molt_type_instancecheck),
            2,
        )),
        "__subclasscheck__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_subclasscheck,
            fn_addr!(molt_type_subclasscheck),
            2,
        )),
        "mro" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.type_mro,
            fn_addr!(molt_type_mro),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn object_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__getattribute__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_getattribute,
            fn_addr!(molt_object_getattribute),
            2,
        )),
        "__new__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_new,
            fn_addr!(molt_object_new_bound),
            1,
        )),
        "__init__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_init,
            fn_addr!(molt_object_init),
            1,
        )),
        "__init_subclass__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_init_subclass,
            fn_addr!(molt_object_init_subclass),
            1,
        )),
        "__setattr__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_setattr,
            fn_addr!(molt_object_setattr),
            3,
        )),
        "__delattr__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_delattr,
            fn_addr!(molt_object_delattr),
            2,
        )),
        "__eq__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_eq,
            fn_addr!(molt_object_eq),
            2,
        )),
        "__ne__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_ne,
            fn_addr!(molt_object_ne),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn memoryview_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "tobytes" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_tobytes,
            fn_addr!(molt_memoryview_tobytes),
            1,
        )),
        "cast" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_cast,
            fn_addr!(molt_memoryview_cast),
            4,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.memoryview_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn file_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial):
    // add remaining file APIs (encoding/errors lookups) once buffer/encoding
    // layers are fully implemented.
    match name {
        "read" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.file_read,
            fn_addr!(molt_file_read),
            2,
            FUNC_DEFAULT_NEG_ONE,
        )),
        "readline" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.file_readline,
            fn_addr!(molt_file_readline),
            2,
            FUNC_DEFAULT_NEG_ONE,
        )),
        "readlines" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.file_readlines,
            fn_addr!(molt_file_readlines),
            2,
            FUNC_DEFAULT_NEG_ONE,
        )),
        "readinto" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readinto,
            fn_addr!(molt_file_readinto),
            2,
        )),
        "readinto1" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readinto1,
            fn_addr!(molt_file_readinto1),
            2,
        )),
        "write" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_write,
            fn_addr!(molt_file_write),
            2,
        )),
        "writelines" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_writelines,
            fn_addr!(molt_file_writelines),
            2,
        )),
        "flush" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_flush,
            fn_addr!(molt_file_flush),
            1,
        )),
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_close,
            fn_addr!(molt_file_close),
            1,
        )),
        "detach" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_detach,
            fn_addr!(molt_file_detach),
            1,
        )),
        "reconfigure" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_reconfigure,
            fn_addr!(molt_file_reconfigure),
            6,
        )),
        "seek" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.file_seek,
            fn_addr!(molt_file_seek),
            3,
            FUNC_DEFAULT_ZERO,
        )),
        "tell" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_tell,
            fn_addr!(molt_file_tell),
            1,
        )),
        "fileno" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_fileno,
            fn_addr!(molt_file_fileno),
            1,
        )),
        "truncate" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.file_truncate,
            fn_addr!(molt_file_truncate),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "readable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_readable,
            fn_addr!(molt_file_readable),
            1,
        )),
        "writable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_writable,
            fn_addr!(molt_file_writable),
            1,
        )),
        "seekable" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_seekable,
            fn_addr!(molt_file_seekable),
            1,
        )),
        "isatty" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_isatty,
            fn_addr!(molt_file_isatty),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_iter,
            fn_addr!(molt_file_iter),
            1,
        )),
        "__next__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_next,
            fn_addr!(molt_file_next),
            1,
        )),
        "__enter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_enter,
            fn_addr!(molt_file_enter),
            1,
        )),
        "__exit__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.file_exit,
            fn_addr!(molt_file_exit_method),
            4,
        )),
        _ => None,
    }
}

pub(crate) fn property_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "getter" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.property_getter,
            fn_addr!(molt_property_getter),
            2,
        )),
        "setter" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.property_setter,
            fn_addr!(molt_property_setter),
            2,
        )),
        "deleter" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.property_deleter,
            fn_addr!(molt_property_deleter),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn generator_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__next__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_next,
            fn_addr!(molt_generator_next_method),
            1,
        )),
        "send" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_send,
            fn_addr!(molt_generator_send_method),
            2,
        )),
        "throw" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_throw,
            fn_addr!(molt_generator_throw_method),
            2,
        )),
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.generator_close,
            fn_addr!(molt_generator_close_method),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn coroutine_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "close" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.coroutine_close,
            fn_addr!(molt_coroutine_close_method),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn asyncgen_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__aiter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_aiter,
            fn_addr!(molt_asyncgen_aiter),
            1,
        )),
        "__anext__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_anext,
            fn_addr!(molt_asyncgen_anext),
            1,
        )),
        "asend" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_asend,
            fn_addr!(molt_asyncgen_asend),
            2,
        )),
        "athrow" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_athrow,
            fn_addr!(molt_asyncgen_athrow),
            2,
        )),
        "aclose" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.asyncgen_aclose,
            fn_addr!(molt_asyncgen_aclose),
            1,
        )),
        _ => None,
    }
}
