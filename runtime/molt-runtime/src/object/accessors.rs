use molt_obj_model::MoltObject;
use std::sync::OnceLock;

use super::field_storage::{self, FieldStorage};
use super::inline_cache::{IC_TABLE_CAPACITY, global_ic_table};
use crate::{
    GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT, GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT, LAYOUT_GUARD_COUNT, LAYOUT_GUARD_FAIL, PyToken,
    STRUCT_FIELD_STORE_COUNT, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_TYPE,
    attr_name_bits_from_bytes, builtin_classes_if_initialized, class_field_offset,
    class_layout_version_bits, dec_ref_bits, dict_get_in_place, dict_set_in_place,
    exception_pending, header_from_obj_ptr, inc_ref_bits, instance_dict_bits, is_missing_bits,
    obj_from_bits, object_class_bits, object_is_exact_builtin_dict, object_mark_has_ptrs,
    object_payload_size, object_type_id, profile_hit, raise_exception, to_i64, usize_from_bits,
};

fn debug_field_bounds_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_DEBUG_FIELD_BOUNDS").ok().as_deref(),
            Some("1")
        )
    })
}

fn debug_field_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_FIELD").is_ok())
}

fn debug_guard_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_GUARD").is_ok())
}

#[inline(always)]
unsafe fn attr_ic_class_key(obj_ptr: *mut u8) -> Option<(u64, *mut u8, u64)> {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if !super::heap_kind_has_class_shape(type_id) && type_id != TYPE_ID_DATACLASS {
            return None;
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits == 0 {
            return None;
        }
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        Some((class_bits, class_ptr, class_layout_version_bits(class_ptr)))
    }
}

pub(crate) fn resolve_obj_ptr(bits: u64) -> Option<*mut u8> {
    if let Some(ptr) = obj_from_bits(bits).as_ptr() {
        return Some(ptr);
    }
    None
}

