use super::*;
use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};

static EQUALITY_CALLS: AtomicUsize = AtomicUsize::new(0);
static CLEAR_DURING_EQUALITY: AtomicU64 = AtomicU64::new(0);
static RICH_RESULT: AtomicU64 = AtomicU64::new(0);

fn subclass(py: &PyToken<'_>, base: u64) -> u64 {
    let name = attr_name_bits_from_bytes(py, b"ComparedSequence").unwrap();
    let class = molt_class_new(name);
    dec_ref_bits(py, name);
    let result = molt_class_set_base(class, base);
    dec_ref_bits(py, result);
    unsafe { crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()) }
        .expect("valid sequence subclass");
    assert!(!exception_pending(py));
    class
}

fn install(py: &PyToken<'_>, class: u64, name: &[u8], target: *const ()) {
    let name = attr_name_bits_from_bytes(py, name).unwrap();
    let function = MoltObject::from_ptr(alloc_runtime_function_obj(
        py,
        runtime_fn_addr("sequence_test_callback", target),
        2,
    ))
    .bits();
    let result = molt_set_attr_name(class, name, function);
    dec_ref_bits(py, result);
    dec_ref_bits(py, name);
    dec_ref_bits(py, function);
    assert!(!exception_pending(py));
}

extern "C" fn equal_callback(_left: u64, _right: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        EQUALITY_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        let clear = CLEAR_DURING_EQUALITY.load(AtomicOrdering::SeqCst);
        if clear != 0 {
            let result = crate::molt_list_clear(clear);
            dec_ref_bits(py, result);
        }
        MoltObject::from_bool(clear == 0).bits()
    })
}

extern "C" fn decline_callback(_left: u64, _right: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { crate::builtins::methods::not_implemented_bits(py) })
}

extern "C" fn result_callback(_left: u64, _right: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = RICH_RESULT.load(AtomicOrdering::SeqCst);
        inc_ref_bits(py, result);
        result
    })
}

#[test]
fn inherited_sequence_descriptors_cover_every_operation_and_declaring_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let one = MoltObject::from_int(1).bits();
        let two = MoltObject::from_int(2).bits();
        for family in [
            SequenceComparison::List,
            SequenceComparison::Tuple,
            SequenceComparison::String,
            SequenceComparison::Bytes,
            SequenceComparison::Bytearray,
        ] {
            let owner = family.owner(py);
            let (left, right) = match family {
                SequenceComparison::List => (alloc_list(py, &[one]), alloc_list(py, &[two])),
                SequenceComparison::Tuple => (alloc_tuple(py, &[one]), alloc_tuple(py, &[two])),
                SequenceComparison::String => (alloc_string(py, b"a"), alloc_string(py, b"b")),
                SequenceComparison::Bytes => (alloc_bytes(py, b"a"), alloc_bytes(py, b"b")),
                SequenceComparison::Bytearray => {
                    (alloc_bytearray(py, b"a"), alloc_bytearray(py, b"b"))
                }
            };
            let left = MoltObject::from_ptr(left).bits();
            let right = MoltObject::from_ptr(right).bits();
            let class = subclass(py, owner);
            let child = unsafe { call_callable1(py, class, left) };
            assert!(!exception_pending(py));
            assert_eq!(type_of_bits(py, child), class);
            for (name, operator, expected) in [
                ("__eq__", molt_eq as extern "C" fn(u64, u64) -> u64, false),
                ("__ne__", molt_ne, true),
                ("__lt__", molt_lt, true),
                ("__le__", molt_le, true),
                ("__gt__", molt_gt, false),
                ("__ge__", molt_ge, false),
            ] {
                let descriptor =
                    crate::builtins::methods::builtin_class_method_bits(py, owner, name)
                        .expect("declared comparison");
                for lhs in [left, child] {
                    let direct = unsafe { call_callable2(py, descriptor, lhs, right) };
                    assert!(!exception_pending(py), "{name}");
                    assert_eq!(direct, MoltObject::from_bool(expected).bits(), "{name}");
                    dec_ref_bits(py, direct);
                    let generic = operator(lhs, right);
                    assert!(!exception_pending(py), "inherited {name}");
                    assert_eq!(
                        generic,
                        MoltObject::from_bool(expected).bits(),
                        "inherited {name}"
                    );
                    dec_ref_bits(py, generic);
                }
                let declined = unsafe { call_callable2(py, descriptor, child, one) };
                assert!(is_not_implemented_bits(py, declined));
                dec_ref_bits(py, declined);
                let invalid = unsafe { call_callable2(py, descriptor, one, right) };
                dec_ref_bits(py, invalid);
                assert!(exception_pending(py));
                let error = molt_exception_last();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "TypeError"
                ));
                crate::clear_exception(py);
                dec_ref_bits(py, error);
            }
            for value in [child, class, right, left] {
                dec_ref_bits(py, value);
            }
        }
    });
}

