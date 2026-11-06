use super::*;

extern "C" fn observe_native_initializer_and_raise() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let frame = DcValue::owned(crate::molt_getframe(MoltObject::from_int(0).bits()));
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        for (name, expected) in [(crate::NameId::FLineno, 2), (crate::NameId::FLasti, 0)] {
            let value = DcValue::owned(crate::molt_get_attr_name(frame.bits, name.bits(py)));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            if obj_from_bits(value.bits).as_int() != Some(expected) {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::AssertionError,
                    "invalid native initializer location",
                );
            }
        }
        raise_exception::<u64>(py, BuiltinExc::ValueError, "observed native initializer")
    })
}

#[test]
fn native_initializer_observes_its_entry_and_captures_one_escaping_origin() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let none = MoltObject::none().bits();
        let int = |value| MoltObject::from_int(value).bits();
        let no = MoltObject::from_bool(false).bits();
        let yes = MoltObject::from_bool(true).bits();
        let class_name = text(py, b"NativeInitializerLocation").unwrap();
        let class = dc_result(py, crate::molt_class_new(class_name.bits)).unwrap();
        let receiver = text(py, b"dc_receiver").unwrap();
        let field = text(py, b"dc_value").unwrap();
        let capture = text(py, b"__dataclass_dflt_dc_value__").unwrap();
        let parameters = owned_tuple(py, &[receiver.bits]).unwrap();
        let capture_names = owned_tuple(py, &[capture.bits]).unwrap();
        let factory_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(
                observe_native_initializer_and_raise as *const (),
            ),
            0,
        );
        assert!(!factory_ptr.is_null());
        let factory = DcValue::owned(MoltObject::from_ptr(factory_ptr).bits());
        let captures = owned_tuple(py, &[factory.bits]).unwrap();
        let store = owned_tuple(py, &[field.bits, int(-1), int(-1), int(0), yes]).unwrap();
        let stores = owned_tuple(py, &[store.bits]).unwrap();
        let empty = owned_tuple(py, &[]).unwrap();
        let absent = owned_tuple(py, &[int(-1), int(-1)]).unwrap();
        let dict =
            || DcValue::owned(MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits());
        let builtins = dict();
        let globals = DcValue::owned(
            MoltObject::from_ptr(crate::alloc_dict_with_pairs(
                py,
                &[crate::NameId::DunderBuiltins.bits(py), builtins.bits],
            ))
            .bits(),
        );
        let annotations = dict();
        // One init-disabled factory field, using the stdlib's real schema and
        // index-based specification, with no compiled template candidate.
        let field_shape = owned_tuple(py, &[field.bits, no, no, no, int(2)]).unwrap();
        let fields = owned_tuple(py, &[field_shape.bits]).unwrap();
        let schema = owned_tuple(py, &[no, no, no, fields.bits, receiver.bits]).unwrap();
        let spec = owned_tuple(
            py,
            &[
                parameters.bits,
                int(1),
                none,
                none,
                capture_names.bits,
                captures.bits,
                stores.bits,
                empty.bits,
                absent.bits,
                absent.bits,
                annotations.bits,
                globals.bits,
                none,
                none,
            ],
        )
        .unwrap();
        let function = build_initializer(py, class.bits, schema.bits, spec.bits).unwrap();
        let code =
            unsafe { crate::function_code_bits(obj_from_bits(function.bits).as_ptr().unwrap()) };
        let instance = dc_result(py, crate::molt_object_new_bound(class.bits)).unwrap();
        let result =
            DcValue::owned(unsafe { crate::call_callable1(py, function.bits, instance.bits) });
        assert!(exception_pending(py));
        let exception = DcValue::owned(crate::molt_exception_last_pending());
        assert!(crate::builtins::exceptions::exception_matches_kind(
            py,
            exception.bits,
            BuiltinExc::ValueError
        ));
        let trace =
            unsafe { crate::exception_trace_bits(obj_from_bits(exception.bits).as_ptr().unwrap()) };
        let trace = obj_from_bits(trace)
            .as_ptr()
            .expect("native initializer captured its escape");
        assert_eq!(
            unsafe { crate::object_type_id(trace) },
            crate::TYPE_ID_TRACEBACK_PAYLOAD
        );
        assert_eq!(
            unsafe { crate::builtins::frames::traceback_payload_code_bits(trace) },
            code
        );
        assert_eq!(
            unsafe { crate::builtins::frames::traceback_payload_line(trace) },
            2
        );
        assert!(
            obj_from_bits(unsafe { crate::builtins::frames::traceback_payload_next_bits(trace) })
                .is_none()
        );
        crate::clear_exception(py);
        drop(result);
    });
}

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
