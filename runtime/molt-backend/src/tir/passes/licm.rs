//! Loop Invariant Code Motion (LICM) for TIR.
//!
//! Hoists operations out of loop bodies when their operands are all
//! defined outside the loop (loop-invariant).  This eliminates redundant
//! recomputation of values that don't change across iterations.
//!
//! ```python
//! for i in range(n):
//!     x = a + b          # invariant: a, b don't change in the loop
//!     result += x * i
//! ```
//!
//! After LICM:
//! ```python
//! x = a + b              # hoisted to preheader
//! for i in range(n):
//!     result += x * i
//! ```
//!
//! Safety conditions:
//! 1. The op must be pure (no side effects).
//! 2. All operands must be defined outside the loop (or be other invariants).
//! 3. The op must dominate all loop exits (guaranteed by hoisting to preheader).
//! 4. Exception-handling regions are conservatively excluded.
//!
//! Reference: Muchnick, "Advanced Compiler Design and Implementation" ch. 13.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

/// Returns `true` if the opcode is pure and safe to hoist out of a loop.
fn is_hoistable(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Add
            | OpCode::Sub
            | OpCode::Mul
            | OpCode::Neg
            | OpCode::Pos
            | OpCode::Eq
            | OpCode::Ne
            | OpCode::Lt
            | OpCode::Le
            | OpCode::Gt
            | OpCode::Ge
            | OpCode::Is
            | OpCode::IsNot
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::BitXor
            | OpCode::BitNot
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::And
            | OpCode::Or
            | OpCode::Not
            | OpCode::Bool
            | OpCode::ConstInt
            | OpCode::ConstFloat
            | OpCode::ConstStr
            | OpCode::ConstBool
            | OpCode::ConstNone
            | OpCode::ConstBytes
            | OpCode::Copy
            | OpCode::BoxVal
            | OpCode::UnboxVal
            | OpCode::TypeGuard
            | OpCode::BuildSlice
    )
}

/// Identify all blocks that belong to a loop whose header is `header_bid`.
fn collect_loop_blocks(func: &TirFunction, header_bid: BlockId) -> HashSet<BlockId> {
    let mut loop_blocks = HashSet::new();
    loop_blocks.insert(header_bid);

    // Build predecessor map.
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (&bid, block) in &func.blocks {
        let succs = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch {
                cases,
                default: default_bid,
                ..
            } => {
                let mut s: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
                s.push(*default_bid);
                s
            }
            _ => vec![],
        };
        for s in succs {
            preds.entry(s).or_default().push(bid);
        }
    }

    // Find back-edge sources: predecessors of the header that are
    // reachable from the header (i.e., there's a forward path from
    // header → ... → predecessor). These form back edges.
    // First, compute the set of blocks reachable from the header
    // (excluding the header itself to avoid trivially including
    // the entry edge).
    let mut reachable_from_header: HashSet<BlockId> = HashSet::new();
    {
        let mut stack = Vec::new();
        if let Some(header_block) = func.blocks.get(&header_bid) {
            let succs = match &header_block.terminator {
                Terminator::Branch { target, .. } => vec![*target],
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => vec![*then_block, *else_block],
                _ => vec![],
            };
            for s in succs {
                if s != header_bid {
                    stack.push(s);
                }
            }
        }
        while let Some(bid) = stack.pop() {
            if !reachable_from_header.insert(bid) {
                continue;
            }
            if let Some(block) = func.blocks.get(&bid) {
                let succs = match &block.terminator {
                    Terminator::Branch { target, .. } => vec![*target],
                    Terminator::CondBranch {
                        then_block,
                        else_block,
                        ..
                    } => vec![*then_block, *else_block],
                    _ => vec![],
                };
                for s in succs {
                    if s != header_bid && !reachable_from_header.contains(&s) {
                        stack.push(s);
                    }
                }
            }
        }
    }

    // Now find back edges: predecessors of header that are reachable from it.
    let mut worklist: Vec<BlockId> = Vec::new();
    if let Some(header_preds) = preds.get(&header_bid) {
        for &p in header_preds {
            if reachable_from_header.contains(&p) && !loop_blocks.contains(&p) {
                loop_blocks.insert(p);
                worklist.push(p);
            }
        }
    }

    while let Some(bid) = worklist.pop() {
        if let Some(block_preds) = preds.get(&bid) {
            for &p in block_preds {
                if !loop_blocks.contains(&p) {
                    loop_blocks.insert(p);
                    worklist.push(p);
                }
            }
        }
    }

    loop_blocks
}