#[test]
fn sequence_equality_length_rules_and_mutation_match_cpython() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        CLEAR_DURING_EQUALITY.store(0, AtomicOrdering::SeqCst);
        let class = subclass(py, builtin_classes(py).object);
        install(py, class, b"__eq__", equal_callback as *const ());
        let left = unsafe { call_callable0(py, class) };
        let right = unsafe { call_callable0(py, class) };
        for (a, b, expected_calls) in [
            (
                alloc_tuple(py, &[left]),
                alloc_tuple(py, &[right, right]),
                1,
            ),
            (alloc_list(py, &[left]), alloc_list(py, &[right, right]), 0),
        ] {
            let a = MoltObject::from_ptr(a).bits();
            let b = MoltObject::from_ptr(b).bits();
            EQUALITY_CALLS.store(0, AtomicOrdering::SeqCst);
            assert_eq!(molt_eq(a, b), MoltObject::from_bool(false).bits());
            assert_eq!(EQUALITY_CALLS.load(AtomicOrdering::SeqCst), expected_calls);
            dec_ref_bits(py, b);
            dec_ref_bits(py, a);
        }
        let a = MoltObject::from_ptr(alloc_list(py, &[left])).bits();
        let b = MoltObject::from_ptr(alloc_list(py, &[right])).bits();
        CLEAR_DURING_EQUALITY.store(a, AtomicOrdering::SeqCst);
        EQUALITY_CALLS.store(0, AtomicOrdering::SeqCst);
        // The equality callback clears lhs: [] < [right] is true. Reusing the
        // old pair would raise TypeError because its elements cannot order.
        assert_eq!(molt_lt(a, b), MoltObject::from_bool(true).bits());
        assert!(!exception_pending(py));
        assert_eq!(EQUALITY_CALLS.load(AtomicOrdering::SeqCst), 1);
        CLEAR_DURING_EQUALITY.store(0, AtomicOrdering::SeqCst);
        for value in [b, a, right, left, class] {
            dec_ref_bits(py, value);
        }
    });
}

#[test]
fn sequence_overrides_reflect_and_notimplemented_falls_back_to_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = subclass(py, builtin_classes(py).tuple);
        let values =
            MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_int(1).bits()])).bits();
        let a = unsafe { call_callable1(py, class, values) };
        let b = unsafe { call_callable1(py, class, values) };
        install(py, class, b"__eq__", decline_callback as *const ());
        install(py, class, b"__ne__", decline_callback as *const ());
        assert_eq!(molt_eq(a, b), MoltObject::from_bool(false).bits());
        assert_eq!(molt_ne(a, b), MoltObject::from_bool(true).bits());
        assert_eq!(molt_eq(a, a), MoltObject::from_bool(true).bits());
        let result = MoltObject::from_ptr(alloc_list(py, &[])).bits();
        RICH_RESULT.store(result, AtomicOrdering::SeqCst);
        install(py, class, b"__ge__", result_callback as *const ());
        let reflected = molt_le(values, a);
        assert_eq!(reflected, result);
        dec_ref_bits(py, reflected);
        // Explicit base descriptor bypasses the subclass override.
        let slot = crate::builtins::methods::builtin_class_method_bits(
            py,
            builtin_classes(py).tuple,
            "__ge__",
        )
        .unwrap();
        assert_eq!(
            unsafe { call_callable2(py, slot, a, values) },
            MoltObject::from_bool(true).bits()
        );
        assert!(!exception_pending(py));
        RICH_RESULT.store(0, AtomicOrdering::SeqCst);
        for value in [result, b, a, values, class] {
            dec_ref_bits(py, value);
        }
    });
}

#[test]
fn bytearray_declaring_comparison_uses_owned_buffer_protocol() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        use crate::builtins::array_mod::{molt_array_append, molt_array_new};
        use molt_obj_model::sequence_compare::RichCompareOp;
        let format = MoltObject::from_ptr(alloc_string(py, b"B")).bits();
        let array = molt_array_new(format);
        let appended = molt_array_append(array, MoltObject::from_int(97).bits());
        dec_ref_bits(py, appended);
        let left = MoltObject::from_ptr(alloc_bytearray(py, b"a")).bits();
        assert_eq!(
            SequenceComparison::Bytearray.invoke(py, left, array, RichCompareOp::Eq),
            MoltObject::from_bool(true).bits()
        );
        // No retained buffer export may prevent a subsequent resize.
        let appended = molt_array_append(array, MoltObject::from_int(98).bits());
        dec_ref_bits(py, appended);
        assert!(!exception_pending(py));
        assert_eq!(
            SequenceComparison::Bytearray.invoke(py, left, array, RichCompareOp::Lt),
            MoltObject::from_bool(true).bits()
        );
        let view = crate::molt_memoryview_new(array);
        let released = crate::molt_memoryview_release(view);
        dec_ref_bits(py, released);
        let declined = SequenceComparison::Bytearray.invoke(py, left, view, RichCompareOp::Eq);
        assert!(is_not_implemented_bits(py, declined));
        assert!(!exception_pending(py));
        dec_ref_bits(py, declined);
        for value in [view, left, array, format] {
            dec_ref_bits(py, value);
        }
    });
}

#[test]
fn identifier_storage_equality_does_not_invoke_string_subclass_comparison() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = subclass(py, builtin_classes(py).str);
        let name = MoltObject::from_ptr(alloc_string(py, b"ordinary")).bits();
        let other = MoltObject::from_ptr(alloc_string(py, b"__annotations__")).bits();
        let child = unsafe { call_callable1(py, class, name) };
        assert!(!exception_pending(py));
        install(py, class, b"__eq__", equal_callback as *const ());
        CLEAR_DURING_EQUALITY.store(0, AtomicOrdering::SeqCst);
        EQUALITY_CALLS.store(0, AtomicOrdering::SeqCst);
        assert_eq!(molt_eq(child, other), MoltObject::from_bool(true).bits());
        assert_eq!(EQUALITY_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            molt_string_eq(child, name),
            MoltObject::from_bool(true).bits()
        );
        assert_eq!(
            molt_string_eq(child, other),
            MoltObject::from_bool(false).bits()
        );
        assert_eq!(EQUALITY_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert!(!exception_pending(py));
        for value in [child, other, name, class] {
            dec_ref_bits(py, value);
        }
    });
}