#[inline]
unsafe fn object_field_slot_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
) -> Option<*mut u64> {
    unsafe {
        if obj_ptr.is_null() {
            return None;
        }
        if object_type_id(obj_ptr) == TYPE_ID_DATACLASS {
            let fields = super::dataclass_fields_ptr(obj_ptr);
            let index = offset / std::mem::size_of::<u64>();
            if offset % std::mem::size_of::<u64>() != 0
                || fields.is_null()
                || index >= (*fields).len()
            {
                raise_exception::<()>(_py, "TypeError", "dataclass field index out of range");
                return None;
            }
            return Some((*fields).as_mut_ptr().add(index));
        }
        if debug_field_bounds_enabled()
            && offset.saturating_add(std::mem::size_of::<u64>()) > object_payload_size(obj_ptr)
        {
            raise_exception::<()>(_py, "RuntimeError", "object field offset out of range");
            return None;
        }
        Some(obj_ptr.add(offset).cast())
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
pub(crate) unsafe fn object_field_get_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
) -> u64 {
    unsafe {
        let Some(slot) = object_field_slot_ptr(_py, obj_ptr, offset) else {
            return MoltObject::none().bits();
        };
        let bits = match field_storage::resolve(_py, obj_ptr, offset, slot) {
            Some(FieldStorage::Inline(slot)) => *slot,
            Some(FieldStorage::Dictionary { dictionary, name }) => {
                // Dictionary equality may call Python. Keep its backing owner
                // alive even when a callback replaces the instance dictionary.
                inc_ref_bits(_py, dictionary);
                inc_ref_bits(_py, name);
                let dict = obj_from_bits(dictionary).as_ptr().unwrap();
                let bits =
                    dict_get_in_place(_py, dict, name).unwrap_or_else(|| crate::missing_bits(_py));
                inc_ref_bits(_py, bits);
                dec_ref_bits(_py, name);
                dec_ref_bits(_py, dictionary);
                return bits;
            }
            None => return MoltObject::none().bits(),
        };
        if debug_field_enabled() {
            eprintln!(
                "[field_get_raw] ptr=0x{:x} offset={} slot=0x{:x} bits=0x{:x}",
                obj_ptr as usize, offset, slot as usize, bits
            );
        }
        inc_ref_bits(_py, bits);
        bits
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
pub(crate) unsafe fn object_field_set_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
    val_bits: u64,
) -> u64 {
    unsafe {
        let Some(slot) = object_field_slot_ptr(_py, obj_ptr, offset) else {
            return MoltObject::none().bits();
        };
        profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        match field_storage::resolve(_py, obj_ptr, offset, slot) {
            Some(FieldStorage::Inline(_)) => {}
            Some(FieldStorage::Dictionary { dictionary, name }) => {
                inc_ref_bits(_py, dictionary);
                inc_ref_bits(_py, name);
                dict_set_in_place(
                    _py,
                    obj_from_bits(dictionary).as_ptr().unwrap(),
                    name,
                    val_bits,
                );
                dec_ref_bits(_py, name);
                dec_ref_bits(_py, dictionary);
                return MoltObject::none().bits();
            }
            None => return MoltObject::none().bits(),
        }
        let old_bits = *slot;
        if debug_field_enabled() {
            eprintln!(
                "[field_set_raw] ptr=0x{:x} offset={} slot=0x{:x} old=0x{:x} val=0x{:x}",
                obj_ptr as usize, offset, slot as usize, old_bits, val_bits
            );
        }
        let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
        let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
        if new_is_ptr {
            object_mark_has_ptrs(_py, obj_ptr);
        }
        let changed = old_bits != val_bits;
        if changed {
            // Heap field assignment must retain the incoming value before the
            // displaced value is released. The old release can cascade through
            // arbitrary object graphs, so the new value must already be owned
            // by the field before that cascade runs.
            if new_is_ptr {
                inc_ref_bits(_py, val_bits);
            }
            *slot = val_bits;
        }
        if changed && old_is_ptr {
            dec_ref_bits(_py, old_bits);
        }
        MoltObject::none().bits()
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
/// The slot must not own a mortal heap value or belong to an observed object.
/// Fresh class fields contain the immortal missing singleton, never float zero.
/// The field retains an incoming heap value just like ordinary assignment.
pub(crate) unsafe fn object_field_init_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
    val_bits: u64,
) -> u64 {
    unsafe {
        if !obj_ptr.is_null() && object_class_bits(obj_ptr) != 0 && instance_dict_bits(obj_ptr) != 0
        {
            return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
        }
        let Some(slot) = object_field_slot_ptr(_py, obj_ptr, offset) else {
            return MoltObject::none().bits();
        };
        let old_bits = *slot;
        debug_assert!(
            obj_from_bits(old_bits).as_ptr().is_none() || is_missing_bits(_py, old_bits),
            "object_field_init used on slot with pointer contents"
        );
        if obj_from_bits(val_bits).as_ptr().is_some() {
            object_mark_has_ptrs(_py, obj_ptr);
            inc_ref_bits(_py, val_bits);
        }
        *slot = val_bits;
        MoltObject::none().bits()
    }
}

/// Instance precedence shared by both generic attribute lookup implementations.
/// An offset probe already consults the authoritative dictionary, so a miss is
/// never retried there (key equality can have observable callbacks).
pub(crate) unsafe fn instance_attribute_lookup(
    py: &PyToken<'_>,
    object: *mut u8,
    name: u64,
    offset: Option<usize>,
) -> Option<u64> {
    unsafe {
        if let Some(offset) = offset {
            let bits = object_field_get_ptr_raw(py, object, offset);
            if is_missing_bits(py, bits) || exception_pending(py) {
                dec_ref_bits(py, bits);
                return None;
            }
            return Some(bits);
        }
        let dictionary = field_storage::current_dictionary(py, object).ok()??;
        inc_ref_bits(py, dictionary);
        let value = dict_get_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), name);
        if let Some(value) = value {
            inc_ref_bits(py, value);
        }
        dec_ref_bits(py, dictionary);
        if exception_pending(py) {
            if let Some(value) = value {
                dec_ref_bits(py, value);
            }
            return None;
        }
        value
    }
}

pub(crate) unsafe fn instance_attribute_delete(
    py: &PyToken<'_>,
    object: *mut u8,
    name: u64,
) -> bool {
    unsafe {
        let Ok(Some(dictionary)) = field_storage::current_dictionary(py, object) else {
            return false;
        };
        inc_ref_bits(py, dictionary);
        let deleted =
            crate::dict_del_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), name);
        dec_ref_bits(py, dictionary);
        deleted
    }
}

