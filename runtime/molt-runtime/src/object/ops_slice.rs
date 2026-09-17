//! Slice and dataclass operations.

use crate::object::{
    ClassEdgeOwnership, ObjectAuxPreselection, object_init_class_edge_unpublished,
};
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::collections::HashMap;

#[cfg(test)]
#[path = "ops_slice_tests.rs"]
mod tests;

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_slice_obj(_py, start_bits, stop_bits, step_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn slice_indices_adjust(mut idx: BigInt, len: &BigInt, lower: &BigInt, upper: &BigInt) -> BigInt {
    if idx.is_negative() {
        idx += len;
    }
    if idx < *lower {
        return lower.clone();
    }
    if idx > *upper {
        return upper.clone();
    }
    idx
}

fn slice_reduce_tuple(_py: &PyToken<'_>, slice_ptr: *mut u8) -> u64 {
    unsafe {
        let start_bits = slice_start_bits(slice_ptr);
        let stop_bits = slice_stop_bits(slice_ptr);
        let step_bits = slice_step_bits(slice_ptr);
        let args_ptr = alloc_tuple(_py, &[start_bits, stop_bits, step_bits]);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = builtin_classes(_py).slice;
        let res_ptr = alloc_tuple(_py, &[class_bits, args_bits]);
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(res_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_indices(slice_bits: u64, length_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            let msg = "slice indices must be integers or None or have an __index__ method";
            let Some(len) = index_bigint_from_obj(_py, length_bits, msg) else {
                return MoltObject::none().bits();
            };
            if len.is_negative() {
                return raise_exception::<_>(_py, "ValueError", "length should not be negative");
            }
            let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
            let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
            let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
            let step = if step_obj.is_none() {
                BigInt::from(1)
            } else {
                let Some(step_val) = index_bigint_from_obj(_py, step_obj.bits(), msg) else {
                    return MoltObject::none().bits();
                };
                step_val
            };
            if step.is_zero() {
                return raise_exception::<_>(_py, "ValueError", "slice step cannot be zero");
            }
            let step_neg = step.is_negative();
            let lower = if step_neg {
                BigInt::from(-1)
            } else {
                BigInt::from(0)
            };
            let upper = if step_neg { &len - 1 } else { len.clone() };
            let start = if start_obj.is_none() {
                if step_neg {
                    upper.clone()
                } else {
                    lower.clone()
                }
            } else {
                let Some(idx) = index_bigint_from_obj(_py, start_obj.bits(), msg) else {
                    return MoltObject::none().bits();
                };
                slice_indices_adjust(idx, &len, &lower, &upper)
            };
            let stop = if stop_obj.is_none() {
                if step_neg {
                    lower.clone()
                } else {
                    upper.clone()
                }
            } else {
                let Some(idx) = index_bigint_from_obj(_py, stop_obj.bits(), msg) else {
                    return MoltObject::none().bits();
                };
                slice_indices_adjust(idx, &len, &lower, &upper)
            };
            let start_bits = int_bits_from_bigint(_py, start);
            let stop_bits = int_bits_from_bigint(_py, stop);
            let step_bits = int_bits_from_bigint(_py, step);
            let tuple_ptr = alloc_tuple(_py, &[start_bits, stop_bits, step_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_hash(slice_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            let start_bits = slice_start_bits(slice_ptr);
            let stop_bits = slice_stop_bits(slice_ptr);
            let step_bits = slice_step_bits(slice_ptr);
            let Some(hash) = hash_slice_bits(_py, start_bits, stop_bits, step_bits) else {
                return MoltObject::none().bits();
            };
            int_bits_from_i64(_py, hash)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_eq(slice_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return not_implemented_bits(_py);
        };
        let Some(other_ptr) = obj_from_bits(other_bits).as_ptr() else {
            return not_implemented_bits(_py);
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return not_implemented_bits(_py);
            }
            if object_type_id(other_ptr) != TYPE_ID_SLICE {
                return not_implemented_bits(_py);
            }
            for (left, right) in [
                (slice_start_bits(slice_ptr), slice_start_bits(other_ptr)),
                (slice_stop_bits(slice_ptr), slice_stop_bits(other_ptr)),
                (slice_step_bits(slice_ptr), slice_step_bits(other_ptr)),
            ] {
                match crate::object::ops_compare::compare_object_eq_bool(
                    _py,
                    obj_from_bits(left),
                    obj_from_bits(right),
                ) {
                    crate::object::ops_compare::CompareBoolOutcome::True => {}
                    crate::object::ops_compare::CompareBoolOutcome::False => {
                        return MoltObject::from_bool(false).bits();
                    }
                    _ => return MoltObject::none().bits(),
                }
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_reduce(slice_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            slice_reduce_tuple(_py, slice_ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_reduce_ex(slice_bits: u64, _protocol_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_slice_reduce(slice_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_new(
    name_bits: u64,
    field_names_bits: u64,
    values_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let values_obj = obj_from_bits(values_bits);
        let values = match decode_value_list(values_obj) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "dataclass values must be a list/tuple",
                );
            }
        };
        dataclass_new_from_value_slice(_py, name_bits, field_names_bits, &values, flags_bits)
    })
}

/// # Safety
/// `values_ptr_bits` must encode `len` contiguous NaN-boxed values when `len > 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_dataclass_new_from_values(
    name_bits: u64,
    field_names_bits: u64,
    values_ptr_bits: u64,
    len: u64,
    flags_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(values_ptr) = crate::provenance::abi::const_ptr::<u64>(values_ptr_bits) else {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "dataclass values address exceeds the active address space",
                );
            };
            let Some(values) = crate::provenance::abi::slice(values_ptr, len) else {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "dataclass values range is invalid for the active target",
                );
            };
            dataclass_new_from_value_slice(_py, name_bits, field_names_bits, values, flags_bits)
        })
    }
}

fn dataclass_new_from_value_slice(
    _py: &PyToken<'_>,
    name_bits: u64,
    field_names_bits: u64,
    values: &[u64],
    flags_bits: u64,
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let name = match string_obj_to_owned(name_obj) {
        Some(val) => val,
        None => return raise_exception::<_>(_py, "TypeError", "dataclass name must be a str"),
    };
    let field_names_obj = obj_from_bits(field_names_bits);
    let field_names = match decode_string_list(field_names_obj) {
        Some(val) => val,
        None => {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass field names must be a list/tuple of str",
            );
        }
    };
    if field_names.len() != values.len() {
        return raise_exception::<_>(_py, "TypeError", "dataclass constructor argument mismatch");
    }
    let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as u64;
    let frozen = (flags & 0x1) != 0;
    let eq = (flags & 0x2) != 0;
    let repr = (flags & 0x4) != 0;
    let slots = (flags & 0x8) != 0;
    let allows_dict = !slots;
    let mut field_name_to_index = HashMap::with_capacity(field_names.len());
    for (idx, field_name) in field_names.iter().enumerate() {
        if field_name_to_index
            .insert(field_name.clone(), idx)
            .is_some()
        {
            return raise_exception::<_>(_py, "TypeError", "duplicate dataclass field name");
        }
    }
    let desc = Box::new(DataclassDesc {
        name,
        field_names,
        field_keys: Vec::new(),
        declared_slots: Vec::new(),
        field_name_to_index,
        frozen,
        eq,
        repr,
        slots,
        allows_dict,
        field_flags: Vec::new(),
        hash_mode: 0,
    });
    let desc_ptr = Box::into_raw(desc);

    let total = std::mem::size_of::<MoltHeader>()
        + std::mem::size_of::<*mut DataclassDesc>()
        + std::mem::size_of::<*mut Vec<u64>>()
        + std::mem::size_of::<u64>();
    let ptr = crate::object::alloc_object_zeroed_unpublished_with_aux(
        _py,
        total,
        TYPE_ID_DATACLASS,
        ObjectAuxPreselection::ClassInline,
    );
    if ptr.is_null() {
        unsafe { drop(Box::from_raw(desc_ptr)) };
        return MoltObject::none().bits();
    }
    unsafe {
        let Some(vec_ptr) =
            crate::object::backing::tracked_vec_box_from_slice(values, values.len())
        else {
            drop(Box::from_raw(desc_ptr));
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            return raise_exception::<_>(_py, "MemoryError", "dataclass allocation failed");
        };
        for &val in values.iter() {
            inc_ref_bits(_py, val);
        }
        *(ptr as *mut *mut DataclassDesc) = desc_ptr;
        *(ptr.add(std::mem::size_of::<*mut DataclassDesc>()) as *mut *mut Vec<u64>) = vec_ptr;
        instance_set_dict_bits(_py, ptr, 0);
    }
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_get(obj_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let idx = match obj_from_bits(index_bits).as_int() {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "dataclass field index must be int");
            }
        };
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_DATACLASS {
                    return MoltObject::none().bits();
                }
                let fields = dataclass_fields_ptr(ptr);
                if idx < 0 || fields.is_null() || idx as usize >= (*fields).len() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dataclass field index out of range",
                    );
                }
                let val = crate::object::accessors::object_field_get_ptr_raw(
                    _py,
                    ptr,
                    idx as usize * std::mem::size_of::<u64>(),
                );
                if exception_pending(_py) {
                    dec_ref_bits(_py, val);
                    return MoltObject::none().bits();
                }
                if is_missing_bits(_py, val) {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    let name = if !desc_ptr.is_null() {
                        let names = &(*desc_ptr).field_names;
                        names
                            .get(idx as usize)
                            .map(|s| s.as_str())
                            .unwrap_or("field")
                    } else {
                        "field"
                    };
                    dec_ref_bits(_py, val);
                    return attr_error(_py, "dataclass", name);
                }
                return val;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_set(obj_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let idx = match obj_from_bits(index_bits).as_int() {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "dataclass field index must be int");
            }
        };
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_DATACLASS {
                    return MoltObject::none().bits();
                }
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && (*desc_ptr).frozen {
                    let field_names = &(*desc_ptr).field_names;
                    let field_name = if idx >= 0 {
                        field_names
                            .get(idx as usize)
                            .map(|name| name.as_str())
                            .unwrap_or("<field>")
                    } else {
                        "<field>"
                    };
                    return raise_frozen_instance_error(
                        _py,
                        &format!("cannot assign to field '{field_name}'"),
                    );
                }
                let fields = dataclass_fields_ptr(ptr);
                if idx < 0 || fields.is_null() || idx as usize >= (*fields).len() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dataclass field index out of range",
                    );
                }
                crate::object::accessors::object_field_set_ptr_raw(
                    _py,
                    ptr,
                    idx as usize * std::mem::size_of::<u64>(),
                    val_bits,
                );
                return obj_bits;
            }
        }
        MoltObject::none().bits()
    })
}

