use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_builtin(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_super_new(type_bits, obj_bits) })
}

// Type-constructor builtins: thin `extern "C"` wrappers so the compiler can
// emit direct calls to `molt_<type>_builtin` for Python's builtin types.

/// `int(x=0, base=10)` — wraps `molt_int_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_int_builtin(val_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            // int() with no args => 0
            return MoltObject::from_int(0).bits();
        }
        let has_base = base_bits != missing;
        let has_base_bits = if has_base { 1u64 } else { 0u64 };
        let actual_base = if has_base {
            base_bits
        } else {
            MoltObject::from_int(10).bits()
        };
        molt_int_from_obj(val_bits, actual_base, has_base_bits)
    })
}

/// `float(x=0.0)` — wraps `molt_float_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_float_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_float(0.0).bits();
        }
        molt_float_from_obj(val_bits)
    })
}

/// `bool(x=False)` — wraps `is_truthy`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bool_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_bool(false).bits();
        }
        let result = is_truthy(_py, obj_from_bits(val_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(result).bits()
    })
}

/// `str(object='')` — wraps `molt_str_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_str_from_obj(val_bits)
    })
}

/// `bytes(source=b'')` — wraps `molt_bytes_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytes_from_obj(val_bits)
    })
}

/// `bytearray(source=bytearray())` — wraps `molt_bytearray_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytearray(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytearray_from_obj(val_bits)
    })
}

/// `list(iterable=())` — constructs a list from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        unsafe {
            let Some(bits) = list_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `tuple(iterable=())` — constructs a tuple from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        unsafe {
            let Some(bits) = tuple_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `dict(mapping_or_iterable=None)` — wraps `molt_dict_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_dict_new(0);
        }
        molt_dict_from_obj(val_bits)
    })
}

/// `set(iterable=())` — constructs a set from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_set_new(0);
        }
        let set_bits = molt_set_new(0);
        if obj_from_bits(set_bits).is_none() {
            return MoltObject::none().bits();
        }
        let _ = molt_set_update(set_bits, val_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, set_bits);
            return MoltObject::none().bits();
        }
        set_bits
    })
}

/// `frozenset(iterable=())` — constructs a frozenset from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_frozenset_new(0);
        }
        unsafe {
            let Some(bits) = frozenset_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `range(stop)` / `range(start, stop[, step])` — wraps `molt_range_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_range_builtin(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "range expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // range(stop) — single-arg form
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            return molt_range_new(zero, start_bits, one);
        }
        let actual_step = if step_bits == missing {
            MoltObject::from_int(1).bits()
        } else {
            step_bits
        };
        molt_range_new(start_bits, stop_bits, actual_step)
    })
}

/// `slice(stop)` / `slice(start, stop[, step])` — wraps `molt_slice_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_builtin(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "slice expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // slice(stop) — single-arg form
            return molt_slice_new(none, start_bits, none);
        }
        let actual_step = if step_bits == missing {
            none
        } else {
            step_bits
        };
        molt_slice_new(start_bits, stop_bits, actual_step)
    })
}

/// `object()` — wraps `molt_object_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_builtin() -> u64 {
    molt_object_new()
}

/// `type(object)` — wraps `molt_builtin_type`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_type_builtin(val_bits: u64) -> u64 {
    molt_builtin_type(val_bits)
}

/// `complex(real=0, imag=0)` — wraps `molt_complex_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_builtin(real_bits: u64, imag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let actual_real = if real_bits == missing {
            MoltObject::from_int(0).bits()
        } else {
            real_bits
        };
        let has_imag = imag_bits != missing;
        let has_imag_bits = if has_imag { 1u64 } else { 0u64 };
        let actual_imag = if has_imag {
            imag_bits
        } else {
            MoltObject::from_int(0).bits()
        };
        molt_complex_from_obj(actual_real, actual_imag, has_imag_bits)
    })
}

/// `memoryview(obj)` — wraps `molt_memoryview_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_builtin(val_bits: u64) -> u64 {
    molt_memoryview_new(val_bits)
}

/// `classmethod(func)` — wraps `molt_classmethod_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_builtin(func_bits: u64) -> u64 {
    molt_classmethod_new(func_bits)
}

/// `staticmethod(func)` — wraps `molt_staticmethod_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_builtin(func_bits: u64) -> u64 {
    molt_staticmethod_new(func_bits)
}

/// `property(fget=None, fset=None, fdel=None)` — wraps `molt_property_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_builtin(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        let g = if get_bits == missing { none } else { get_bits };
        let s = if set_bits == missing { none } else { set_bits };
        let d = if del_bits == missing { none } else { del_bits };
        molt_property_new(g, s, d)
    })
}

/// `isinstance(obj, classinfo)` — wraps `molt_isinstance`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isinstance_builtin(val_bits: u64, class_bits: u64) -> u64 {
    molt_isinstance(val_bits, class_bits)
}

/// `issubclass(sub, classinfo)` — wraps `molt_issubclass`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_issubclass_builtin(sub_bits: u64, class_bits: u64) -> u64 {
    molt_issubclass(sub_bits, class_bits)
}

/// `hasattr(obj, name)` — wraps `molt_has_attr_name`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_hasattr_builtin(obj_bits: u64, name_bits: u64) -> u64 {
    molt_has_attr_name(obj_bits, name_bits)
}

/// `aiter(async_iterable)` — wraps `molt_aiter`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_aiter_builtin(obj_bits: u64) -> u64 {
    molt_aiter(obj_bits)
}

/// `iter(object)` — wraps `molt_iter_checked`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_builtin(obj_bits: u64) -> u64 {
    molt_iter_checked(obj_bits)
}
