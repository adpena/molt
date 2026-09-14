mod common;
mod core_types;
mod dispatch;
mod io;
mod numeric;
mod sequence;
mod singletons;
mod specialized;

/// Bridge cached builtin allocation into optional method lookup. A missing name
/// is None without an exception; a failed allocation is None with its original
/// exception. Zero is never a callable value, even though it is a scalar bit
/// pattern. Cached methods are borrowed, so rejecting a result releases nothing.
pub(crate) fn method_dispatch(
    py: &crate::PyToken<'_>,
    lookup: impl FnOnce() -> Option<u64>,
) -> Option<u64> {
    if crate::exception_pending(py) {
        return None;
    }
    let result = lookup();
    if crate::exception_pending(py) {
        return None;
    }
    if result == Some(0) {
        crate::raise_exception::<u64>(
            py,
            "SystemError",
            "builtin method lookup returned a null callable without an exception",
        );
        None
    } else {
        result
    }
}

pub(crate) use common::{
    alloc_builtin_function, alloc_builtin_function_with_defaults, builtin_func_bits,
    builtin_func_bits_with_bind_kind, builtin_func_bits_with_defaults_tuple,
    builtin_func_bits_with_signature, builtin_variadic_func_bits, configure_builtin_signature,
    set_function_defaults,
};
pub(crate) use core_types::{
    memoryview_method_bits, object_method_bits, range_method_bits, type_method_bits,
};
pub(crate) use dispatch::builtin_class_method_bits;
pub(crate) use io::file_method_bits;
pub(crate) use numeric::{complex_method_bits, float_method_bits, int_method_bits};
pub(crate) use sequence::{
    bytearray_method_bits, bytes_method_bits, slice_method_bits, string_method_bits,
};
pub(crate) use singletons::{
    ellipsis_bits, is_missing_bits, is_not_implemented_bits, missing_bits, not_implemented_bits,
};
pub(crate) use specialized::{
    asyncgen_method_bits, coroutine_method_bits, generator_method_bits, weakref_method_bits,
};

#[cfg(test)]
mod tests {
    use super::specialized::property_method_bits;
    use super::*;
    use crate::builtins::exceptions::molt_exception_last_pending;
    use crate::*;

    #[test]
    fn method_dispatch_distinguishes_missing_from_failed_allocation() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            assert_eq!(method_dispatch(py, || None), None);
            assert_eq!(method_dispatch(py, || Some(0)), None);
            assert!(exception_pending(py));
            let _ = molt_exception_clear();
            let cached = object_method_bits(py, "__getattribute__").unwrap();
            assert_ne!(cached, 0);
            assert_eq!(method_dispatch(py, || Some(cached)), Some(cached));

            let failure = std::cell::Cell::new(0);
            assert_eq!(
                method_dispatch(py, || {
                    raise_exception::<()>(py, "MemoryError", "method allocation test failure");
                    failure.set(molt_exception_last_pending());
                    Some(0)
                }),
                None
            );
            assert!(exception_pending(py));

            // Pending failure must bypass even a populated cache. None remains
            // a failure, not permission for the next dispatcher to initialize.
            let entered = std::cell::Cell::new(false);
            assert_eq!(
                method_dispatch(py, || {
                    entered.set(true);
                    Some(cached)
                }),
                None
            );
            assert!(!entered.get());
            assert_eq!(object_method_bits(py, "__getattribute__"), None);
            assert_eq!(type_method_bits(py, "__call__"), None);
            assert_eq!(int_method_bits(py, "__new__"), None);
            assert_eq!(string_method_bits(py, "join"), None);
            assert_eq!(file_method_bits(py, "read"), None);
            assert_eq!(property_method_bits(py, "__get__"), None);
            assert_eq!(
                crate::builtins::containers::dict_method_bits(py, "get"),
                None
            );
            assert_eq!(
                crate::builtins::exceptions::exception_method_bits(py, "__new__"),
                None
            );
            let observed = molt_exception_last_pending();
            assert_eq!(observed, failure.get());
            molt_exception_clear();
            dec_ref_bits(py, observed);
            dec_ref_bits(py, failure.get());
            assert_eq!(object_method_bits(py, "__getattribute__"), Some(cached));
        });
    }
}
