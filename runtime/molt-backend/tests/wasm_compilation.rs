#![cfg(feature = "wasm-backend")]
// This test reads process settings to gate and isolate its fixtures.
#![allow(clippy::disallowed_methods)]

//! Tests for basic IR compilation: empty modules, single functions,
//! string constants, arithmetic, function calls, and exception handling.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use molt_backend::{FunctionIR, ModuleRegistryIR, OpIR, SimpleIR};
use molt_backend::{WasmBackend, WasmCompileOptions};
use std::f64::consts::PI;
use wasmparser::{Operator, Parser, Payload, TypeRef, Validator};

fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR::new(kind)
}

fn compile_ir(ir: SimpleIR) -> Vec<u8> {
    WasmBackend::new().compile(ir)
}

fn compile_single_function(ops: Vec<OpIR>, params: &[&str]) -> Vec<u8> {
    compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_func".to_string(),
            params: params.iter().map(|p| (*p).to_string()).collect(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    })
}

fn compile_single_function_with_param_types(
    ops: Vec<OpIR>,
    params: &[&str],
    param_types: &[&str],
) -> Vec<u8> {
    assert_eq!(
        params.len(),
        param_types.len(),
        "test helper requires one param type per parameter"
    );
    compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_func".to_string(),
            params: params.iter().map(|p| (*p).to_string()).collect(),
            ops,
            param_types: Some(param_types.iter().map(|ty| (*ty).to_string()).collect()),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    })
}

fn compile_ir_with_env(ir: SimpleIR, env: &[(&str, Option<&str>)]) -> Vec<u8> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock");
    let prior: Vec<(String, Option<std::ffi::OsString>)> = env
        .iter()
        .map(|(key, _)| ((*key).to_string(), std::env::var_os(key)))
        .collect();
    for (key, value) in env {
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
    let wasm = compile_ir(ir);
    for (key, value) in prior {
        match value {
            Some(value) => unsafe { std::env::set_var(&key, value) },
            None => unsafe { std::env::remove_var(&key) },
        }
    }
    wasm
}

fn extract_exports(wasm: &[u8]) -> Vec<String> {
    let mut exports = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::ExportSection(section) = payload.expect("valid payload") {
            for export in section.into_iter() {
                let export = export.expect("valid export");
                exports.push(export.name.to_string());
            }
        }
    }
    exports
}

fn count_code_sections(wasm: &[u8]) -> u32 {
    let mut count = 0;
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(_) = payload.expect("valid payload") {
            count += 1;
        }
    }
    count
}

fn import_call_counts(wasm: &[u8]) -> HashMap<String, usize> {
    let mut imported_function_names: Vec<String> = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid wasm payload") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("valid wasm import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        imported_function_names.push(import.name.to_string());
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut reader = body.get_operators_reader().expect("operators reader");
                while !reader.eof() {
                    if let Operator::Call { function_index } =
                        reader.read().expect("valid wasm operator")
                    {
                        let idx = function_index as usize;
                        if idx < imported_function_names.len() {
                            let name = imported_function_names[idx].clone();
                            *counts.entry(name).or_insert(0) += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    counts
}

fn import_call_sequence(wasm: &[u8]) -> Vec<String> {
    let mut imported_function_names: Vec<String> = Vec::new();
    let mut calls = Vec::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid wasm payload") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("valid wasm import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        imported_function_names.push(import.name.to_string());
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut reader = body.get_operators_reader().expect("operators reader");
                while !reader.eof() {
                    if let Operator::Call { function_index } =
                        reader.read().expect("valid wasm operator")
                    {
                        let idx = function_index as usize;
                        if idx < imported_function_names.len() {
                            calls.push(imported_function_names[idx].clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    calls
}

fn return_call_count(wasm: &[u8]) -> usize {
    let mut count = 0;

    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::CodeSectionEntry(body) = payload.expect("valid wasm payload") {
            let mut reader = body.get_operators_reader().expect("operators reader");
            while !reader.eof() {
                if let Operator::ReturnCall { .. } = reader.read().expect("valid wasm operator") {
                    count += 1;
                }
            }
        }
    }

    count
}

fn count_import(calls: &HashMap<String, usize>, name: &str) -> usize {
    calls.get(name).copied().unwrap_or(0)
}

fn validate_wasm(wasm: &[u8]) -> Result<(), wasmparser::BinaryReaderError> {
    Validator::new().validate_all(wasm).map(|_| ())
}

fn wasm_contains_name_fragment(wasm: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(wasm).contains(needle)
}

fn ret_value(name: &str) -> OpIR {
    let mut ret = op(molt_ir::OpKind::Ret);
    ret.args = Some(vec![name.to_string()]);
    ret
}

// -----------------------------------------------------------------------
// Empty / minimal module tests
// -----------------------------------------------------------------------

#[test]
fn empty_module_compiles_to_valid_wasm() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op(molt_ir::OpKind::RetVoid)],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    });

    // Should start with WASM magic bytes
    assert!(wasm.len() > 8, "WASM output too short");
    assert_eq!(&wasm[0..4], b"\0asm", "missing WASM magic bytes");
}

#[test]
fn module_registry_emits_valid_dense_module_id_dispatch() {
    let init = |name: &str| FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: name.to_string(),
        params: vec![],
        ops: vec![op(molt_ir::OpKind::RetVoid)],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let registry = ModuleRegistryIR {
        schema: 2,
        registry_digest: "module-id-dispatch-test".to_string(),
        blob: vec![1, 2, 3, 4],
        relocs: vec![],
        init_symbols: vec!["molt_init_alpha".to_string(), "molt_init_gamma".to_string()],
        rows: vec![
            molt_ir::ModuleRegistryRowIR {
                name: "__main__".into(),
                init_symbol: None,
            },
            molt_ir::ModuleRegistryRowIR {
                name: "alpha".into(),
                init_symbol: Some("molt_init_alpha".into()),
            },
            molt_ir::ModuleRegistryRowIR {
                name: "beta".into(),
                init_symbol: None,
            },
            molt_ir::ModuleRegistryRowIR {
                name: "gamma".into(),
                init_symbol: Some("molt_init_gamma".into()),
            },
        ],
    };
    let wasm = WasmBackend::new()
        .with_module_registry(Some(registry))
        .compile(SimpleIR {
            functions: vec![
                init("molt_main"),
                init("molt_init_alpha"),
                init("molt_init_gamma"),
            ],
            profile: None,
        });

    validate_wasm(&wasm).expect("ModuleId projection must be valid WASM");
    assert!(
        extract_exports(&wasm)
            .iter()
            .any(|name| name == "molt_isolate_import")
    );
    let import_calls = import_call_counts(&wasm);
    assert!(
        wasm_contains_name_fragment(&wasm, "module_registry_install"),
        "registry install import must survive import stripping"
    );
    assert_eq!(
        count_import(&import_calls, "module_registry_install"),
        1,
        "host init must install the exact registry segment before runtime init: {import_calls:?}"
    );
    let mut br_tables = 0;
    for payload in Parser::new(0).parse_all(&wasm) {
        if let Payload::CodeSectionEntry(body) = payload.expect("valid wasm payload") {
            let mut reader = body.get_operators_reader().expect("operators reader");
            while !reader.eof() {
                if matches!(
                    reader.read().expect("valid operator"),
                    Operator::BrTable { .. }
                ) {
                    br_tables += 1;
                }
            }
        }
    }
    assert_eq!(
        br_tables, 1,
        "one dense ModuleId dispatch table owns all rows"
    );
}

#[test]
fn empty_module_is_structurally_valid_wasm() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op(molt_ir::OpKind::RetVoid)],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    });

    validate_wasm(&wasm).expect("compiled wasm should validate structurally");
}

