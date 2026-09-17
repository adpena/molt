//! Namespace custody is independent of call shape, source names and suspension.
use super::*;
use crate::dict_set_in_place;
use crate::object::layout::{
    code_set_frame_slot_id, function_builtins_bits, function_globals_bits, function_set_code_bits,
    function_set_globals_bits,
};

fn dict(py: &PyToken<'_>) -> u64 {
    let ptr = alloc_dict_with_pairs(py, &[]);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn refs(bits: u64) -> u32 {
    unsafe {
        (*crate::header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
    }
}

fn bind_code_target(py: &PyToken<'_>, code: u64, target: u64) {
    let function = crate::builtins::functions::alloc_runtime_function_obj(py, target, 0);
    assert!(!function.is_null());
    assert!(unsafe { function_set_code_bits(py, function, code) });
    dec_ref_bits(py, MoltObject::from_ptr(function).bits());
}

#[test]
fn locals_identity_and_snapshot_contents_follow_runtime_target_version() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let state = crate::runtime_state(py);
        let saved_version = state.sys_version_info.lock().unwrap().clone();
        for minor in [12, 13, 14] {
            *state.sys_version_info.lock().unwrap() =
                Some(crate::state::runtime_state::PythonVersionInfo {
                    major: 3,
                    minor,
                    micro: 0,
                    releaselevel: "final".to_string(),
                    serial: 0,
                });
            for module in [false, true] {
                let name = if module {
                    b"<module>".as_slice()
                } else {
                    b"optimized".as_slice()
                };
                let name = MoltObject::from_ptr(crate::alloc_string(py, name)).bits();
                let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
                let code = MoltObject::from_ptr(crate::alloc_code_obj(
                    py,
                    name,
                    name,
                    1,
                    MoltObject::none().bits(),
                    empty,
                    empty,
                    0,
                    0,
                    0,
                ))
                .bits();
                let namespace = dict(py);
                let key = MoltObject::from_ptr(crate::alloc_string(py, b"value")).bits();
                let namespace_ptr = obj_from_bits(namespace).as_ptr().unwrap();
                unsafe {
                    dict_set_in_place(py, namespace_ptr, key, MoltObject::from_int(1).bits())
                };
                crate::molt_code_slots_init(1);
                crate::molt_code_slot_set(0, code, namespace);
                crate::molt_trace_enter_slot(0);
                molt_frame_locals_set(namespace);
                let first = molt_locals_builtin();
                unsafe {
                    dict_set_in_place(py, namespace_ptr, key, MoltObject::from_int(2).bits())
                };
                let second = molt_locals_builtin();
                let aliases = module || minor == 12;
                assert_eq!(first == namespace, aliases, "3.{minor}, module={module}");
                assert_eq!(first == second, aliases, "3.{minor}, module={module}");
                unsafe {
                    assert_eq!(
                        crate::dict_get_in_place(py, obj_from_bits(first).as_ptr().unwrap(), key),
                        Some(MoltObject::from_int(if aliases { 2 } else { 1 }).bits())
                    );
                    assert_eq!(
                        crate::dict_get_in_place(py, obj_from_bits(second).as_ptr().unwrap(), key),
                        Some(MoltObject::from_int(2).bits())
                    );
                }
                assert!(!crate::exception_pending(py));
                crate::molt_trace_exit();
                crate::molt_code_slots_init(0);
                for bits in [first, second, key, namespace, code, empty, name] {
                    dec_ref_bits(py, bits);
                }
            }
        }
        *state.sys_version_info.lock().unwrap() = saved_version;
    });
}

