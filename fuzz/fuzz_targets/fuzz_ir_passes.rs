//! Fuzz target: exercise IR optimization passes on arbitrary IR.
//!
//! Invariant tested: no pass panics on any structurally valid IR.
//!
//! The target runs the module stage (`apply_profile_order`), then every pass in
//! `SIMPLE_IR_FUNCTION_PASSES` in pipeline order, then dead-op cleanup and dead
//! function elimination. It reads the pass list from the pipeline's own table,
//! so a pass added to or removed from the pipeline is fuzzed without an edit
//! here. It runs every registered pass on every function, not only the passes
//! one target admits: each pass must be robust on any input.

#![no_main]
use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use molt_backend::ir_schema::validate_simple_ir_op_shapes;
use molt_backend::passes::{SIMPLE_IR_FUNCTION_PASSES, apply_profile_order};
use molt_backend::tir::target_info::TargetInfo;
use molt_backend::{
    FunctionIR, OpIR, OpKind, SimpleIR, eliminate_dead_functions, eliminate_dead_ops,
};

/// Generate a structurally valid IR with ops that exercise the passes.
#[derive(Debug)]
struct PassFuzzInput {
    ir: SimpleIR,
}

/// Registered SimpleIR kinds that exercise the passes. A kind or shape that
/// admission rejects fails the target on its first input, not silently.
const PASS_OP_KINDS: &[OpKind] = &[
    OpKind::Const,
    OpKind::ConstBool,
    OpKind::ConstNone,
    OpKind::ConstFloat,
    OpKind::Add,
    OpKind::Sub,
    OpKind::Mul,
    OpKind::Neg,
    OpKind::Lt,
    OpKind::Ret,
    OpKind::Call,
    OpKind::Nop,
    OpKind::Label,
    OpKind::Jump,
    OpKind::BrIf,
    OpKind::AllocClass,
    OpKind::Store,
    OpKind::Load,
    OpKind::IncRef,
    OpKind::DecRef,
    OpKind::LoopStart,
    OpKind::LoopEnd,
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
                let kind = PASS_OP_KINDS[kind_idx];
                let out_name = format!("v{next_var}");
                next_var += 1;

                let op = match kind {
                    OpKind::Const => OpIR {
                        value: Some(u.int_in_range(-10000..=10000)?),
                        out: Some(out_name),
                        ..OpIR::new(kind)
                    },
                    OpKind::ConstBool => OpIR {
                        value: Some(if u.arbitrary()? { 1 } else { 0 }),
                        out: Some(out_name),
                        ..OpIR::new(kind)
                    },
                    OpKind::ConstNone => OpIR {
                        out: Some(out_name),
                        ..OpIR::new(kind)
                    },
                    OpKind::ConstFloat => OpIR {
                        f_value: Some(u.arbitrary::<f64>().unwrap_or(0.0)),
                        out: Some(out_name),
                        ..OpIR::new(kind)
                    },
                    OpKind::Add | OpKind::Sub | OpKind::Mul | OpKind::Lt => {
                        let a = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        let b = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            args: Some(vec![a, b]),
                            out: Some(out_name),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::Neg => {
                        let a = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            args: Some(vec![a]),
                            out: Some(out_name),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::Call => {
                        let target_idx = u.int_in_range(0..=num_functions.max(1) - 1)?;
                        let target = if target_idx == 0 {
                            "molt_main".to_string()
                        } else {
                            format!("func_{target_idx}")
                        };
                        OpIR {
                            s_value: Some(target),
                            args: Some(vec![]),
                            out: Some(out_name),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::AllocClass => OpIR {
                        s_value: Some("MyClass".to_string()),
                        out: Some(out_name),
                        ..OpIR::new(kind)
                    },
                    OpKind::Store => {
                        let obj = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        let val = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            args: Some(vec![obj, val]),
                            s_value: Some("field".to_string()),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::Load => {
                        let obj = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            args: Some(vec![obj]),
                            s_value: Some("field".to_string()),
                            out: Some(out_name),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::IncRef | OpKind::DecRef => {
                        let var = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            args: Some(vec![var]),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::Label | OpKind::Jump => OpIR {
                        value: Some(u.int_in_range(0..=10)?),
                        ..OpIR::new(kind)
                    },
                    OpKind::BrIf => {
                        let cond = format!("v{}", u.int_in_range(0..=next_var.max(1) - 1)?);
                        OpIR {
                            value: Some(u.int_in_range(0..=10)?),
                            args: Some(vec![cond]),
                            ..OpIR::new(kind)
                        }
                    }
                    OpKind::LoopStart | OpKind::LoopEnd => OpIR {
                        s_value: Some(format!("loop_{}", u.int_in_range(0..=3)?)),
                        ..OpIR::new(kind)
                    },
                    OpKind::Ret => OpIR {
                        args: Some(vec![format!(
                            "v{}",
                            u.int_in_range(0..=next_var.max(1) - 1)?
                        )]),
                        ..OpIR::new(kind)
                    },
                    OpKind::Nop => OpIR::new(kind),
                    _ => unreachable!("pass fuzz palette has no shape for {kind}"),
                };
                ops.push(op);
            }

            // Always end with a return.
            ops.push(OpIR {
                args: Some(vec!["__ret".to_string()]),
                ..OpIR::new(OpKind::Ret)
            });

            functions.push(FunctionIR {
                return_abi: molt_backend::ir::FunctionReturnAbi::Value,
                name,
                params,
                ops,
                param_types: None,
                ..FunctionIR::default()
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
    // The passes' precondition is the backends' admission check. The generator
    // emits only admitted shapes, so a rejection is a stale generator.
    if let Err(error) = validate_simple_ir_op_shapes(&ir) {
        panic!("the generator emitted IR that admission rejects: {error}");
    }

    apply_profile_order(&mut ir);
    for function in &mut ir.functions {
        for pass in SIMPLE_IR_FUNCTION_PASSES {
            (pass.run)(function);
        }
    }
    eliminate_dead_ops(&mut ir, &TargetInfo::native_release_fast());
    eliminate_dead_functions(&mut ir);
});