fn raise_frozen_instance_error(_py: &PyToken<'_>, message: &str) -> u64 {
    let module_name_ptr = alloc_string(_py, b"dataclasses");
    if module_name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"FrozenInstanceError") else {
        dec_ref_bits(_py, module_bits);
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let class_bits = molt_getattr_builtin(module_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if class_bits == missing {
        return raise_exception::<u64>(_py, "RuntimeError", "FrozenInstanceError unavailable");
    }
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        dec_ref_bits(_py, class_bits);
        return raise_exception::<u64>(_py, "TypeError", "FrozenInstanceError class is invalid");
    };
    let message_ptr = alloc_string(_py, message.as_bytes());
    if message_ptr.is_null() {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let message_bits = MoltObject::from_ptr(message_ptr).bits();
    let exc_bits = unsafe { call_class_init_with_args(_py, class_ptr, &[message_bits]) };
    dec_ref_bits(_py, message_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::molt_raise(exc_bits)
}

unsafe fn validate_dataclass_class_target(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    class_bits: u64,
) -> Result<(), u64> {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DATACLASS {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass expects object",
            ));
        }
        if class_bits == 0 || obj_from_bits(class_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass class must be a type",
            ));
        }
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass class must be a type",
            ));
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass class must be a type",
            ));
        }
        if crate::object::class_finish_definition(_py, class_ptr).is_err() {
            return Err(MoltObject::none().bits());
        }
        Ok(())
    }
}

