use super::*;

#[test]
fn code_reconstruction_retains_its_body_plan_and_terminal_release_retires_it() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let name = text(py, b"__init__").unwrap();
        let filename = text(py, b"<string>").unwrap();
        let empty = owned_tuple(py, &[]).unwrap();
        let source = crate::alloc_code_obj(
            py,
            filename.bits,
            name.bits,
            2,
            MoltObject::none().bits(),
            empty.bits,
            empty.bits,
            0,
            0,
            0,
        );
        assert!(!source.is_null());
        let plan = Arc::new(InitPlan {
            stores: vec![],
            post_init: None,
            factory_marker: None,
            frozen_object: None,
            arity: 0,
            cells: 0,
            frame: crate::builtins::frames::FramePlan::default(),
        });
        unsafe {
            code_plan_word(source).write(Arc::into_raw(plan.clone()));
        }
        // A retained body plan must not change execution-kind metadata.
        assert!(!unsafe { crate::object::layout::code_is_module(source) });
        let clone =
            unsafe { crate::object::builders::clone_code_obj_with_protocol_flags(py, source, 0) };
        assert!(!clone.is_null());
        assert!(!unsafe { crate::object::layout::code_is_module(clone) });
        assert_eq!(Arc::strong_count(&plan), 3);
        assert_eq!(unsafe { *code_plan_word(clone) }, Arc::as_ptr(&plan));
        dec_ref_bits(py, MoltObject::from_ptr(source).bits());
        assert_eq!(Arc::strong_count(&plan), 2);
        // The clone still owns the body independently of the original code.
        assert_eq!(unsafe { (**code_plan_word(clone)).arity }, 0);
        dec_ref_bits(py, MoltObject::from_ptr(clone).bits());
        assert_eq!(Arc::strong_count(&plan), 1);
        assert!(!exception_pending(py));
    });
}