fn dict_subclass_instance(py: &PyToken<'_>) -> (u64, u64) {
    let name = MoltObject::from_ptr(crate::alloc_string(py, b"FrameGlobalsDict")).bits();
    let namespace = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
    let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[builtin_classes(py).dict])).bits();
    let class = crate::builtins::types::molt_type_new(
        builtin_classes(py).type_obj,
        name,
        bases,
        namespace,
        MoltObject::none().bits(),
    );
    assert!(!crate::exception_pending(py));
    let class_ptr = obj_from_bits(class).as_ptr().expect("dict subclass");
    let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
    let instance_ptr = obj_from_bits(instance)
        .as_ptr()
        .expect("dict subclass instance");
    assert_eq!(
        unsafe { object_type_id(instance_ptr) },
        crate::TYPE_ID_OBJECT
    );
    assert_eq!(unsafe { crate::object_class_bits(instance_ptr) }, class);
    for bits in [bases, namespace, name] {
        dec_ref_bits(py, bits);
    }
    (instance, class)
}

extern "C" fn builtins_mapping_error(_self: u64, _key: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        raise_exception::<u64>(py, "ValueError", "custom builtins lookup failed")
    })
}

fn error_mapping_instance(py: &PyToken<'_>) -> (u64, u64, u64) {
    let function = crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::builtins::functions::runtime_fn_addr(
            "builtins_mapping_error",
            builtins_mapping_error as *const (),
        ),
        2,
    );
    assert!(!function.is_null());
    let function = MoltObject::from_ptr(function).bits();
    let name = MoltObject::from_ptr(crate::alloc_string(py, b"BuiltinsMapping")).bits();
    let getitem = MoltObject::from_ptr(crate::alloc_string(py, b"__getitem__")).bits();
    let namespace =
        MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[getitem, function])).bits();
    let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[builtin_classes(py).object])).bits();
    let class = crate::builtins::types::molt_type_new(
        builtin_classes(py).type_obj,
        name,
        bases,
        namespace,
        MoltObject::none().bits(),
    );
    assert!(!crate::exception_pending(py));
    let class_ptr = obj_from_bits(class).as_ptr().expect("mapping class");
    let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
    for bits in [bases, namespace, getitem, name] {
        dec_ref_bits(py, bits);
    }
    (instance, class, function)
}

fn clear_expected_exception(py: &PyToken<'_>, name: &str) {
    assert!(crate::exception_pending(py));
    let exception = crate::molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        py, exception, name
    ));
    crate::clear_exception(py);
    dec_ref_bits(py, exception);
}

