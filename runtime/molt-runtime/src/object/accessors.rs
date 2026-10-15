#[cfg(test)]
use crate::object_payload_size;
use molt_obj_model::MoltObject;
use std::sync::OnceLock;

use super::field_storage::{self, FieldStorage};
use super::layout::{ClassDefinition, class_definition};
use crate::{
    GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT, GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT, LAYOUT_GUARD_COUNT, LAYOUT_GUARD_FAIL, PyToken,
    STRUCT_FIELD_STORE_COUNT, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_TYPE,
    class_layout_version_bits, dec_ref_bits, dict_get_in_place, dict_set_in_place, inc_ref_bits,
    instance_dict_bits, is_missing_bits, obj_from_bits, object_class_bits, object_mark_has_ptrs,
    object_type_id, profile_hit, raise_exception, to_i64, usize_from_bits,
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

fn debug_guard_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_DEBUG_GUARD").is_ok())
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
            if !offset.is_multiple_of(std::mem::size_of::<u64>())
                || fields.is_null()
                || index >= (*fields).len()
            {
                raise_exception::<()>(_py, "TypeError", "dataclass field index out of range");
                return None;
            }
            return Some((*fields).as_mut_ptr().add(index));
        }
        if (debug_field_bounds_enabled() || super::native_instance::has_fields(obj_ptr))
            && offset
                .checked_add(std::mem::size_of::<u64>())
                .is_none_or(|end| {
                    end > super::native_instance::field_payload_size(obj_ptr)
                        .saturating_sub(std::mem::size_of::<u64>())
                })
        {
            raise_exception::<()>(_py, "RuntimeError", "object field offset out of range");
            return None;
        }
        Some(
            super::native_instance::field_base(obj_ptr)
                .add(offset)
                .cast(),
        )
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
pub(crate) unsafe fn object_field_get_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
) -> Result<u64, ()> {
    unsafe {
        let Some(slot) = object_field_slot_ptr(_py, obj_ptr, offset) else {
            return Err(());
        };
        let bits = match field_storage::resolve(_py, obj_ptr, offset, slot) {
            Some(FieldStorage::Inline(slot)) => *slot,
            Some(FieldStorage::Dictionary { dictionary, name }) => {
                // Dictionary equality may call Python. Keep its backing owner
                // alive even when a callback replaces the instance dictionary.
                inc_ref_bits(_py, dictionary);
                inc_ref_bits(_py, name);
                let dict = obj_from_bits(dictionary).as_ptr().unwrap();
                let found = dict_get_in_place(_py, dict, name);
                field_storage::debug::dictionary(
                    _py,
                    "field_get",
                    obj_ptr,
                    dictionary,
                    Some(name),
                    found.as_ref().ok().copied().flatten(),
                );
                let result = found.map(|found| {
                    let bits = found.unwrap_or_else(|| crate::missing_bits(_py));
                    inc_ref_bits(_py, bits);
                    bits
                });
                dec_ref_bits(_py, name);
                dec_ref_bits(_py, dictionary);
                return result;
            }
            None => return Err(()),
        };
        if field_storage::debug::field_enabled(_py, obj_ptr, offset) {
            eprintln!(
                "[field_get_raw] ptr=0x{:x} offset={} slot=0x{:x} bits=0x{:x}",
                obj_ptr as usize, offset, slot as usize, bits
            );
        }
        inc_ref_bits(_py, bits);
        Ok(bits)
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset`.
pub(crate) unsafe fn object_field_set_ptr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    offset: usize,
    val_bits: u64,
) -> Result<(), ()> {
    unsafe {
        let Some(slot) = object_field_slot_ptr(_py, obj_ptr, offset) else {
            return Err(());
        };
        profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        match field_storage::resolve(_py, obj_ptr, offset, slot) {
            Some(FieldStorage::Inline(_)) => {}
            Some(FieldStorage::Dictionary { dictionary, name }) => {
                inc_ref_bits(_py, dictionary);
                inc_ref_bits(_py, name);
                field_storage::debug::dictionary(
                    _py,
                    "field_set_before",
                    obj_ptr,
                    dictionary,
                    Some(name),
                    Some(val_bits),
                );
                let completed = dict_set_in_place(
                    _py,
                    obj_from_bits(dictionary).as_ptr().unwrap(),
                    name,
                    val_bits,
                );
                if !completed {
                    dec_ref_bits(_py, name);
                    dec_ref_bits(_py, dictionary);
                    return Err(());
                }
                field_storage::debug::dictionary(
                    _py,
                    "field_set_after",
                    obj_ptr,
                    dictionary,
                    Some(name),
                    Some(val_bits),
                );
                dec_ref_bits(_py, name);
                dec_ref_bits(_py, dictionary);
                return Ok(());
            }
            None => return Err(()),
        }
        let old_bits = *slot;
        if field_storage::debug::field_enabled(_py, obj_ptr, offset) {
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
        Ok(())
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
) -> Result<(), ()> {
    unsafe {
        if !obj_ptr.is_null() && object_class_bits(obj_ptr) != 0 && instance_dict_bits(obj_ptr) != 0
        {
            return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
        }
        let Some(slot) = object_field_slot_ptr(_py, obj_ptr, offset) else {
            return Err(());
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
        if field_storage::debug::field_enabled(_py, obj_ptr, offset) {
            eprintln!(
                "[field_init_raw] ptr=0x{:x} offset={offset} slot=0x{:x} old=0x{old_bits:x} val=0x{val_bits:x}",
                obj_ptr as usize, slot as usize
            );
        }
        Ok(())
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
) -> Result<Option<u64>, ()> {
    unsafe { instance_attribute_lookup_with_policy(py, object, name, offset, false) }
}

pub(crate) unsafe fn instance_attribute_lookup_with_policy(
    py: &PyToken<'_>,
    object: *mut u8,
    name: u64,
    offset: Option<usize>,
    suppress: bool,
) -> Result<Option<u64>, ()> {
    unsafe {
        field_storage::debug::lookup(py, object, name, offset);
        if let Some(offset) = offset {
            let result = object_field_get_ptr_raw(py, object, offset);
            return match result {
                Ok(bits) if is_missing_bits(py, bits) => {
                    dec_ref_bits(py, bits);
                    Ok(None)
                }
                Ok(bits) => Ok(Some(bits)),
                Err(())
                    if suppress && crate::builtins::attr::clear_attribute_error_if_pending(py) =>
                {
                    Ok(None)
                }
                Err(()) => Err(()),
            };
        }
        let Some(dictionary) = field_storage::current_dictionary(py, object)? else {
            return Ok(None);
        };
        field_storage::debug::dictionary(py, "instance_get", object, dictionary, Some(name), None);
        attribute_dictionary_lookup(py, dictionary, name, suppress)
    }
}

/// One dictionary probe for the instance-precedence tier. Generic C lookup may
/// replace the receiver dictionary and suppress AttributeError from key equality
/// before continuing to the class descriptor tier. Other failures stay pending.
pub(crate) unsafe fn attribute_dictionary_lookup(
    py: &PyToken<'_>,
    dictionary: u64,
    name: u64,
    suppress: bool,
) -> Result<Option<u64>, ()> {
    unsafe {
        let Some(pointer) = obj_from_bits(dictionary)
            .as_ptr()
            .filter(|pointer| object_type_id(*pointer) == TYPE_ID_DICT)
        else {
            raise_exception::<u64>(py, "SystemError", "bad argument to internal function");
            return Err(());
        };
        inc_ref_bits(py, dictionary);
        let value = dict_get_in_place(py, pointer, name);
        field_storage::debug::dictionary(
            py,
            "attribute_get",
            std::ptr::null_mut(),
            dictionary,
            Some(name),
            value.as_ref().ok().copied().flatten(),
        );
        if let Ok(Some(value)) = value {
            inc_ref_bits(py, value);
        }
        dec_ref_bits(py, dictionary);
        match value {
            Err(()) if suppress && crate::builtins::attr::clear_attribute_error_if_pending(py) => {
                Ok(None)
            }
            result => result,
        }
    }
}

pub(crate) unsafe fn instance_attribute_delete(
    py: &PyToken<'_>,
    object: *mut u8,
    name: u64,
) -> Result<bool, ()> {
    unsafe {
        let Some(dictionary) = field_storage::current_dictionary(py, object)? else {
            return Ok(false);
        };
        inc_ref_bits(py, dictionary);
        field_storage::debug::dictionary(
            py,
            "instance_delete",
            object,
            dictionary,
            Some(name),
            None,
        );
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
        let value = match object_field_get_ptr_raw(py, object, offset) {
            Ok(value) => value,
            Err(()) => return MoltObject::none().bits(),
        };
        if !is_missing_bits(py, value) {
            return value;
        }
        dec_ref_bits(py, value);
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
) -> Result<bool, ()> {
    unsafe {
        let Some(slot) = object_field_slot_ptr(py, object, offset) else {
            return Err(());
        };
        match field_storage::resolve(py, object, offset, slot) {
            Some(FieldStorage::Inline(slot)) => {
                let old = *slot;
                if field_storage::debug::field_enabled(py, object, offset) {
                    eprintln!(
                        "[field_delete_raw] ptr=0x{:x} offset={offset} slot=0x{:x} old=0x{old:x}",
                        object as usize, slot as usize
                    );
                }
                if is_missing_bits(py, old) {
                    return Ok(false);
                }
                let missing = crate::missing_bits(py);
                inc_ref_bits(py, missing);
                object_mark_has_ptrs(py, object);
                *slot = missing;
                dec_ref_bits(py, old);
                Ok(true)
            }
            Some(FieldStorage::Dictionary { dictionary, name }) => {
                inc_ref_bits(py, dictionary);
                inc_ref_bits(py, name);
                field_storage::debug::dictionary(
                    py,
                    "field_delete",
                    object,
                    dictionary,
                    Some(name),
                    None,
                );
                let deleted =
                    crate::dict_del_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), name);
                dec_ref_bits(py, name);
                dec_ref_bits(py, dictionary);
                deleted
            }
            None => Err(()),
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
            // Terminal statement ABI: errors remain pending; both statuses return None.
            let _completed = object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
            MoltObject::none().bits()
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
            // Terminal statement ABI: errors remain pending; both statuses return None.
            let _completed = object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits);
            MoltObject::none().bits()
        })
    }
}

/// The layout guard (HF-220): the receiver's class was created by the compiled
/// class statement bound to `definition_slot`, and its layout version is still
/// the statement's. Field offsets are facts of the statement, so every class it
/// creates, on any run, admits them; no other class does, whatever name binds
/// it. A class no compiled statement stamped never matches.
unsafe fn guard_layout_match(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    definition_slot: u64,
    expected_version: u64,
) -> bool {
    unsafe {
        profile_hit(_py, &LAYOUT_GUARD_COUNT);
        if obj_ptr.is_null() {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT);
            return false;
        }
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
        if !super::object_has_class_shape(obj_ptr) {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT);
            return false;
        }
        let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr() else {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT);
            return false;
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT);
            return false;
        }
        let definition = class_definition(class_ptr);
        if debug_guard_enabled() {
            eprintln!(
                "[guard] ptr=0x{:x} class=0x{:x} definition={:?} expected_slot={}",
                obj_ptr as usize, class_ptr as usize, definition, definition_slot
            );
        }
        if definition.is_none() || definition != ClassDefinition::from_program_slot(definition_slot)
        {
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
/// `obj_bits` is a tagged runtime value; `definition_slot` is a raw program
/// slot. A non-object receiver is a safe guard miss.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guard_layout(
    obj_bits: u64,
    definition_slot: u64,
    expected_version: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = resolve_obj_ptr(obj_bits).unwrap_or(std::ptr::null_mut());
            let matches = guard_layout_match(_py, obj_ptr, definition_slot, expected_version);
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
    definition_slot: u64,
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
                && guard_layout_match(_py, obj_ptr, definition_slot, expected_version)
            {
                if instance_dict_bits(obj_ptr) != 0 {
                    return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits);
                }
                let bits = match object_field_get_ptr_raw(_py, obj_ptr, offset) {
                    Ok(value) => value,
                    Err(()) => return MoltObject::none().bits(),
                };
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
    definition_slot: u64,
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
                && guard_layout_match(_py, obj_ptr, definition_slot, expected_version)
            {
                let _completed = object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
                return MoltObject::none().bits();
            }
            crate::molt_set_attr_object(obj_bits, attr_name_ptr, attr_name_len_bits, val_bits)
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
            // Terminal statement ABI: errors remain pending; both statuses return None.
            let _completed = object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
            MoltObject::none().bits()
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
            // Terminal statement ABI: errors remain pending; both statuses return None.
            let _completed = object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits);
            MoltObject::none().bits()
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
