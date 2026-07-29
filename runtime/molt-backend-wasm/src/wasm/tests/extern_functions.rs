use super::support::*;

fn extern_function(name: &str, arity: usize, returns_value: bool) -> FunctionIR {
    let ops = if returns_value {
        vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some(molt_ir::EXTERN_SIGNATURE_RETURN_VALUE.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec![molt_ir::EXTERN_SIGNATURE_RETURN_VALUE.to_string()]),
                ..OpIR::default()
            },
        ]
    } else {
        vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }]
    };
    FunctionIR {
        name: name.to_string(),
        params: (0..arity).map(|index| format!("arg{index}")).collect(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: true,
        execution_context: Default::default(),
    }
}

fn extern_linkage_ir() -> SimpleIR {
    let call_void = OpIR {
        kind: "call_internal".to_string(),
        s_value: Some("stdlib_void".to_string()),
        out: Some("void_none".to_string()),
        ..OpIR::default()
    };
    let mut call_value = wasm_test_op("call_internal", Some("external"), vec!["arg0", "arg1"]);
    call_value.s_value = Some("stdlib_value".to_string());
    let mut call_local = wasm_test_op("call_internal", Some("answer"), vec!["external"]);
    call_local.s_value = Some("local_body".to_string());
    let guarded_void = OpIR {
        kind: "call_guarded".to_string(),
        s_value: Some("stdlib_void".to_string()),
        args: Some(vec!["guarded_callee".to_string()]),
        out: Some("guarded_none".to_string()),
        ..OpIR::default()
    };

    SimpleIR {
        functions: vec![
            wasm_test_function(
                "molt_main",
                vec![],
                None,
                vec![
                    wasm_test_op("const_none", Some("arg0"), vec![]),
                    wasm_test_op("const_none", Some("arg1"), vec![]),
                    wasm_test_op("const_none", Some("guarded_callee"), vec![]),
                    call_void,
                    guarded_void,
                    call_value,
                    call_local,
                    wasm_test_op("ret", None, vec!["answer"]),
                ],
            ),
            extern_function("stdlib_void", 0, false),
            wasm_test_function(
                "local_body",
                vec!["value"],
                None,
                vec![wasm_test_op("ret", None, vec!["value"])],
            ),
            extern_function("stdlib_value", 2, true),
        ],
        profile: None,
    }
}

fn code_body_count(wasm: &[u8]) -> usize {
    Parser::new(0)
        .parse_all(wasm)
        .filter(|payload| matches!(payload, Ok(Payload::CodeSectionEntry(_))))
        .count()
}

fn values_immediately_after_direct_call(wasm: &[u8], target_index: u32) -> Vec<i64> {
    let mut values = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Ok(Payload::CodeSectionEntry(body)) = payload else {
            continue;
        };
        let mut operators = body
            .get_operators_reader()
            .expect("read extern-linkage WASM operators");
        while !operators.eof() {
            let operator = operators.read().expect("read extern-linkage operator");
            if matches!(
                operator,
                wasmparser::Operator::Call { function_index } if function_index == target_index
            ) && let wasmparser::Operator::I64Const { value } = operators
                .read()
                .expect("void extern call must have a following result normalizer")
            {
                values.push(value);
            }
        }
    }
    values
}

#[test]
fn full_compile_pipeline_preserves_extern_signature_metadata() {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        reloc_enabled: false,
        native_eh_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(extern_linkage_ir());
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("optimized extern linkage must emit valid WASM");

    let import_types = wasm_function_import_type_indices(&wasm);
    let signatures = wasm_type_section_signatures(&wasm);
    assert_eq!(signatures[import_types["stdlib_void"] as usize], (0, 0));
    assert_eq!(signatures[import_types["stdlib_value"] as usize], (2, 1));
}

#[test]
fn json_and_ndjson_transport_preserve_extern_wasm_identity_bit_exactly() {
    let ir = extern_linkage_ir();
    let json = serde_json::to_string(&ir).expect("serialize canonical SimpleIR JSON");
    let from_json = SimpleIR::from_json_str(&json).expect("parse canonical SimpleIR JSON");

    let mut ndjson = vec![
        serde_json::json!({
            "kind": "ir_stream_start",
            "profile": ir.profile,
        })
        .to_string(),
    ];
    for function in &ir.functions {
        let mut value = serde_json::to_value(function).expect("serialize NDJSON function");
        value
            .as_object_mut()
            .expect("FunctionIR serializes as an object")
            .insert("kind".to_string(), serde_json::json!("function"));
        ndjson.push(value.to_string());
    }
    ndjson.push(serde_json::json!({"kind": "ir_stream_end"}).to_string());
    let from_ndjson =
        SimpleIR::from_ndjson_reader(std::io::BufReader::new(ndjson.join("\n").as_bytes()))
            .expect("parse canonical SimpleIR NDJSON");

    let compile = |input| {
        WasmBackend::with_options(WasmCompileOptions {
            reloc_enabled: false,
            native_eh_enabled: false,
            wasm_profile: WasmProfile::Auto,
            ..WasmCompileOptions::default()
        })
        .compile(input)
    };
    let direct_wasm = compile(ir);
    assert_eq!(compile(from_json), direct_wasm);
    assert_eq!(compile(from_ndjson), direct_wasm);
}

