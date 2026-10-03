/// One native method declaration drives attribute lookup and namespace
/// materialization. Only the requested owner is materialized for __dict__.
macro_rules! native_method_table {
    // Lexicographic native families declare the same six ordinary slots. The
    // generated arms also drive namespace publication; there is no second
    // list of methods for inherited lookup or class dictionaries.
    ($lookup:ident, $publish:ident, $py:ident, $name:ident, [$($extra:ident),*],
     comparison: $family:expr, {$($setup:tt)*}, {$($members:tt)*}) => {
        crate::builtins::methods::native_method_table!(
            $lookup, $publish, $py, $name, [$($extra),*], {$($setup)*}, {
                "__eq__" => crate::builtins::methods::native_comparison_descriptor!($py, $family, Eq),
                "__ne__" => crate::builtins::methods::native_comparison_descriptor!($py, $family, Ne),
                "__lt__" => crate::builtins::methods::native_comparison_descriptor!($py, $family, Lt),
                "__le__" => crate::builtins::methods::native_comparison_descriptor!($py, $family, Le),
                "__gt__" => crate::builtins::methods::native_comparison_descriptor!($py, $family, Gt),
                "__ge__" => crate::builtins::methods::native_comparison_descriptor!($py, $family, Ge),
                $($members)*
            }
        );
    };

    ($lookup:ident, $publish:ident, $py:ident, $name:ident, [$($extra:ident),*],
     {$($setup:tt)*}, {$($member:literal $(if $guard:expr)? => $body:expr,)*}) => {
        pub(crate) fn $lookup($py: &crate::PyToken<'_>, $($extra: u64,)* $name: &str) -> Option<u64> {
            crate::builtins::methods::method_dispatch($py, || {
                $($setup)*
                match $name { $($member $(if $guard)? => $body,)* _ => None }
            })
        }
        pub(crate) fn $publish($py: &crate::PyToken<'_>, $($extra: u64,)*) -> bool {
            for name in [$($member),*] {
                let _ = $lookup($py, $($extra,)* name);
                if crate::exception_pending($py) { return false; }
            }
            true
        }
    };
}
pub(crate) use native_method_table;

// Each expansion has its own typed ABI entry, retaining the declaring family
// when a base descriptor is called explicitly on a subclass receiver.
macro_rules! native_comparison_descriptor {
    ($py:ident, $family:expr, $op:ident) => {{
        use molt_obj_model::sequence_compare::RichCompareOp;
        extern "C" fn invoke(left: u64, right: u64) -> u64 {
            crate::with_gil_entry_nopanic!(py, {
                $family.invoke(py, left, right, RichCompareOp::$op)
            })
        }
        Some(crate::builtins::methods::builtin_func_bits(
            $py,
            crate::builtins::functions::native_callable::NativeCallableSpec::declared(
                crate::builtins::functions::native_callable::NativeCallableKind::WrapperDescriptor,
                $family.owner($py),
                RichCompareOp::$op.method_name(),
            ).with_text_signature("($self, value, /)"),
            fn_addr!(invoke),
            2,
        ))
    }};
}
pub(crate) use native_comparison_descriptor;


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
    // A fully published owner may have had this declaration removed through
    // its admitted C dictionary. None is its authoritative namespace miss.
    if result == Some(crate::MoltObject::none().bits()) {
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
    init_native_callable, set_function_defaults,
};
pub(crate) use core_types::{
    memoryview_method_bits, object_method_bits, range_method_bits, type_method_bits,
};
pub(crate) use dispatch::{builtin_class_method_bits, publish_builtin_class_methods};
pub(crate) use io::file_method_bits;
pub(crate) use numeric::{complex_method_bits, float_method_bits, int_method_bits};
pub(crate) use sequence::{
    bytearray_method_bits, bytes_method_bits, slice_method_bits, string_method_bits,
};
pub(crate) use singletons::{
    ellipsis_bits, is_missing_bits, is_not_implemented_bits, missing_bits, not_implemented_bits,
};
pub(crate) use specialized::{asyncgen_method_bits, coroutine_method_bits, generator_method_bits};

#[cfg(test)]
mod tests {
    use super::specialized::property_method_bits;
    use super::*;
    use crate::builtins::exceptions::molt_exception_last_pending;
    use crate::*;

    #[test]
    fn suspended_execution_classes_publish_only_their_protocol_methods() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let classes = builtin_classes(py);
            assert_ne!(classes.coroutine, classes.coroutine_wrapper);
            assert!(classes.anchors().contains(&classes.coroutine_wrapper));
            assert!(
                !crate::builtins::classes::public_builtin_classes(py)
                    .any(|(_, class)| class == classes.coroutine_wrapper)
            );
            for explicit_object_new in [false, true] {
                let constructed = if explicit_object_new {
                    molt_object_new_bound(classes.coroutine_wrapper)
                } else {
                    unsafe { call_callable0(py, classes.coroutine_wrapper) }
                };
                dec_ref_bits(py, constructed);
                assert!(exception_pending(py));
                let exception = molt_exception_last();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    exception,
                    "TypeError",
                ));
                crate::clear_exception(py);
                dec_ref_bits(py, exception);
            }
            let child_name = attr_name_bits_from_bytes(py, b"ForgedWrapper").unwrap();
            let child = molt_class_new(child_name);
            dec_ref_bits(py, child_name);
            let inherited = molt_class_set_base(child, classes.coroutine_wrapper);
            dec_ref_bits(py, inherited);
            assert!(exception_pending(py));
            let exception = molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "TypeError",
            ));
            crate::clear_exception(py);
            dec_ref_bits(py, exception);
            dec_ref_bits(py, child);
            for (class, required, absent) in [
                (
                    classes.coroutine,
                    &["__await__", "send", "throw", "close"][..],
                    &["__iter__", "__next__"][..],
                ),
                (
                    classes.coroutine_wrapper,
                    &["__iter__", "__next__", "send", "throw", "close"][..],
                    &["__await__"][..],
                ),
                (
                    classes.generator,
                    &["__iter__", "__next__", "send", "throw", "close"][..],
                    &["__await__"][..],
                ),
            ] {
                let directory = molt_dir_builtin(class);
                assert!(!exception_pending(py));
                for name in required {
                    let first =
                        builtin_class_method_bits(py, class, name).expect("required method");
                    assert_eq!(builtin_class_method_bits(py, class, name), Some(first));
                    let name_bits = attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
                    let contains = molt_contains(directory, name_bits);
                    assert!(is_truthy(py, obj_from_bits(contains)));
                    dec_ref_bits(py, contains);
                    dec_ref_bits(py, name_bits);
                }
                for name in absent {
                    assert_eq!(builtin_class_method_bits(py, class, name), None);
                    let name_bits = attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
                    let contains = molt_contains(directory, name_bits);
                    assert!(!is_truthy(py, obj_from_bits(contains)));
                    dec_ref_bits(py, contains);
                    dec_ref_bits(py, name_bits);
                }
                dec_ref_bits(py, directory);
                assert!(!exception_pending(py));
            }
        });
    }

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
            assert_eq!(file_method_bits(py, builtin_classes(py).file, "read"), None);
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
