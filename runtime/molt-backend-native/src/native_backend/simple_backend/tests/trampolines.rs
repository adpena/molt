use super::*;

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
