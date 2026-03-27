//! Slice and dataclass operations.

use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::collections::HashMap;

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
            let start_eq = molt_eq(slice_start_bits(slice_ptr), slice_start_bits(other_ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !is_truthy(_py, obj_from_bits(start_eq)) {
                return MoltObject::from_bool(false).bits();
            }
            let stop_eq = molt_eq(slice_stop_bits(slice_ptr), slice_stop_bits(other_ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !is_truthy(_py, obj_from_bits(stop_eq)) {
                return MoltObject::from_bool(false).bits();
            }
            let step_eq = molt_eq(slice_step_bits(slice_ptr), slice_step_bits(other_ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !is_truthy(_py, obj_from_bits(step_eq)) {
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_reduce(slice_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, { molt_slice_reduce(slice_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_new(
    name_bits: u64,
    field_names_bits: u64,
    values_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
        if field_names.len() != values.len() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass constructor argument mismatch",
            );
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as u64;
        let frozen = (flags & 0x1) != 0;
        let eq = (flags & 0x2) != 0;
        let repr = (flags & 0x4) != 0;
        let slots = (flags & 0x8) != 0;
        let mut field_name_to_index = HashMap::with_capacity(field_names.len());
        for (idx, field_name) in field_names.iter().enumerate() {
            field_name_to_index.insert(field_name.clone(), idx);
        }
        let desc = Box::new(DataclassDesc {
            name,
            field_names,
            field_name_to_index,
            frozen,
            eq,
            repr,
            slots,
            class_bits: 0,
            field_flags: Vec::new(),
            hash_mode: 0,
        });
        let desc_ptr = Box::into_raw(desc);

        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut DataclassDesc>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<u64>();
        let ptr = alloc_object(_py, total, TYPE_ID_DATACLASS);
        if ptr.is_null() {
            unsafe { drop(Box::from_raw(desc_ptr)) };
            return MoltObject::none().bits();
        }
        unsafe {
            let mut vec = Vec::with_capacity(values.len());
            vec.extend_from_slice(&values);
            for &val in values.iter() {
                inc_ref_bits(_py, val);
            }
            let vec_ptr = Box::into_raw(Box::new(vec));
            *(ptr as *mut *mut DataclassDesc) = desc_ptr;
            *(ptr.add(std::mem::size_of::<*mut DataclassDesc>()) as *mut *mut Vec<u64>) = vec_ptr;
            dataclass_set_dict_bits(_py, ptr, 0);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_get(obj_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                let fields = dataclass_fields_ref(ptr);
                if idx < 0 || idx as usize >= fields.len() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dataclass field index out of range",
                    );
                }
                let val = fields[idx as usize];
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
                    return attr_error(_py, "dataclass", name) as u64;
                }
                inc_ref_bits(_py, val);
                return val;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_set(obj_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                let fields = dataclass_fields_mut(ptr);
                if idx < 0 || idx as usize >= fields.len() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dataclass field index out of range",
                    );
                }
                let old_bits = fields[idx as usize];
                if old_bits != val_bits {
                    dec_ref_bits(_py, old_bits);
                    inc_ref_bits(_py, val_bits);
                    fields[idx as usize] = val_bits;
                }
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

pub(crate) unsafe fn dataclass_set_class_raw(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    class_bits: u64,
) -> u64 {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DATACLASS {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        }
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
        }
        let desc_ptr = dataclass_desc_ptr(ptr);
        if !desc_ptr.is_null() {
            let old_bits = (*desc_ptr).class_bits;
            if old_bits != 0 {
                dec_ref_bits(_py, old_bits);
            }
            (*desc_ptr).class_bits = class_bits;
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
            }
            object_set_class_bits(_py, ptr, class_bits);
            if class_bits != 0 {
                let class_obj = obj_from_bits(class_bits);
                if let Some(class_ptr) = class_obj.as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let flags_name =
                        attr_name_bits_from_bytes(_py, b"__molt_dataclass_field_flags__");
                    if let Some(flags_name) = flags_name {
                        if let Some(flags_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, flags_name)
                        {
                            let flags_obj = obj_from_bits(flags_bits);
                            let flags_ptr = flags_obj.as_ptr();
                            if let Some(flags_ptr) = flags_ptr {
                                let type_id = object_type_id(flags_ptr);
                                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                                    let elems = seq_vec_ref(flags_ptr);
                                    let mut out = Vec::with_capacity(elems.len());
                                    for &elem_bits in elems.iter() {
                                        let elem_obj = obj_from_bits(elem_bits);
                                        let Some(val) = to_i64(elem_obj) else {
                                            out.clear();
                                            break;
                                        };
                                        if val < 0 || val > u8::MAX as i64 {
                                            out.clear();
                                            break;
                                        }
                                        out.push(val as u8);
                                    }
                                    if !out.is_empty() {
                                        (*desc_ptr).field_flags = out;
                                    }
                                }
                            }
                        }
                        dec_ref_bits(_py, flags_name);
                    }
                    let hash_name = attr_name_bits_from_bytes(_py, b"__molt_dataclass_hash__");
                    if let Some(hash_name) = hash_name {
                        if let Some(hash_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, hash_name)
                        {
                            let hash_obj = obj_from_bits(hash_bits);
                            if let Some(val) = to_i64(hash_obj)
                                && val >= 0
                                && val <= u8::MAX as i64
                            {
                                (*desc_ptr).hash_mode = val as u8;
                            }
                        }
                        dec_ref_bits(_py, hash_name);
                    }
                }
            }
        }
        MoltObject::none().bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_set_class(obj_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        };
        unsafe { dataclass_set_class_raw(_py, ptr, class_bits) }
    })
}
