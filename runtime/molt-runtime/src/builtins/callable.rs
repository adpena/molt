use molt_obj_model::MoltObject;

use crate::call::function::function_code_execution_kind;
use crate::call::{has_type_call_attr, is_exact_staticmethod_wrapper};
use crate::object::layout::CodeExecutionKind;
use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_FOREIGN, TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS, TYPE_ID_TYPE,
    function_closure_bits, function_dict_bits, maybe_ptr_from_bits, obj_from_bits, object_type_id,
    raise_exception,
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
            MoltObject::from_bool(
                function_code_execution_kind(ptr) == Some(CodeExecutionKind::Generator),
            )
            .bits()
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
            MoltObject::from_bool(
                function_code_execution_kind(ptr) == Some(CodeExecutionKind::Coroutine),
            )
            .bits()
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
        // Exact wrappers always have tp_call, even before initialization or
        // when wrapping a non-callable. Subclasses use normal special lookup;
        // call dispatch separately resolves builtin forwarding versus overrides.
        if is_exact_staticmethod_wrapper(_py, ptr) {
            return true;
        }
        match object_type_id(ptr) {
            TYPE_ID_FUNCTION | TYPE_ID_BOUND_METHOD | TYPE_ID_TYPE | TYPE_ID_GENERIC_ALIAS => true,
            TYPE_ID_FOREIGN => molt_cpython_abi::bridge::molt_foreign_is_callable(
                crate::object::foreign::foreign_ptr_from_obj(ptr),
            ),
            _ => has_type_call_attr(_py, ptr),
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

#[cfg(test)]
mod execution_kind_tests {
    use super::*;
    use crate::builtins::inspect::{
        molt_inspect_isasyncgenfunction, molt_inspect_isawaitable, molt_inspect_iscoroutine,
        molt_inspect_iscoroutinefunction, molt_inspect_isgeneratorfunction,
    };
    use crate::object::layout::{code_publish_execution_kind, function_set_code_bits};

    #[test]
    fn private_markers_cannot_override_shared_code_kind_or_inspection() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = crate::alloc_string(py, b"kind_probe");
            let empty = crate::alloc_tuple(py, &[]);
            assert!(!name.is_null() && !empty.is_null());
            let name_bits = MoltObject::from_ptr(name).bits();
            let empty_bits = MoltObject::from_ptr(empty).bits();
            for kind in [
                CodeExecutionKind::Direct,
                CodeExecutionKind::Generator,
                CodeExecutionKind::Coroutine,
                CodeExecutionKind::AsyncGenerator,
            ] {
                let code = crate::alloc_code_obj(
                    py,
                    name_bits,
                    name_bits,
                    1,
                    MoltObject::none().bits(),
                    empty_bits,
                    empty_bits,
                    0,
                    0,
                    0,
                );
                assert!(!code.is_null());
                let code_bits = MoltObject::from_ptr(code).bits();
                unsafe {
                    assert_eq!(code_publish_execution_kind(code, kind), Ok(()));
                }
                for marker_value in [true, false] {
                    let function = crate::alloc_function_obj(py, 1, 0);
                    assert!(!function.is_null());
                    let bits = MoltObject::from_ptr(function).bits();
                    unsafe {
                        assert!(function_set_code_bits(py, function, code_bits));
                    }
                    for marker in [
                        b"__molt_is_generator__".as_slice(),
                        b"__molt_is_coroutine__".as_slice(),
                        b"__molt_is_async_generator__".as_slice(),
                    ] {
                        let key = crate::attr_name_bits_from_bytes(py, marker).unwrap();
                        let result = crate::molt_object_setattr(
                            bits,
                            key,
                            MoltObject::from_bool(marker_value).bits(),
                        );
                        crate::dec_ref_bits(py, result);
                        crate::dec_ref_bits(py, key);
                    }
                    assert!(!crate::exception_pending(py));
                    assert_eq!(
                        obj_from_bits(molt_function_is_generator(bits)).as_bool(),
                        Some(kind == CodeExecutionKind::Generator),
                    );
                    assert_eq!(
                        obj_from_bits(molt_function_is_coroutine(bits)).as_bool(),
                        Some(kind == CodeExecutionKind::Coroutine),
                    );
                    let bound = crate::alloc_bound_method_obj(py, bits, MoltObject::none().bits());
                    assert!(!bound.is_null());
                    let bound_bits = MoltObject::from_ptr(bound).bits();
                    for candidate in [bits, bound_bits] {
                        assert_eq!(
                            obj_from_bits(molt_inspect_isgeneratorfunction(candidate)).as_bool(),
                            Some(kind == CodeExecutionKind::Generator),
                        );
                        assert_eq!(
                            obj_from_bits(molt_inspect_iscoroutinefunction(candidate)).as_bool(),
                            Some(kind == CodeExecutionKind::Coroutine),
                        );
                        assert_eq!(
                            obj_from_bits(molt_inspect_isasyncgenfunction(candidate)).as_bool(),
                            Some(kind == CodeExecutionKind::AsyncGenerator),
                        );
                    }
                    assert_eq!(
                        obj_from_bits(molt_inspect_iscoroutine(bits)).as_bool(),
                        Some(false)
                    );
                    assert_eq!(
                        obj_from_bits(molt_inspect_isawaitable(bits)).as_bool(),
                        Some(false)
                    );
                    crate::dec_ref_bits(py, bound_bits);
                    crate::dec_ref_bits(py, bits);
                }
                crate::dec_ref_bits(py, code_bits);
            }
            crate::dec_ref_bits(py, empty_bits);
            crate::dec_ref_bits(py, name_bits);
        });
    }
}