#[test]
fn empty_module_exports_molt_main() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op(molt_ir::OpKind::RetVoid)],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    });

    let exports = extract_exports(&wasm);
    assert!(
        exports.contains(&"molt_main".to_string()),
        "should export molt_main, found: {:?}",
        exports
    );
}

#[test]
fn empty_module_exports_memory() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op(molt_ir::OpKind::RetVoid)],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    });

    let exports = extract_exports(&wasm);
    assert!(
        exports.contains(&"molt_memory".to_string()),
        "should export molt_memory, found: {:?}",
        exports
    );
}

#[test]
fn single_function_produces_code_section() {
    let wasm = compile_single_function(vec![op(molt_ir::OpKind::RetVoid)], &[]);
    let code_count = count_code_sections(&wasm);
    assert!(
        code_count > 0,
        "should have at least one code section entry"
    );
}

// -----------------------------------------------------------------------
// Constant compilation tests
// -----------------------------------------------------------------------

#[test]
fn const_int_compiles() {
    let mut c = op(molt_ir::OpKind::Const);
    c.value = Some(42);
    c.out = Some("v0".to_string());

    // Should not panic — the constant should be inlined as i64.const.
    let wasm = compile_single_function(vec![c, op(molt_ir::OpKind::RetVoid)], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_float_compiles() {
    let mut c = op(molt_ir::OpKind::ConstFloat);
    c.f_value = Some(PI);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op(molt_ir::OpKind::RetVoid)], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_none_compiles() {
    let mut c = op(molt_ir::OpKind::ConstNone);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op(molt_ir::OpKind::RetVoid)], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_bool_true_compiles() {
    let mut c = op(molt_ir::OpKind::ConstBool);
    c.value = Some(1);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op(molt_ir::OpKind::RetVoid)], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_bool_false_compiles() {
    let mut c = op(molt_ir::OpKind::ConstBool);
    c.value = Some(0);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op(molt_ir::OpKind::RetVoid)], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_str_compiles_and_calls_string_from_bytes() {
    let mut c = op(molt_ir::OpKind::ConstStr);
    c.s_value = Some("hello".to_string());
    c.out = Some("v0".to_string());

    // Keep the string value live so TIR DCE does not erase the pure const_str op.
    let wasm = compile_single_function(vec![c, ret_value("v0")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "string_from_bytes") > 0,
        "const_str should call string_from_bytes"
    );
}

// -----------------------------------------------------------------------
// Arithmetic compilation tests
// -----------------------------------------------------------------------

#[test]
fn add_op_calls_add_import() {
    let mut add = op(molt_ir::OpKind::Add);
    add.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    add.out = Some("v0".to_string());

    // Keep the arithmetic result live so DCE cannot erase the pure add op.
    let wasm = compile_single_function(vec![add, ret_value("v0")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "add") > 0,
        "add op should call add import"
    );
}

#[test]
fn scalar_type_hint_alone_does_not_select_wasm_str_concat() {
    let mut add = op(molt_ir::OpKind::Add);
    add.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    add.out = Some("v0".to_string());
    add.type_hint = Some("str".to_string());

    let mut inplace_add = op(molt_ir::OpKind::InplaceAdd);
    inplace_add.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    inplace_add.out = Some("v1".to_string());
    inplace_add.type_hint = Some("str".to_string());

    let wasm = compile_single_function(vec![add, inplace_add, ret_value("v1")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert_eq!(
        count_import(&calls, "str_concat"),
        0,
        "result-side type_hint=str must not select WASM str_concat"
    );
}

#[test]
fn scalar_type_hint_alone_does_not_select_wasm_truthiness_fast_imports() {
    let mut bool_hint = op(molt_ir::OpKind::Bool);
    bool_hint.args = Some(vec!["p0".to_string()]);
    bool_hint.out = Some("v0".to_string());
    bool_hint.type_hint = Some("bool".to_string());

    let mut int_hint = op(molt_ir::OpKind::CastBool);
    int_hint.args = Some(vec!["p1".to_string()]);
    int_hint.out = Some("v1".to_string());
    int_hint.type_hint = Some("int".to_string());

    let wasm = compile_single_function(vec![bool_hint, int_hint, ret_value("v1")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert_eq!(
        count_import(&calls, "is_truthy_bool"),
        0,
        "result-side type_hint=bool must not select WASM bool truthiness"
    );
    assert_eq!(
        count_import(&calls, "is_truthy_int"),
        0,
        "result-side type_hint=int must not select WASM int truthiness"
    );
    assert!(
        count_import(&calls, "is_truthy") > 0,
        "unknown truthiness must use the generic truthiness import"
    );
}

#[test]
fn scalar_type_hint_alone_does_not_select_wasm_len_specialization() {
    let mut hinted_len = op(molt_ir::OpKind::Len);
    hinted_len.args = Some(vec!["p0".to_string()]);
    hinted_len.out = Some("v0".to_string());
    hinted_len.type_hint = Some("str".to_string());

    let wasm = compile_single_function(vec![hinted_len, ret_value("v0")], &["p0"]);
    let calls = import_call_counts(&wasm);
    assert_eq!(
        count_import(&calls, "len_str"),
        0,
        "result-side type_hint=str must not select WASM len_str"
    );
    assert!(
        count_import(&calls, "len") > 0,
        "unknown len operands should use generic len"
    );

    let mut transport_len = op(molt_ir::OpKind::Len);
    transport_len.args = Some(vec!["p0".to_string()]);
    transport_len.out = Some("v0".to_string());
    transport_len.container_type = Some("str".to_string());

    let wasm = compile_single_function(vec![transport_len, ret_value("v0")], &["p0"]);
    let calls = import_call_counts(&wasm);
    assert_eq!(
        count_import(&calls, "len_str"),
        0,
        "transport-only container_type=str must not select WASM len_str"
    );
    assert!(
        count_import(&calls, "len") > 0,
        "transport-only container_type=str should use generic len"
    );

    let mut structured_len = op(molt_ir::OpKind::Len);
    structured_len.args = Some(vec!["p0".to_string()]);
    structured_len.out = Some("v0".to_string());

    let wasm = compile_single_function_with_param_types(
        vec![structured_len, ret_value("v0")],
        &["p0"],
        &["str"],
    );
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "len_str") > 0,
        "structured param type str should select WASM len_str"
    );
}

#[test]
fn sub_op_calls_sub_import() {
    let mut sub = op(molt_ir::OpKind::Sub);
    sub.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    sub.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![sub, ret_value("v0")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "sub") > 0,
        "sub op should call sub import"
    );
}

#[test]
fn mul_op_calls_mul_import() {
    let mut mul = op(molt_ir::OpKind::Mul);
    mul.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    mul.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![mul, ret_value("v0")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "mul") > 0,
        "mul op should call mul import"
    );
}

// -----------------------------------------------------------------------
// Function call tests
// -----------------------------------------------------------------------

#[test]
fn call_func_uses_dispatch() {
    // call_func args: [callee, arg0, arg1, ...] — first is the callee object.
    // The WASM backend now outlines call_func via call_func_dispatch (spills
    // args to linear memory instead of using callargs_new/call_bind_ic).
    let mut call = op(molt_ir::OpKind::CallFunc);
    call.args = Some(vec!["p0".to_string(), "p1".to_string(), "p2".to_string()]);
    call.out = Some("v0".to_string());

    let wasm = compile_single_function(
        vec![call, op(molt_ir::OpKind::RetVoid)],
        &["p0", "p1", "p2"],
    );
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "call_func_dispatch") > 0,
        "call_func should use call_func_dispatch"
    );
}