unsafe fn prepare_dataclass_class_metadata(
    py: &PyToken<'_>,
    ptr: *mut u8,
    class_bits: u64,
) -> Result<(), ()> {
    unsafe {
        let desc = dataclass_desc_ptr(ptr);
        if desc.is_null() {
            raise_exception::<()>(py, "SystemError", "dataclass descriptor is missing");
            return Err(());
        }
        let class = obj_from_bits(class_bits)
            .as_ptr()
            .expect("validated dataclass class");
        // All fallible work precedes class attachment. A rejected private payload
        // cannot dispatch __del__, resurrect, or escape without GC publication.
        let empty = missing_bits(py);
        if exception_pending(py) || obj_from_bits(empty).as_ptr().is_none() {
            return Err(());
        }
        for index in 0..(*desc).field_names.len() {
            let key = if let Some(&key) = (&(*desc).field_keys).get(index) {
                key
            } else {
                let name = (&(*desc).field_names)[index].as_bytes();
                let key = attr_name_bits_from_bytes(py, name).ok_or(())?;
                (*desc).field_keys.push(key);
                key
            };
            let declared = (*desc).slots
                || class_mro_view(py, class).iter().copied().any(|base| {
                    obj_from_bits(base).as_ptr().is_some_and(|base| {
                        object_type_id(base) == TYPE_ID_TYPE
                            && crate::builtins::attr::class_own_slot_field_offset(py, base, key)
                                .is_some()
                    })
                });
            if let Some(slot) = (&mut (*desc).declared_slots).get_mut(index) {
                *slot = declared;
            } else {
                (*desc).declared_slots.push(declared);
            }
        }
        (*desc).allows_dict = !(*desc).slots
            || crate::builtins::attr::class_slots_info(py, class)
                .is_some_and(|info| info.allows_dict);
        // Reset the private projection on retry with a different validated class.
        (*desc).field_flags.clear();
        (*desc).hash_mode = 0;
        let flags_name =
            attr_name_bits_from_bytes(py, b"__molt_dataclass_field_flags__").ok_or(())?;
        if let Some(flags) = class_attr_lookup_raw_mro(py, class, flags_name)
            && let Some(flags) = obj_from_bits(flags).as_ptr()
            && matches!(object_type_id(flags), TYPE_ID_LIST | TYPE_ID_TUPLE)
        {
            let flags = crate::object::seq_access::with_borrowed(flags, |elements| {
                elements
                    .iter()
                    .map(|&bits| {
                        to_i64(obj_from_bits(bits)).and_then(|value| u8::try_from(value).ok())
                    })
                    .collect::<Option<Vec<_>>>()
            });
            if let Some(flags) = flags {
                (*desc).field_flags = flags;
            }
        }
        dec_ref_bits(py, flags_name);
        if exception_pending(py) {
            return Err(());
        }
        let hash_name = attr_name_bits_from_bytes(py, b"__molt_dataclass_hash__").ok_or(())?;
        if let Some(hash) = class_attr_lookup_raw_mro(py, class, hash_name)
            && let Some(hash) =
                to_i64(obj_from_bits(hash)).and_then(|value| u8::try_from(value).ok())
        {
            (*desc).hash_mode = hash;
        }
        dec_ref_bits(py, hash_name);
        if exception_pending(py) {
            Err(())
        } else {
            Ok(())
        }
    }
}

