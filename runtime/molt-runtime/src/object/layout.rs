use crate::{
    alloc_code_obj, alloc_string, dec_ref_bits, dict_get_in_place, fn_ptr_code_set, inc_ref_bits,
    intern_static_name, obj_from_bits, object_type_id, runtime_state, MoltObject, PyToken,
    TYPE_ID_DICT, TYPE_ID_STRING,
};

pub(crate) unsafe fn seq_vec_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

pub(crate) unsafe fn seq_vec(ptr: *mut u8) -> &'static mut Vec<u64> {
    let vec_ptr = seq_vec_ptr(ptr);
    &mut *vec_ptr
}

pub(crate) unsafe fn seq_vec_ref(ptr: *mut u8) -> &'static Vec<u64> {
    let vec_ptr = seq_vec_ptr(ptr);
    &*vec_ptr
}

pub(crate) unsafe fn bytearray_vec_ptr(ptr: *mut u8) -> *mut Vec<u8> {
    *(ptr as *mut *mut Vec<u8>)
}

pub(crate) unsafe fn bytearray_vec(ptr: *mut u8) -> &'static mut Vec<u8> {
    let vec_ptr = bytearray_vec_ptr(ptr);
    &mut *vec_ptr
}

pub(crate) unsafe fn bytearray_vec_ref(ptr: *mut u8) -> &'static Vec<u8> {
    let vec_ptr = bytearray_vec_ptr(ptr);
    &*vec_ptr
}

pub(crate) unsafe fn bytearray_len(ptr: *mut u8) -> usize {
    bytearray_vec_ref(ptr).len()
}

pub(crate) unsafe fn bytearray_data(ptr: *mut u8) -> *const u8 {
    bytearray_vec_ref(ptr).as_ptr()
}

pub(crate) unsafe fn iter_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn iter_index(ptr: *mut u8) -> usize {
    *(ptr.add(std::mem::size_of::<u64>()) as *const usize)
}

pub(crate) unsafe fn iter_set_index(ptr: *mut u8, idx: usize) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
}

pub(crate) unsafe fn enumerate_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn enumerate_index_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn enumerate_set_index_bits(ptr: *mut u8, idx_bits: u64) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = idx_bits;
}

pub(crate) unsafe fn call_iter_callable_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn call_iter_sentinel_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn reversed_target_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn reversed_index(ptr: *mut u8) -> usize {
    *(ptr.add(std::mem::size_of::<u64>()) as *const usize)
}

pub(crate) unsafe fn reversed_set_index(ptr: *mut u8, idx: usize) {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut usize) = idx;
}

pub(crate) unsafe fn zip_iters_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr as *mut *mut Vec<u64>)
}

pub(crate) unsafe fn map_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn map_iters_ptr(ptr: *mut u8) -> *mut Vec<u64> {
    *(ptr.add(std::mem::size_of::<u64>()) as *mut *mut Vec<u64>)
}

pub(crate) unsafe fn filter_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn filter_iter_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn range_start(ptr: *mut u8) -> i64 {
    *(ptr as *const i64)
}

pub(crate) unsafe fn range_stop(ptr: *mut u8) -> i64 {
    *(ptr.add(std::mem::size_of::<i64>()) as *const i64)
}

pub(crate) unsafe fn range_step(ptr: *mut u8) -> i64 {
    *(ptr.add(2 * std::mem::size_of::<i64>()) as *const i64)
}

pub(crate) unsafe fn slice_start_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn slice_stop_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn slice_step_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn generic_alias_origin_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn generic_alias_args_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_fn_ptr(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_arity(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn function_name_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let dict_bits = function_dict_bits(ptr);
    if dict_bits != 0 {
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let qual_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.qualname_name,
                    b"__qualname__",
                );
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, qual_bits) {
                    return bits;
                }
                let name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.name_name, b"__name__");
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, name_bits) {
                    return bits;
                }
            }
        }
    }
    MoltObject::none().bits()
}

pub(crate) unsafe fn function_set_dict_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn function_closure_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn function_set_closure_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    if bits != 0 {
        inc_ref_bits(_py, bits);
    }
}