#[test]
fn call_bind_without_output_compiles() {
    let mut call = op(molt_ir::OpKind::CallBind);
    call.args = Some(vec!["p0".to_string(), "p1".to_string()]);

    let wasm = compile_single_function(vec![call, op(molt_ir::OpKind::RetVoid)], &["p0", "p1"]);
    validate_wasm(&wasm).expect("output-less call_bind should still produce valid wasm");
}

#[test]
fn call_indirect_without_output_compiles() {
    let mut call = op(molt_ir::OpKind::CallIndirect);
    call.args = Some(vec!["p0".to_string(), "p1".to_string()]);

    let wasm = compile_single_function(vec![call, op(molt_ir::OpKind::RetVoid)], &["p0", "p1"]);
    validate_wasm(&wasm).expect("output-less call_indirect should still produce valid wasm");
}

fn tail_call_candidate_ir() -> SimpleIR {
    let mut call = op(molt_ir::OpKind::CallInternal);
    call.s_value = Some("tail_target".to_string());
    call.args = Some(vec!["p0".to_string()]);
    call.out = Some("v0".to_string());

    let mut recursive_call = op(molt_ir::OpKind::CallInternal);
    recursive_call.s_value = Some("tail_target".to_string());
    recursive_call.args = Some(vec!["p0".to_string()]);
    recursive_call.out = Some("v1".to_string());

    SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_test_func".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![call, ret_value("v0")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "tail_target".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![recursive_call, ret_value("v1")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    }
}

