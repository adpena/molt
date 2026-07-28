use super::support::*;
use crate::ir::ExecutionContextPolicy;

#[test]
fn wasm_compiles_split_local_frame_with_inherited_chunks() {
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
        name: "wasm_framed_large".to_string(),
        ops,
        execution_context: ExecutionContextPolicy::Local,
        ..FunctionIR::default()
    };
    let mut occupied = BTreeSet::from([original.name.clone()]);
    let (stub, chunks) = crate::passes::split_large_function(original, 3, &mut occupied).unwrap();
    let chunk_names = chunks
        .iter()
        .map(|chunk| chunk.name.clone())
        .collect::<Vec<_>>();
    let ir = SimpleIR {
        functions: std::iter::once(stub).chain(chunks).collect(),
        profile: None,
    };
    crate::validate_simple_ir(&ir).unwrap();
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled: false,
        wasm_profile: WasmProfile::Auto,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
    wasmparser::Validator::new().validate_all(&wasm).unwrap();
    let imports = wasm_function_import_names(&wasm);
    assert!(imports.iter().any(|name| name == "trace_enter_slot"));
    assert!(imports.iter().any(|name| name == "trace_exit"));
    let exports = wasm_function_export_indices(&wasm);
    for chunk_name in chunk_names {
        assert!(
            exports.contains_key(&chunk_name),
            "missing split chunk {chunk_name}"
        );
    }
}
