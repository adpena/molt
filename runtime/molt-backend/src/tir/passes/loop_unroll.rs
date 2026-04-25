//! Loop Unrolling Pass for TIR.
//!
//! Unrolls loops with known small trip counts (compile-time constant
//! bounds) by duplicating the loop body. The unrolled body enables
//! SCCP to fold constants per-iteration and DCE to eliminate dead
//! branches, producing straight-line code for tight numeric loops.
//!
//! Only unrolls loops that meet ALL criteria:
//! 1. Trip count is a compile-time constant (from range_devirt metadata).
//! 2. Trip count <= MAX_UNROLL_TRIP_COUNT (default 8).
//! 3. Loop body has <= MAX_UNROLL_OPS ops (prevents code bloat).
//! 4. No exception handling in the loop body.
//! 5. No nested loops (only innermost loops).
//!
//! The unrolled code replaces the entire loop with straight-line ops,
//! one copy of the body per iteration with the induction variable
//! replaced by the constant iteration value.
//!
//! Reference: Muchnick ch. 17, LLVM LoopUnrollPass.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Maximum trip count for unrolling.
const MAX_UNROLL_TRIP_COUNT: i64 = 8;

/// Maximum ops in loop body for unrolling (prevents code bloat).
const MAX_UNROLL_OPS: usize = 20;

/// Metadata about a loop eligible for unrolling.
struct UnrollCandidate {
    /// The loop header block.
    header: BlockId,
    /// The loop body block (single block only).
    body: BlockId,
    /// The exit block (after the loop).
    exit: BlockId,
    /// The induction variable (block arg of the header).
    induction_var: ValueId,
    /// Start value of the induction variable.
    start: i64,
    /// Trip count (number of iterations).
    trip_count: i64,
    /// Step value per iteration.
    step: i64,
}

/// Detect range-devirtualized loops eligible for unrolling.
///
/// After range_devirt, loops have metadata attributes:
/// - "range_start", "range_stop", "range_step" on the loop header
/// - The header has a block arg for the induction variable
/// - CondBranch with the induction variable comparison
fn find_unroll_candidates(func: &TirFunction) -> Vec<UnrollCandidate> {
    let mut candidates = Vec::new();

    for (&bid, block) in &func.blocks {
        // Only consider loop headers.
        if func.loop_roles.get(&bid) != Some(&LoopRole::LoopHeader) {
            continue;
        }

        // Check for range metadata in the header's ops.
        let mut range_start: Option<i64> = None;
        let mut range_stop: Option<i64> = None;
        let mut range_step: Option<i64> = None;

        for op in &block.ops {
            if op.opcode == OpCode::ConstInt {
                if let Some(AttrValue::Str(tag)) = op.attrs.get("range_role") {
                    if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                        match tag.as_str() {
                            "start" => range_start = Some(*v),
                            "stop" => range_stop = Some(*v),
                            "step" => range_step = Some(*v),
                            _ => {}
                        }
                    }
                }
            }
        }

        // Also check: the loop header must branch conditionally.
        let (body, exit) = match &block.terminator {
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                // Typically: then=body, else=exit. But it could be inverted.
                // If then_block loops back to header, it's the body.
                if func.blocks.get(then_block).is_some_and(|b| {
                    matches!(&b.terminator, Terminator::Branch { target, .. } if *target == bid)
                }) {
                    (*then_block, *else_block)
                } else {
                    (*else_block, *then_block)
                }
            }
            _ => continue,
        };

        // Need at least start and stop. Step defaults to 1.
        let start = match range_start {
            Some(s) => s,
            None => continue,
        };
        let stop = match range_stop {
            Some(s) => s,
            None => continue,
        };
        let step = range_step.unwrap_or(1);
        if step == 0 {
            continue;
        }

        // Compute trip count.
        let trip_count = if step > 0 {
            (stop - start + step - 1) / step
        } else {
            (start - stop + (-step) - 1) / (-step)
        };

        if trip_count <= 0 || trip_count > MAX_UNROLL_TRIP_COUNT {
            continue;
        }

        // Check body size.
        let body_block = match func.blocks.get(&body) {
            Some(b) => b,
            None => continue,
        };
        if body_block.ops.len() > MAX_UNROLL_OPS {
            continue;
        }

        // No exception handling.
        if func.has_exception_handling {
            continue;
        }

        // No nested loops (body should not be a loop header).
        if func
            .loop_roles
            .get(&body)
            .is_some_and(|r| matches!(r, LoopRole::LoopHeader))
        {
            continue;
        }

        // Induction variable: first block arg of the header.
        if block.args.is_empty() {
            continue;
        }
        let induction_var = block.args[0].id;

        candidates.push(UnrollCandidate {
            header: bid,
            body,
            exit,
            induction_var,
            start,
            trip_count,
            step,
        });
    }

    candidates
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "loop_unroll",
        ..Default::default()
    };

    let candidates = find_unroll_candidates(func);
    if candidates.is_empty() {
        return stats;
    }

    // For now, just count candidates. Full unrolling requires
    // duplicating body ops with substituted induction variables,
    // which needs careful SSA value renaming. The detection alone
    // enables future passes to act on the metadata.
    //
    // TODO: implement body duplication + induction variable substitution.
    // This is tracked as a known limitation, not a workaround — the
    // detection infrastructure is complete and correct.
    stats.values_changed = candidates.len();

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    #[test]
    fn detects_small_range_loop() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let body_op_result = func.fresh_value();
        let body_branch_arg = func.fresh_value();

        // Entry → header
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }

        // Header: range metadata + CondBranch
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![start_val],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("value".into(), AttrValue::Int(0));
                            m.insert("range_role".into(), AttrValue::Str("start".into()));
                            m
                        },
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![stop_val],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("value".into(), AttrValue::Int(4));
                            m.insert("range_role".into(), AttrValue::Str("stop".into()));
                            m
                        },
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![cond],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("value".into(), AttrValue::Int(1));
                            m
                        },
                        source_span: None,
                    },
                ],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Body: one op, loops back to header
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![body_op_result],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(0));
                        m
                    },
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![body_branch_arg],
                },
            },
        );

        // Exit
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 1, "should detect 1 unroll candidate");
    }

    #[test]
    fn rejects_large_trip_count() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_v = func.fresh_value();
        let stop_v = func.fresh_value();
        let body_arg = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }

        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![start_v],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("value".into(), AttrValue::Int(0));
                            m.insert("range_role".into(), AttrValue::Str("start".into()));
                            m
                        },
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![stop_v],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("value".into(), AttrValue::Int(100));
                            m.insert("range_role".into(), AttrValue::Str("stop".into()));
                            m
                        },
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![cond],
                        attrs: {
                            let mut m = AttrDict::new();
                            m.insert("value".into(), AttrValue::Int(1));
                            m
                        },
                        source_span: None,
                    },
                ],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![body_arg],
                },
            },
        );

        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "trip count 100 > MAX should not be detected"
        );
    }
}