#[test]
fn tail_call_candidate_respects_compile_feature_gate() {
    let disabled = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        tail_call_enabled: false,
        ..WasmCompileOptions::default()
    })
    .compile_with_diagnostics(tail_call_candidate_ir());
    validate_wasm(&disabled.wasm).expect("ordinary call fallback should validate");
    assert_eq!(
        disabled.diagnostics.tail_calls_emitted, 0,
        "tail-call diagnostics must stay disabled by default"
    );
    assert_eq!(
        return_call_count(&disabled.wasm),
        0,
        "tail-call proposal opcodes must stay disabled by default"
    );

    let enabled = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        tail_call_enabled: true,
        ..WasmCompileOptions::default()
    })
    .compile_with_diagnostics(tail_call_candidate_ir());
    assert!(
        enabled.diagnostics.tail_calls_emitted > 0,
        "explicit tail-call feature opt-in should emit return_call"
    );
    assert_eq!(
        return_call_count(&enabled.wasm),
        enabled.diagnostics.tail_calls_emitted,
        "diagnostics must match emitted return_call opcodes"
    );
}

#[test]
fn tail_call_default_env_keeps_feature_disabled() {
    let disabled = compile_ir_with_env(
        tail_call_candidate_ir(),
        &[
            ("MOLT_WASM_TAIL_CALL", None),
            ("MOLT_WASM_NATIVE_EH", Some("0")),
        ],
    );
    validate_wasm(&disabled).expect("ordinary call fallback should validate");
    assert_eq!(
        return_call_count(&disabled),
        0,
        "tail-call proposal opcodes must stay disabled by default"
    );
}

#[test]
fn call_guarded_escaped_function_dispatches_on_object() {
    let mut func_new = op(molt_ir::OpKind::FuncNew);
    func_new.s_value = Some("guarded_target".to_string());
    func_new.value = Some(2);
    func_new.out = Some("v_func".to_string());

    let mut call = op(molt_ir::OpKind::CallGuarded);
    call.s_value = Some("guarded_target".to_string());
    call.args = Some(vec![
        "v_func".to_string(),
        "p0".to_string(),
        "p1".to_string(),
    ]);
    call.out = Some("v0".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_test_func".to_string(),
                params: vec!["p0".to_string(), "p1".to_string()],
                ops: vec![func_new, call, ret_value("v0")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "guarded_target".to_string(),
                params: vec!["arg0".to_string(), "arg1".to_string()],
                ops: vec![op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    });
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "call_func_dispatch") > 0,
        "escaped call_guarded should use call_func_dispatch; calls={calls:?}"
    );
    assert_eq!(
        count_import(&calls, "handle_resolve"),
        0,
        "escaped call_guarded should skip handle_resolve and dispatch on the function object directly; calls={calls:?}"
    );
}

#[test]
fn class_def_uses_guarded_class_def_import() {
    let mut class_name = op(molt_ir::OpKind::ConstStr);
    class_name.s_value = Some("A".to_string());
    class_name.out = Some("v_name".to_string());

    let mut attr_name = op(molt_ir::OpKind::ConstStr);
    attr_name.s_value = Some("x".to_string());
    attr_name.out = Some("v_attr".to_string());

    let mut attr_value = op(molt_ir::OpKind::Const);
    attr_value.value = Some(1);
    attr_value.out = Some("v_value".to_string());

    let mut class_def = op(molt_ir::OpKind::ClassDef);
    class_def.args = Some(vec![
        "v_name".to_string(),
        "v_attr".to_string(),
        "v_value".to_string(),
    ]);
    class_def.s_value = Some("0,1,0,0,0".to_string());
    class_def.out = Some("v_cls".to_string());

    let wasm = compile_single_function(
        vec![
            class_name,
            attr_name,
            attr_value,
            class_def,
            ret_value("v_cls"),
        ],
        &[],
    );
    validate_wasm(&wasm).expect("class_def should produce structurally valid wasm");
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "guarded_class_def") > 0,
        "class_def should call guarded_class_def"
    );
}

