//! Loop Type Narrowing pass for TIR.
//!
//! When a loop body contains arithmetic ops tagged with `_fast_int` attributes
//! (from the frontend's type hint propagation), this pass refines their result
//! types from DynBox to Box(I64). This information feeds into the downstream
//! unboxing pass and the lower_to_simple back-conversion, which emits fast_int
//! flags that the Cranelift/LLVM backends use for native integer arithmetic.
//!
//! This is a lightweight type refinement focused on loops — the hot paths where
//! NaN-boxing overhead matters most.

use std::collections::HashSet;

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};

use super::PassStats;

/// Run the loop type narrowing pass.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "loop_narrow",
        ..Default::default()
    };

    // Find loop headers
    let loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter(|(_, role)| matches!(role, LoopRole::LoopHeader))
        .map(|(bid, _)| *bid)
        .collect();

    if loop_headers.is_empty() {
        return stats;
    }

    // For each loop, find _fast_int ops and mark their results as narrowed
    for &header in &loop_headers {
        let loop_blocks = collect_loop_body(func, header);

        for &bid in &loop_blocks {
            let Some(block) = func.blocks.get_mut(&bid) else {
                continue;
            };
            for op in &mut block.ops {
                let is_fast_int = op
                    .attrs
                    .get("_fast_int")
                    .map(|v| matches!(v, AttrValue::Bool(true)))
                    .unwrap_or(false);

                if !is_fast_int {
                    continue;
                }

                // Only process arithmetic and comparison ops
                if !matches!(
                    op.opcode,
                    OpCode::Add
                        | OpCode::Sub
                        | OpCode::Mul
                        | OpCode::InplaceAdd
                        | OpCode::InplaceSub
                        | OpCode::InplaceMul
                        | OpCode::Lt
                        | OpCode::Le
                        | OpCode::Gt
                        | OpCode::Ge
                        | OpCode::Eq
                        | OpCode::Ne
                ) {
                    continue;
                }

                // Mark this op as type-narrowed so downstream passes know
                // the result is a NaN-boxed int (not arbitrary DynBox).
                // This enables the unboxing pass to eliminate Box/Unbox pairs.
                if !op.attrs.contains_key("_narrowed_int") {
                    op.attrs
                        .insert("_narrowed_int".into(), AttrValue::Bool(true));
                    stats.values_changed += 1;
                }
            }
        }
    }

    stats
}

/// Collect blocks belonging to a loop body rooted at `header`.
fn collect_loop_body(func: &TirFunction, header: BlockId) -> Vec<BlockId> {
    let mut body = vec![header];
    let mut visited = HashSet::new();
    visited.insert(header);

    // Walk successors from the header, collecting blocks that are
    // reachable and have higher BlockIds (forward in the CFG).
    let mut worklist = vec![header];
    while let Some(bid) = worklist.pop() {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };

        let successors: Vec<BlockId> = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            _ => vec![],
        };

        for succ in successors {
            if succ == header {
                // Back-edge to header — this confirms it's a loop
                continue;
            }
            if succ.0 < header.0 {
                // Backward to before the loop — skip
                continue;
            }
            if visited.insert(succ) {
                body.push(succ);
                worklist.push(succ);
            }
        }
    }

    body
}
