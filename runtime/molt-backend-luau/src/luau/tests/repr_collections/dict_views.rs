use super::super::*;

#[test]
fn test_dict_view_ops_emit_luau_helpers() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "dict_view_ops".to_string(),
            params: vec!["k".to_string(), "v".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "build_dict".to_string(),
                    args: Some(vec!["k".to_string(), "v".to_string()]),
                    out: Some("d".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_keys".to_string(),
                    args: Some(vec!["d".to_string()]),
                    out: Some("keys".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_values".to_string(),
                    args: Some(vec!["d".to_string()]),
                    out: Some("values".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_items".to_string(),
                    args: Some(vec!["d".to_string()]),
                    out: Some("items".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let mut backend = LuauBackend::new();
    let output = backend.compile(&ir);
    assert!(
        output.contains("local keys = molt_dict_keys(d)")
            && output.contains("local values = molt_dict_values(d)")
            && output.contains("local items = molt_dict_items(d)"),
        "dict view ops must dispatch through Luau helper calls, got:\n{output}"
    );
    assert!(
        output.contains("local function molt_dict_keys")
            && output.contains("local function molt_dict_values")
            && output.contains("local function molt_dict_items"),
        "dict view helper definitions must be included when called, got:\n{output}"
    );
    assert!(
        !output.contains("[unsupported op: dict_"),
        "dict view ops must not fall through to unsupported lowering, got:\n{output}"
    );
}