#[test]
fn class_def_pins_spilled_refs_across_guarded_helper() {
    let mut class_name = op(molt_ir::OpKind::ConstStr);
    class_name.s_value = Some("Huge".to_string());
    class_name.out = Some("v_name".to_string());

    let mut attr_name_a = op(molt_ir::OpKind::ConstStr);
    attr_name_a.s_value = Some("a".to_string());
    attr_name_a.out = Some("v_attr_a".to_string());

    let mut attr_name_b = op(molt_ir::OpKind::ConstStr);
    attr_name_b.s_value = Some("b".to_string());
    attr_name_b.out = Some("v_attr_b".to_string());

    let mut attr_value_a = op(molt_ir::OpKind::Const);
    attr_value_a.value = Some(1);
    attr_value_a.out = Some("v_value_a".to_string());

    let mut attr_value_b = op(molt_ir::OpKind::Const);
    attr_value_b.value = Some(2);
    attr_value_b.out = Some("v_value_b".to_string());

    let mut class_def = op(molt_ir::OpKind::ClassDef);
    class_def.args = Some(vec![
        "v_name".to_string(),
        "v_attr_a".to_string(),
        "v_value_a".to_string(),
        "v_attr_b".to_string(),
        "v_value_b".to_string(),
    ]);
    class_def.s_value = Some("0,2,0,0,0".to_string());
    class_def.out = Some("v_cls".to_string());

    let wasm = compile_single_function(
        vec![
            class_name,
            attr_name_a,
            attr_name_b,
            attr_value_a,
            attr_value_b,
            class_def,
            ret_value("v_cls"),
        ],
        &[],
    );
    validate_wasm(&wasm).expect("class_def cleanup must preserve the helper result stack");
    let calls = import_call_sequence(&wasm);
    let guarded_idx = calls
        .iter()
        .position(|name| name == "guarded_class_def")
        .expect("class_def should call guarded_class_def");
    let before = &calls[..guarded_idx];
    let after = &calls[guarded_idx + 1..];
    let trailing_before = before
        .iter()
        .rev()
        .take_while(|name| *name == "inc_ref_obj")
        .count();
    let leading_after = after
        .iter()
        .take_while(|name| *name == "dec_ref_obj")
        .count();
    assert_eq!(
        trailing_before, 5,
        "class_def should pin each spilled arg before guarded_class_def; calls={calls:?}"
    );
    assert!(
        leading_after >= 5,
        "class_def should release each pinned arg immediately after guarded_class_def; calls={calls:?}"
    );
}

#[test]
fn ret_with_value_compiles() {
    let mut c = op(molt_ir::OpKind::Const);
    c.value = Some(42);
    c.out = Some("v0".to_string());

    let mut ret = op(molt_ir::OpKind::Ret);
    ret.args = Some(vec!["v0".to_string()]);

    let wasm = compile_single_function(vec![c, ret], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn function_without_explicit_ret_still_validates() {
    let mut c = op(molt_ir::OpKind::Const);
    c.value = Some(42);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c], &[]);
    validate_wasm(&wasm).expect("implicit-None function should validate structurally");
}

#[test]
fn multiple_functions_compile() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_helper".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![{
                    let mut ret = op(molt_ir::OpKind::Ret);
                    ret.args = Some(vec!["p0".to_string()]);
                    ret
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };
    let wasm = compile_ir(ir);
    assert!(wasm.len() > 8);
}

// -----------------------------------------------------------------------
// Exception handling compilation tests
// -----------------------------------------------------------------------

#[test]
fn try_block_compiles() {
    let mut dict_new = op(molt_ir::OpKind::DictNew);
    dict_new.out = Some("v0".to_string());
    dict_new.args = Some(vec![]);

    let wasm = compile_single_function(
        vec![
            op(molt_ir::OpKind::TryStart),
            dict_new,
            op(molt_ir::OpKind::CheckException),
            op(molt_ir::OpKind::TryEnd),
            op(molt_ir::OpKind::RetVoid),
        ],
        &[],
    );
    assert!(wasm.len() > 8);
}

#[test]
fn nested_try_blocks_compile() {
    let mut dict1 = op(molt_ir::OpKind::DictNew);
    dict1.out = Some("v0".to_string());
    dict1.args = Some(vec![]);

    let mut dict2 = op(molt_ir::OpKind::DictNew);
    dict2.out = Some("v1".to_string());
    dict2.args = Some(vec![]);

    let wasm = compile_single_function(
        vec![
            op(molt_ir::OpKind::TryStart),
            dict1,
            op(molt_ir::OpKind::CheckException),
            op(molt_ir::OpKind::TryStart),
            dict2,
            op(molt_ir::OpKind::CheckException),
            op(molt_ir::OpKind::TryEnd),
            op(molt_ir::OpKind::TryEnd),
            op(molt_ir::OpKind::RetVoid),
        ],
        &[],
    );
    assert!(wasm.len() > 8);
}

// -----------------------------------------------------------------------
// Collection operations compilation tests
// -----------------------------------------------------------------------

