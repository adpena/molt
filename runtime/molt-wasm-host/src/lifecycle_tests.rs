use super::*;
use std::sync::Mutex;

fn execute_test_lifetime(
    store: &mut Store<HostState>,
    instance: &Instance,
) -> Result<GuestTermination> {
    let mut lifetime = MoltRuntimeLifetime::admit(store, instance)?;
    let result = call_guest_entrypoint(
        store,
        GuestEntrypoint::MoltApplication {
            application: instance,
            runtime: instance,
            lifetime: &mut lifetime,
        },
    );
    lifetime.finish(store, result)
}

fn lifetime_fixture(
    engine: &Engine,
    main_body: &str,
    pending: i64,
    enter_body: &str,
    leave_body: &str,
    shutdown_body: &str,
) -> (Store<HostState>, Instance, Arc<Mutex<Vec<i32>>>) {
    let module = Module::new(
        engine,
        format!(
            r#"(module
        (import "test" "mark" (func $mark (param i32)))
        (global $pending (mut i64) (i64.const {pending}))
        (func (export "molt_exception_pending") (result i64) global.get $pending)
        (func (export "molt_main") (result i64)
            i32.const 2 call $mark {main_body} i64.const 0)
        (func (export "molt_runtime_execution_enter") (result i64)
            i32.const 1 call $mark {enter_body})
        (func (export "molt_runtime_execution_leave") (param i64)
            local.get 0 i64.const 41 i64.ne if unreachable end
            i32.const 3 call $mark {leave_body})
        (func (export "molt_runtime_shutdown") (result i64)
            i32.const 4 call $mark
            i64.const 0 global.set $pending {shutdown_body} i64.const 1))"#
        ),
    )
    .unwrap();
    let mut store = Store::new(engine, main_tests::test_host_state());
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let mark = Func::wrap(&mut store, move |value: i32| {
        observed.lock().unwrap().push(value)
    });
    let instance = Instance::new(&mut store, &module, &[mark.into()]).unwrap();
    (store, instance, events)
}

#[test]
fn finite_lifetime_releases_then_finalizes_on_return_failure_and_empty_token() {
    let engine = build_engine().unwrap();
    for (main, pending, enter, leave, shutdown, expected, diagnostic) in [
        ("", 0, "i64.const 41", "", "", vec![1, 2, 3, 4], None),
        (
            "",
            0,
            "i64.const 41",
            "",
            "i64.const 0 return",
            vec![1, 2, 3, 4],
            Some("shutdown did not complete"),
        ),
        (
            "",
            0,
            "i64.const 41",
            "",
            "i64.const 2 return",
            vec![1, 2, 3, 4],
            Some("shutdown did not complete"),
        ),
        (
            "unreachable",
            0,
            "i64.const 41",
            "",
            "",
            vec![1, 2, 3, 4],
            Some("call molt_main"),
        ),
        (
            "",
            1,
            "i64.const 41",
            "",
            "",
            vec![1, 3, 4],
            Some("before molt_main"),
        ),
        (
            "",
            2,
            "i64.const 41",
            "",
            "",
            vec![1, 3, 4],
            Some("malformed molt_exception_pending status"),
        ),
        (
            "i64.const 1 global.set $pending",
            0,
            "i64.const 41",
            "",
            "",
            vec![1, 2, 3, 4],
            Some("pending runtime exception"),
        ),
        (
            "",
            0,
            "i64.const 41",
            "unreachable",
            "",
            vec![1, 2, 3, 4],
            Some("leave runtime execution"),
        ),
        (
            "",
            0,
            "i64.const 41",
            "",
            "unreachable",
            vec![1, 2, 3, 4],
            Some("finalize Molt application"),
        ),
        (
            "",
            0,
            "i64.const 0",
            "",
            "",
            vec![1, 4],
            Some("empty execution-boundary token"),
        ),
        (
            "",
            0,
            "unreachable",
            "",
            "",
            vec![1, 4],
            Some("enter runtime execution"),
        ),
    ] {
        let (mut store, instance, events) =
            lifetime_fixture(&engine, main, pending, enter, leave, shutdown);
        let result = execute_test_lifetime(&mut store, &instance);
        assert_eq!(*events.lock().unwrap(), expected);
        if let Some(diagnostic) = diagnostic {
            let error = result.unwrap_err();
            assert!(format!("{error:#}").contains(diagnostic), "{error:#}");
        } else {
            assert_eq!(result.unwrap(), GuestTermination::Returned);
        }
    }
}

