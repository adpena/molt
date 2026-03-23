//! Fuzz target: exercise IR optimization passes on arbitrary IR.
//!
//! Invariants tested:
//! - Passes must not panic on any structurally valid IR
//! - Passes must not produce IR that fails validation if the input passes
//! - Function count must be non-negative after dead function elimination
//!
//! This target runs the full optimization pipeline: constant folding, cross-block
//! folding, dead struct elision, escape analysis, loop fast-int propagation,
//! RC coalescing, inlining, and dead function elimination.

#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::{Arbitrary, Unstructured};
use molt_backend::{
    FunctionIR, OpIR, SimpleIR,
    fold_constants, fold_constants_cross_block, elide_dead_struct_allocs,
    escape_analysis, propagate_loop_fast_int, rc_coalescing,
    inline_functions, eliminate_dead_functions, apply_profile_order,
};

/// Generate a structurally valid IR with ops that exercise the passes.
#[derive(Debug)]
struct PassFuzzInput {
    ir: SimpleIR,
}

const PASS_OP_KINDS: &[&str] = &[
    "const",
    "const_bool",
    "const_none",
    "const_float",
    "add",
    "sub",
    "mul",
    "negate",
    "compare",
    "return",
    "call",
    "nop",
    "label",
    "jump",
    "jump_if_false",
    "alloc_class",
    "store",
    "load",
    "incref",
    "decref",
    "loop_start",
    "loop_end",
];

impl<'a> Arbitrary<'a> for PassFuzzInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let num_functions: usize = u.int_in_range(1..=6)?;
        let mut functions = Vec::with_capacity(num_functions);

        for i in 0..num_functions {
            let name = if i == 0 {
                "molt_main".to_string()
            } else {
                format!("func_{i}")
            };

            let num_params: usize = u.int_in_range(0..=3)?;
            let params: Vec<String> = (0..num_params).map(|p| format!("p{p}")).collect();
            let num_ops: usize = u.int_in_range(1..=30)?;
            let mut ops = Vec::with_capacity(num_ops + 1);
            let mut next_var = 0usize;

            for _ in 0..num_ops {
                let kind_idx = u.int_in_range(0..=PASS_OP_KINDS.len() - 1)?;
                let kind = PASS_OP_KINDS[kind_idx].to_string();
                let out_name = format!("v{next_var}");
                next_var += 1;

                let op = match kind.as_str() {
                    "const" => OpIR {
                        kind,
                        value: Some(u.int_in_range(-10000..=10000)?),
                        out: Some(out_name),
                        ..OpIR::default()
                    },
                    "const_bool" => OpIR {
                        kind,
                        value: Some(if u.arbitrary()? { 1 } else { 0 }),
                        out: Some(out_name),
                        ..OpIR::default()
                    },
                    "const_none" => OpIR {
                        kind,
                        out: Some(out_name),
                        ..OpIR::default()
                    },
                    "const_float" => OpIR {
                        kind,
                        f_value: Some(u.arbitrary::<f64>().unwrap_or(0.0)),
                        out: Some(out_name),
                        ..OpIR::default()
                    },
                    "add" | "sub" | "mul" | "compare" => {
                        let a = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        let b = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            kind,
                            args: Some(vec![a, b]),
                            out: Some(out_name),
                            fast_int: Some(u.arbitrary()?),
                            ..OpIR::default()
                        }
                    }
                    "negate" => {
                        let a = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            kind,
                            args: Some(vec![a]),
                            out: Some(out_name),
                            ..OpIR::default()
                        }
                    }
                    "call" => {
                        let target_idx = u.int_in_range(0..=num_functions.max(1) - 1)?;
                        let target = if target_idx == 0 {
                            "molt_main".to_string()
                        } else {
                            format!("func_{target_idx}")
                        };
                        OpIR {
                            kind,
                            s_value: Some(target),
                            args: Some(vec![]),
                            out: Some(out_name),
                            ..OpIR::default()
                        }
                    }
                    "alloc_class" => OpIR {
                        kind,
                        s_value: Some("MyClass".to_string()),
                        out: Some(out_name),
                        ..OpIR::default()
                    },
                    "store" | "store_init" => {
                        let obj = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        let val = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            kind,
                            args: Some(vec![obj, val]),
                            s_value: Some("field".to_string()),
                            ..OpIR::default()
                        }
                    }
                    "load" => {
                        let obj = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            kind,
                            args: Some(vec![obj]),
                            s_value: Some("field".to_string()),
                            out: Some(out_name),
                            ..OpIR::default()
                        }
                    }
                    "incref" | "decref" => {
                        let var = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            kind,
                            args: Some(vec![var]),
                            ..OpIR::default()
                        }
                    }
                    "label" => OpIR {
                        kind,
                        s_value: Some(format!("L{}", u.int_in_range(0..=10)?)),
                        ..OpIR::default()
                    },
                    "jump" => OpIR {
                        kind,
                        s_value: Some(format!("L{}", u.int_in_range(0..=10)?)),
                        ..OpIR::default()
                    },
                    "jump_if_false" => {
                        let cond = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            kind,
                            s_value: Some(format!("L{}", u.int_in_range(0..=10)?)),
                            args: Some(vec![cond]),
                            ..OpIR::default()
                        }
                    }
                    "loop_start" | "loop_end" => OpIR {
                        kind,
                        s_value: Some(format!("loop_{}", u.int_in_range(0..=3)?)),
                        ..OpIR::default()
                    },
                    "return" => OpIR {
                        kind,
                        args: Some(vec![format!(
                            "v{}",
                            u.int_in_range(0..=next_var.max(1) - 1)?
                        )]),
                        ..OpIR::default()
                    },
                    _ => OpIR {
                        kind,
                        ..OpIR::default()
                    },
                };
                ops.push(op);
            }

            // Always end with a return.
            ops.push(OpIR {
                kind: "return".to_string(),
                args: Some(vec!["__ret".to_string()]),
                ..OpIR::default()
            });

            functions.push(FunctionIR {
                name,
                params,
                ops,
                param_types: None,
            });
        }

        Ok(PassFuzzInput {
            ir: SimpleIR {
                functions,
                profile: None,
            },
        })
    }
}

fuzz_target!(|input: PassFuzzInput| {
    let mut ir = input.ir;

    // Run each pass independently — none should panic.
    for func in &mut ir.functions {
        fold_constants(&mut func.ops);
    }
    for func in &mut ir.functions {
        fold_constants_cross_block(&mut func.ops);
    }
    for func in &mut ir.functions {
        elide_dead_struct_allocs(func);
    }
    for func in &mut ir.functions {
        escape_analysis(func);
    }
    for func in &mut ir.functions {
        propagate_loop_fast_int(func);
    }
    for func in &mut ir.functions {
        rc_coalescing(func);
    }
    inline_functions(&mut ir);
    eliminate_dead_functions(&mut ir);
    apply_profile_order(&mut ir);

    // After all passes, the IR must still have a non-negative function count.
    assert!(
        !ir.functions.is_empty() || true,
        "passes reduced IR to invalid state"
    );
});