#[test]
fn dict_literal_calls_full_pairs_import_without_incremental_mutation() {
    let mut d = op(molt_ir::OpKind::DictNew);
    d.out = Some("v0".to_string());
    d.args = Some(vec![]);

    let wasm = compile_single_function(vec![d, op(molt_ir::OpKind::RetVoid)], &[]);
    let calls = import_call_counts(&wasm);
    assert_eq!(
        count_import(&calls, "dict_from_pairs"),
        1,
        "an empty dictionary literal calls the complete-pairs constructor once"
    );
    assert_eq!(count_import(&calls, "dict_new"), 0);
    assert_eq!(count_import(&calls, "dict_set"), 0);
    assert_eq!(
        count_import(&calls, "scratch_alloc"),
        0,
        "empty pairs have no range"
    );
}

/// Fixed-arity lists borrow one private scratch word range through the
/// canonical constructor; the incremental builder is a host API only.
fn assert_list_uses_word_range_constructor(kind: molt_ir::OpKind, wasm: &[u8]) {
    let calls = import_call_counts(wasm);
    for builder in [
        "list_builder_new",
        "list_builder_append",
        "list_builder_finish",
    ] {
        assert_eq!(
            count_import(&calls, builder),
            0,
            "{kind} must not construct through {builder}"
        );
    }
    let sequence = import_call_sequence(wasm);
    let constructor = sequence
        .iter()
        .position(|name| name == "list_from_values")
        .unwrap_or_else(|| panic!("{kind} should call list_from_values; calls={sequence:?}"));
    assert!(
        sequence[..constructor]
            .iter()
            .any(|name| name == "scratch_alloc")
            && sequence[constructor + 1..]
                .iter()
                .any(|name| name == "scratch_free"),
        "{kind}: the constructor reads a range allocated before it and freed after it; \
         calls={sequence:?}"
    );
}

#[test]
fn list_new_compiles_using_word_range_constructor() {
    let mut list = op(molt_ir::OpKind::ListNew);
    list.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    list.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![list, op(molt_ir::OpKind::RetVoid)], &["p0", "p1"]);
    assert_list_uses_word_range_constructor(molt_ir::OpKind::ListNew, &wasm);
}

#[test]
fn build_list_compiles_using_word_range_constructor() {
    let mut list = op(molt_ir::OpKind::BuildList);
    list.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    list.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![list, ret_value("v0")], &["p0", "p1"]);
    assert_list_uses_word_range_constructor(molt_ir::OpKind::BuildList, &wasm);
}

#[test]
fn iter_next_unboxed_compiles_using_iter_next_and_index_imports() {
    let mut iter_next = op(molt_ir::OpKind::IterNextUnboxed);
    iter_next.args = Some(vec!["p0".to_string()]);
    iter_next.var = Some("value".to_string());
    iter_next.out = Some("done".to_string());

    let wasm = compile_single_function(vec![iter_next, ret_value("done")], &["p0"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "iter_next") > 0,
        "iter_next_unboxed should lower through iter_next on wasm"
    );
    assert!(
        count_import(&calls, "index") > 0,
        "iter_next_unboxed should extract value/done via index on wasm"
    );
}

#[test]
fn guard_tag_compiles_using_guard_type_import() {
    let mut guard = op(molt_ir::OpKind::GuardTag);
    guard.args = Some(vec!["p0".to_string(), "p1".to_string()]);

    let wasm = compile_single_function(vec![guard, op(molt_ir::OpKind::RetVoid)], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "guard_type") > 0,
        "guard_tag should lower through guard_type on wasm"
    );
}

#[test]
fn alloc_task_generator_keeps_task_new_import() {
    let mut alloc_task = op(molt_ir::OpKind::AllocTask);
    alloc_task.s_value = Some("__main_____f_poll".to_string());
    alloc_task.value = Some(48);
    alloc_task.task_kind = Some("generator".to_string());
    alloc_task.args = Some(vec![]);
    alloc_task.out = Some("task".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_test_func".to_string(),
                params: vec![],
                ops: vec![alloc_task, ret_value("task")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "__main_____f_poll".to_string(),
                params: vec!["self".to_string()],
                ops: vec![op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    });
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "task_new") > 0,
        "generator alloc_task must retain task_new import on wasm"
    );
}

#[test]
fn alloc_task_future_without_args_compiles_without_resolve_local() {
    let mut alloc_task = op(molt_ir::OpKind::AllocTask);
    alloc_task.s_value = Some("__main_____future_poll".to_string());
    alloc_task.value = Some(0);
    alloc_task.task_kind = Some("future".to_string());
    alloc_task.args = Some(vec![]);
    alloc_task.out = Some("task".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_test_func".to_string(),
                params: vec![],
                ops: vec![alloc_task, ret_value("task")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "__main_____future_poll".to_string(),
                params: vec!["self".to_string()],
                ops: vec![op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    });
    validate_wasm(&wasm).expect("future alloc_task without args should compile");
}

#[test]
#[should_panic(expected = "call_async targets molt_async_sleep, expected *_poll")]
fn call_async_rejects_non_poll_table_target() {
    let mut call_async = op(molt_ir::OpKind::CallAsync);
    call_async.s_value = Some("molt_async_sleep".to_string());
    call_async.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    call_async.out = Some("v0".to_string());

    compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "molt_test_func".to_string(),
            params: vec!["p0".to_string(), "p1".to_string()],
            ops: vec![call_async, ret_value("v0")],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }],
        profile: None,
    });
}

