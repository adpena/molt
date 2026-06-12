use molt_obj_model::MoltObject;

use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS,
    TYPE_ID_OBJECT, TYPE_ID_TYPE, class_attr_lookup_raw_mro, dataclass_desc_ptr,
    dataclass_dict_bits, dict_get_in_place, function_attr_bits, function_closure_bits,
    function_dict_bits, instance_dict_bits, intern_static_name, is_truthy, maybe_ptr_from_bits,
    obj_from_bits, object_class_bits, object_type_id, raise_exception, runtime_state,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_bound_method(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let is_bound = maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BOUND_METHOD });
        MoltObject::from_bool(is_bound).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_function_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_mode = std::env::var("MOLT_TRACE_IS_FUNCTION").ok();
        let log_all = matches!(trace_mode.as_deref(), Some("all"));
        let log_none = matches!(trace_mode.as_deref(), Some("1"));
        let ptr = maybe_ptr_from_bits(obj_bits);
        let is_func = ptr.is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FUNCTION });
        if log_all || (log_none && obj_bits == MoltObject::none().bits()) {
            let type_id = ptr.map(|ptr| unsafe { object_type_id(ptr) });
            eprintln!(
                "molt is_function_obj bits=0x{obj_bits:x} ptr={:?} type_id={:?} is_func={}",
                ptr, type_id, is_func
            );
        }
        MoltObject::from_bool(is_func).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_is_generator(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return MoltObject::from_bool(false).bits();
            }
            let name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_is_generator,
                b"__molt_is_generator__",
            );
            let Some(bits) = function_attr_bits(_py, ptr, name_bits) else {
                return MoltObject::from_bool(false).bits();
            };
            MoltObject::from_bool(is_truthy(_py, obj_from_bits(bits))).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_is_coroutine(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return MoltObject::from_bool(false).bits();
            }
            let name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_is_coroutine,
                b"__molt_is_coroutine__",
            );
            let Some(bits) = function_attr_bits(_py, ptr, name_bits) else {
                return MoltObject::from_bool(false).bits();
            };
            MoltObject::from_bool(is_truthy(_py, obj_from_bits(bits))).bits()
        }
    })
}

/// Single source of truth for Python callability.
///
/// Returns a Rust `bool` so that no caller has to decode a NaN-boxed result.
/// Both the C-ABI [`molt_is_callable`] (Python `callable()` / `bool`-object
/// result) and the C-ABI [`molt_is_callable_bool`] (cross-crate `i32` result)
/// delegate here, guaranteeing every callable-oracle consumer observes the
/// identical predicate. A `&PyToken` is required because the `__call__`
/// dunder lookup walks the MRO under the GIL.
pub(crate) fn is_callable_impl(_py: &crate::PyToken<'_>, obj_bits: u64) -> bool {
    maybe_ptr_from_bits(obj_bits).is_some_and(|ptr| unsafe { is_callable_for_ptr(_py, ptr) })
}

#[inline]
unsafe fn is_callable_for_ptr(_py: &crate::PyToken<'_>, ptr: *mut u8) -> bool {
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE | TYPE_ID_GENERIC_ALIAS => true,
            TYPE_ID_OBJECT => {
                // NOTE (parity baton): this consults the instance `__dict__`
                // for `__call__`, which diverges from CPython (`tp_call` is
                // type-only). It is intentionally PRESERVED here so this
                // oracle stays consistent with molt's instance-call dispatch
                // (`call::lookup_call_attr`), which also honors instance-dict
                // `__call__`. Fixing the divergence requires a coordinated
                // type-only `__call__` resolver shared by BOTH this oracle
                // and every `lookup_call_attr` call site (see batoned bug).
                let call_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
                let dict_bits = instance_dict_bits(ptr);
                if dict_bits != 0
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(found_bits) = dict_get_in_place(_py, dict_ptr, call_bits)
                    && !obj_from_bits(found_bits).is_none()
                {
                    return true;
                }
                let class_bits = object_class_bits(ptr);
                if class_bits != 0
                    && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    if let Some(found_bits) = class_attr_lookup_raw_mro(_py, class_ptr, call_bits) {
                        return !obj_from_bits(found_bits).is_none();
                    }
                    return false;
                }
                false
            }
            TYPE_ID_DATACLASS => {
                // See TYPE_ID_OBJECT note: instance-dict `__call__` honored
                // for consistency with the call dispatch (batoned divergence).
                let call_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.call_name, b"__call__");
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(ptr);
                    if dict_bits != 0
                        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                        && let Some(found_bits) = dict_get_in_place(_py, dict_ptr, call_bits)
                        && !obj_from_bits(found_bits).is_none()
                    {
                        return true;
                    }
                }
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0
                        && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && object_type_id(class_ptr) == TYPE_ID_TYPE
                    {
                        if let Some(found_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, call_bits)
                        {
                            return !obj_from_bits(found_bits).is_none();
                        }
                        return false;
                    }
                }
                false
            }
            _ => false,
        }
    }
}

/// Python `callable()` ABI: returns a NaN-boxed `bool` MoltObject.
///
/// Callers inside `molt-runtime` decode the result via `is_truthy`/`as_bool`.
/// Cross-crate consumers should prefer [`molt_is_callable_bool`] to avoid
/// re-implementing the bool decode (the bug class that made tkinter `bind`
/// reject genuine callables: `as_int()` rejects a `TAG_BOOL` value).
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_callable(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(is_callable_impl(_py, obj_bits)).bits()
    })
}

/// Cross-crate callability oracle returning a C-ABI bool (`1`/`0`).
///
/// This is the single decode-free authority for runtime extension crates
/// (e.g. `molt-runtime-tk`) so they never have to interpret the NaN-boxed
/// `bool` object themselves. Backed by the same [`is_callable_impl`] that
/// powers Python `callable()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_callable_bool(obj_bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, { i32::from(is_callable_impl(_py, obj_bits)) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_default_kind(func_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return 0;
            }
            let dict_bits = function_dict_bits(ptr);
            if dict_bits == 0 {
                return 0;
            }
            obj_from_bits(dict_bits).as_int().unwrap_or(0)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_closure_bits(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(func_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FUNCTION {
                return 0;
            }
            function_closure_bits(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_call_arity_error(expected: i64, got: i64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let msg = format!("call arity mismatch (expected {expected}, got {got})");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}