#[test]
fn finite_lifetime_preserves_primary_typed_trap_and_both_cleanup_failures() {
    let engine = build_engine().unwrap();
    let (mut store, instance, events) = lifetime_fixture(
        &engine,
        "i64.const 1 global.set $pending i32.const 1 i32.const 0 i32.div_s drop",
        0,
        "i64.const 41",
        "unreachable",
        "unreachable",
    );
    let error = execute_test_lifetime(&mut store, &instance).unwrap_err();
    assert_eq!(*events.lock().unwrap(), vec![1, 2, 3, 4]);
    assert_eq!(
        error.downcast_ref::<wasmtime::Trap>(),
        Some(&wasmtime::Trap::IntegerDivisionByZero)
    );
    let text = format!("{error:#}");
    for phase in [
        "call molt_main",
        "pending runtime exception before runtime shutdown",
        "leave runtime execution",
        "finalize Molt application",
    ] {
        assert!(text.contains(phase), "{text}");
    }
}

#[test]
fn finite_lifetime_retains_pending_failure_without_successful_execution_admission() {
    let engine = build_engine().unwrap();
    for enter in [
        "i64.const 1 global.set $pending unreachable",
        "i64.const 1 global.set $pending i64.const 0",
    ] {
        let (mut store, instance, events) = lifetime_fixture(&engine, "", 0, enter, "", "");
        let error = execute_test_lifetime(&mut store, &instance).unwrap_err();
        assert_eq!(*events.lock().unwrap(), vec![1, 4]);
        let text = format!("{error:#}");
        assert!(
            text.contains("pending runtime exception before runtime shutdown"),
            "{text}"
        );
        assert!(
            text.contains(if enter.ends_with("unreachable") {
                "enter runtime execution"
            } else {
                "empty execution-boundary token"
            }),
            "{text}"
        );
    }
    // An admitted instance can already carry a core-start/setup exception.
    let (mut store, instance, events) = lifetime_fixture(&engine, "", 1, "i64.const 41", "", "");
    let lifetime = MoltRuntimeLifetime::admit(&mut store, &instance).unwrap();
    let error = lifetime.finish(&mut store, Ok(())).unwrap_err();
    assert_eq!(*events.lock().unwrap(), vec![4]);
    assert!(format!("{error:#}").contains("pending runtime exception before runtime shutdown"));
}

