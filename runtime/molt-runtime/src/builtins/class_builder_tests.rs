use super::*;
use crate::builtins::frames::{frame_stack_pop, frame_stack_push_owned};
use crate::object::ops::{dict_del_in_place, dict_set_in_place};

#[test]
fn class_builder_identity_restore_and_missing_binding() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        crate::with_gil_entry_nopanic!(py, {
            // Compiled module construction owns the intrinsic registry.
            let module_name = crate::NameId::Builtins.bits(py);
            let module = crate::molt_module_new_attached_body(py, module_name);
            assert!(!exception_pending(py));
            let canonical = crate::intrinsics::registry::class_builder_bits(py)
                .expect("canonical public builder");
            // The canonical object survives a detached intrinsic namespace;
            // registry materialization must not create a second identity.
            let anchor = &runtime_state(py).intrinsic_registry_module;
            let registry = anchor.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
            let detached = crate::intrinsics::registry::class_builder_bits(py);
            anchor.store(registry, std::sync::atomic::Ordering::Release);
            if let Some(bits) = detached {
                dec_ref_bits(py, bits);
            }
            assert_eq!(detached, Some(canonical));
            let globals = alloc_dict_with_pairs(py, &[]);
            let builtins = alloc_dict_with_pairs(py, &[]);
            let key = crate::NameId::DunderBuildClass.bits(py);
            assert!(unsafe { dict_set_in_place(py, builtins, key, canonical) });
            frame_stack_push_owned(
                py,
                0,
                MoltObject::from_ptr(globals).bits(),
                MoltObject::from_ptr(builtins).bits(),
                0,
            );
            molt_class_builder_check_attached_body(py);
            assert!(!exception_pending(py));
            assert!(unsafe { dict_set_in_place(py, builtins, key, MoltObject::none().bits()) });
            molt_class_builder_check_attached_body(py);
            let error = crate::molt_exception_last_pending_attached_body(py);
            assert!(crate::builtins::exceptions::exception_matches_kind(
                py,
                error,
                BuiltinExc::TypeError
            ));
            dec_ref_bits(py, error);
            clear_exception(py);
            assert!(unsafe { dict_set_in_place(py, builtins, key, canonical) });
            molt_class_builder_check_attached_body(py);
            assert!(
                !exception_pending(py),
                "restoring the exact object is admitted"
            );
            assert_eq!(unsafe { dict_del_in_place(py, builtins, key) }, Ok(true));
            molt_class_builder_check_attached_body(py);
            let error = crate::molt_exception_last_pending_attached_body(py);
            assert!(crate::builtins::exceptions::exception_matches_kind(
                py,
                error,
                BuiltinExc::NameError
            ));
            dec_ref_bits(py, error);
            clear_exception(py);
            frame_stack_pop(py);
            dec_ref_bits(py, canonical);
            dec_ref_bits(py, module);
        });
    });
}

#[test]
fn direct_builder_call_raises_the_static_model_exclusion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        molt_build_class_builtin_attached_body(
            py,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        );
        let error = crate::molt_exception_last_pending_attached_body(py);
        assert!(crate::builtins::exceptions::exception_matches_kind(
            py,
            error,
            BuiltinExc::TypeError
        ));
        dec_ref_bits(py, error);
        clear_exception(py);
    });
}
