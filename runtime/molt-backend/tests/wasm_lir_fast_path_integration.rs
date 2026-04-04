#![cfg(feature = "wasm-backend")]

use molt_backend::wasm::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{Operator, Parser, Payload, TypeRef};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn compile_ir(ir: SimpleIR) -> Vec<u8> {
    WasmBackend::new().compile(ir)
}

fn function_has_i64_add(wasm: &[u8], export_name: &str) -> bool {
    let mut imported_funcs = 0u32;
    let mut export_func_index: Option<u32> = None;
    let mut function_type_count = 0u32;
    let mut code_bodies: Vec<bool> = Vec::new();

    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid wasm payload") {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.expect("valid import");
                    if matches!(import.ty, TypeRef::Func(_)) {
                        imported_funcs += 1;
                    }
                }
            }
            Payload::ExportSection(section) => {
                for export in section.into_iter() {
                    let export = export.expect("valid export");
                    if export.name == export_name {
                        if let wasmparser::ExternalKind::Func = export.kind {
                            export_func_index = Some(export.index);
                        }
                    }
                }
            }
            Payload::FunctionSection(section) => {
                function_type_count = section.count();
            }
            Payload::CodeSectionEntry(body) => {
                let mut reader = body.get_operators_reader().expect("operators reader");
                let mut has_i64_add = false;
                while !reader.eof() {
                    match reader.read().expect("valid operator") {
                        Operator::I64Add => has_i64_add = true,
                        _ => {}
                    }
                }
                code_bodies.push(has_i64_add);
            }
            _ => {}
        }
    }

    let export_func_index = export_func_index.expect("exported function should exist");
    let internal_index = export_func_index
        .checked_sub(imported_funcs)
        .expect("exported function should be internally defined");
    assert!(
        internal_index < function_type_count,
        "internal function index must refer to a code body"
    );
    code_bodies[internal_index as usize]
}

#[test]
fn wasm_backend_uses_lir_fast_path_for_simple_scalar_function() {
    let mut add = op("add");
    add.args = Some(vec!["a".to_string(), "b".to_string()]);
    add.out = Some("sum".to_string());
    add.fast_int = Some(true);

    let mut ret = op("ret");
    ret.var = Some("sum".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_wasm_lir_fast_path____molt_globals_builtin__".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            ops: vec![add, ret],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let has_i64_add = function_has_i64_add(
        &wasm,
        "molt_test_wasm_lir_fast_path____molt_globals_builtin__",
    );

    assert!(has_i64_add, "fast path should emit native i64.add");
}

#[test]
fn wasm_backend_skips_lir_fast_path_for_void_return_function() {
    let mut add = op("add");
    add.args = Some(vec!["a".to_string(), "b".to_string()]);
    add.out = Some("sum".to_string());
    add.fast_int = Some(true);

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_test_wasm_lir_fast_path_void____molt_globals_builtin__".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            ops: vec![add, op("ret_void")],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let has_i64_add = function_has_i64_add(
        &wasm,
        "molt_test_wasm_lir_fast_path_void____molt_globals_builtin__",
    );

    assert!(
        !has_i64_add,
        "void-return functions should stay off the boxed-i64 fast path"
    );
}