#[test]
fn list_from_intrinsic_list_survives_tir_roundtrip() {
    let mut require_intrinsic = op(molt_ir::OpKind::BuiltinFunc);
    require_intrinsic.s_value = Some("molt_getargv".to_string());
    require_intrinsic.value = Some(0);
    require_intrinsic.out = Some("v0".to_string());

    let mut callargs_new = op(molt_ir::OpKind::CallargsNew);
    callargs_new.out = Some("v1".to_string());

    let mut call_indirect = op(molt_ir::OpKind::CallIndirect);
    call_indirect.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    call_indirect.out = Some("v2".to_string());

    let mut list_new = op(molt_ir::OpKind::ListNew);
    list_new.args = Some(vec![]);
    list_new.out = Some("v3".to_string());
    list_new.type_hint = Some("list".to_string());

    let mut iter = op(molt_ir::OpKind::Iter);
    iter.args = Some(vec!["v2".to_string()]);
    iter.out = Some("v4".to_string());
    iter.type_hint = Some("iter".to_string());

    let mut none_a = op(molt_ir::OpKind::ConstNone);
    none_a.out = Some("v5".to_string());
    let mut none_b = op(molt_ir::OpKind::ConstNone);
    none_b.out = Some("v6".to_string());

    let mut is_op = op(molt_ir::OpKind::Is);
    is_op.args = Some(vec!["v4".to_string(), "v6".to_string()]);
    is_op.out = Some("v7".to_string());

    let mut if_op = op(molt_ir::OpKind::If);
    if_op.args = Some(vec!["v7".to_string()]);
    if_op.type_hint = Some("bool".to_string());

    let mut err_msg = op(molt_ir::OpKind::ConstStr);
    err_msg.s_value = Some("object is not iterable".to_string());
    err_msg.out = Some("v8".to_string());

    let mut err_type = op(molt_ir::OpKind::ConstStr);
    err_type.s_value = Some("TypeError".to_string());
    err_type.out = Some("v9".to_string());

    let mut err_args = op(molt_ir::OpKind::TupleNew);
    err_args.args = Some(vec!["v8".to_string()]);
    err_args.out = Some("v10".to_string());
    err_args.type_hint = Some("tuple".to_string());

    let mut err_obj = op(molt_ir::OpKind::ExceptionNew);
    err_obj.args = Some(vec!["v9".to_string(), "v10".to_string()]);
    err_obj.out = Some("v11".to_string());

    let mut raise_op = op(molt_ir::OpKind::Raise);
    raise_op.args = Some(vec!["v11".to_string()]);
    raise_op.out = Some("none".to_string());

    let mut zero = op(molt_ir::OpKind::Const);
    zero.value = Some(0);
    zero.out = Some("v12".to_string());

    let mut one = op(molt_ir::OpKind::Const);
    one.value = Some(1);
    one.out = Some("v13".to_string());

    let mut iter_next = op(molt_ir::OpKind::IterNext);
    iter_next.args = Some(vec!["v4".to_string()]);
    iter_next.out = Some("v14".to_string());

    let mut done_index = op(molt_ir::OpKind::Index);
    done_index.args = Some(vec!["v14".to_string(), "v13".to_string()]);
    done_index.out = Some("v15".to_string());

    let mut loop_break = op(molt_ir::OpKind::LoopBreakIfTrue);
    loop_break.args = Some(vec!["v15".to_string()]);
    loop_break.type_hint = Some("bool".to_string());

    let mut value_index = op(molt_ir::OpKind::Index);
    value_index.args = Some(vec!["v14".to_string(), "v12".to_string()]);
    value_index.out = Some("v16".to_string());

    let mut list_append = op(molt_ir::OpKind::ListAppend);
    list_append.args = Some(vec!["v3".to_string(), "v16".to_string()]);
    list_append.out = Some("none".to_string());
    list_append.type_hint = Some("list".to_string());

    let wasm = compile_single_function(
        vec![
            require_intrinsic,
            op(molt_ir::OpKind::CheckException),
            callargs_new,
            call_indirect,
            op(molt_ir::OpKind::CheckException),
            list_new,
            iter,
            op(molt_ir::OpKind::CheckException),
            none_a,
            none_b,
            is_op,
            if_op,
            err_msg,
            err_type,
            err_args,
            err_obj,
            op(molt_ir::OpKind::CheckException),
            raise_op,
            op(molt_ir::OpKind::CheckException),
            op(molt_ir::OpKind::EndIf),
            zero,
            one,
            op(molt_ir::OpKind::LoopStart),
            iter_next,
            op(molt_ir::OpKind::CheckException),
            done_index,
            op(molt_ir::OpKind::CheckException),
            loop_break,
            value_index,
            op(molt_ir::OpKind::CheckException),
            list_append,
            op(molt_ir::OpKind::CheckException),
            op(molt_ir::OpKind::LoopContinue),
            op(molt_ir::OpKind::LoopEnd),
            ret_value("v3"),
        ],
        &[],
    );
    validate_wasm(&wasm).expect("list(raw) shape should survive TIR roundtrip on wasm");
}

// -----------------------------------------------------------------------
// Comparison operations
// -----------------------------------------------------------------------