#[test]
fn compiled_slots_hold_atomic_owned_pairs_and_release_after_detachment() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, first_code) = super::tests::alloc_test_code(py);
        let (_, second_code) = super::tests::alloc_test_code(py);
        let first_globals = dict(py);
        let second_globals = dict(py);
        let slot = CompiledCodeSlot::default();
        slot.replace(
            py,
            CodeNamespace {
                code_bits: first_code,
                globals_bits: first_globals,
            },
        );
        let acquired = slot.acquire(py);
        slot.replace(
            py,
            CodeNamespace {
                code_bits: second_code,
                globals_bits: second_globals,
            },
        );
        assert_eq!(
            [acquired.code_bits, acquired.globals_bits],
            [first_code, first_globals]
        );
        assert_eq!(refs(first_globals), 2);
        acquired.release(py);
        assert_eq!(refs(first_globals), 1);
        let detached = slot.take(py);
        assert_eq!(slot.acquire(py).code_bits, 0);
        detached.release(py);
        for bits in [first_code, second_code, first_globals, second_globals] {
            assert_eq!(refs(bits), 1);
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn invocation_handoff_is_keyed_nested_single_use_and_balanced_when_unconsumed() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (first_ptr, first_code) = super::tests::alloc_test_code(py);
        let (second_ptr, second_code) = super::tests::alloc_test_code(py);
        unsafe {
            code_set_frame_slot_id(first_ptr, 0);
            code_set_frame_slot_id(second_ptr, 1);
        }
        bind_code_target(py, first_code, 101);
        bind_code_target(py, second_code, 102);
        let first = dict(py);
        let second = dict(py);
        let outer =
            FrameInvocationGuard::for_suspended_namespace(py, first_code, first, first).unwrap();
        assert!(take_invocation_namespace(1).is_none());
        {
            let inner =
                FrameInvocationGuard::for_suspended_namespace(py, second_code, second, second)
                    .unwrap();
            assert!(acquire_pending_invocation_context(py, 101).is_none());
            let captured = acquire_pending_invocation_context(py, 102).unwrap();
            assert_eq!(captured, [second, second, second_code]);
            assert_eq!(refs(second), 5);
            for bits in captured {
                dec_ref_bits(py, bits);
            }
            assert!(take_invocation_namespace(0).is_none());
            let [globals, builtins, code] = take_invocation_namespace(1).unwrap();
            assert_eq!([globals, builtins, code], [second, second, second_code]);
            dec_ref_bits(py, globals);
            dec_ref_bits(py, builtins);
            dec_ref_bits(py, code);
            assert!(take_invocation_namespace(1).is_none());
            assert!(acquire_pending_invocation_context(py, 102).is_none());
            drop(inner);
        }
        let token = outer.into_token();
        assert_eq!(refs(first), 3);
        FrameInvocationGuard::exit_token(py, token);
        assert_eq!(refs(first), 1);
        let consumed =
            FrameInvocationGuard::for_suspended_namespace(py, first_code, first, second).unwrap();
        let [globals, builtins, code] = take_invocation_namespace(0).unwrap();
        assert!(
            take_invocation_namespace(0).is_none(),
            "a recursive symbol entry cannot consume twice"
        );
        drop(consumed);
        dec_ref_bits(py, globals);
        dec_ref_bits(py, builtins);
        dec_ref_bits(py, code);
        for bits in [first_code, second_code, first, second] {
            assert_eq!(refs(bits), 1);
            dec_ref_bits(py, bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
extern "C" fn namespace_probe() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::molt_trace_enter_slot(0);
        assert_eq!(
            FRAME_STACK.with(|stack| stack.borrow().len()),
            1,
            "dispatch must not create a frame"
        );
        let globals = super::molt_globals_builtin();
        crate::molt_trace_exit();
        assert!(!crate::exception_pending(py));
        globals
    })
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn object_and_typed_invocation_share_one_frame_and_preserve_rebound_namespace() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let lexical = dict(py);
        let rebound = dict(py);
        crate::molt_code_slots_init(1);
        crate::molt_code_slot_set(0, code, lexical);
        let address = namespace_probe as *const () as usize as u64;
        let function = crate::builtins::functions::alloc_runtime_function_obj(py, address, 0);
        unsafe {
            assert!(function_set_code_bits(py, function, code));
            function_set_globals_bits(py, function, rebound);
        }
        let callable = MoltObject::from_ptr(function).bits();
        for dispatch in 0..4 {
            let result = match dispatch {
                0 => crate::molt_call_func_fast0(callable),
                1 => unsafe { crate::call::function::call_function_obj0(py, callable) },
                2 => unsafe {
                    crate::molt_guarded_call_obj(address, std::ptr::null(), 0, callable)
                },
                _ => {
                    let token = crate::molt_frame_invocation_enter(callable);
                    assert_ne!(token, 0);
                    let result = namespace_probe();
                    crate::molt_frame_invocation_exit(token);
                    result
                }
            };
            assert_eq!(result, rebound, "dispatch {dispatch}");
            dec_ref_bits(py, result);
        }
        let direct = namespace_probe();
        assert_eq!(direct, lexical);
        dec_ref_bits(py, direct);
        assert!(FRAME_STACK.with(|stack| stack.borrow().is_empty()));
        for bits in [callable, code, lexical, rebound] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn function_type_code_slot_and_invocation_preserve_dict_subclass_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let (globals, globals_class) = dict_subclass_instance(py);
        let builtins = dict(py);
        let builtins_name = MoltObject::from_ptr(crate::alloc_string(py, b"__builtins__")).bits();
        let globals_storage = globals_namespace_storage_ptr(py, globals).unwrap();
        unsafe { dict_set_in_place(py, globals_storage, builtins_name, builtins) };

        let target = namespace_probe as *const () as usize as u64;
        bind_code_target(py, code, target);
        let callable = unsafe {
            crate::builtins::functions::function_type_new_from_args(py, &[code, globals])
        };
        assert!(!crate::exception_pending(py));
        let callable_ptr = obj_from_bits(callable)
            .as_ptr()
            .expect("FunctionType result");
        assert_eq!(unsafe { function_globals_bits(callable_ptr) }, globals);
        assert_eq!(unsafe { function_builtins_bits(callable_ptr) }, builtins);

        crate::molt_code_slots_init(1);
        let set = crate::molt_code_slot_set(0, code, globals);
        assert!(obj_from_bits(set).is_none());
        assert!(!crate::exception_pending(py));

        let token = crate::molt_frame_invocation_enter(callable);
        assert_ne!(token, 0);
        let observed = namespace_probe();
        crate::molt_frame_invocation_exit(token);
        assert_eq!(observed, globals);
        assert_eq!(
            unsafe { crate::object_class_bits(obj_from_bits(observed).as_ptr().unwrap()) },
            globals_class
        );
        dec_ref_bits(py, observed);

        for bits in [
            callable,
            builtins_name,
            builtins,
            globals,
            globals_class,
            code,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn captured_builtins_values_survive_suspension_and_lookup_never_falls_back() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let builtins_name = MoltObject::from_ptr(crate::alloc_string(py, b"__builtins__")).bits();
        let missing_name =
            MoltObject::from_ptr(crate::alloc_string(py, b"captured_missing")).bits();
        let (mapping, mapping_class, mapping_function) = error_mapping_instance(py);
        let cases = [
            (MoltObject::none().bits(), "TypeError"),
            (MoltObject::from_int(41).bits(), "TypeError"),
            (mapping, "ValueError"),
        ];

        for (captured_value, expected_error) in cases {
            let globals = dict(py);
            let globals_ptr = obj_from_bits(globals).as_ptr().unwrap();
            unsafe { dict_set_in_place(py, globals_ptr, builtins_name, captured_value) };
            let captured = frame_effective_builtins_bits(py, globals);
            assert_eq!(captured, captured_value);

            let task =
                crate::molt_task_new(1, crate::GEN_CONTROL_SIZE as u64, crate::TASK_KIND_FUTURE);
            let task_ptr = obj_from_bits(task).as_ptr().expect("suspended task");
            assert!(unsafe {
                crate::object::aux_header::object_init_frame_context_unpublished(
                    py, task_ptr, globals, captured, code,
                )
            });
            assert_eq!(
                crate::object::aux_header::object_frame_context_bits(task_ptr),
                [globals, captured, code]
            );

            inc_ref_bits(py, globals);
            inc_ref_bits(py, captured);
            inc_ref_bits(py, code);
            frame_stack_push_owned(py, code, globals, captured);
            let result = crate::builtins::modules::molt_module_get_global(
                MoltObject::none().bits(),
                missing_name,
            );
            frame_stack_pop(py);
            assert!(obj_from_bits(result).is_none());
            clear_expected_exception(py, expected_error);

            dec_ref_bits(py, task);
            dec_ref_bits(py, globals);
        }

        for bits in [
            mapping,
            mapping_class,
            mapping_function,
            missing_name,
            builtins_name,
            code,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn captured_builtins_module_normalizes_to_its_namespace() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let globals = dict(py);
        let globals_ptr = obj_from_bits(globals).as_ptr().unwrap();
        let builtins_name = MoltObject::from_ptr(crate::alloc_string(py, b"__builtins__")).bits();
        let module_name =
            MoltObject::from_ptr(crate::alloc_string(py, b"captured_builtins")).bits();
        let module = crate::builtins::modules::molt_module_new(module_name);
        let module_ptr = obj_from_bits(module).as_ptr().expect("builtins module");
        let module_namespace = unsafe { module_dict_bits(module_ptr) };
        unsafe { dict_set_in_place(py, globals_ptr, builtins_name, module) };

        assert_eq!(frame_effective_builtins_bits(py, globals), module_namespace);
        assert_ne!(module_namespace, module);

        for bits in [module, module_name, builtins_name, globals] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn lazy_traceback_keeps_namespaces_and_locals_after_live_frame_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let globals = dict(py);
        let locals = dict(py);
        let builtins = dict(py);
        for bits in [code, globals, builtins] {
            inc_ref_bits(py, bits);
        }
        frame_stack_push_owned(py, code, globals, builtins);
        frame_stack_set_locals_dict(py, locals);
        let payload = frame_stack_trace_payload_bits(py, None, false).unwrap();
        frame_stack_pop(py);
        let entry =
            unsafe { traceback_payload_frame_entry(obj_from_bits(payload).as_ptr().unwrap()) };
        assert_eq!(
            [entry.globals_bits, entry.locals_bits, entry.builtins_bits],
            [globals, locals, builtins]
        );
        let frame =
            unsafe { alloc_frame_obj(py, entry, 1, MoltObject::none().bits(), -1).unwrap() };
        assert_eq!(
            unsafe { crate::object_class_bits(obj_from_bits(frame).as_ptr().unwrap()) },
            builtin_classes(py).frame
        );
        let frame_dict = unsafe { instance_dict_bits(obj_from_bits(frame).as_ptr().unwrap()) };
        for (name, expected) in [
            (b"f_globals".as_slice(), globals),
            (b"f_locals".as_slice(), locals),
            (b"f_builtins".as_slice(), builtins),
        ] {
            let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
            assert_eq!(
                unsafe { dict_get_in_place(py, obj_from_bits(frame_dict).as_ptr().unwrap(), key) },
                Some(expected)
            );
            dec_ref_bits(py, key);
        }
        dec_ref_bits(py, frame);
        dec_ref_bits(py, payload);
        for bits in [code, globals, locals, builtins] {
            assert_eq!(refs(bits), 1);
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn runtime_native_tasks_never_fabricate_python_frames() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for kind in [
            crate::TASK_KIND_GENERATOR,
            crate::TASK_KIND_COROUTINE,
            crate::TASK_KIND_FUTURE,
        ] {
            let task = crate::molt_task_new(0, crate::GEN_CONTROL_SIZE as u64, kind);
            let ptr = obj_from_bits(task).as_ptr().unwrap();
            assert_eq!(
                unsafe { suspended_frame_bits(py, ptr, -1) },
                MoltObject::none().bits()
            );
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, task);
        }
    });
}

