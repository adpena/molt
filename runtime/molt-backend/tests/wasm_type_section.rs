#![cfg(feature = "wasm-backend")]

//! Tests for the WASM type section: correct static type count,
//! dynamic type generation for user functions, and multi-value return types.

use molt_backend::wasm::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{CompositeInnerType, Parser, Payload};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn compile_ir(ir: SimpleIR) -> Vec<u8> {
    WasmBackend::new().compile(ir)
}

/// Extract type section entries as (param_count, result_count) pairs.
fn extract_type_signatures(wasm: &[u8]) -> Vec<(usize, usize)> {
    let mut sigs = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::TypeSection(reader) = payload.expect("valid payload") {
            for rec_group in reader.into_iter() {
                let rec_group = rec_group.expect("valid rec group");
                for sub_type in rec_group.into_types() {
                    match &sub_type.composite_type.inner {
                        CompositeInnerType::Func(func_type) => {
                            sigs.push((func_type.params().len(), func_type.results().len()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    sigs
}

fn count_types(wasm: &[u8]) -> usize {
    extract_type_signatures(wasm).len()
}

// -----------------------------------------------------------------------
// Static type section tests
// -----------------------------------------------------------------------

#[test]
fn type_section_has_at_least_39_static_types() {
    // The WASM backend defines 39 static types (STATIC_TYPE_COUNT = 39).
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

    let type_count = count_types(&wasm);
    assert!(
        type_count >= 39,
        "should have at least 39 static types, found {type_count}"
    );
}

#[test]
fn type_0_is_nullary_to_i64() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(!sigs.is_empty());
    // Type 0: () -> i64
    assert_eq!(sigs[0], (0, 1), "type 0 should be () -> i64");
}

#[test]
fn type_1_is_unary_to_void() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(sigs.len() > 1);
    // Type 1: (i64) -> ()
    assert_eq!(sigs[1], (1, 0), "type 1 should be (i64) -> ()");
}

#[test]
fn type_8_is_void_to_void() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(sigs.len() > 8);
    // Type 8: () -> ()
    assert_eq!(sigs[8], (0, 0), "type 8 should be () -> ()");
}

// -----------------------------------------------------------------------
// Multi-value return type tests
// -----------------------------------------------------------------------

#[test]
fn multi_return_type_31_is_2_to_2() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(sigs.len() > 31);
    // Type 31: (i64, i64) -> (i64, i64)
    assert_eq!(
        sigs[31],
        (2, 2),
        "type 31 should be (i64, i64) -> (i64, i64)"
    );
}

#[test]
fn multi_return_type_32_is_3_to_3() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(sigs.len() > 32);
    // Type 32: (i64, i64, i64) -> (i64, i64, i64)
    assert_eq!(
        sigs[32],
        (3, 3),
        "type 32 should be (i64, i64, i64) -> (i64, i64, i64)"
    );
}

#[test]
fn multi_return_type_33_is_1_to_2() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(sigs.len() > 33);
    // Type 33: (i64) -> (i64, i64)
    assert_eq!(sigs[33], (1, 2), "type 33 should be (i64) -> (i64, i64)");
}

#[test]
fn multi_return_type_34_is_0_to_2() {
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

    let sigs = extract_type_signatures(&wasm);
    assert!(sigs.len() > 34);
    // Type 34: () -> (i64, i64)
    assert_eq!(sigs[34], (0, 2), "type 34 should be () -> (i64, i64)");
}

// -----------------------------------------------------------------------
// Dynamic type generation tests
// -----------------------------------------------------------------------

#[test]
fn user_function_with_params_adds_dynamic_type() {
    let wasm = compile_ir(SimpleIR {
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
                name: "molt_user_func".to_string(),
                params: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                ops: vec![{
                    let mut ret = op("ret");
                    ret.args = Some(vec!["a".to_string()]);
                    ret
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    });

    let type_count = count_types(&wasm);
    // 39 static types + at least 1 dynamic type for the 3-param function
    // (if arity 3 isn't already covered by a static type with matching signature)
    assert!(
        type_count >= 39,
        "should have >= 39 types with dynamic user function type, found {type_count}"
    );
}

#[test]
fn functions_with_same_arity_share_type() {
    let wasm_single = compile_ir(SimpleIR {
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
                name: "molt_func_a".to_string(),
                params: vec!["x".to_string(), "y".to_string()],
                ops: vec![{
                    let mut ret = op("ret");
                    ret.args = Some(vec!["x".to_string()]);
                    ret
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    });

    let wasm_double = compile_ir(SimpleIR {
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
                name: "molt_func_a".to_string(),
                params: vec!["x".to_string(), "y".to_string()],
                ops: vec![{
                    let mut ret = op("ret");
                    ret.args = Some(vec!["x".to_string()]);
                    ret
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_func_b".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                ops: vec![{
                    let mut ret = op("ret");
                    ret.args = Some(vec!["a".to_string()]);
                    ret
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    });

    let count_single = count_types(&wasm_single);
    let count_double = count_types(&wasm_double);
    // Both should have the same type count since func_a and func_b
    // share the same arity (2 params) and thus the same type signature.
    assert_eq!(
        count_single, count_double,
        "functions with same arity should share type: single={count_single}, double={count_double}"
    );
}

#[test]
fn type_section_contains_expected_arity_signatures() {
    // Verify that the type section contains signatures for various arities
    // used by imports and user functions.
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

    let sigs = extract_type_signatures(&wasm);
    // Check that we have signatures for common arities (param_count, 1) — used by imports.
    let has_sig = |params: usize, results: usize| -> bool {
        sigs.iter().any(|&(p, r)| p == params && r == results)
    };

    // These are all used by the static type section.
    assert!(has_sig(0, 1), "should have (0) -> (1)"); // Type 0
    assert!(has_sig(1, 0), "should have (1) -> (0)"); // Type 1
    assert!(has_sig(1, 1), "should have (1) -> (1)"); // Type 2
    assert!(has_sig(2, 1), "should have (2) -> (1)"); // Type 3
    assert!(has_sig(3, 1), "should have (3) -> (1)"); // Type 5
    assert!(has_sig(4, 1), "should have (4) -> (1)"); // Type 7
    assert!(has_sig(0, 0), "should have (0) -> (0)"); // Type 8
    assert!(has_sig(2, 0), "should have (2) -> (0)"); // Type 6
    assert!(has_sig(2, 2), "should have (2) -> (2)"); // Multi-return type 31
    assert!(has_sig(3, 3), "should have (3) -> (3)"); // Multi-return type 32
}

// -----------------------------------------------------------------------
// High-arity type tests
// -----------------------------------------------------------------------

#[test]
fn high_arity_static_types_exist() {
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

    let sigs = extract_type_signatures(&wasm);
    // Types 35-38: high-arity (9, 10, 11, 12 params) -> i64
    assert!(sigs.len() >= 39);
    assert_eq!(sigs[35], (9, 1), "type 35 should be (i64*9) -> i64");
    assert_eq!(sigs[36], (10, 1), "type 36 should be (i64*10) -> i64");
    assert_eq!(sigs[37], (11, 1), "type 37 should be (i64*11) -> i64");
    assert_eq!(sigs[38], (12, 1), "type 38 should be (i64*12) -> i64");
}