#[test]
fn comparison_ops_compile() {
    for op_name in &[
        molt_ir::OpKind::Lt,
        molt_ir::OpKind::Le,
        molt_ir::OpKind::Gt,
        molt_ir::OpKind::Ge,
        molt_ir::OpKind::Eq,
        molt_ir::OpKind::Ne,
    ] {
        let mut cmp = op(*op_name);
        cmp.args = Some(vec!["p0".to_string(), "p1".to_string()]);
        cmp.out = Some("v0".to_string());

        let wasm = compile_single_function(vec![cmp, ret_value("v0")], &["p0", "p1"]);
        let calls = import_call_counts(&wasm);
        assert!(
            count_import(&calls, op_name.as_str()) > 0,
            "{op_name} op should call {op_name} import"
        );
    }
}

// -----------------------------------------------------------------------
// Singleton compilation tests
// -----------------------------------------------------------------------

#[test]
fn missing_singleton_compiles() {
    let mut m = op(molt_ir::OpKind::Missing);
    m.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![m, ret_value("v0")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "missing") > 0,
        "missing op should call missing import"
    );
}

#[test]
fn not_implemented_singleton_compiles() {
    let mut m = op(molt_ir::OpKind::ConstNotImplemented);
    m.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![m, ret_value("v0")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "not_implemented") > 0,
        "const_not_implemented should call not_implemented import"
    );
}

#[test]
fn ellipsis_singleton_compiles() {
    let mut m = op(molt_ir::OpKind::ConstEllipsis);
    m.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![m, ret_value("v0")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "ellipsis") > 0,
        "const_ellipsis should call ellipsis import"
    );
}

#[test]
fn jumpful_br_if_function_validates() {
    let mut cond = op(molt_ir::OpKind::ConstBool);
    cond.value = Some(1);
    cond.out = Some("v0".to_string());

    let mut br_if = op(molt_ir::OpKind::BrIf);
    br_if.args = Some(vec!["v0".to_string()]);
    br_if.value = Some(2);

    let mut one = op(molt_ir::OpKind::Const);
    one.value = Some(1);
    one.out = Some("v1".to_string());

    let mut jump = op(molt_ir::OpKind::Jump);
    jump.value = Some(3);

    let mut label_then = op(molt_ir::OpKind::Label);
    label_then.value = Some(2);

    let mut two = op(molt_ir::OpKind::Const);
    two.value = Some(2);
    two.out = Some("v1".to_string());

    let mut label_join = op(molt_ir::OpKind::Label);
    label_join.value = Some(3);

    let mut ret = op(molt_ir::OpKind::Ret);
    ret.args = Some(vec!["v1".to_string()]);

    let wasm = compile_single_function(
        vec![cond, br_if, one, jump, label_then, two, label_join, ret],
        &[],
    );
    validate_wasm(&wasm).expect("jumpful br_if function should validate structurally");
}

#[test]
fn wasm_does_not_split_non_linear_control_functions() {
    let mut cond = op(molt_ir::OpKind::ConstBool);
    cond.value = Some(1);
    cond.out = Some("v0".to_string());

    let mut br_if = op(molt_ir::OpKind::BrIf);
    br_if.args = Some(vec!["v0".to_string()]);
    br_if.value = Some(2);

    let mut one = op(molt_ir::OpKind::Const);
    one.value = Some(1);
    one.out = Some("v1".to_string());

    let mut jump = op(molt_ir::OpKind::Jump);
    jump.value = Some(3);

    let mut label_then = op(molt_ir::OpKind::Label);
    label_then.value = Some(2);

    let mut two = op(molt_ir::OpKind::Const);
    two.value = Some(2);
    two.out = Some("v1".to_string());

    let mut label_join = op(molt_ir::OpKind::Label);
    label_join.value = Some(3);

    let wasm = compile_ir_with_env(
        SimpleIR {
            functions: vec![FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_test_func".to_string(),
                params: vec![],
                ops: vec![
                    cond,
                    br_if,
                    one,
                    jump,
                    label_then,
                    two,
                    label_join,
                    ret_value("v1"),
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            }],
            profile: None,
        },
        &[("MOLT_MAX_FUNCTION_OPS", Some("2"))],
    );

    assert!(
        !wasm_contains_name_fragment(&wasm, "__molt_chunk_molt_test_func_"),
        "wasm backend must not chunk non-linear control functions with the current sequential chunk stub model"
    );
}

#[test]
fn wasm_preserves_frontend_module_chunk_boundaries() {
    let mut one = op(molt_ir::OpKind::Const);
    one.value = Some(1);
    one.out = Some("v1".to_string());

    let mut two = op(molt_ir::OpKind::Const);
    two.value = Some(2);
    two.out = Some("v2".to_string());

    let wasm = compile_ir_with_env(
        SimpleIR {
            functions: vec![FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "app__molt_module_chunk_6".to_string(),
                params: vec!["__molt_module_obj__".to_string()],
                ops: vec![one, two, op(molt_ir::OpKind::RetVoid)],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            }],
            profile: None,
        },
        &[("MOLT_MAX_FUNCTION_OPS", Some("2"))],
    );

    assert!(
        !wasm_contains_name_fragment(&wasm, "__molt_chunk_app__molt_module_chunk_6_"),
        "wasm backend must not backend-split frontend module chunk functions"
    );
}