/// Finish a dataclass's private payload and class edge, then publish the object
/// exactly once across its constructor boundary.
pub(crate) unsafe fn dataclass_finish_construction_unpublished(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    class_bits: u64,
) -> u64 {
    unsafe {
        if ptr.is_null() || object_type_id(ptr) != TYPE_ID_DATACLASS {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        }
        let header = &*crate::object::header_from_obj_ptr(ptr);
        if header.gc_is_published() || object_class_bits(ptr) != 0 {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass construction is already finalized",
            );
        }
        if let Err(bits) = validate_dataclass_class_target(_py, ptr, class_bits) {
            return bits;
        }
        if prepare_dataclass_class_metadata(_py, ptr, class_bits).is_err() {
            if !exception_pending(_py) {
                return raise_exception::<_>(
                    _py,
                    "MemoryError",
                    "dataclass metadata preparation failed",
                );
            }
            return MoltObject::none().bits();
        }
        if !object_init_class_edge_unpublished(_py, ptr, class_bits, ClassEdgeOwnership::Owned) {
            return raise_exception::<_>(
                _py,
                "MemoryError",
                "dataclass class metadata allocation failed",
            );
        }
        crate::object::gc::gc_publish_initialized(_py, ptr);
        MoltObject::none().bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_set_class(obj_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        };
        unsafe { dataclass_finish_construction_unpublished(_py, ptr, class_bits) }
    })
}
