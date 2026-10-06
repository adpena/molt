use super::*;
use crate::object::builders::alloc_list;

#[test]
fn trace_call_type_builder_gate_requires_explicit_opt_in() {
    assert!(!trace_call_type_builder_enabled_raw(None));
    assert!(!trace_call_type_builder_enabled_raw(Some("0")));
    assert!(!trace_call_type_builder_enabled_raw(Some("true")));
    assert!(trace_call_type_builder_enabled_raw(Some("1")));
}

#[test]
fn resolve_construct_after_init_no_pending_returns_instance_unchanged() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let list_ptr = alloc_list(_py, &[MoltObject::from_int(7).bits()]);
        assert!(!list_ptr.is_null());
        let inst_bits = MoltObject::from_ptr(list_ptr).bits();
        let before =
            unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };
        assert_eq!(crate::molt_exception_pending(), 0, "no exception expected");
        // No pending exception: the owning reference is handed back as-is.
        let out = unsafe {
            crate::call::class_init::resolve_construct_after_init(
                _py,
                inst_bits,
                MoltObject::none().bits(),
            )
        };
        assert_eq!(out, inst_bits, "must return the constructed instance");
        let after = unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };
        assert_eq!(after, before, "success path must not perturb the refcount");
        dec_ref_bits(_py, inst_bits);
    });
}

#[test]
fn resolve_construct_after_init_pending_drops_instance_and_returns_none() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        // Hold an extra owning reference so the helper's drop is observable
        // without freeing the object out from under the test.
        let list_ptr = alloc_list(_py, &[MoltObject::from_int(9).bits()]);
        assert!(!list_ptr.is_null());
        let inst_bits = MoltObject::from_ptr(list_ptr).bits();
        crate::call::bind::inc_ref_bits(_py, inst_bits);
        let before =
            unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };

        // Simulate `__init__` having raised: set a pending exception, then
        // resolve. The helper must drop the instance's owning reference and
        // surface the raise via a `none` result.
        let _: u64 =
            crate::builtins::exceptions::raise_exception(_py, "ValueError", "task60 init raise");
        assert_eq!(
            crate::molt_exception_pending(),
            1,
            "exception must be pending"
        );

        let out = unsafe {
            crate::call::class_init::resolve_construct_after_init(
                _py,
                inst_bits,
                MoltObject::none().bits(),
            )
        };
        assert!(
            MoltObject::from_bits(out).is_none(),
            "a pending __init__ exception must yield the None sentinel, not the instance"
        );
        assert_eq!(
            crate::molt_exception_pending(),
            1,
            "the helper must not clear the pending exception — the caller propagates it"
        );
        let after = unsafe { (*crate::object::header_from_obj_ptr(list_ptr)).ref_count_snapshot() };
        assert_eq!(
            after,
            before - 1,
            "the exception path must drop exactly one (the instance's) owning reference"
        );

        let _ = crate::molt_exception_clear();
        assert_eq!(crate::molt_exception_pending(), 0);
        // Release the extra reference taken above.
        dec_ref_bits(_py, inst_bits);
    });
}

#[test]
fn resolve_construct_after_init_rejects_and_consumes_non_none_result() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let inst_ptr = alloc_list(_py, &[MoltObject::from_int(11).bits()]);
        let result_ptr = alloc_list(_py, &[MoltObject::from_int(13).bits()]);
        assert!(!inst_ptr.is_null());
        assert!(!result_ptr.is_null());
        let inst_bits = MoltObject::from_ptr(inst_ptr).bits();
        let result_bits = MoltObject::from_ptr(result_ptr).bits();
        crate::call::bind::inc_ref_bits(_py, inst_bits);
        crate::call::bind::inc_ref_bits(_py, result_bits);
        let inst_before =
            unsafe { (*crate::object::header_from_obj_ptr(inst_ptr)).ref_count_snapshot() };
        let result_before =
            unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() };

        let out = unsafe {
            crate::call::class_init::resolve_construct_after_init(_py, inst_bits, result_bits)
        };
        assert!(MoltObject::from_bits(out).is_none());
        assert_eq!(crate::molt_exception_pending(), 1);
        assert_eq!(
            unsafe { (*crate::object::header_from_obj_ptr(inst_ptr)).ref_count_snapshot() },
            inst_before - 1,
            "invalid __init__ return must consume the constructed instance"
        );
        assert_eq!(
            unsafe { (*crate::object::header_from_obj_ptr(result_ptr)).ref_count_snapshot() },
            result_before - 1,
            "invalid __init__ return must consume its owned call result"
        );

        let _ = crate::molt_exception_clear();
        dec_ref_bits(_py, inst_bits);
        dec_ref_bits(_py, result_bits);
    });
}
