macro_rules! fn_addr {
    ($func:path) => {
        $func as *const () as usize as u64
    };
}

use crate::{
    FUNC_DEFAULT_DICT_POP, FUNC_DEFAULT_DICT_UPDATE, FUNC_DEFAULT_MISSING, FUNC_DEFAULT_NONE,
    MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_DICT_ITEMS_VIEW, TYPE_ID_DICT_KEYS_VIEW,
    TYPE_ID_FROZENSET, TYPE_ID_SET, alloc_tuple, builtin_func_bits, builtin_func_bits_with_default,
    dec_ref_bits, dict_clear_method, dict_copy_method, dict_fromkeys_method, dict_get_method,
    dict_items_method, dict_keys_method, dict_pop_method, dict_popitem_method,
    dict_setdefault_method, dict_update_method, dict_values_method, exception_pending,
    molt_contains, molt_delitem_method, molt_frozenset_copy_method,
    molt_frozenset_difference_multi, molt_frozenset_intersection_multi, molt_frozenset_isdisjoint,
    molt_frozenset_issubset, molt_frozenset_issuperset, molt_frozenset_symmetric_difference,
    molt_frozenset_union_multi, molt_getitem_method, molt_inplace_add, molt_inplace_mul, molt_iter,
    molt_len, molt_list_add_method, molt_list_append, molt_list_clear, molt_list_copy,
    molt_list_count, molt_list_extend, molt_list_index_range, molt_list_init_method,
    molt_list_insert, molt_list_mul_method, molt_list_pop, molt_list_remove, molt_list_reverse,
    molt_list_sort, molt_reversed_builtin, molt_set_add, molt_set_clear, molt_set_copy_method,
    molt_set_difference_multi, molt_set_difference_update_multi, molt_set_discard,
    molt_set_intersection_multi, molt_set_intersection_update_multi, molt_set_isdisjoint,
    molt_set_issubset, molt_set_issuperset, molt_set_new, molt_set_pop, molt_set_remove,
    molt_set_symmetric_difference, molt_set_symmetric_difference_update, molt_set_union_multi,
    molt_set_update_multi, molt_setitem_method, molt_tuple_new_bound, obj_from_bits,
    object_type_id, runtime_state, seq_vec_ref, set_add_in_place,
};

pub(crate) fn is_set_like_type(type_id: u32) -> bool {
    type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET
}

pub(crate) fn is_set_inplace_rhs_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_SET | TYPE_ID_FROZENSET | TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW
    )
}

pub(crate) fn is_set_view_type(type_id: u32) -> bool {
    matches!(type_id, TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW)
}

