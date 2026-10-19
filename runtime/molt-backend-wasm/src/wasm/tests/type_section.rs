//! Structural type identity is independent of allocation order and table size.

use super::support::*;

fn function(name: &str, arity: usize) -> FunctionIR {
    FunctionIR {
        name: name.into(),
        params: (0..arity).map(|i| format!("arg{i}")).collect(),
        ops: vec![wasm_test_op("ret_void", None, vec![])],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ..Default::default()
    }
}

fn compile(functions: Vec<FunctionIR>) -> Vec<u8> {
    let bytes = WasmBackend::new().compile(SimpleIR {
        functions,
        profile: None,
    });
    wasmparser::Validator::new().validate_all(&bytes).unwrap();
    bytes
}

#[test]
fn module_types_are_unique_structural_signatures_with_one_result_authority() {
    let bytes = compile(vec![function("molt_main", 0)]);
    let signatures = wasm_type_section_value_signatures(&bytes);
    assert!(!signatures.is_empty());
    assert_eq!(
        signatures.iter().collect::<BTreeSet<_>>().len(),
        signatures.len()
    );
    assert!(signatures.iter().all(|(_, results)| results.len() <= 1));
    for (name, signature) in wasm_function_import_type_indices(&bytes) {
        let import = wasm_runtime_import(&name).expect("declared runtime import");
        let (params, results) = import.machine_signature();
        assert_eq!(
            signatures[signature as usize],
            (
                params.iter().map(|ty| format!("{ty:?}")).collect(),
                results.iter().map(|ty| format!("{ty:?}")).collect()
            ),
            "runtime import {name} must reference its exact machine type"
        );
    }
}

#[test]
fn equally_typed_user_functions_reference_one_module_type() {
    let mut first = function("first", 2);
    // Escaping the callable preserves both functions through reachability and
    // prevents inlining from erasing the independent type references.
    let mut callable = wasm_test_op("func_new", Some("callable"), vec![]);
    callable.s_value = Some("second".into());
    callable.value = Some(2);
    first.ops = vec![callable, wasm_test_op("ret", None, vec!["callable"])];
    let bytes = compile(vec![first, function("second", 2)]);
    let signatures = wasm_type_section_value_signatures(&bytes);
    let requested = (vec!["I64".to_string(); 2], vec!["I64".to_string()]);
    assert_eq!(signatures.iter().filter(|ty| **ty == requested).count(), 1);
    let imported = wasm_function_import_indices(&bytes).len() as u32;
    let functions = wasm_function_section_type_indices(&bytes);
    let exports = wasm_function_export_indices(&bytes);
    let first = functions[(exports["first"] - imported) as usize];
    let second = functions[(exports["second"] - imported) as usize];
    assert_eq!(first, second);
    assert_eq!(signatures[first as usize], requested);
}

#[test]
fn a_large_user_arity_adds_one_type_without_reserving_neighboring_arities() {
    let baseline = compile(vec![function("sparse", 0)]);
    let sparse = compile(vec![function("sparse", 512)]);
    let before = wasm_type_section_value_signatures(&baseline)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let after = wasm_type_section_value_signatures(&sparse)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let added = after.difference(&before).collect::<Vec<_>>();
    assert_eq!(
        added,
        vec![&(vec!["I64".to_string(); 512], vec!["I64".to_string()])]
    );
}
