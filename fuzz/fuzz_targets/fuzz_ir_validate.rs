//! Fuzz target: exercise IR validation with arbitrary JSON and structured IR.
//!
//! This tests two paths:
//! 1. `SimpleIR::from_json_str` followed by `validate_simple_ir` — the JSON
//!    deserialization + validation pipeline must never panic.
//! 2. Directly constructed IR with edge-case field combinations — tests that
//!    the validator catches all invalid states without panicking.

#![no_main]
use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use molt_backend::{FunctionIR, OpIR, OpKind, SimpleIR, validate_simple_ir};

#[derive(Debug)]
enum ValidationInput {
    /// Raw JSON string — tests the full parse+validate path.
    RawJson(String),
    /// Unknown spellings are decoder inputs, never typed operation identities.
    UnknownWireKind {
        spelling: &'static str,
        json: String,
    },
    /// Structured IR with edge-case ops — tests validator directly.
    StructuredIR(SimpleIR),
}

/// Op kinds that probe edge cases in the validator: conflicting flags,
/// missing required fields, unusual combinations.
const EDGE_CASE_KINDS: &[OpKind] = &[
    OpKind::Const,
    OpKind::BytearrayFillRange,
    OpKind::Call,
    OpKind::Add,
    OpKind::Store,
    OpKind::Load,
    OpKind::Nop,
    OpKind::Compare,
    OpKind::Subscript,
    OpKind::Slice,
    OpKind::BuildList,
    OpKind::BuildTuple,
    OpKind::BuildDict,
];

// Preserve the old invalid-kind cases at the only boundary that accepts text.
const INVALID_WIRE_SPELLINGS: &[&str] = &["return", "unpack"];

impl<'a> Arbitrary<'a> for ValidationInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        if u.arbitrary::<bool>()? {
            // Raw JSON path, including valid envelopes with unknown kinds.
            if u.arbitrary::<bool>()? {
                let spelling = *u.choose(INVALID_WIRE_SPELLINGS)?;
                let json = format!(
                    r#"{{"functions":[{{"name":"molt_main","return_abi":"void","params":[],"ops":[{{"kind":"{spelling}"}}]}}]}}"#
                );
                return Ok(ValidationInput::UnknownWireKind { spelling, json });
            }
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
                    let kind = EDGE_CASE_KINDS[kind_idx];

                    // Randomly set or omit each field to explore edge cases.
                    let op = OpIR {
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
                            Some((0..n).map(|a| format!("a{a}")).collect())
                        } else {
                            None
                        },
                        out: if u.arbitrary()? {
                            Some(format!("o{j}"))
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
                        ..OpIR::new(kind)
                    };
                    ops.push(op);
                }

                functions.push(FunctionIR {
                    return_abi: if u.arbitrary()? {
                        molt_backend::ir::FunctionReturnAbi::Value
                    } else {
                        molt_backend::ir::FunctionReturnAbi::Void
                    },
                    name,
                    params,
                    ops,
                    param_types: if u.arbitrary()? {
                        Some(vec!["int".to_string()])
                    } else {
                        None
                    },
                    ..FunctionIR::default()
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
        ValidationInput::UnknownWireKind { spelling, json } => {
            let error = SimpleIR::from_json_str(&json)
                .expect_err("unknown wire spelling must be rejected during decode");
            assert!(
                error.contains(spelling),
                "decoder must name {spelling}: {error}"
            );
        }
        ValidationInput::StructuredIR(ir) => {
            // Validate directly — must not panic.
            let _ = validate_simple_ir(&ir);
        }
    }
});