pub(crate) fn dict_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "keys" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_keys,
            fn_addr!(dict_keys_method),
            1,
        )),
        "values" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_values,
            fn_addr!(dict_values_method),
            1,
        )),
        "items" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_items,
            fn_addr!(dict_items_method),
            1,
        )),
        "get" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.dict_get,
            fn_addr!(dict_get_method),
            3,
            FUNC_DEFAULT_NONE,
        )),
        "pop" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.dict_pop,
            fn_addr!(dict_pop_method),
            4,
            FUNC_DEFAULT_DICT_POP,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_clear,
            fn_addr!(dict_clear_method),
            1,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_copy,
            fn_addr!(dict_copy_method),
            1,
        )),
        "popitem" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_popitem,
            fn_addr!(dict_popitem_method),
            1,
        )),
        "setdefault" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.dict_setdefault,
            fn_addr!(dict_setdefault_method),
            3,
            FUNC_DEFAULT_NONE,
        )),
        "update" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.dict_update,
            fn_addr!(dict_update_method),
            2,
            FUNC_DEFAULT_DICT_UPDATE,
        )),
        "fromkeys" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.dict_fromkeys,
            fn_addr!(dict_fromkeys_method),
            3,
            FUNC_DEFAULT_NONE,
        )),
        "__getitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_getitem,
            fn_addr!(molt_getitem_method),
            2,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "__reversed__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.dict_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn set_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "add" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_add,
            fn_addr!(molt_set_add),
            2,
        )),
        "discard" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_discard,
            fn_addr!(molt_set_discard),
            2,
        )),
        "remove" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_remove,
            fn_addr!(molt_set_remove),
            2,
        )),
        "pop" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_pop,
            fn_addr!(molt_set_pop),
            1,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_clear,
            fn_addr!(molt_set_clear),
            1,
        )),
        "update" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_update,
            fn_addr!(molt_set_update_multi),
            2,
        )),
        "union" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_union,
            fn_addr!(molt_set_union_multi),
            2,
        )),
        "intersection" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_intersection,
            fn_addr!(molt_set_intersection_multi),
            2,
        )),
        "difference" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_difference,
            fn_addr!(molt_set_difference_multi),
            2,
        )),
        "symmetric_difference" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_symdiff,
            fn_addr!(molt_set_symmetric_difference),
            2,
        )),
        "intersection_update" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_intersection_update,
            fn_addr!(molt_set_intersection_update_multi),
            2,
        )),
        "difference_update" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_difference_update,
            fn_addr!(molt_set_difference_update_multi),
            2,
        )),
        "symmetric_difference_update" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_symdiff_update,
            fn_addr!(molt_set_symmetric_difference_update),
            2,
        )),
        "isdisjoint" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_isdisjoint,
            fn_addr!(molt_set_isdisjoint),
            2,
        )),
        "issubset" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_issubset,
            fn_addr!(molt_set_issubset),
            2,
        )),
        "issuperset" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_issuperset,
            fn_addr!(molt_set_issuperset),
            2,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_copy,
            fn_addr!(molt_set_copy_method),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.set_contains,
            fn_addr!(molt_contains),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn frozenset_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "union" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_union,
            fn_addr!(molt_frozenset_union_multi),
            2,
        )),
        "intersection" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_intersection,
            fn_addr!(molt_frozenset_intersection_multi),
            2,
        )),
        "difference" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_difference,
            fn_addr!(molt_frozenset_difference_multi),
            2,
        )),
        "symmetric_difference" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_symdiff,
            fn_addr!(molt_frozenset_symmetric_difference),
            2,
        )),
        "isdisjoint" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_isdisjoint,
            fn_addr!(molt_frozenset_isdisjoint),
            2,
        )),
        "issubset" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_issubset,
            fn_addr!(molt_frozenset_issubset),
            2,
        )),
        "issuperset" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_issuperset,
            fn_addr!(molt_frozenset_issuperset),
            2,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_copy,
            fn_addr!(molt_frozenset_copy_method),
            1,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.frozenset_contains,
            fn_addr!(molt_contains),
            2,
        )),
        _ => None,
    }
}

pub(crate) fn list_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "append" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_append,
            fn_addr!(molt_list_append),
            2,
        )),
        "extend" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_extend,
            fn_addr!(molt_list_extend),
            2,
        )),
        "insert" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_insert,
            fn_addr!(molt_list_insert),
            3,
        )),
        "remove" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_remove,
            fn_addr!(molt_list_remove),
            2,
        )),
        "pop" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.list_pop,
            fn_addr!(molt_list_pop),
            2,
            FUNC_DEFAULT_NONE,
        )),
        "clear" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_clear,
            fn_addr!(molt_list_clear),
            1,
        )),
        "__init__" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.list_init,
            fn_addr!(molt_list_init_method),
            2,
            FUNC_DEFAULT_MISSING,
        )),
        "copy" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_copy,
            fn_addr!(molt_list_copy),
            1,
        )),
        "reverse" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_reverse,
            fn_addr!(molt_list_reverse),
            1,
        )),
        "count" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_count,
            fn_addr!(molt_list_count),
            2,
        )),
        "index" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_index,
            fn_addr!(molt_list_index_range),
            4,
        )),
        "sort" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_sort,
            fn_addr!(molt_list_sort),
            3,
        )),
        "__add__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_add,
            fn_addr!(molt_list_add_method),
            2,
        )),
        "__mul__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_mul,
            fn_addr!(molt_list_mul_method),
            2,
        )),
        "__rmul__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_rmul,
            fn_addr!(molt_list_mul_method),
            2,
        )),
        "__iadd__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_iadd,
            fn_addr!(molt_inplace_add),
            2,
        )),
        "__imul__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_imul,
            fn_addr!(molt_inplace_mul),
            2,
        )),
        "__getitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_getitem,
            fn_addr!(molt_getitem_method),
            2,
        )),
        "__setitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_setitem,
            fn_addr!(molt_setitem_method),
            3,
        )),
        "__delitem__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_delitem,
            fn_addr!(molt_delitem_method),
            2,
        )),
        "__iter__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_iter,
            fn_addr!(molt_iter),
            1,
        )),
        "__len__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_len,
            fn_addr!(molt_len),
            1,
        )),
        "__contains__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_contains,
            fn_addr!(molt_contains),
            2,
        )),
        "__reversed__" => Some(builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.list_reversed,
            fn_addr!(molt_reversed_builtin),
            1,
        )),
        _ => None,
    }
}