/// Find the preheader block for a loop header.
/// The preheader is the unique predecessor of the header that is NOT
/// part of the loop body.  If no unique preheader exists, returns None.
fn find_preheader(
    func: &TirFunction,
    header_bid: BlockId,
    loop_blocks: &HashSet<BlockId>,
) -> Option<BlockId> {
    let mut preds: Vec<BlockId> = Vec::new();
    for (&bid, block) in &func.blocks {
        let targets = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            _ => vec![],
        };
        if targets.contains(&header_bid) && !loop_blocks.contains(&bid) {
            preds.push(bid);
        }
    }
    if preds.len() == 1 {
        Some(preds[0])
    } else {
        None
    }
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "licm",
        ..Default::default()
    };

    // Skip functions with exception handling (conservative).
    if func.has_exception_handling {
        return stats;
    }

    // Find loop headers from loop_roles metadata.
    let loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| {
            if matches!(role, LoopRole::LoopHeader) {
                Some(*bid)
            } else {
                None
            }
        })
        .collect();

    if loop_headers.is_empty() {
        return stats;
    }

    // Build a set of all values defined in each block for quick lookup.
    let mut value_def_block: HashMap<ValueId, BlockId> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            value_def_block.insert(arg.id, bid);
        }
        for op in &block.ops {
            for &res in &op.results {
                value_def_block.insert(res, bid);
            }
        }
    }
    // Function parameters are defined "before" the entry block.
    for i in 0..func.param_types.len() {
        let v = ValueId(i as u32);
        value_def_block.entry(v).or_insert(func.entry_block);
    }

    // Collect all values used as block arguments in terminators.
    // These participate in phi resolution and must not be hoisted.
    let mut phi_values: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        match &block.terminator {
            Terminator::Branch { args, .. } => {
                for v in args {
                    phi_values.insert(*v);
                }
            }
            Terminator::CondBranch {
                then_args,
                else_args,
                ..
            } => {
                for v in then_args.iter().chain(else_args.iter()) {
                    phi_values.insert(*v);
                }
            }
            _ => {}
        }
    }

    for header_bid in &loop_headers {
        let loop_blocks = collect_loop_blocks(func, *header_bid);

        let preheader = match find_preheader(func, *header_bid, &loop_blocks) {
            Some(p) => p,
            None => continue, // No unique preheader — can't hoist.
        };

        // Collect invariant ops: ops in loop blocks whose operands are
        // all defined outside the loop.
        // Iterate to fixpoint: hoisting one op may make another invariant.
        for _round in 0..10 {
            let mut hoisted_this_round = 0usize;

            for &loop_bid in &loop_blocks {
                let block = match func.blocks.get(&loop_bid) {
                    Some(b) => b,
                    None => continue,
                };

                let mut to_hoist: Vec<usize> = Vec::new();

                for (i, op) in block.ops.iter().enumerate() {
                    if !is_hoistable(op.opcode) {
                        continue;
                    }
                    if op.results.is_empty() {
                        continue;
                    }

                    // Skip ops whose results participate in phi nodes.
                    let result_is_phi = op.results.iter().any(|r| phi_values.contains(r));
                    if result_is_phi {
                        continue;
                    }

                    // Check if all operands are defined outside the loop.
                    let all_invariant = op.operands.iter().all(|&operand| {
                        match value_def_block.get(&operand) {
                            Some(def_bid) => !loop_blocks.contains(def_bid),
                            None => true, // Unknown def = conservative: treat as external.
                        }
                    });

                    if all_invariant {
                        to_hoist.push(i);
                    }
                }

                // Hoist ops from back to front to preserve indices.
                for &idx in to_hoist.iter().rev() {
                    let op = func.blocks.get_mut(&loop_bid).unwrap().ops.remove(idx);
                    // Update def_block for the hoisted values.
                    for &res in &op.results {
                        value_def_block.insert(res, preheader);
                    }
                    // Insert at the end of the preheader (before the terminator,
                    // which is handled structurally, not in the ops vec).
                    func.blocks.get_mut(&preheader).unwrap().ops.push(op);
                    hoisted_this_round += 1;
                    stats.ops_removed += 1; // removed from loop
                    stats.ops_added += 1; // added to preheader
                }
            }

            if hoisted_this_round == 0 {
                break;
            }
        }
    }

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

    fn make_const_int(value: i64, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(value));
                m
            },
            source_span: None,
        }
    }

    fn make_binop(opcode: OpCode, lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Build:  entry → preheader → loop_header → loop_body → loop_header
    ///                                         ↘ exit
    /// with a+b computed inside the loop body.
    #[test]
    fn invariant_add_hoisted_to_preheader() {
        let mut func =
            TirFunction::new("f".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let a = ValueId(0); // param
        let b = ValueId(1); // param

        let preheader = func.fresh_block();
        let loop_header = func.fresh_block();
        let loop_body = func.fresh_block();
        let exit = func.fresh_block();

        let loop_var = func.fresh_value();
        let sum_ab = func.fresh_value(); // a + b — loop invariant
        let result = func.fresh_value();
        let cond = func.fresh_value();

        // Entry → preheader
        {
            let init = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(0, init));
            entry.terminator = Terminator::Branch {
                target: preheader,
                args: vec![],
            };
        }

        // Preheader → loop_header
        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![],
                },
            },
        );

        // Loop header: CondBranch → body or exit
        func.blocks.insert(
            loop_header,
            TirBlock {
                id: loop_header,
                args: vec![TirValue {
                    id: loop_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: loop_body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // Loop body: compute a+b (invariant!), use it locally, and loop back.
        // sum_ab is NOT passed as a branch arg (phi value), so it can be hoisted.
        let use_val = func.fresh_value();
        func.blocks.insert(
            loop_body,
            TirBlock {
                id: loop_body,
                args: vec![],
                ops: vec![
                    make_binop(OpCode::Add, a, b, sum_ab), // invariant — should be hoisted
                    make_binop(OpCode::Add, sum_ab, loop_var, use_val), // uses sum_ab locally
                ],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![use_val],
                },
            },
        );

        // Exit
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: result,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![result],
                },
            },
        );

        // Mark loop_header as a loop header.
        func.loop_roles
            .insert(loop_header, LoopRole::LoopHeader);

        let stats = run(&mut func);

        // The invariant Add(a, b) → sum_ab should have been hoisted.
        // The non-invariant Add(sum_ab, loop_var) → use_val should remain.
        let body_ops = &func.blocks[&loop_body].ops;
        let invariant_add_remains = body_ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![a, b]);
        assert!(
            !invariant_add_remains,
            "Invariant Add(a, b) should have been hoisted out of the loop body"
        );

        let preheader_ops = &func.blocks[&preheader].ops;
        assert!(
            preheader_ops.iter().any(|op| op.opcode == OpCode::Add),
            "Add should appear in the preheader"
        );

        assert!(stats.ops_removed > 0 || stats.ops_added > 0);
    }

    #[test]
    fn non_invariant_not_hoisted() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let a = ValueId(0);

        let preheader = func.fresh_block();
        let loop_header = func.fresh_block();
        let loop_body = func.fresh_block();
        let exit = func.fresh_block();

        let loop_var = func.fresh_value();
        let sum = func.fresh_value(); // a + loop_var — NOT invariant
        let cond = func.fresh_value();
        let result = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: preheader,
                args: vec![],
            };
        }

        func.blocks.insert(
            preheader,
            TirBlock {
                id: preheader,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            loop_header,
            TirBlock {
                id: loop_header,
                args: vec![TirValue {
                    id: loop_var,
                    ty: TirType::I64,
                }],
                ops: vec![make_const_int(1, cond)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: loop_body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );

        // a + loop_var: loop_var is defined inside the loop → NOT invariant.
        func.blocks.insert(
            loop_body,
            TirBlock {
                id: loop_body,
                args: vec![],
                ops: vec![make_binop(OpCode::Add, a, loop_var, sum)],
                terminator: Terminator::Branch {
                    target: loop_header,
                    args: vec![sum],
                },
            },
        );

        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![TirValue {
                    id: result,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![result],
                },
            },
        );

        func.loop_roles
            .insert(loop_header, LoopRole::LoopHeader);

        let _stats = run(&mut func);

        // The Add uses loop_var (defined in loop header) — should NOT be hoisted.
        let body_ops = &func.blocks[&loop_body].ops;
        assert!(
            body_ops.iter().any(|op| op.opcode == OpCode::Add),
            "Add should remain in the loop body (not invariant)"
        );
    }
}
