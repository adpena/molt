//! Loop-aware fast-int scan for TIR.
//!
//! The frontend already marks arithmetic ops with `_fast_int` when it can
//! prove they are integer-specializable. Downstream lowering and type
//! extraction consume that attribute today.
//!
//! A future version of this pass can rewrite loop-local values into a richer
//! unboxed form once `TirFunction` grows a stable mutable result-type carrier.
//! The current TIR surface does not persist op-result types in the function
//! itself, so this pass stays conservative and analysis-only: it discovers
//! loop-local fast-int arithmetic candidates without mutating IR state.

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};

use super::PassStats;

/// Run the loop-aware fast-int scan.
pub fn run(func: &mut TirFunction) -> PassStats {
    let stats = PassStats {
        name: "loop_narrow",
        ..Default::default()
    };

    let loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| (*role == LoopRole::LoopHeader).then_some(*bid))
        .collect();

    if loop_headers.is_empty() {
        return stats;
    }

    // Conservatively walk loop bodies so the pass remains integrated and its
    // candidate-discovery logic can evolve without guessing at non-existent
    // mutable type tables on TirFunction.
    for header in loop_headers {
        let loop_blocks = collect_loop_body(func, header);
        for bid in loop_blocks {
            let Some(block) = func.blocks.get(&bid) else {
                continue;
            };
            for op in &block.ops {
                let is_fast_int = op
                    .attrs
                    .get("_fast_int")
                    .is_some_and(|v| matches!(v, AttrValue::Bool(true)));
                if !is_fast_int {
                    continue;
                }
                if matches!(
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
                    // Candidate recognized. No IR mutation yet; the current
                    // pipeline already preserves the `_fast_int` signal and
                    // later stages consume it directly.
                }
            }
        }
    }

    stats
}

/// Collect all blocks that belong to a loop body rooted at `header`.
/// Uses preserved loop metadata and deterministic block ordering.
fn collect_loop_body(func: &TirFunction, header: BlockId) -> Vec<BlockId> {
    let mut ordered_blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    ordered_blocks.sort_by_key(|bid| bid.0);

    let mut body = vec![header];
    for bid in ordered_blocks {
        if bid == header || bid.0 <= header.0 {
            continue;
        }

        let role = func.loop_roles.get(&bid).cloned().unwrap_or(LoopRole::None);
        if role == LoopRole::LoopHeader {
            break;
        }

        body.push(bid);

        if let Some(block) = func.blocks.get(&bid) {
            let branches_to_header = match &block.terminator {
                Terminator::Branch { target, .. } => *target == header,
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => *then_block == header || *else_block == header,
                _ => false,
            };
            if branches_to_header {
                break;
            }
        }
    }

    body
}