pub(crate) fn tuple_method_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    match name {
        "__new__" => Some(builtin_func_bits_with_default(
            _py,
            &runtime_state(_py).method_cache.tuple_new,
            fn_addr!(molt_tuple_new_bound),
            2,
            FUNC_DEFAULT_MISSING,
        )),
        _ => None,
    }
}

pub(crate) unsafe fn list_len(ptr: *mut u8) -> usize {
    unsafe { seq_vec_ref(ptr).len() }
}

pub(crate) unsafe fn tuple_len(ptr: *mut u8) -> usize {
    unsafe { seq_vec_ref(ptr).len() }
}

pub(crate) unsafe fn dict_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

pub(crate) unsafe fn dict_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    unsafe { *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) }
}

pub(crate) unsafe fn dict_order(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = dict_order_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn dict_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    unsafe {
        let vec_ptr = dict_table_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn dict_len(ptr: *mut u8) -> usize {
    unsafe { dict_order(ptr).len() / 2 }
}

pub(crate) unsafe fn set_order_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    unsafe { *(ptr as *mut *mut Vec<u64>) }
}

pub(crate) unsafe fn set_table_ptr(ptr: *mut u8) -> *mut Vec<usize> {
    unsafe { *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) }
}

pub(crate) unsafe fn set_order(ptr: *mut u8) -> &'static mut Vec<u64> {
    unsafe {
        let vec_ptr = set_order_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn set_table(ptr: *mut u8) -> &'static mut Vec<usize> {
    unsafe {
        let vec_ptr = set_table_ptr(ptr);
        &mut *vec_ptr
    }
}

pub(crate) unsafe fn set_len(ptr: *mut u8) -> usize {
    unsafe { set_order(ptr).len() }
}

pub(crate) unsafe fn dict_view_dict_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

pub(crate) unsafe fn dict_view_len(ptr: *mut u8) -> usize {
    unsafe {
        let dict_bits = dict_view_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                return dict_len(dict_ptr);
            }
        }
        0
    }
}

pub(crate) unsafe fn dict_view_entry(ptr: *mut u8, idx: usize) -> Option<(u64, u64)> {
    unsafe {
        let dict_bits = dict_view_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr() {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return None;
            }
            let order = dict_order(dict_ptr);
            let entry = idx * 2;
            if entry + 1 >= order.len() {
                return None;
            }
            return Some((order[entry], order[entry + 1]));
        }
        None
    }
}

pub(crate) unsafe fn dict_view_as_set_bits(
    _py: &PyToken<'_>,
    view_ptr: *mut u8,
    view_type: u32,
) -> Option<u64> {
    unsafe {
        if !is_set_view_type(view_type) {
            return None;
        }
        let len = dict_view_len(view_ptr);
        let set_bits = molt_set_new(len as u64);
        let set_ptr = obj_from_bits(set_bits).as_ptr()?;
        for idx in 0..len {
            if let Some((key_bits, val_bits)) = dict_view_entry(view_ptr, idx) {
                let (entry_bits, needs_drop) = if view_type == TYPE_ID_DICT_ITEMS_VIEW {
                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                    if tuple_ptr.is_null() {
                        dec_ref_bits(_py, set_bits);
                        return None;
                    }
                    (MoltObject::from_ptr(tuple_ptr).bits(), true)
                } else {
                    (key_bits, false)
                };
                set_add_in_place(_py, set_ptr, entry_bits);
                if needs_drop {
                    dec_ref_bits(_py, entry_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, set_bits);
                    return None;
                }
            }
        }
        Some(set_bits)
    }
}