#[test]
fn frame_materialization_denial_never_looks_like_an_absent_frame() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    struct RestoreBudget;
    impl Drop for RestoreBudget {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let globals = dict(py);
        let locals = dict(py);
        let builtins = dict(py);
        let entry = FrameEntry {
            code_bits: code,
            globals_bits: globals,
            builtins_bits: builtins,
            locals_bits: locals,
            line: 1,
            col_offset: -1,
            end_col_offset: -1,
            python_context: PythonFrameContext::default(),
        };
        let construct = || unsafe { alloc_frame_obj(py, entry, 1, MoltObject::none().bits(), -1) };
        // Warm interned field names and sealed class metadata before denial.
        dec_ref_bits(py, construct().unwrap());
        let owned = [code, globals, locals, builtins];
        let baseline = owned.map(refs);
        let mut failures = 0;
        let mut completed = false;
        for limit in 0..=16 {
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_allocations: Some(limit),
                ..Default::default()
            })));
            let budget = RestoreBudget;
            match construct() {
                Some(frame) => {
                    assert!(!crate::exception_pending(py));
                    dec_ref_bits(py, frame);
                    completed = true;
                }
                None => {
                    failures += 1;
                    assert!(
                        crate::exception_pending(py),
                        "silent frame failure at budget {limit}"
                    );
                }
            }
            assert_eq!(
                owned.map(refs),
                baseline,
                "failed or retired frame must release its edges"
            );
            drop(budget);
            crate::clear_exception(py);
            if completed {
                break;
            }
        }
        assert!(completed);
        assert!(
            failures >= 5,
            "exercise frame, dict and all backing buffers"
        );
        for bits in owned {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn invocation_retains_exact_code_when_a_later_definition_replaces_the_slot() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, original) = super::tests::alloc_test_code(py);
        let (_, replacement) = super::tests::alloc_test_code(py);
        bind_code_target(py, original, 103);
        bind_code_target(py, replacement, 103);
        let globals = dict(py);
        crate::molt_code_slots_init(1);
        crate::molt_code_slot_set(0, original, globals);
        let invocation = FrameInvocationGuard::for_namespace(py, original, globals).unwrap();
        crate::molt_code_slot_set(0, replacement, globals);
        let pending = acquire_pending_invocation_context(py, 103).unwrap();
        assert_eq!(pending[2], original);
        for bits in pending {
            dec_ref_bits(py, bits);
        }
        assert_eq!(crate::molt_trace_enter_slot(0), original);
        assert_eq!(
            FRAME_STACK.with(|stack| stack.borrow().last().unwrap().code_bits),
            original
        );
        crate::molt_trace_exit();
        drop(invocation);
        assert_eq!(refs(original), 1);
        for bits in [original, replacement, globals] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn failed_task_allocation_releases_acquired_context_without_consuming_invocation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (code_ptr, code) = super::tests::alloc_test_code(py);
        unsafe {
            code_set_frame_slot_id(code_ptr, 0);
        }
        bind_code_target(py, code, 104);
        let globals = dict(py);
        let builtins = dict(py);
        for kind in [
            crate::TASK_KIND_FUTURE,
            crate::TASK_KIND_COROUTINE,
            crate::TASK_KIND_GENERATOR,
        ] {
            let invocation =
                FrameInvocationGuard::for_suspended_namespace(py, code, globals, builtins).unwrap();
            // Exceeds every supported address space or overflows the header addition;
            // no machine-wide allocator exhaustion is required to prove the failure.
            assert_eq!(
                crate::molt_task_new(104, u64::MAX, kind),
                MoltObject::none().bits()
            );
            assert!(crate::exception_pending(py));
            crate::molt_exception_clear();
            for bits in [code, globals, builtins] {
                assert_eq!(
                    refs(bits),
                    2,
                    "only the caller and invocation retain custody"
                );
            }
            let pending = acquire_pending_invocation_context(py, 104).unwrap();
            assert_eq!(pending, [globals, builtins, code]);
            for bits in pending {
                dec_ref_bits(py, bits);
            }
            drop(invocation);
            assert!(acquire_pending_invocation_context(py, 104).is_none());
            for bits in [code, globals, builtins] {
                assert_eq!(refs(bits), 1);
            }
        }
        for bits in [code, globals, builtins] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn invalid_compiled_namespace_is_diagnosed_without_publishing_an_invocation() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (ptr, code) = super::tests::alloc_test_code(py);
        unsafe {
            code_set_frame_slot_id(ptr, 0);
        }
        assert!(FrameInvocationGuard::for_namespace(py, code, MoltObject::none().bits()).is_none());
        assert!(crate::exception_pending(py));
        assert!(take_invocation_namespace(0).is_none());
        crate::molt_exception_clear();
        assert_eq!(refs(code), 1);
        dec_ref_bits(py, code);
    });
}

#[test]
fn compiled_code_slot_identity_cannot_be_reassigned() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let globals = dict(py);
        crate::molt_code_slots_init(2);
        crate::molt_code_slot_set(0, code, globals);
        crate::molt_code_slot_set(1, code, globals);
        assert!(crate::exception_pending(py));
        assert_eq!(compiled_slot_for_code(code), Some(0));
        crate::molt_exception_clear();
        assert_eq!(crate::molt_trace_enter_slot(0), code);
        crate::molt_trace_exit();
        crate::molt_trace_enter_slot(1);
        assert!(crate::exception_pending(py));
        crate::molt_trace_exit();
        crate::molt_exception_clear();
        assert!(FRAME_STACK.with(|stack| stack.borrow().is_empty()));
        for bits in [code, globals] {
            dec_ref_bits(py, bits);
        }
    });
}
