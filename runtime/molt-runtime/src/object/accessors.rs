use molt_obj_model::MoltObject;

use crate::{
    class_layout_version_bits, dec_ref_bits, header_from_obj_ptr, inc_ref_bits, is_missing_bits,
    obj_from_bits, object_class_bits, object_mark_has_ptrs, object_type_id, profile_hit,
    raise_exception, to_i64, usize_from_bits, PyToken, LAYOUT_GUARD_COUNT, LAYOUT_GUARD_FAIL,
    STRUCT_FIELD_STORE_COUNT, TYPE_ID_OBJECT, TYPE_ID_TYPE,
};

pub(crate) fn resolve_obj_ptr(bits: u64) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        return Some(ptr);
    }
    None
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
pub(crate) unsafe fn object_field_get_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
    }
    let slot = obj_ptr.add(offset) as *const u64;
    let bits = *slot;
    inc_ref_bits(_py, bits);
    bits
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
pub(crate) unsafe fn object_field_set_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
    val_bits: u64,
) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
    }
    profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
    let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
    if new_is_ptr {
        object_mark_has_ptrs(_py, obj_ptr);
    }
    if !old_is_ptr && !new_is_ptr {
        *slot = val_bits;
        return MoltObject::none().bits();
    }
    if old_bits != val_bits {
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, val_bits);
        *slot = val_bits;
    }
    MoltObject::none().bits()
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
/// Intended for initializing freshly allocated objects with immediate values.
pub(crate) unsafe fn object_field_init_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
    val_bits: u64,
) -> u64 {
    if obj_ptr.is_null() {
        return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
    }
    let slot = obj_ptr.add(offset) as *mut u64;
    let old_bits = *slot;
    debug_assert!(
        old_bits == 0 || obj_from_bits(old_bits).as_ptr().is_none(),
        "object_field_init used on slot with pointer contents"
    );
    if obj_from_bits(val_bits).as_ptr().is_some() {
        object_mark_has_ptrs(_py, obj_ptr);
        inc_ref_bits(_py, val_bits);
    }
    *slot = val_bits;
    MoltObject::none().bits()
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_get_ptr(obj_ptr: *mut u8, offset_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let offset = usize_from_bits(offset_bits);
        object_field_get_ptr_raw(_py, obj_ptr, offset)
    })
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_set_ptr(
    obj_ptr: *mut u8,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let offset = usize_from_bits(offset_bits);
        object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits)
    })
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_init_ptr(
    obj_ptr: *mut u8,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let offset = usize_from_bits(offset_bits);
        object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits)
    })
}

unsafe fn guard_layout_match(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
) -> bool {
    profile_hit(_py, &LAYOUT_GUARD_COUNT);
    if obj_ptr.is_null() {
        profile_hit(_py, &LAYOUT_GUARD_FAIL);
        return false;
    }
    let header = header_from_obj_ptr(obj_ptr);
    if (*header).type_id != TYPE_ID_OBJECT {
        profile_hit(_py, &LAYOUT_GUARD_FAIL);
        return false;
    }
    let obj_class_bits = object_class_bits(obj_ptr);
    if obj_class_bits == 0 || obj_class_bits != class_bits {
        profile_hit(_py, &LAYOUT_GUARD_FAIL);
        return false;
    }
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        profile_hit(_py, &LAYOUT_GUARD_FAIL);
        return false;
    };
    if object_type_id(class_ptr) != TYPE_ID_TYPE {
        profile_hit(_py, &LAYOUT_GUARD_FAIL);
        return false;
    }
    let version = class_layout_version_bits(class_ptr);
    let expected = match to_i64(obj_from_bits(expected_version)) {
        Some(val) if val >= 0 => val as u64,
        _ => {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            return false;
        }
    };
    if version != expected {
        profile_hit(_py, &LAYOUT_GUARD_FAIL);
        return false;
    }
    true
}

/// # Safety
/// `obj_ptr` must point to a valid object with a class.
#[no_mangle]
pub unsafe extern "C" fn molt_guard_layout_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(guard_layout_match(
            _py,
            obj_ptr,
            class_bits,
            expected_version,
        ))
        .bits()
    })
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_get_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let offset = usize_from_bits(offset_bits);
        if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
            let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
            if is_missing_bits(_py, bits) {
                dec_ref_bits(_py, bits);
                return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits) as u64;
            }
            return bits;
        }
        crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits) as u64
    })
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_set_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let offset = usize_from_bits(offset_bits);
        if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
            return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
        }
        crate::molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits) as u64
    })
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_field_init_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let offset = usize_from_bits(offset_bits);
        if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
            return object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits);
        }
        crate::molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits) as u64
    })
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_get(obj_bits: u64, offset_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
        };
        let offset = usize_from_bits(offset_bits);
        let slot = obj_ptr.add(offset) as *const u64;
        let bits = *slot;
        inc_ref_bits(_py, bits);
        bits
    })
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_set(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
        };
        let offset = usize_from_bits(offset_bits);
        profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        let slot = obj_ptr.add(offset) as *mut u64;
        let old_bits = *slot;
        let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
        let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
        if new_is_ptr {
            object_mark_has_ptrs(_py, obj_ptr);
        }
        if !old_is_ptr && !new_is_ptr {
            *slot = val_bits;
            return MoltObject::none().bits();
        }
        if old_bits != val_bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, val_bits);
            *slot = val_bits;
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[no_mangle]
pub unsafe extern "C" fn molt_object_field_init(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
        };
        let offset = usize_from_bits(offset_bits);
        let slot = obj_ptr.add(offset) as *mut u64;
        let old_bits = *slot;
        debug_assert!(
            old_bits == 0 || obj_from_bits(old_bits).as_ptr().is_none(),
            "object_field_init used on slot with pointer contents"
        );
        if obj_from_bits(val_bits).as_ptr().is_some() {
            object_mark_has_ptrs(_py, obj_ptr);
        }
        *slot = val_bits;
        MoltObject::none().bits()
    })
}
