use super::*;
use crate::BuiltinExc;

pub(crate) fn molt_super_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    type_bits: u64,
    obj_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        molt_super_new_attached_body(_py, type_bits, obj_bits)
    })
}

// Type-constructor builtins: thin `extern "C"` wrappers so the compiler can
// emit direct calls to `molt_<type>_builtin` for Python's builtin types.

/// `int(x=0, base=10)` — wraps `molt_int_from_obj`.

pub(crate) fn molt_int_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    base_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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
        molt_int_from_obj_attached_body(_py, val_bits, actual_base, has_base_bits)
    })
}

/// `float(x=0.0)` — wraps `molt_float_from_obj`.

pub(crate) fn molt_float_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_float(0.0).bits();
        }
        molt_float_from_obj_attached_body(_py, val_bits)
    })
}

/// `bool(x=False)` — wraps `is_truthy`.

pub(crate) fn molt_bool_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_str_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_str_from_obj_attached_body(_py, val_bits)
    })
}

/// `bytes(source=b'')` — wraps `molt_bytes_from_obj`.

pub(crate) fn molt_bytes_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytes_from_obj_attached_body(_py, val_bits)
    })
}

/// `bytearray(source=bytearray())` — wraps `molt_bytearray_from_obj`.

pub(crate) fn molt_bytearray_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytearray(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytearray_from_obj_attached_body(_py, val_bits)
    })
}

/// `list(iterable=())` — constructs a list from an iterable.

pub(crate) fn molt_list_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_tuple_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_dict_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_dict_new_attached_body(_py, 0);
        }
        molt_dict_from_obj_attached_body(_py, val_bits)
    })
}

/// `set(iterable=())` — constructs a set from an iterable.

pub(crate) fn molt_set_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_set_new_attached_body(_py, 0);
        }
        let set_bits = molt_set_new_attached_body(_py, 0);
        if obj_from_bits(set_bits).is_none() {
            return MoltObject::none().bits();
        }
        let _ = molt_set_update_attached_body(_py, set_bits, val_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, set_bits);
            return MoltObject::none().bits();
        }
        set_bits
    })
}

/// `frozenset(iterable=())` — constructs a frozenset from an iterable.

pub(crate) fn molt_frozenset_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_frozenset_new_attached_body(_py, 0);
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

pub(crate) fn molt_range_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "range expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // range(stop) — single-arg form
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            return molt_range_new_attached_body(_py, zero, start_bits, one);
        }
        let actual_step = if step_bits == missing {
            MoltObject::from_int(1).bits()
        } else {
            step_bits
        };
        molt_range_new_attached_body(_py, start_bits, stop_bits, actual_step)
    })
}

/// `slice(stop)` / `slice(start, stop[, step])` — wraps `molt_slice_new`.

pub(crate) fn molt_slice_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "slice expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // slice(stop) — single-arg form
            return molt_slice_new_attached_body(_py, none, start_bits, none);
        }
        let actual_step = if step_bits == missing {
            none
        } else {
            step_bits
        };
        molt_slice_new_attached_body(_py, start_bits, stop_bits, actual_step)
    })
}

/// `object()` — wraps `molt_object_new`.

pub(crate) fn molt_object_builtin_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    molt_object_new_attached_body(_py)
}

/// `type(object)` — wraps `molt_builtin_type`.

pub(crate) fn molt_type_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    molt_builtin_type_attached_body(_py, val_bits)
}

/// `complex(real=0, imag=0)` — wraps `molt_complex_from_obj`.

pub(crate) fn molt_complex_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    real_bits: u64,
    imag_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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
        molt_complex_from_obj_attached_body(_py, actual_real, actual_imag, has_imag_bits)
    })
}

/// `classmethod(func)` — wraps `molt_classmethod_new`.

pub(crate) fn molt_classmethod_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
) -> u64 {
    molt_classmethod_new_attached_body(_py, func_bits)
}

/// `staticmethod(func)` — wraps `molt_staticmethod_new`.

pub(crate) fn molt_staticmethod_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    func_bits: u64,
) -> u64 {
    molt_staticmethod_new_attached_body(_py, func_bits)
}

/// `property(fget=None, fset=None, fdel=None)` — wraps `molt_property_new`.

pub(crate) fn molt_property_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    get_bits: u64,
    set_bits: u64,
    del_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        let g = if get_bits == missing { none } else { get_bits };
        let s = if set_bits == missing { none } else { set_bits };
        let d = if del_bits == missing { none } else { del_bits };
        molt_property_new_attached_body(_py, g, s, d)
    })
}

/// `isinstance(obj, classinfo)` — wraps `molt_isinstance`.

pub(crate) fn molt_isinstance_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    class_bits: u64,
) -> u64 {
    molt_isinstance_attached_body(_py, val_bits, class_bits)
}

/// `issubclass(sub, classinfo)` — wraps `molt_issubclass`.

pub(crate) fn molt_issubclass_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    sub_bits: u64,
    class_bits: u64,
) -> u64 {
    molt_issubclass_attached_body(_py, sub_bits, class_bits)
}

/// `hasattr(obj, name)` — wraps `molt_has_attr_name`.

pub(crate) fn molt_hasattr_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name_bits: u64,
) -> u64 {
    molt_has_attr_name_attached_body(_py, obj_bits, name_bits)
}

/// `aiter(async_iterable)` — wraps `molt_aiter`.

pub(crate) fn molt_aiter_builtin_attached_body(_py: &crate::PyToken<'_>, obj_bits: u64) -> u64 {
    molt_aiter_attached_body(_py, obj_bits)
}

/// Public `iter(object[, sentinel])`; omission is distinct from Python None.
/// Both overloads reuse the canonical protocol constructors.

pub(crate) fn molt_iter_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    sentinel_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        if sentinel_bits == missing_bits(py) {
            crate::object::ops_iter::iter_checked(py, obj_bits)
        } else {
            crate::object::ops_iter::iter_sentinel(py, obj_bits, sentinel_bits)
        }
    })
}

include!("../attached_exports_generated/object/ops_builtins_constructors.rs");