pub(crate) unsafe fn function_code_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

#[allow(dead_code)]
pub(crate) unsafe fn function_trampoline_ptr(ptr: *mut u8) -> u64 {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn function_annotations_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn function_set_annotations_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(_py, old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(_py, bits);
    }
}

pub(crate) unsafe fn function_annotate_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(7 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn function_set_annotate_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = ptr.add(7 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(_py, old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(_py, bits);
    }
}

pub(crate) unsafe fn function_set_code_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != bits {
        if old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
        if bits != 0 {
            inc_ref_bits(_py, bits);
        }
        *slot = bits;
    }
    let fn_ptr = function_fn_ptr(ptr);
    fn_ptr_code_set(_py, fn_ptr, bits);
}

pub(crate) unsafe fn function_set_trampoline_ptr(ptr: *mut u8, bits: u64) {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn ensure_function_code_bits(_py: &PyToken<'_>, func_ptr: *mut u8) -> u64 {
    let existing = function_code_bits(func_ptr);
    if existing != 0 {
        return existing;
    }
    let mut name_bits = function_name_bits(_py, func_ptr);
    let mut owned_name = false;
    let name_ok = if let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() {
        object_type_id(name_ptr) == TYPE_ID_STRING
    } else {
        false
    };
    if !name_ok {
        let name_ptr = alloc_string(_py, b"<unknown>");
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        name_bits = MoltObject::from_ptr(name_ptr).bits();
        owned_name = true;
    }
    let filename_ptr = alloc_string(_py, b"<molt-builtin>");
    if filename_ptr.is_null() {
        if owned_name {
            dec_ref_bits(_py, name_bits);
        }
        return MoltObject::none().bits();
    }
    let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
    let code_ptr = alloc_code_obj(_py, filename_bits, name_bits, 0, MoltObject::none().bits());
    dec_ref_bits(_py, filename_bits);
    if owned_name {
        dec_ref_bits(_py, name_bits);
    }
    if code_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let code_bits = MoltObject::from_ptr(code_ptr).bits();
    function_set_code_bits(_py, func_ptr, code_bits);
    code_bits
}

pub(crate) unsafe fn code_filename_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn code_name_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn code_firstlineno(ptr: *mut u8) -> i64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const i64)
}

pub(crate) unsafe fn code_linetable_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn bound_method_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn bound_method_self_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn module_name_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn module_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_name_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn class_dict_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_bases_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_set_bases_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn class_mro_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_set_mro_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(3 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn class_layout_version_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_set_layout_version_bits(ptr: *mut u8, bits: u64) {
    *(ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64) = bits;
}

pub(crate) unsafe fn class_annotations_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(5 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_set_annotations_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(_py, old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(_py, bits);
    }
}

pub(crate) unsafe fn class_annotate_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(6 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn class_set_annotate_bits(_py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
    crate::gil_assert();
    let slot = ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
    let old_bits = *slot;
    if old_bits != 0 {
        dec_ref_bits(_py, old_bits);
    }
    *slot = bits;
    if bits != 0 {
        inc_ref_bits(_py, bits);
    }
}

pub(crate) unsafe fn class_bump_layout_version(ptr: *mut u8) {
    let current = class_layout_version_bits(ptr);
    class_set_layout_version_bits(ptr, current.wrapping_add(1));
}

pub(crate) unsafe fn classmethod_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn staticmethod_func_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn property_get_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn property_set_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn property_del_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) unsafe fn super_type_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

pub(crate) unsafe fn super_obj_bits(ptr: *mut u8) -> u64 {
    *(ptr.add(std::mem::size_of::<u64>()) as *const u64)
}

pub(crate) fn range_len_i64(start: i64, stop: i64, step: i64) -> i64 {
    if step == 0 {
        return 0;
    }
    if step > 0 {
        if start >= stop {
            return 0;
        }
        let span = stop - start - 1;
        return 1 + span / step;
    }
    if start <= stop {
        return 0;
    }
    let step_abs = -step;
    let span = start - stop - 1;
    1 + span / step_abs
}
