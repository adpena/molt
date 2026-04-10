#![cfg(feature = "wasm-backend")]

//! Tests for basic IR compilation: empty modules, single functions,
//! string constants, arithmetic, function calls, and exception handling.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use molt_backend::wasm::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use std::f64::consts::PI;
use wasmparser::{Operator, Parser, Payload, TypeRef, Validator};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn compile_ir(ir: SimpleIR) -> Vec<u8> {
    WasmBackend::new().compile(ir)
}

fn compile_single_function(ops: Vec<OpIR>, params: &[&str]) -> Vec<u8> {
    compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_func".to_string(),
            params: params.iter().map(|p| (*p).to_string()).collect(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
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

fn compile_single_function_without_tir(ops: Vec<OpIR>, params: &[&str]) -> Vec<u8> {
    compile_ir_with_env(
        SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_test_func".to_string(),
                params: params.iter().map(|p| (*p).to_string()).collect(),
                ops,
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        },
        &[("MOLT_TIR_OPT", Some("0"))],
    )
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

fn count_import(calls: &HashMap<String, usize>, name: &str) -> usize {
    calls.get(name).copied().unwrap_or(0)
}

fn validate_wasm(wasm: &[u8]) -> Result<(), wasmparser::BinaryReaderError> {
    Validator::new().validate_all(wasm).map(|_| ())
}

fn ret_value(name: &str) -> OpIR {
    let mut ret = op("ret");
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
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    // Should start with WASM magic bytes
    assert!(wasm.len() > 8, "WASM output too short");
    assert_eq!(&wasm[0..4], b"\0asm", "missing WASM magic bytes");
}

#[test]
fn empty_module_is_structurally_valid_wasm() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    validate_wasm(&wasm).expect("compiled wasm should validate structurally");
}

#[test]
fn empty_module_exports_molt_main() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
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
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let wasm = compile_single_function(vec![op("ret_void")], &[]);
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
    let mut c = op("const");
    c.value = Some(42);
    c.out = Some("v0".to_string());

    // Should not panic — the constant should be inlined as i64.const.
    let wasm = compile_single_function(vec![c, op("ret_void")], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_float_compiles() {
    let mut c = op("const_float");
    c.f_value = Some(PI);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op("ret_void")], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_none_compiles() {
    let mut c = op("const_none");
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op("ret_void")], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_bool_true_compiles() {
    let mut c = op("const_bool");
    c.value = Some(1);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op("ret_void")], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_bool_false_compiles() {
    let mut c = op("const_bool");
    c.value = Some(0);
    c.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![c, op("ret_void")], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn const_str_compiles_and_calls_string_from_bytes() {
    let mut c = op("const_str");
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
    let mut add = op("add");
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
fn sub_op_calls_sub_import() {
    let mut sub = op("sub");
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
    let mut mul = op("mul");
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
    let mut call = op("call_func");
    call.args = Some(vec!["p0".to_string(), "p1".to_string(), "p2".to_string()]);
    call.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![call, op("ret_void")], &["p0", "p1", "p2"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "call_func_dispatch") > 0,
        "call_func should use call_func_dispatch"
    );
}

#[test]
fn call_bind_without_output_compiles() {
    let mut call = op("call_bind");
    call.args = Some(vec!["p0".to_string(), "p1".to_string()]);

    let wasm = compile_single_function(vec![call, op("ret_void")], &["p0", "p1"]);
    validate_wasm(&wasm).expect("output-less call_bind should still produce valid wasm");
}

#[test]
fn call_indirect_without_output_compiles() {
    let mut call = op("call_indirect");
    call.args = Some(vec!["p0".to_string(), "p1".to_string()]);

    let wasm = compile_single_function(vec![call, op("ret_void")], &["p0", "p1"]);
    validate_wasm(&wasm).expect("output-less call_indirect should still produce valid wasm");
}

#[test]
fn call_guarded_escaped_function_dispatches_on_object() {
    let mut func_new = op("func_new");
    func_new.s_value = Some("guarded_target".to_string());
    func_new.value = Some(2);
    func_new.out = Some("v_func".to_string());

    let mut call = op("call_guarded");
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
                name: "molt_test_func".to_string(),
                params: vec!["p0".to_string(), "p1".to_string()],
                ops: vec![func_new, call, ret_value("v0")],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "guarded_target".to_string(),
                params: vec!["arg0".to_string(), "arg1".to_string()],
                ops: vec![op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
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
    let mut class_name = op("const_str");
    class_name.s_value = Some("A".to_string());
    class_name.out = Some("v_name".to_string());

    let mut attr_name = op("const_str");
    attr_name.s_value = Some("x".to_string());
    attr_name.out = Some("v_attr".to_string());

    let mut attr_value = op("const");
    attr_value.value = Some(1);
    attr_value.out = Some("v_value".to_string());

    let mut class_def = op("class_def");
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
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "guarded_class_def") > 0,
        "class_def should call guarded_class_def"
    );
}

#[test]
fn ret_with_value_compiles() {
    let mut c = op("const");
    c.value = Some(42);
    c.out = Some("v0".to_string());

    let mut ret = op("ret");
    ret.args = Some(vec!["v0".to_string()]);

    let wasm = compile_single_function(vec![c, ret], &[]);
    assert!(wasm.len() > 8);
}

#[test]
fn function_without_explicit_ret_still_validates() {
    let mut c = op("const");
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
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_helper".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![{
                    let mut ret = op("ret");
                    ret.args = Some(vec!["p0".to_string()]);
                    ret
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
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
    let mut dict_new = op("dict_new");
    dict_new.out = Some("v0".to_string());
    dict_new.args = Some(vec![]);

    let wasm = compile_single_function(
        vec![
            op("try_start"),
            dict_new,
            op("check_exception"),
            op("try_end"),
            op("ret_void"),
        ],
        &[],
    );
    assert!(wasm.len() > 8);
}

#[test]
fn nested_try_blocks_compile() {
    let mut dict1 = op("dict_new");
    dict1.out = Some("v0".to_string());
    dict1.args = Some(vec![]);

    let mut dict2 = op("dict_new");
    dict2.out = Some("v1".to_string());
    dict2.args = Some(vec![]);

    let wasm = compile_single_function(
        vec![
            op("try_start"),
            dict1,
            op("check_exception"),
            op("try_start"),
            dict2,
            op("check_exception"),
            op("try_end"),
            op("try_end"),
            op("ret_void"),
        ],
        &[],
    );
    assert!(wasm.len() > 8);
}

// -----------------------------------------------------------------------
// Collection operations compilation tests
// -----------------------------------------------------------------------

#[test]
fn dict_new_calls_dict_new_import() {
    let mut d = op("dict_new");
    d.out = Some("v0".to_string());
    d.args = Some(vec![]);

    let wasm = compile_single_function(vec![d, op("ret_void")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "dict_new") > 0,
        "dict_new op should call dict_new import"
    );
}

#[test]
fn list_new_compiles_using_builder_imports() {
    // The "list_new" IR op uses list_builder_new + list_builder_append + list_builder_finish.
    let mut list = op("list_new");
    list.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    list.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![list, op("ret_void")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "list_builder_new") > 0,
        "list_new should call list_builder_new"
    );
    assert!(
        count_import(&calls, "list_builder_append") > 0,
        "list_new should call list_builder_append"
    );
    assert!(
        count_import(&calls, "list_builder_finish") > 0,
        "list_new should call list_builder_finish"
    );
}

#[test]
fn build_list_compiles_using_builder_imports() {
    let mut list = op("build_list");
    list.args = Some(vec!["p0".to_string(), "p1".to_string()]);
    list.out = Some("v0".to_string());

    let wasm = compile_single_function_without_tir(vec![list, op("ret_void")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "list_builder_new") > 0,
        "build_list should call list_builder_new"
    );
    assert!(
        count_import(&calls, "list_builder_append") > 0,
        "build_list should call list_builder_append"
    );
    assert!(
        count_import(&calls, "list_builder_finish") > 0,
        "build_list should call list_builder_finish"
    );
}

#[test]
fn iter_next_unboxed_compiles_using_iter_next_and_index_imports() {
    let mut iter_next = op("iter_next_unboxed");
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
    let mut guard = op("guard_tag");
    guard.args = Some(vec!["p0".to_string(), "p1".to_string()]);

    let wasm = compile_single_function(vec![guard, op("ret_void")], &["p0", "p1"]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "guard_type") > 0,
        "guard_tag should lower through guard_type on wasm"
    );
}