/// A public typed load still follows class fallback when the dictionary entry
/// was deleted. Generic lookup uses the raw missing-sentinel probe above, so
/// this named fallback cannot recurse into another public typed load.
unsafe fn object_field_load_ptr_raw(py: &PyToken<'_>, object: *mut u8, offset: usize) -> u64 {
    unsafe {
        // A dictionary miss can execute key equality. Enter named resolution
        // directly instead of probing and repeating that observable lookup.
        if !object.is_null()
            && object_class_bits(object) != 0
            && instance_dict_bits(object) != 0
            && let Some(field) = field_storage::field_at_offset(py, object, offset)
        {
            inc_ref_bits(py, field.name);
            let value = crate::molt_get_attr_name(MoltObject::from_ptr(object).bits(), field.name);
            dec_ref_bits(py, field.name);
            return value;
        }
        let value = object_field_get_ptr_raw(py, object, offset);
        if !is_missing_bits(py, value) {
            return value;
        }
        dec_ref_bits(py, value);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if let Some(field) = field_storage::field_at_offset(py, object, offset) {
            inc_ref_bits(py, field.name);
            let value = crate::molt_get_attr_name(MoltObject::from_ptr(object).bits(), field.name);
            dec_ref_bits(py, field.name);
            return value;
        }
        raise_exception::<u64>(py, "AttributeError", "object field is uninitialized")
    }
}

/// Delete only the selected storage owner. A same-name dictionary key must not
/// be removed when deleting a genuine slot, and vice versa.
pub(crate) unsafe fn object_field_delete_ptr_raw(
    py: &PyToken<'_>,
    object: *mut u8,
    offset: usize,
) -> bool {
    unsafe {
        let Some(slot) = object_field_slot_ptr(py, object, offset) else {
            return false;
        };
        match field_storage::resolve(py, object, offset, slot) {
            Some(FieldStorage::Inline(slot)) => {
                let old = *slot;
                if is_missing_bits(py, old) {
                    return false;
                }
                let missing = crate::missing_bits(py);
                inc_ref_bits(py, missing);
                object_mark_has_ptrs(py, object);
                *slot = missing;
                dec_ref_bits(py, old);
                true
            }
            Some(FieldStorage::Dictionary { dictionary, name }) => {
                inc_ref_bits(py, dictionary);
                inc_ref_bits(py, name);
                let deleted =
                    crate::dict_del_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), name);
                dec_ref_bits(py, name);
                dec_ref_bits(py, dictionary);
                deleted
            }
            None => false,
        }
    }
}

/// # Safety
/// `obj_ptr_bits` must encode a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_get_ptr(obj_ptr_bits: u64, offset_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr =
                crate::provenance::abi::mut_ptr::<u8>(obj_ptr_bits).unwrap_or(std::ptr::null_mut());
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            object_field_load_ptr_raw(_py, obj_ptr, offset)
        })
    }
}

/// # Safety
/// `obj_ptr_bits` must encode a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_set_ptr(
    obj_ptr_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr =
                crate::provenance::abi::mut_ptr::<u8>(obj_ptr_bits).unwrap_or(std::ptr::null_mut());
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits)
        })
    }
}

/// # Safety
/// `obj_ptr_bits` must encode a valid object with enough payload for `offset_bits`.
/// The slot must not own a heap value or belong to an already observed object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_init_ptr(
    obj_ptr_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr =
                crate::provenance::abi::mut_ptr::<u8>(obj_ptr_bits).unwrap_or(std::ptr::null_mut());
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits)
        })
    }
}

unsafe fn guard_layout_match(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
) -> bool {
    unsafe {
        profile_hit(_py, &LAYOUT_GUARD_COUNT);
        if obj_ptr.is_null() {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT);
            return false;
        }
        if debug_guard_enabled() {
            let header = header_from_obj_ptr(obj_ptr);
            let tid = (*header).type_id;
            let ocb = object_class_bits(obj_ptr);
            eprintln!(
                "[guard] ptr=0x{:x} type_id={} obj_class_bits=0x{:x} expected_class=0x{:x}",
                obj_ptr as usize, tid, ocb, class_bits
            );
        }
        let header = header_from_obj_ptr(obj_ptr);
        let expected = match to_i64(obj_from_bits(expected_version)) {
            Some(val) if val >= 0 => val as u64,
            _ => {
                profile_hit(_py, &LAYOUT_GUARD_FAIL);
                profile_hit(
                    _py,
                    &GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT,
                );
                return false;
            }
        };
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT);
            return false;
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT);
            return false;
        }
        if (*header).type_id == TYPE_ID_DICT {
            let expected_is_builtin_dict = builtin_classes_if_initialized(_py)
                .is_some_and(|builtins| class_bits == builtins.dict);
            if !expected_is_builtin_dict || !object_is_exact_builtin_dict(_py, obj_ptr) {
                profile_hit(_py, &LAYOUT_GUARD_FAIL);
                profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT);
                return false;
            }
            let version = class_layout_version_bits(class_ptr);
            if version != expected {
                profile_hit(_py, &LAYOUT_GUARD_FAIL);
                profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT);
                return false;
            }
            return true;
        }
        if !super::heap_kind_has_class_shape((*header).type_id) {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT);
            return false;
        }
        let obj_class_bits = object_class_bits(obj_ptr);
        if obj_class_bits == 0 || obj_class_bits != class_bits {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT);
            return false;
        }
        let version = class_layout_version_bits(class_ptr);
        if version != expected {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT);
            return false;
        }
        true
    }
}