#[test]
fn extern_declarations_are_typed_imports_and_direct_calls_use_import_indices() {
    let ir = extern_linkage_ir();
    crate::validate_simple_ir(&ir).expect("canonical extern declarations and direct calls");
    let wasm = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir).wasm;
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("extern linkage must emit valid WASM");

    let imports = wasm_function_import_indices(&wasm);
    let modules = wasm_function_import_modules(&wasm);
    let import_types = wasm_function_import_type_indices(&wasm);
    let signatures = wasm_type_section_signatures(&wasm);
    let void_index = imports["stdlib_void"];
    let value_index = imports["stdlib_value"];
    assert_eq!(modules["stdlib_void"], "env");
    assert_eq!(modules["stdlib_value"], "env");
    assert_eq!(signatures[import_types["stdlib_void"] as usize], (0, 0));
    assert_eq!(signatures[import_types["stdlib_value"] as usize], (2, 1));

    let exports = wasm_function_export_indices(&wasm);
    assert!(!exports.contains_key("stdlib_void"));
    assert!(!exports.contains_key("stdlib_value"));
    let main_index = exports["molt_main"];
    let local_index = exports["local_body"];
    let import_count = imports.len() as u32;
    assert!(
        main_index >= import_count,
        "molt_main must own a local body"
    );
    assert!(
        local_index >= import_count,
        "local_body must own a local body"
    );
    assert_eq!(
        wasm_function_section_type_indices(&wasm).len(),
        code_body_count(&wasm),
        "extern imports must not create FunctionSection/CodeSection entries"
    );

    let main_calls = wasm_direct_call_indices_for_export(&wasm, "molt_main");
    assert!(
        main_calls.contains(&void_index),
        "void extern call index drifted"
    );
    assert!(
        main_calls.contains(&value_index),
        "value extern call index drifted"
    );
    assert!(
        main_calls.contains(&local_index),
        "defined-function index drifted by the interleaved extern imports"
    );

    let table_entries = wasm_element_function_indices(&wasm);
    for expected in [void_index, value_index, main_index, local_index] {
        assert!(
            table_entries.contains(&expected),
            "callable table omitted function index {expected}: {table_entries:?}"
        );
    }
    let all_calls = wasm_direct_call_indices(&wasm);
    assert!(
        all_calls
            .iter()
            .filter(|&&index| index == void_index)
            .count()
            >= 2,
        "void extern must be called by both its direct site and argv trampoline"
    );
    assert!(
        all_calls
            .iter()
            .filter(|&&index| index == value_index)
            .count()
            >= 2,
        "value extern must be called by both its direct site and argv trampoline"
    );

    let normalized_void_results = values_immediately_after_direct_call(&wasm, void_index);
    assert!(
        normalized_void_results.len() >= 2,
        "bound direct/guarded calls and the callable trampoline must normalize void extern results: {normalized_void_results:?}"
    );
    assert!(
        normalized_void_results
            .iter()
            .all(|value| *value == molt_codegen_abi::box_none_bits()),
        "every void extern result lane must synthesize canonical boxed None, not numeric zero: {normalized_void_results:?}"
    );
}

#[test]
fn extern_declarations_survive_relocatable_symbol_and_table_emission() {
    let ir = extern_linkage_ir();
    let analysis = super::super::trampoline_analysis::analyze_wasm_trampolines(&ir);
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        reloc_enabled: true,
        native_eh_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .emit_wasm_module(ir, BTreeMap::new(), analysis)
    .wasm;

    for symbol in ["stdlib_void", "stdlib_value"] {
        assert!(
            wasm.windows(symbol.len())
                .any(|bytes| bytes == symbol.as_bytes()),
            "relocatable symbol table omitted extern {symbol}"
        );
    }
}

#[test]
fn malformed_extern_declaration_is_rejected_by_shared_validation() {
    let mut malformed = extern_function("malformed_external", 0, false);
    malformed.ops.insert(
        0,
        OpIR {
            kind: "const_none".to_string(),
            out: Some("body_value".to_string()),
            ..OpIR::default()
        },
    );
    let error = crate::validate_simple_ir(&SimpleIR {
        functions: vec![malformed],
        profile: None,
    })
    .expect_err("extern executable bodies must fail at the shared IR boundary");

    assert!(
        error.contains("must contain only canonical return-signature metadata"),
        "unexpected validation error: {error}"
    );
}