#[test]
fn alloc_task_generator_keeps_task_new_import() {
    let mut alloc_task = op("alloc_task");
    alloc_task.s_value = Some("__main_____f_poll".to_string());
    alloc_task.value = Some(48);
    alloc_task.task_kind = Some("generator".to_string());
    alloc_task.args = Some(vec![]);
    alloc_task.out = Some("task".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_test_func".to_string(),
                params: vec![],
                ops: vec![alloc_task, ret_value("task")],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "__main_____f_poll".to_string(),
                params: vec!["self".to_string()],
                ops: vec![op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
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
    let mut alloc_task = op("alloc_task");
    alloc_task.s_value = Some("__main_____future_poll".to_string());
    alloc_task.value = Some(0);
    alloc_task.task_kind = Some("future".to_string());
    alloc_task.args = Some(vec![]);
    alloc_task.out = Some("task".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_test_func".to_string(),
                params: vec![],
                ops: vec![alloc_task, ret_value("task")],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "__main_____future_poll".to_string(),
                params: vec!["self".to_string()],
                ops: vec![op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    });
    validate_wasm(&wasm).expect("future alloc_task without args should compile");
}

#[test]
fn list_from_intrinsic_list_survives_tir_roundtrip() {
    let mut require_intrinsic = op("builtin_func");
    require_intrinsic.s_value = Some("molt_getargv".to_string());
    require_intrinsic.value = Some(0);
    require_intrinsic.out = Some("v0".to_string());

    let mut callargs_new = op("callargs_new");
    callargs_new.out = Some("v1".to_string());

    let mut call_indirect = op("call_indirect");
    call_indirect.args = Some(vec!["v0".to_string(), "v1".to_string()]);
    call_indirect.out = Some("v2".to_string());

    let mut list_new = op("list_new");
    list_new.args = Some(vec![]);
    list_new.out = Some("v3".to_string());
    list_new.type_hint = Some("list".to_string());

    let mut iter = op("iter");
    iter.args = Some(vec!["v2".to_string()]);
    iter.out = Some("v4".to_string());
    iter.type_hint = Some("iter".to_string());

    let mut none_a = op("const_none");
    none_a.out = Some("v5".to_string());
    let mut none_b = op("const_none");
    none_b.out = Some("v6".to_string());

    let mut is_op = op("is");
    is_op.args = Some(vec!["v4".to_string(), "v6".to_string()]);
    is_op.out = Some("v7".to_string());

    let mut if_op = op("if");
    if_op.args = Some(vec!["v7".to_string()]);
    if_op.type_hint = Some("bool".to_string());

    let mut err_msg = op("const_str");
    err_msg.s_value = Some("object is not iterable".to_string());
    err_msg.out = Some("v8".to_string());

    let mut err_type = op("const_str");
    err_type.s_value = Some("TypeError".to_string());
    err_type.out = Some("v9".to_string());

    let mut err_args = op("tuple_new");
    err_args.args = Some(vec!["v8".to_string()]);
    err_args.out = Some("v10".to_string());
    err_args.type_hint = Some("tuple".to_string());

    let mut err_obj = op("exception_new");
    err_obj.args = Some(vec!["v9".to_string(), "v10".to_string()]);
    err_obj.out = Some("v11".to_string());

    let mut raise_op = op("raise");
    raise_op.args = Some(vec!["v11".to_string()]);
    raise_op.out = Some("none".to_string());

    let mut zero = op("const");
    zero.value = Some(0);
    zero.out = Some("v12".to_string());

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("v13".to_string());

    let mut iter_next = op("iter_next");
    iter_next.args = Some(vec!["v4".to_string()]);
    iter_next.out = Some("v14".to_string());

    let mut done_index = op("index");
    done_index.args = Some(vec!["v14".to_string(), "v13".to_string()]);
    done_index.out = Some("v15".to_string());

    let mut loop_break = op("loop_break_if_true");
    loop_break.args = Some(vec!["v15".to_string()]);
    loop_break.type_hint = Some("bool".to_string());

    let mut value_index = op("index");
    value_index.args = Some(vec!["v14".to_string(), "v12".to_string()]);
    value_index.out = Some("v16".to_string());

    let mut list_append = op("list_append");
    list_append.args = Some(vec!["v3".to_string(), "v16".to_string()]);
    list_append.out = Some("none".to_string());
    list_append.type_hint = Some("list".to_string());

    let wasm = compile_single_function(
        vec![
            require_intrinsic,
            op("check_exception"),
            callargs_new,
            call_indirect,
            op("check_exception"),
            list_new,
            iter,
            op("check_exception"),
            none_a,
            none_b,
            is_op,
            if_op,
            err_msg,
            err_type,
            err_args,
            err_obj,
            op("check_exception"),
            raise_op,
            op("check_exception"),
            op("end_if"),
            zero,
            one,
            op("loop_start"),
            iter_next,
            op("check_exception"),
            done_index,
            op("check_exception"),
            loop_break,
            value_index,
            op("check_exception"),
            list_append,
            op("check_exception"),
            op("loop_continue"),
            op("loop_end"),
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
    for op_name in &["lt", "le", "gt", "ge", "eq", "ne"] {
        let mut cmp = op(op_name);
        cmp.args = Some(vec!["p0".to_string(), "p1".to_string()]);
        cmp.out = Some("v0".to_string());

        let wasm = compile_single_function(vec![cmp, ret_value("v0")], &["p0", "p1"]);
        let calls = import_call_counts(&wasm);
        assert!(
            count_import(&calls, op_name) > 0,
            "{op_name} op should call {op_name} import"
        );
    }
}

// -----------------------------------------------------------------------
// Singleton compilation tests
// -----------------------------------------------------------------------

#[test]
fn missing_singleton_compiles() {
    let mut m = op("missing");
    m.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![m, op("ret_void")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "missing") > 0,
        "missing op should call missing import"
    );
}

#[test]
fn not_implemented_singleton_compiles() {
    let mut m = op("const_not_implemented");
    m.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![m, op("ret_void")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "not_implemented") > 0,
        "const_not_implemented should call not_implemented import"
    );
}

#[test]
fn ellipsis_singleton_compiles() {
    let mut m = op("const_ellipsis");
    m.out = Some("v0".to_string());

    let wasm = compile_single_function(vec![m, op("ret_void")], &[]);
    let calls = import_call_counts(&wasm);
    assert!(
        count_import(&calls, "ellipsis") > 0,
        "const_ellipsis should call ellipsis import"
    );
}

#[test]
fn jumpful_br_if_function_validates() {
    let mut cond = op("const_bool");
    cond.value = Some(1);
    cond.out = Some("v0".to_string());

    let mut br_if = op("br_if");
    br_if.args = Some(vec!["v0".to_string()]);
    br_if.value = Some(2);

    let mut one = op("const");
    one.value = Some(1);
    one.out = Some("v1".to_string());

    let mut jump = op("jump");
    jump.value = Some(3);

    let mut label_then = op("label");
    label_then.value = Some(2);

    let mut two = op("const");
    two.value = Some(2);
    two.out = Some("v1".to_string());

    let mut label_join = op("label");
    label_join.value = Some(3);

    let mut ret = op("ret");
    ret.var = Some("v1".to_string());

    let wasm = compile_single_function(
        vec![cond, br_if, one, jump, label_then, two, label_join, ret],
        &[],
    );
    validate_wasm(&wasm).expect("jumpful br_if function should validate structurally");
}