/// # Safety
/// `obj_bits` and `class_bits` are tagged runtime values. Non-object or
/// non-class candidates are safe guard misses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guard_layout(
    obj_bits: u64,
    class_bits: u64,
    expected_version: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = resolve_obj_ptr(obj_bits).unwrap_or(std::ptr::null_mut());
            let matches = guard_layout_match(_py, obj_ptr, class_bits, expected_version);
            if !matches {
                profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT);
            }
            MoltObject::from_bool(matches).bits()
        })
    }
}

/// Returns a tagged value, using the generic attribute ABI on a guard miss.
/// Failure returns tagged None with an exception pending, not raw zero (which
/// is a valid floating-point value). Consumers must check exception state.
///
/// # Safety
/// `obj_bits` is a tagged runtime value. `attr_name_ptr_bits` must encode valid
/// UTF-8 bytes. A matching object must have enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_field_get(
    obj_bits: u64,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    attr_name_ptr_bits: u64,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_ptr) = crate::provenance::abi::const_ptr::<u8>(attr_name_ptr_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            if let Some(obj_ptr) = resolve_obj_ptr(obj_bits)
                && guard_layout_match(_py, obj_ptr, class_bits, expected_version)
            {
                if instance_dict_bits(obj_ptr) != 0 {
                    return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits);
                }
                let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                if is_missing_bits(_py, bits) {
                    dec_ref_bits(_py, bits);
                    return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits);
                }
                return bits;
            }
            crate::molt_get_attr_object(obj_bits, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// Returns tagged None on success or failure, using the generic attribute ABI
/// on a guard miss. Exception state, never the return bits, distinguishes them.
///
/// # Safety
/// `obj_bits` is a tagged runtime value. `attr_name_ptr_bits` must encode valid
/// UTF-8 bytes. A matching object must have enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_field_set(
    obj_bits: u64,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr_bits: u64,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_ptr) = crate::provenance::abi::const_ptr::<u8>(attr_name_ptr_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            if let Some(obj_ptr) = resolve_obj_ptr(obj_bits)
                && guard_layout_match(_py, obj_ptr, class_bits, expected_version)
            {
                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
            }
            crate::molt_set_attr_object(obj_bits, attr_name_ptr, attr_name_len_bits, val_bits)
        })
    }
}

/// # Safety
/// `obj_ptr_bits` must encode a valid object with enough payload for
/// `offset_bits`; `attr_name_ptr_bits` must encode valid UTF-8 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_field_init_ptr(
    obj_ptr_bits: u64,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr_bits: u64,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(obj_ptr) = crate::provenance::abi::mut_ptr::<u8>(obj_ptr_bits) else {
                return MoltObject::none().bits();
            };
            let Some(attr_name_ptr) = crate::provenance::abi::const_ptr::<u8>(attr_name_ptr_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
                return object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits);
            }
            crate::molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits)
        })
    }
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_get(obj_bits: u64, offset_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
            };
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            object_field_load_ptr_raw(_py, obj_ptr, offset)
        })
    }
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_set(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
            };
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits)
        })
    }
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
/// The slot must not own a heap value or belong to an already observed object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_init(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
                return raise_exception::<_>(_py, "TypeError", "object field access on non-object");
            };
            let Some(offset) = usize_from_bits(offset_bits) else {
                return MoltObject::none().bits();
            };
            object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits)
        })
    }
}

