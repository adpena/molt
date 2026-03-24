//! Fuzz target: exercise IR validation with arbitrary JSON and structured IR.
//!
//! This tests two paths:
//! 1. `SimpleIR::from_json_str` followed by `validate_simple_ir` — the JSON
//!    deserialization + validation pipeline must never panic.
//! 2. Directly constructed IR with edge-case field combinations — tests that
//!    the validator catches all invalid states without panicking.

#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::{Arbitrary, Unstructured};
use molt_backend::{FunctionIR, OpIR, SimpleIR, validate_simple_ir};

#[derive(Debug)]
enum ValidationInput {
    /// Raw JSON string — tests the full parse+validate path.
    RawJson(String),
    /// Structured IR with edge-case ops — tests validator directly.
    StructuredIR(SimpleIR),
}

/// Op kinds that probe edge cases in the validator: conflicting flags,
/// missing required fields, unusual combinations.
const EDGE_CASE_KINDS: &[&str] = &[
    "const",
    "list_repeat_range",
    "bytearray_fill_range",
    "call",
    "return",
    "add",
    "store",
    "load",
    "nop",
    "compare",
    "subscript",
    "slice",
    "unpack",
    "build_list",
    "build_tuple",
    "build_dict",
];

impl<'a> Arbitrary<'a> for ValidationInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        if u.arbitrary::<bool>()? {
            // Raw JSON path.
            let len: usize = u.int_in_range(0..=512)?;
            let bytes: Vec<u8> = (0..len)
                .map(|_| u.int_in_range(0x20u8..=0x7e))
                .collect::<Result<_, _>>()?;
            Ok(ValidationInput::RawJson(
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        } else {
            // Structured IR path.
            let num_funcs: usize = u.int_in_range(1..=3)?;
            let mut functions = Vec::with_capacity(num_funcs);

            for i in 0..num_funcs {
                let name = if i == 0 {
                    "molt_main".to_string()
                } else {
                    format!("f{i}")
                };
                let num_params: usize = u.int_in_range(0..=4)?;
                let params: Vec<String> = (0..num_params).map(|p| format!("p{p}")).collect();
                let num_ops: usize = u.int_in_range(0..=15)?;
                let mut ops = Vec::with_capacity(num_ops);

                for j in 0..num_ops {
                    let kind_idx = u.int_in_range(0..=EDGE_CASE_KINDS.len() - 1)?;
                    let kind = EDGE_CASE_KINDS[kind_idx].to_string();

                    // Randomly set or omit each field to explore edge cases.
                    let op = OpIR {
                        kind,
                        value: if u.arbitrary()? {
                            Some(u.int_in_range(-100..=100)?)
                        } else {
                            None
                        },
                        f_value: if u.arbitrary()? {
                            Some(u.arbitrary::<f64>().unwrap_or(0.0))
                        } else {
                            None
                        },
                        s_value: if u.arbitrary()? {
                            Some(format!("s{j}"))
                        } else {
                            None
                        },
                        bytes: if u.arbitrary()? {
                            let len = u.int_in_range(0..=8)?;
                            Some(u.bytes(len)?.to_vec())
                        } else {
                            None
                        },
                        var: if u.arbitrary()? {
                            Some(format!("v{j}"))
                        } else {
                            None
                        },
                        args: if u.arbitrary()? {
                            let n = u.int_in_range(0..=5)?;
                            Some(
                                (0..n)
                                    .map(|a| format!("a{a}"))
                                    .collect(),
                            )
                        } else {
                            None
                        },
                        out: if u.arbitrary()? {
                            Some(format!("o{j}"))
                        } else {
                            None
                        },
                        // Test conflicting flags — validator should catch these.
                        fast_int: if u.arbitrary()? {
                            Some(u.arbitrary()?)
                        } else {
                            None
                        },
                        fast_float: if u.arbitrary()? {
                            Some(u.arbitrary()?)
                        } else {
                            None
                        },
                        raw_int: if u.arbitrary()? {
                            Some(u.arbitrary()?)
                        } else {
                            None
                        },
                        stack_eligible: if u.arbitrary()? {
                            Some(u.arbitrary()?)
                        } else {
                            None
                        },
                        task_kind: None,
                        container_type: None,
                        type_hint: None,
                    };
                    ops.push(op);
                }

                functions.push(FunctionIR {
                    name,
                    params,
                    ops,
                    param_types: if u.arbitrary()? {
                        Some(vec!["int".to_string()])
                    } else {
                        None
                    },
                });
            }

            Ok(ValidationInput::StructuredIR(SimpleIR {
                functions,
                profile: None,
            }))
        }
    }
}

fuzz_target!(|input: ValidationInput| {
    match input {
        ValidationInput::RawJson(json) => {
            // Parse + validate — must not panic.
            if let Ok(ir) = SimpleIR::from_json_str(&json) {
                let _ = validate_simple_ir(&ir);
            }
        }
        ValidationInput::StructuredIR(ir) => {
            // Validate directly — must not panic.
            let _ = validate_simple_ir(&ir);
        }
    }
});
