use super::*;
use crate::TrampolineSpec;
use cranelift_module::Linkage;
use molt_tir::trampolines::TrampolineTaskKind;

#[derive(Clone, Copy, Debug)]
enum TaskPayloadCase {
    None,
    Positional,
    Closure,
}

impl TaskPayloadCase {
    const fn arity(self) -> usize {
        match self {
            Self::Positional => 1,
            Self::None | Self::Closure => 0,
        }
    }

    const fn has_closure(self) -> bool {
        matches!(self, Self::Closure)
    }

    const fn slot_count(self) -> usize {
        match self {
            Self::None => 0,
            Self::Positional | Self::Closure => 1,
        }
    }
}

fn task_trampoline_clif(task_kind: TrampolineTaskKind, payload: TaskPayloadCase) -> String {
    let mut backend = SimpleBackend::new();
    let SimpleBackend {
        module, import_ids, ..
    } = &mut backend;
    let layout = task_kind.constructor_layout();
    SimpleBackend::task_trampoline_clif_for_test(
        module,
        import_ids,
        "allocation_probe",
        TrampolineSpec {
            arity: payload.arity(),
            has_closure: payload.has_closure(),
            kind: task_kind.trampoline_kind(),
            closure_size: i64::from(layout.payload_base_offset(GENERATOR_CONTROL_BYTES))
                + (payload.slot_count() as i64) * 8,
            target_has_ret: true,
        },
    )
}

fn assert_task_allocation_guard(
    clif: &str,
    task_kind: TrampolineTaskKind,
    payload: TaskPayloadCase,
) {
    let lines: Vec<&str> = clif.lines().map(str::trim).collect();
    let task_call_index = lines
        .iter()
        .position(|line| line.contains(" = call "))
        .unwrap_or_else(|| panic!("missing task_new call for {task_kind:?}:\n{clif}"));
    let task_value = lines[task_call_index]
        .split_once(" = call ")
        .map(|(result, _)| result.trim())
        .expect("task_new call must return the task value");
    let none_bits =
        cranelift_codegen::ir::immediates::Imm64::new(molt_codegen_abi::box_none_bits())
            .to_string();
    let guard_index = lines
        .iter()
        .enumerate()
        .skip(task_call_index + 1)
        .find_map(|(index, line)| {
            (line.contains("icmp_imm eq") && line.contains(task_value) && line.contains(&none_bits))
                .then_some(index)
        })
        .unwrap_or_else(|| {
            panic!("{task_kind:?} must compare task_new's exact result with boxed None:\n{clif}")
        });
    let branch_index = lines
        .iter()
        .enumerate()
        .skip(guard_index + 1)
        .find_map(|(index, line)| line.starts_with("brif ").then_some(index))
        .unwrap_or_else(|| panic!("{task_kind:?} allocation guard must branch:\n{clif}"));

    assert!(
        lines[(task_call_index + 1)..branch_index]
            .iter()
            .all(|line| !line.contains("load")
                && !line.contains("store")
                && !line.contains("call ")),
        "{task_kind:?} must branch before payload access, RC, token registration, or wrapping:\n{clif}"
    );
    let failure_block = lines[branch_index]
        .split(',')
        .nth(1)
        .map(str::trim)
        .expect("allocation branch must name a failure block");
    let failure_label = format!("{failure_block}:");
    let failure_label_index = lines
        .iter()
        .position(|line| *line == failure_label)
        .expect("allocation failure block must be present");
    let failure_instruction = lines[(failure_label_index + 1)..]
        .iter()
        .find(|line| !line.is_empty())
        .expect("allocation failure block must return");
    assert_eq!(
        *failure_instruction,
        format!("return {task_value}"),
        "{task_kind:?} failure edge must immediately return task_new's exact boxed result:\n{clif}"
    );

    let store_count = lines
        .iter()
        .filter(|line| line.starts_with("store "))
        .count();
    assert_eq!(store_count, payload.slot_count(), "{clif}");

    let call_count = lines.iter().filter(|line| line.contains("call ")).count();
    let completion_calls = match task_kind {
        TrampolineTaskKind::Generator => 0,
        TrampolineTaskKind::Coroutine | TrampolineTaskKind::AsyncGen => 2,
    };
    assert_eq!(
        call_count,
        1 + payload.slot_count() + completion_calls,
        "{task_kind:?} must emit task_new, payload retains, and typed completion exactly once:\n{clif}"
    );

    if task_kind == TrampolineTaskKind::AsyncGen {
        let calls: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| line.contains("call "))
            .collect();
        let wrap_call = calls[calls.len() - 2];
        let release_call = calls[calls.len() - 1];
        assert!(
            wrap_call.contains(" = call ") && !release_call.contains(" = call "),
            "async-generator completion must preserve its result then release the task:\n{clif}"
        );
        let wrapper_value = wrap_call
            .split_once(" = call ")
            .map(|(result, _)| result.trim())
            .unwrap();
        assert!(
            lines
                .iter()
                .any(|line| *line == format!("return {wrapper_value}")),
            "async-generator completion must return the exact wrapper/error result:\n{clif}"
        );
    }
}