#[test]
fn lifetime_export_admission_precedes_any_guest_start() {
    let engine = build_engine().unwrap();
    let exports = [
        ("molt_runtime_execution_enter", "(result i64) i64.const 41"),
        ("molt_runtime_execution_leave", "(param i64)"),
        ("molt_runtime_shutdown", "(result i64) i64.const 0"),
    ];
    for (missing, _) in exports {
        for malformed in [None, Some("global"), Some("params"), Some("results")] {
            let definitions = exports
                .iter()
                .map(|(name, body)| {
                    if *name == missing {
                        match malformed {
                            None => String::new(),
                            Some("global") => {
                                format!(r#"(global (export "{name}") i64 (i64.const 0))"#)
                            }
                            Some("params") => format!(r#"(func (export "{name}") (param i32))"#),
                            Some("results") => {
                                format!(r#"(func (export "{name}") (result i32) i32.const 0)"#)
                            }
                            Some(_) => unreachable!(),
                        }
                    } else {
                        format!(r#"(func (export "{name}") {body})"#)
                    }
                })
                .collect::<String>();
            let module = Module::new(
                &engine,
                format!(
                    r#"(module
                (func $start unreachable) (start $start)
                (func (export "molt_main") (result i64) i64.const 0)
                (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
                (func (export "molt_isolate_import") (param i64) (result i64) local.get 0)
                (func (export "molt_exception_pending") (result i64) i64.const 0)
                {definitions})"#
                ),
            )
            .unwrap();
            let error = execute_loaded_guest(
                &engine,
                &module,
                None,
                LoadedGuestOptions {
                    kind: LoadedGuestKind::MoltApplication { linked: true },
                    vfs_envs: &[],
                    guest_args: &[],
                    wasm_table_base: None,
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains(missing), "{error:#}");
            assert!(
                error.downcast_ref::<wasmtime::Trap>().is_none(),
                "{error:#}"
            );
        }
    }
}

#[test]
fn finite_lifetime_finalizes_after_linked_and_split_setup_failures() {
    let engine = build_engine().unwrap();
    for (linked, fail_table_setup) in [(true, true), (false, true), (false, false)] {
        let runtime_fields = r#"
            (func (export "molt_anchor"))
            (func (export "molt_runtime_execution_enter") (result i64) unreachable)
            (func (export "molt_runtime_execution_leave") (param i64) unreachable)
            (func (export "molt_runtime_shutdown") (result i64) unreachable)
            (func (export "molt_exception_pending") (result i64) i64.const 0)
            (func (export "molt_set_wasm_table_base") (param i64)
                i32.const 1 i32.const 0 i32.div_s drop)
        "#;
        let runtime =
            (!linked).then(|| Module::new(&engine, format!("(module {runtime_fields})")).unwrap());
        let app_import = if linked {
            ""
        } else {
            r#"(import "molt_runtime" "anchor" (func))"#
        };
        let core_start = if fail_table_setup {
            ""
        } else {
            "(func $start i32.const 1 i32.const 0 i32.div_s drop) (start $start)"
        };
        let app_runtime_fields = if linked { runtime_fields } else { "" };
        let application = Module::new(
            &engine,
            format!(
                r#"(module
            {app_import}
            {app_runtime_fields}
            {core_start}
            (func (export "molt_main") (result i64) unreachable)
            (func (export "molt_isolate_bootstrap") (result i64) i64.const 0)
            (func (export "molt_isolate_import") (param i64) (result i64) i64.const 0))"#
            ),
        )
        .unwrap();
        let error = execute_loaded_guest(
            &engine,
            &application,
            runtime.as_ref(),
            LoadedGuestOptions {
                kind: LoadedGuestKind::MoltApplication { linked },
                vfs_envs: &[],
                guest_args: &[],
                wasm_table_base: fail_table_setup.then_some(4096),
            },
        )
        .unwrap_err();
        // Setup must remain the primary typed error. A distinct teardown trap
        // proves that shutdown ran even though execution was never entered.
        assert_eq!(
            error.downcast_ref::<wasmtime::Trap>(),
            Some(&wasmtime::Trap::IntegerDivisionByZero)
        );
        let text = format!("{error:#}");
        assert!(text.contains("finalize Molt application runtime"), "{text}");
        assert!(
            text.contains(if fail_table_setup {
                "call molt_set_wasm_table_base"
            } else {
                "instantiate output"
            }),
            "{text}"
        );
    }
}

#[test]
fn host_resource_cleanup_closes_owned_sockets_and_is_repeatable() {
    let mut state = main_tests::test_host_state();
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    socket
        .bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, 0)).into())
        .unwrap();
    let address = socket.local_addr().unwrap();
    state.socket_manager.insert(socket);
    state.close_resources().unwrap();
    state.close_resources().unwrap();
    assert!(state.socket_manager.sockets.is_empty());
    // Rebinding the same exclusive UDP address proves the OS resource closed,
    // not merely that the host registry forgot its handle.
    let replacement = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    replacement.bind(&address).unwrap();
}
