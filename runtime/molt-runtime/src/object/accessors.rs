use molt_obj_model::MoltObject;
use std::sync::OnceLock;

use super::inline_cache::{IC_TABLE_CAPACITY, global_ic_table};
use crate::{
    GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT, GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT,
    GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT, LAYOUT_GUARD_COUNT, LAYOUT_GUARD_FAIL, PyToken,
    STRUCT_FIELD_STORE_COUNT, TYPE_ID_DATACLASS, TYPE_ID_OBJECT, TYPE_ID_TYPE,
    attr_name_bits_from_bytes, class_field_offset, class_layout_version_bits, dec_ref_bits,
    exception_pending, global_type_version, header_from_obj_ptr, inc_ref_bits, is_missing_bits,
    obj_from_bits, object_class_bits, object_mark_has_ptrs, object_payload_size, object_type_id,
    profile_hit, raise_exception, to_i64, usize_from_bits,
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
    unsafe {
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if debug_field_bounds_enabled() {
            let payload = object_payload_size(obj_ptr);
            if offset.saturating_add(std::mem::size_of::<u64>()) > payload {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "object field offset out of range",
                );
            }
        }
        let slot = obj_ptr.add(offset) as *const u64;
        let bits = *slot;
        if std::env::var("MOLT_DEBUG_FIELD").is_ok() {
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
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if debug_field_bounds_enabled() {
            let payload = object_payload_size(obj_ptr);
            if offset.saturating_add(std::mem::size_of::<u64>()) > payload {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "object field offset out of range",
                );
            }
        }
        profile_hit(_py, &STRUCT_FIELD_STORE_COUNT);
        let slot = obj_ptr.add(offset) as *mut u64;
        let old_val = *slot;
        if std::env::var("MOLT_DEBUG_FIELD").is_ok() {
            eprintln!(
                "[field_set_raw] ptr=0x{:x} offset={} slot=0x{:x} old=0x{:x} val=0x{:x}",
                obj_ptr as usize, offset, slot as usize, old_val, val_bits
            );
        }
        let old_bits = *slot;
        let old_is_ptr = obj_from_bits(old_bits).as_ptr().is_some();
        let new_is_ptr = obj_from_bits(val_bits).as_ptr().is_some();
        if new_is_ptr {
            object_mark_has_ptrs(_py, obj_ptr);
        }
        if !old_is_ptr && !new_is_ptr {
            *slot = val_bits;
            // DEBUG: verify write persisted
            let readback = *slot;
            if std::env::var("MOLT_DEBUG_FIELD").is_ok() && readback != val_bits {
                eprintln!(
                    "[field_set_raw] WRITE FAILED! readback=0x{:x} expected=0x{:x}",
                    readback, val_bits
                );
            }
            return MoltObject::none().bits();
        }
        if old_bits != val_bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, val_bits);
            *slot = val_bits;
        }
        MoltObject::none().bits()
    }
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
    unsafe {
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
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
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_get_ptr(obj_ptr: *mut u8, offset_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let offset = usize_from_bits(offset_bits);
            object_field_get_ptr_raw(_py, obj_ptr, offset)
        })
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_set_ptr(
    obj_ptr: *mut u8,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let offset = usize_from_bits(offset_bits);
            object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits)
        })
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_init_ptr(
    obj_ptr: *mut u8,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let offset = usize_from_bits(offset_bits);
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
        if std::env::var("MOLT_DEBUG_GUARD").is_ok() {
            let header = header_from_obj_ptr(obj_ptr);
            let tid = (*header).type_id;
            let ocb = object_class_bits(obj_ptr);
            eprintln!(
                "[guard] ptr=0x{:x} type_id={} obj_class_bits=0x{:x} expected_class=0x{:x}",
                obj_ptr as usize, tid, ocb, class_bits
            );
        }
        if obj_ptr.is_null() {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT);
            return false;
        }
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).type_id != TYPE_ID_OBJECT {
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
        let version = class_layout_version_bits(class_ptr);
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
        if version != expected {
            profile_hit(_py, &LAYOUT_GUARD_FAIL);
            profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT);
            return false;
        }
        true
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with a class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guard_layout_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let matches = guard_layout_match(_py, obj_ptr, class_bits, expected_version);
            if !matches {
                profile_hit(_py, &GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT);
            }
            MoltObject::from_bool(matches).bits()
        })
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_field_get_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let offset = usize_from_bits(offset_bits);
            if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
                let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                if is_missing_bits(_py, bits) {
                    dec_ref_bits(_py, bits);
                    return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits)
                        as u64;
                }
                return bits;
            }
            crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits) as u64
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_field_set_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let offset = usize_from_bits(offset_bits);
            if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
                // Write to field slot only. __dict__ is synthesized lazily
                // when accessed (merges field slots into instance_dict).
                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
            }
            crate::molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits) as u64
        })
    }
}

/// # Safety
/// `obj_ptr` must point to a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_field_init_ptr(
    obj_ptr: *mut u8,
    class_bits: u64,
    expected_version: u64,
    offset_bits: u64,
    val_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let offset = usize_from_bits(offset_bits);
            if guard_layout_match(_py, obj_ptr, class_bits, expected_version) {
                return object_field_init_ptr_raw(_py, obj_ptr, offset, val_bits);
            }
            crate::molt_set_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits) as u64
        })
    }
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_get(obj_bits: u64, offset_bits: u64) -> u64 {
    unsafe {
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
}

/// # Safety
/// `obj_bits` must reference a valid object with enough payload for `offset_bits`.
/// Intended for initializing freshly allocated objects with immediate values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_field_init(
    obj_bits: u64,
    offset_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
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
}