#[test]
fn native_task_trampolines_guard_allocation_before_every_payload_and_completion_path() {
    for task_kind in [
        TrampolineTaskKind::Generator,
        TrampolineTaskKind::Coroutine,
        TrampolineTaskKind::AsyncGen,
    ] {
        for payload in [
            TaskPayloadCase::None,
            TaskPayloadCase::Positional,
            TaskPayloadCase::Closure,
        ] {
            let clif = task_trampoline_clif(task_kind, payload);
            assert_task_allocation_guard(&clif, task_kind, payload);
        }
    }
}

#[test]
fn trampoline_key_distinguishes_void_and_value_targets() {
    let value_key = TrampolineKey {
        name: "helper".to_string(),
        arity: 1,
        has_closure: false,
        is_import: false,
        kind: TrampolineKind::Plain,
        closure_size: 0,
        target_has_ret: true,
    };
    let void_key = TrampolineKey {
        target_has_ret: false,
        ..value_key.clone()
    };

    assert_ne!(value_key, void_key);
}

#[test]
fn native_call_frame_trampoline_forwards_the_canonical_three_argument_abi() {
    let mut backend = SimpleBackend::new();
    let SimpleBackend {
        module,
        trampoline_ids,
        import_ids,
        ..
    } = &mut backend;
    let trampoline_id = SimpleBackend::ensure_trampoline(
        module,
        trampoline_ids,
        import_ids,
        "call_frame_target",
        Linkage::Import,
        TrampolineSpec {
            arity: 3,
            has_closure: false,
            kind: TrampolineKind::CallFrame,
            closure_size: 0,
            target_has_ret: true,
        },
    );

    assert_eq!(trampoline_ids.len(), 1);
    assert_eq!(trampoline_ids.values().next(), Some(&trampoline_id));
    let key = trampoline_ids.keys().next().unwrap();
    assert_eq!(key.kind, TrampolineKind::CallFrame);
}

#[test]
fn native_backend_preserves_split_stub_calls_to_void_and_value_chunks() {
    let chunk0 = "__molt_chunk_demo__molt_module_chunk_1_0".to_string();
    let chunk1 = "__molt_chunk_demo__molt_module_chunk_1_1".to_string();
    let stub = "demo__molt_module_chunk_1".to_string();
    let clif = compile_function_to_clif_text(
        vec![
            FunctionIR {
                name: chunk0,
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                name: chunk1,
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("chunk_ret".to_string()),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["chunk_ret".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                name: stub.clone(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("__molt_chunk_demo__molt_module_chunk_1_0".to_string()),
                        out: Some("__chunk_discard_0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("__molt_chunk_demo__molt_module_chunk_1_1".to_string()),
                        out: Some("__chunk_ret".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["__chunk_ret".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
        ],
        &stub,
    );
    let local_callees: Vec<String> = clif
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.split_once(" = colocated")
                .map(|(name, _)| name.to_string())
        })
        .collect();
    assert_eq!(
        local_callees.len(),
        2,
        "stub CLIF should reference exactly two local chunk callees:\n{clif}",
    );
    assert!(
        local_callees
            .iter()
            .any(|callee| clif.contains(&format!("call {callee}("))),
        "split stub must retain the direct call to the first void-returning chunk:\n{clif}",
    );
    assert!(
        local_callees
            .iter()
            .any(|callee| clif.contains(&format!("= call {callee}("))),
        "split stub must retain the direct call to the final value-returning chunk:\n{clif}",
    );
}

#[test]
fn native_backend_compiles_split_local_frame_with_inherited_chunks() {
    let mut ops = vec![OpIR {
        kind: "trace_enter_slot".to_string(),
        value: Some(5),
        ..OpIR::default()
    }];
    for line in 1..=6 {
        ops.push(OpIR {
            kind: "line".to_string(),
            value: Some(line),
            ..OpIR::default()
        });
        ops.push(OpIR {
            kind: "const_none".to_string(),
            out: Some(format!("v{line}")),
            ..OpIR::default()
        });
    }
    ops.extend([
        OpIR {
            kind: "trace_exit".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
    ]);
    let original = FunctionIR {
        name: "native_framed_large".to_string(),
        ops,
        execution_context: crate::ir::ExecutionContextPolicy::Local,
        ..FunctionIR::default()
    };
    let mut occupied = BTreeSet::from([original.name.clone()]);
    let (stub, chunks) = crate::passes::split_large_function(original, 3, &mut occupied).unwrap();
    let stub_name = stub.name.clone();
    let functions = std::iter::once(stub).chain(chunks).collect::<Vec<_>>();
    crate::validate_simple_ir(&SimpleIR {
        functions: functions.clone(),
        profile: None,
    })
    .unwrap();
    let object = {
        let _guard = acquire_backend_env_lock();
        let _trace_env = ScopedEnvVar::set("MOLT_BACKEND_EMIT_TRACES", Some("0"));
        SimpleBackend::new()
            .compile(SimpleIR {
                functions: functions.clone(),
                profile: None,
            })
            .bytes
    };
    for symbol in [
        b"molt_trace_enter_slot".as_slice(),
        b"molt_trace_exit".as_slice(),
    ] {
        assert!(
            object.windows(symbol.len()).any(|window| window == symbol),
            "trace-enabled native object is missing {}",
            String::from_utf8_lossy(symbol)
        );
    }

    let clif = compile_function_to_clif_text(functions, &stub_name);
    assert!(clif.matches("call fn").count() >= 2, "{clif}");
}
