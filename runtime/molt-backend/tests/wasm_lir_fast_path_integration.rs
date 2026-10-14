#![cfg(feature = "wasm-backend")]

use molt_backend::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{
    CompositeInnerType, ExternalKind, FuncType, Parser, Payload, TypeRef, ValType, Validator,
};

fn exported_function_type(wasm: &[u8], name: &str) -> FuncType {
    Validator::new()
        .validate_all(wasm)
        .expect("valid emitted WASM module");
    let mut types = Vec::new();
    let mut functions = Vec::new();
    let mut imported = 0;
    let mut exported = None;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid WASM payload") {
            Payload::TypeSection(section) => {
                for group in section {
                    for subtype in group.unwrap().into_types() {
                        types.push(match subtype.composite_type.inner {
                            CompositeInnerType::Func(function) => Some(function),
                            _ => None,
                        });
                    }
                }
            }
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    if let TypeRef::Func(index) = import.unwrap().ty {
                        functions.push(index);
                        imported += 1;
                    }
                }
            }
            Payload::FunctionSection(section) => {
                functions.extend(section.map(|index| index.unwrap()));
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.unwrap();
                    if export.name == name {
                        assert_eq!(export.kind, ExternalKind::Func);
                        assert!(exported.replace(export.index).is_none());
                    }
                }
            }
            _ => {}
        }
    }
    let index = exported.expect("exact fixture export") as usize;
    assert!(index >= imported, "fixture export must be locally defined");
    types[functions[index] as usize]
        .clone()
        .expect("function export type")
}

// Path admission is tested against the canonical plan in molt-backend-wasm;
// I64Add is also emitted by generic arithmetic and frame address calculations.
// This boundary checks the real exported ABI and module validity. The existing
// execution_context_split authority executes value and authored-Void returns.
#[test]
fn wasm_backend_exports_value_and_authored_void_under_boxed_user_abi() {
    for return_abi in [
        molt_ir::FunctionReturnAbi::Value,
        molt_ir::FunctionReturnAbi::Void,
    ] {
        let name = "molt_test_return_abi____molt_globals_builtin__";
        let returns_value = return_abi == molt_ir::FunctionReturnAbi::Value;
        let wasm = WasmBackend::new().compile(SimpleIR {
            functions: vec![FunctionIR {
                name: name.into(),
                return_abi,
                ops: vec![
                    OpIR {
                        kind: "const".into(),
                        value: Some(42),
                        out: Some("result".into()),
                        ..Default::default()
                    },
                    OpIR {
                        kind: if returns_value { "ret" } else { "ret_void" }.into(),
                        args: returns_value.then(|| vec!["result".into()]),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            profile: None,
        });
        let signature = exported_function_type(&wasm, name);
        assert!(signature.params().is_empty(), "{return_abi:?}");
        assert_eq!(
            signature.results(),
            &[ValType::I64],
            "{return_abi:?}: local Void returns boxed None; only void externs have zero results"
        );
    }
}
