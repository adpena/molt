use super::support::*;

#[test]
fn ordinary_task_initialization_is_success_only_and_preserves_result() {
    for (op_kind, task_kind) in [
        ("alloc_task", "generator"),
        ("alloc_task", "future"),
        ("alloc_task", "coroutine"),
        ("call_async", "future"),
    ] {
        for result in [None, Some("none"), Some("unused"), Some("task")] {
            let bound = result == Some("task");
            let mut allocation = wasm_test_op(op_kind, result, vec!["arg"]);
            allocation.s_value = Some("task_allocation_poll".into());
            allocation.value = Some(64);
            allocation.task_kind = (op_kind == "alloc_task").then(|| task_kind.into());
            let wasm = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![
                    wasm_test_function(
                        "molt_main",
                        vec!["arg"],
                        None,
                        vec![
                            allocation,
                            wasm_test_op("ret", None, vec![if bound { "task" } else { "none" }]),
                        ],
                    ),
                    wasm_test_function(
                        "task_allocation_poll",
                        vec!["task"],
                        None,
                        vec![wasm_test_op("ret", None, vec!["task"])],
                    ),
                ],
                profile: None,
            })
            .wasm;
            wasmparser::Validator::new().validate_all(&wasm).unwrap();
            let imports = wasm_function_import_indices(&wasm);
            let task_new = imports["task_new"];
            let mut checked = false;
            for payload in Parser::new(0).parse_all(&wasm) {
                let Payload::CodeSectionEntry(body) = payload.unwrap() else {
                    continue;
                };
                let ops = body
                    .get_operators_reader()
                    .unwrap()
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let Some(call) = ops.iter().position(|op| matches!(op, wasmparser::Operator::Call {function_index} if *function_index == task_new)) else {continue};
                let wasmparser::Operator::LocalSet {
                    local_index: result,
                } = ops[call + 1]
                else {
                    panic!("task result must have an owner")
                };
                assert!(
                    matches!(ops[call + 2], wasmparser::Operator::LocalGet {local_index} if local_index == result)
                );
                assert!(
                    matches!(ops[call + 3], wasmparser::Operator::I64Const {value} if value == molt_codegen_abi::box_none_bits())
                );
                assert!(matches!(ops[call + 4], wasmparser::Operator::I64Ne));
                assert!(matches!(ops[call + 5], wasmparser::Operator::If { .. }));
                let initialize = &ops[call + 6..];
                let end = initialize
                    .iter()
                    .position(|op| matches!(op, wasmparser::Operator::End))
                    .unwrap();
                assert!(
                    initialize[..end]
                        .iter()
                        .any(|op| matches!(op, wasmparser::Operator::I64Store { .. }))
                );
                assert!(
                    !initialize[..end]
                        .iter()
                        .any(|op| matches!(op, wasmparser::Operator::Return))
                );
                if op_kind == "alloc_task" && task_kind != "generator" {
                    let register = imports["task_register_token_owned"];
                    assert!(initialize[..end].iter().any(|op| matches!(op, wasmparser::Operator::Call {function_index} if *function_index == register)));
                }
                let releases = imports.get("dec_ref_obj").map_or(0, |release| {
                ops.iter().filter(|op| matches!(op, wasmparser::Operator::Call {function_index} if function_index == release)).count()
            });
                assert_eq!(
                    releases,
                    usize::from(!bound),
                    "{op_kind}/{task_kind}/{result:?}"
                );
                if !bound {
                    let release = imports["dec_ref_obj"];
                    let position = ops
                        .iter()
                        .position(|op| {
                            matches!(op,
                    wasmparser::Operator::Call {function_index} if *function_index == release)
                        })
                        .unwrap();
                    assert!(
                        position > call + 6 + end,
                        "release must follow initialization"
                    );
                }
                checked = true;
            }
            assert!(
                checked,
                "{op_kind}/{task_kind} must emit the allocation consumer"
            );
        }
    }
}
