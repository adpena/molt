#![cfg(feature = "wasm-backend")]

use std::collections::HashMap;

use molt_backend::wasm::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{Operator, Parser, Payload, TypeRef};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn compile_single_function(ir_op: OpIR, params: &[&str]) -> Vec<u8> {
    compile_ops(vec![ir_op, op("ret_void")], params)
}

fn compile_ops(ops: Vec<OpIR>, params: &[&str]) -> Vec<u8> {
    compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_wasm_fastcall_lowering".to_string(),
            params: params.iter().map(|p| (*p).to_string()).collect(),
            ops,
            param_types: None,
        }],
        profile: None,
    })
}

fn compile_ir(ir: SimpleIR) -> Vec<u8> {
    WasmBackend::new().compile(ir)
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

fn count(calls: &HashMap<String, usize>, import_name: &str) -> usize {
    calls.get(import_name).copied().unwrap_or(0)
}

#[test]
fn wasm_lowers_small_arity_call_func_to_generic_ic() {
    let mut call = op("call_func");
    call.args = Some(vec!["p0".to_string(), "p1".to_string(), "p2".to_string()]);
    call.out = Some("v0".to_string());

    let wasm = compile_single_function(call, &["p0", "p1", "p2"]);
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "call_bind_ic") > 0);
}

#[test]
fn wasm_lowers_medium_arity_call_func_to_generic_ic() {
    let mut call = op("call_func");
    call.args = Some(vec![
        "p0".to_string(),
        "p1".to_string(),
        "p2".to_string(),
        "p3".to_string(),
        "p4".to_string(),
        "p5".to_string(),
        "p6".to_string(),
    ]);
    call.out = Some("v0".to_string());

    let wasm = compile_single_function(call, &["p0", "p1", "p2", "p3", "p4", "p5", "p6"]);
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "call_bind_ic") > 0);
}

#[test]
fn wasm_lowers_small_arity_call_method_to_generic_ic() {
    let mut call = op("call_method");
    call.args = Some(vec![
        "p0".to_string(),
        "p1".to_string(),
        "p2".to_string(),
        "p3".to_string(),
    ]);
    call.out = Some("v0".to_string());

    let wasm = compile_single_function(call, &["p0", "p1", "p2", "p3"]);
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "call_bind_ic") > 0);
}

#[test]
fn wasm_lowers_small_arity_invoke_ffi_to_generic_ic() {
    let mut invoke = op("invoke_ffi");
    invoke.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    invoke.out = Some("v0".to_string());
    invoke.s_value = Some("bridge".to_string());

    let wasm = compile_single_function(invoke, &["p0", "p1"]);
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "invoke_ffi_ic") > 0);
}

#[test]
fn wasm_lowers_medium_arity_invoke_ffi_to_generic_ic() {
    let mut invoke = op("invoke_ffi");
    invoke.args = Some(vec![
        "p0".to_string(),
        "p1".to_string(),
        "p2".to_string(),
        "p3".to_string(),
        "p4".to_string(),
        "p5".to_string(),
        "p6".to_string(),
        "p7".to_string(),
        "p8".to_string(),
    ]);
    invoke.out = Some("v0".to_string());
    invoke.s_value = Some("bridge".to_string());

    let wasm = compile_single_function(
        invoke,
        &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8"],
    );
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "invoke_ffi_ic") > 0);
}

#[test]
fn wasm_uses_builder_fallback_for_large_arity_call_func() {
    let mut call = op("call_func");
    call.args = Some(vec![
        "p0".to_string(),
        "p1".to_string(),
        "p2".to_string(),
        "p3".to_string(),
        "p4".to_string(),
        "p5".to_string(),
        "p6".to_string(),
        "p7".to_string(),
        "p8".to_string(),
        "p9".to_string(),
    ]);
    call.out = Some("v0".to_string());

    let wasm = compile_single_function(
        call,
        &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"],
    );
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "call_bind_ic") > 0);
}

#[test]
fn wasm_call_guarded_requires_known_target() {
    let mut guarded = op("call_guarded");
    guarded.s_value = Some("molt_missing_target_for_guarded_test".to_string());
    guarded.args = Some(vec!["p0".to_string(), "p1".to_string(), "p2".to_string()]);
    guarded.out = Some("v0".to_string());

    let result = std::panic::catch_unwind(|| compile_single_function(guarded, &["p0", "p1", "p2"]));
    assert!(result.is_err());
}

#[test]
fn wasm_lowers_call_guarded_matched_arity_with_generic_slow_path() {
    let mut target_ret = op("ret");
    target_ret.args = Some(vec!["t0".to_string()]);

    let mut guarded = op("call_guarded");
    guarded.s_value = Some("molt_test_guarded_target".to_string());
    guarded.args = Some(vec!["p0".to_string(), "p1".to_string(), "p2".to_string()]);
    guarded.out = Some("v0".to_string());

    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_test_guarded_target".to_string(),
                params: vec!["t0".to_string(), "t1".to_string()],
                ops: vec![target_ret],
                param_types: None,
            },
            FunctionIR {
                name: "molt_test_wasm_fastcall_guarded_matched".to_string(),
                params: vec!["p0".to_string(), "p1".to_string(), "p2".to_string()],
                ops: vec![guarded, op("ret_void")],
                param_types: None,
            },
        ],
        profile: None,
    };
    let wasm = compile_ir(ir);
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "callargs_new") > 0);
    assert!(count(&calls, "call_bind_ic") > 0);
}

#[test]
fn wasm_check_exception_uses_exception_pending_probe() {
    let mut dict_new = op("dict_new");
    dict_new.out = Some("v0".to_string());
    dict_new.args = Some(vec![]);

    let wasm = compile_ops(
        vec![
            op("try_start"),
            dict_new,
            op("check_exception"),
            op("try_end"),
            op("ret_void"),
        ],
        &[],
    );
    let calls = import_call_counts(&wasm);

    assert!(count(&calls, "exception_pending") > 0);
}

#[test]
fn wasm_check_exception_keeps_explicit_probes_after_non_raising_op() {
    let mut dict_new = op("dict_new");
    dict_new.out = Some("v0".to_string());
    dict_new.args = Some(vec![]);

    let mut const_one = op("const");
    const_one.value = Some(1);
    const_one.out = Some("v1".to_string());

    let wasm = compile_ops(
        vec![
            op("try_start"),
            dict_new,
            op("check_exception"),
            const_one,
            op("check_exception"),
            op("try_end"),
            op("ret_void"),
        ],
        &[],
    );
    let calls = import_call_counts(&wasm);

    assert_eq!(count(&calls, "exception_pending"), 2);
}
