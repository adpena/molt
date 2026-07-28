use super::*;
use crate::TrampolineSpec;
use cranelift_module::Linkage;

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
                        var: Some("chunk_ret".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
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
                        var: Some("__chunk_ret".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
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
        let _trace_env = ScopedEnvVar::set("MOLT_BACKEND_EMIT_TRACES", Some("1"));
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
