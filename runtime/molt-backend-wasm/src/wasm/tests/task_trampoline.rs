use super::support::*;

#[derive(Clone, Copy, Debug)]
enum TaskPayloadCase {
    None,
    Positional,
    Closure,
}

fn task_constructor_ir(task_kind: &str, target: &str, payload: TaskPayloadCase) -> SimpleIR {
    let mut main_ops = Vec::new();
    let (constructor_kind, constructor_args, arity) = match payload {
        TaskPayloadCase::None => ("func_new", vec![], 0),
        TaskPayloadCase::Positional => ("func_new", vec![], 1),
        TaskPayloadCase::Closure => {
            main_ops.push(wasm_test_op("const_none", Some("closure"), vec![]));
            ("func_new_closure", vec!["closure"], 0)
        }
    };
    let mut function_object =
        wasm_test_op(constructor_kind, Some("function_object"), constructor_args);
    function_object.s_value = Some(target.to_string());
    function_object.value = Some(arity);
    function_object.task_kind = Some(task_kind.to_string());
    function_object.task_closure_size = Some(64);
    let mut return_function = wasm_test_op("ret", None, vec!["function_object"]);
    return_function.args = Some(vec!["function_object".to_string()]);
    main_ops.extend([function_object, return_function]);

    SimpleIR {
        functions: vec![
            wasm_test_function("molt_main", vec![], None, main_ops),
            wasm_test_function(
                target,
                vec!["task"],
                None,
                vec![
                    wasm_test_op("const_none", Some("result"), vec![]),
                    wasm_test_op("ret", None, vec!["result"]),
                ],
            ),
        ],
        profile: None,
    }
}

fn task_trampoline_ops(wasm: &[u8], task_new_index: u32) -> Vec<String> {
    let task_new = format!("Call {{ function_index: {task_new_index} }}");
    let mut matches = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Ok(Payload::CodeSectionEntry(body)) = payload else {
            continue;
        };
        let mut reader = body
            .get_operators_reader()
            .expect("task trampoline body must decode");
        let mut ops = Vec::new();
        while !reader.eof() {
            ops.push(format!(
                "{:?}",
                reader.read().expect("task trampoline operator must decode")
            ));
        }
        if ops.iter().any(|op| op == &task_new) {
            matches.push(ops);
        }
    }
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one generated trampoline body to call task_new"
    );
    matches.pop().unwrap()
}

fn call_position(ops: &[String], function_index: u32) -> usize {
    let call = format!("Call {{ function_index: {function_index} }}");
    ops.iter()
        .position(|op| op == &call)
        .unwrap_or_else(|| panic!("missing {call} in task trampoline: {ops:?}"))
}

#[test]
fn typed_task_constructors_guard_allocation_before_payload_and_completion() {
    for (task_kind, target, completion_imports) in [
        ("generator", "generator_body", &[][..]),
        (
            "coroutine",
            "coroutine_body",
            &["cancel_token_get_current", "task_register_token_owned"][..],
        ),
        (
            "async_generator",
            "asyncgen_body",
            &["asyncgen_new", "dec_ref_obj"][..],
        ),
    ] {
        for payload in [
            TaskPayloadCase::None,
            TaskPayloadCase::Positional,
            TaskPayloadCase::Closure,
        ] {
            let wasm = WasmBackend::with_options(WasmCompileOptions {
                native_eh_enabled: false,
                reloc_enabled: false,
                wasm_profile: WasmProfile::Auto,
                ..WasmCompileOptions::default()
            })
            .compile(task_constructor_ir(task_kind, target, payload));

            wasmparser::Validator::new()
                .validate_all(&wasm)
                .unwrap_or_else(|error| {
                    panic!("{task_kind}/{payload:?} task trampoline emitted invalid WASM: {error}")
                });

            let imports = wasm_function_import_indices(&wasm);
            let task_new = *imports
                .get("task_new")
                .unwrap_or_else(|| panic!("{task_kind} must import task_new; imports={imports:?}"));
            let ops = task_trampoline_ops(&wasm, task_new);
            let task_call = call_position(&ops, task_new);
            let expected_guard = [
                "LocalSet { local_index: 3 }".to_string(),
                "LocalGet { local_index: 3 }".to_string(),
                format!(
                    "I64Const {{ value: {} }}",
                    molt_codegen_abi::box_none_bits()
                ),
                "I64Eq".to_string(),
            ];
            assert_eq!(&ops[(task_call + 1)..(task_call + 5)], expected_guard);
            assert!(ops[task_call + 5].starts_with("If"), "{ops:?}");
            assert_eq!(ops[task_call + 6], "LocalGet { local_index: 3 }");
            assert_eq!(ops[task_call + 7], "Return");
            assert_eq!(ops[task_call + 8], "End");
            let guard_end = task_call + 8;

            if !matches!(payload, TaskPayloadCase::None) {
                let resolve = *imports
                    .get("handle_resolve")
                    .expect("payload trampoline must import handle_resolve");
                let retain = *imports
                    .get("inc_ref_obj")
                    .expect("payload trampoline must import inc_ref_obj");
                assert!(call_position(&ops, resolve) > guard_end);
                assert!(call_position(&ops, retain) > guard_end);
                assert!(
                    ops[(guard_end + 1)..]
                        .iter()
                        .any(|op| op.starts_with("I64Store")),
                    "payload store must remain on allocation-success edge: {ops:?}"
                );
            } else {
                assert!(
                    !ops[(guard_end + 1)..]
                        .iter()
                        .any(|op| op.starts_with("I64Store")),
                    "zero-payload trampoline must not store payload: {ops:?}"
                );
            }

            let mut previous_completion = guard_end;
            for import_name in completion_imports {
                let import_index = *imports.get(*import_name).unwrap_or_else(|| {
                    panic!("{task_kind} must import {import_name}; imports={imports:?}")
                });
                let position = call_position(&ops, import_index);
                assert!(position > previous_completion, "{ops:?}");
                previous_completion = position;
            }
            if task_kind == "async_generator" {
                assert_eq!(
                    ops[previous_completion + 1],
                    "LocalGet { local_index: 5 }",
                    "asyncgen must restore exact wrapper/error after task release: {ops:?}"
                );
            }
        }
    }
}

#[test]
fn ordinary_poll_named_functions_keep_their_physical_arity() {
    for arity in [0, 2] {
        let mut ir = task_constructor_ir("generator", "ordinary_poll", TaskPayloadCase::None);
        let constructor = &mut ir.functions[0].ops[0];
        constructor.task_kind = None;
        constructor.task_closure_size = None;
        constructor.value = Some(arity as i64);
        ir.functions[1].params = (0..arity).map(|index| format!("arg{index}")).collect();
        let wasm = WasmBackend::with_options(WasmCompileOptions {
            native_eh_enabled: false,
            reloc_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(ir);
        wasmparser::Validator::new()
            .validate_all(&wasm)
            .unwrap_or_else(|error| {
                panic!("ordinary _poll function with arity {arity} emitted invalid WASM: {error}")
            });
        assert!(!wasm_function_import_indices(&wasm).contains_key("task_new"));
    }
}