// ---------------------------------------------------------------------------
// IC-accelerated attribute access
// ---------------------------------------------------------------------------

/// Inline-cache-accelerated attribute get for pointer-based objects.
///
/// # Fast path
/// 1. Read the object's `type_id` from its header.
/// 2. Probe the global lock-free IC table at `ic_index` with `(type_id, global_type_version)`.
/// 3. On hit the cached value is a byte offset into the object's payload — read the
///    field at that offset and return it (same as `object_field_get_ptr_raw`).
///
/// # Slow path
/// Falls through to `molt_get_attr_generic` (the existing full-resolution path).
/// After a successful lookup, attempts to resolve the attribute to a struct-field
/// byte offset via `class_field_offset` and populates the IC entry so subsequent
/// calls hit the fast path.
///
/// # Safety
/// `obj_ptr` must point to a valid molt object.
/// `attr_name_ptr` must be valid UTF-8 of length encoded in `attr_name_len_bits`.
/// `ic_index_bits` encodes a u64 index into the global IC table (must be < IC_TABLE_CAPACITY).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_getattr_ic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    ic_index_bits: u64,
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if obj_ptr.is_null() {
                return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits);
            }

            let type_id = object_type_id(obj_ptr);

            // IC fast path: only applicable to OBJECT and DATACLASS types whose
            // attributes may resolve to fixed-offset struct fields.
            if type_id == TYPE_ID_OBJECT || type_id == TYPE_ID_DATACLASS {
                let ic_index = usize_from_bits(ic_index_bits);
                if ic_index < IC_TABLE_CAPACITY {
                    let version = global_type_version();
                    let ic = global_ic_table().get(ic_index);

                    if let Some(cached_offset) = ic.probe(type_id, version) {
                        let offset = cached_offset as usize;
                        // Bounds-check the offset against the object's payload.
                        let payload = object_payload_size(obj_ptr);
                        if offset.saturating_add(std::mem::size_of::<u64>()) <= payload {
                            let slot = obj_ptr.add(offset) as *const u64;
                            let bits = *slot;
                            // A cached offset might point at an uninitialised /
                            // "missing" sentinel slot (e.g. the field was deleted
                            // after the IC was written). Treat that as a miss and
                            // fall through to the slow path.
                            if !is_missing_bits(_py, bits) && bits != 0 {
                                inc_ref_bits(_py, bits);
                                return bits as i64;
                            }
                        }
                    }

                    // --- Slow path: full resolution, then populate IC ---
                    let result =
                        crate::molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits);

                    // Only try to populate the IC when the lookup succeeded and no
                    // exception is pending.
                    if result != 0
                        && !obj_from_bits(result as u64).is_none()
                        && !exception_pending(_py)
                    {
                        let class_bits = object_class_bits(obj_ptr);
                        if class_bits != 0
                            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                            && object_type_id(class_ptr) == TYPE_ID_TYPE
                        {
                            let attr_len = usize_from_bits(attr_name_len_bits);
                            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_len);
                            if let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) {
                                if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
                                    && offset <= u32::MAX as usize
                                {
                                    let version = global_type_version();
                                    ic.update(type_id, offset as u32, version);
                                }
                                dec_ref_bits(_py, attr_bits);
                            }
                        }
                    }

                    return result;
                }
            }

            // Non-OBJECT/DATACLASS or invalid IC index: direct dispatch.
            crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

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
pub unsafe extern "C" fn molt_ic_probe_fast(obj_ptr: *mut u8, ic_index: u64) -> i64 {
    unsafe {
        if obj_ptr.is_null() {
            return 0;
        }

        let type_id = (*header_from_obj_ptr(obj_ptr)).type_id;

        // IC is only applicable to OBJECT and DATACLASS types.
        if type_id != TYPE_ID_OBJECT && type_id != TYPE_ID_DATACLASS {
            return 0;
        }

        let idx = ic_index as usize;
        if idx >= IC_TABLE_CAPACITY {
            return 0;
        }

        let version = global_type_version();
        let ic = global_ic_table().get(idx);

        if let Some(cached_offset) = ic.probe(type_id, version) {
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
                            if ((*header).flags & super::HEADER_FLAG_IMMORTAL) == 0 {
                                (*header)
                                    .ref_count
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        return bits as i64;
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
) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if obj_ptr.is_null() {
                return crate::molt_get_attr_ptr(obj_ptr, attr_name_ptr, attr_name_len_bits);
            }

            let type_id = object_type_id(obj_ptr);
            let idx = ic_index as usize;

            // Full resolution.
            let result = crate::molt_get_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits);

            // Populate the IC on success.
            if idx < IC_TABLE_CAPACITY
                && (type_id == TYPE_ID_OBJECT || type_id == TYPE_ID_DATACLASS)
                && result != 0
                && !obj_from_bits(result as u64).is_none()
                && !exception_pending(_py)
            {
                let class_bits = object_class_bits(obj_ptr);
                if class_bits != 0
                    && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let attr_len = usize_from_bits(attr_name_len_bits);
                    let slice = std::slice::from_raw_parts(attr_name_ptr, attr_len);
                    if let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) {
                        if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
                            && offset <= u32::MAX as usize
                        {
                            let version = global_type_version();
                            let ic = global_ic_table().get(idx);
                            ic.update(type_id, offset as u32, version);
                        }
                        dec_ref_bits(_py, attr_bits);
                    }
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
    crate::with_gil_entry!(_py, {
        unsafe { object_field_get_ptr_raw(_py, obj_ptr, offset as usize) }
    })
}