// ---------------------------------------------------------------------------
// IC-accelerated attribute access
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// GIL-free inline-cache probe for the native backend's split-phase IC.
//
// The native backend emits:
//   fast_result = molt_ic_probe_fast(obj_ptr, ic_index)
//   if fast_result != 0:
//       result = fast_result   // IC hit — no function-call overhead for getattr
//   else:
//       result = molt_getattr_ic_slow(obj_ptr, attr_name_ptr, attr_len, ic_index)
//
// This function performs *only* the IC probe and the slot read.  It does NOT
// acquire the GIL because:
//   - The IC fields are atomics with relaxed ordering (safe without GIL).
//   - The object header and payload are immutable during single-threaded
//     execution (the GIL is held by the caller at the compiled-code level).
//   - The refcount bump is a relaxed atomic add.
//
// Returns the NaN-boxed slot value on hit (with refcount incremented), or 0
// on any miss.
// ---------------------------------------------------------------------------

/// # Safety
/// `obj_ptr` must point to a valid molt object (or be null, which returns 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_ic_probe_fast(obj_ptr: *mut u8, ic_index: u64) -> u64 {
    unsafe {
        if obj_ptr.is_null() {
            return 0;
        }

        // Materialization retires inferred inline words. The slow path knows
        // whether this offset is a real slot or dictionary-backed attribute.
        if instance_dict_bits(obj_ptr) != 0 {
            return 0;
        }

        let Some((class_bits, _class_ptr, class_version)) = attr_ic_class_key(obj_ptr) else {
            return 0;
        };

        let Some(idx) = usize_from_bits(ic_index) else {
            return 0;
        };
        if idx >= IC_TABLE_CAPACITY {
            return 0;
        }

        let ic = global_ic_table().get(idx);

        if let Some(cached_offset) = ic.probe(class_bits, class_version) {
            let offset = cached_offset as usize;
            let payload = object_payload_size(obj_ptr);
            if offset.saturating_add(std::mem::size_of::<u64>()) <= payload {
                let slot = obj_ptr.add(offset) as *const u64;
                let bits = *slot;
                // Skip uninitialised / missing sentinel slots.
                if bits != 0 {
                    // Check for the "missing" sentinel — canonical NaN-boxed None.
                    let none_bits = molt_obj_model::MoltObject::none().bits();
                    if bits != none_bits {
                        // Bump refcount — safe as a relaxed atomic even without GIL.
                        let ptr = obj_from_bits(bits).as_ptr();
                        if let Some(p) = ptr {
                            let header = p.sub(std::mem::size_of::<super::MoltHeader>())
                                as *mut super::MoltHeader;
                            let flags = (*header).load_synchronized_flags();
                            (*header).retain_owned_mirrored(bits, 1, "molt_ic_probe_fast", flags);
                        }
                        return bits;
                    }
                }
            }
        }

        0 // miss
    }
}

/// IC slow path: full attribute resolution with GIL, populates the IC on success.
///
/// This is the complement to `molt_ic_probe_fast`.  The caller already did the
/// IC probe and got a miss, so this function skips the probe and goes straight
/// to full attribute resolution.  On a successful lookup it populates the IC
/// entry so subsequent calls hit the fast path.
///
/// # Safety
/// Same preconditions as `molt_getattr_ic`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_getattr_ic_slow(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    ic_index: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if obj_ptr.is_null() {
                return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits);
            }

            let idx = usize_from_bits(ic_index).unwrap_or(IC_TABLE_CAPACITY);

            // Full resolution.
            let result = crate::molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits);

            // Populate the IC on success.
            if idx < IC_TABLE_CAPACITY
                && result != 0
                && !obj_from_bits(result).is_none()
                && !exception_pending(_py)
                && let Some((class_bits, class_ptr, class_version)) = attr_ic_class_key(obj_ptr)
                && let Some(attr_len) = usize_from_bits(attr_name_len_bits)
            {
                let slice = std::slice::from_raw_parts(attr_name_ptr, attr_len);
                if let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) {
                    if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
                        && offset <= u32::MAX as usize
                    {
                        let ic = global_ic_table().get(idx);
                        ic.update(class_bits, offset as u32, class_version);
                    }
                    dec_ref_bits(_py, attr_bits);
                }
            }

            result
        })
    }
}

/// Runtime helper for field load — called from Cranelift codegen.
/// Reads a NaN-boxed value at `obj_ptr + offset` and inc-refs it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_load(obj_ptr: *mut u8, offset: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(offset) = usize_from_bits(offset) else {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "object field offset exceeds the active address space",
            );
        };
        unsafe { object_field_load_ptr_raw(_py, obj_ptr, offset) }
    })
}

#[cfg(test)]
#[path = "accessors_tests.rs"]
mod tests;
