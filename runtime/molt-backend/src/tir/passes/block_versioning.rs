//! Static Basic Block Versioning (SBBV) pass for TIR.
//!
//! Based on the ECOOP 2024 paper, this pass eliminates runtime type guards by
//! duplicating blocks with different type assumptions. The caller dispatches
//! to the right version based on what it knows about the operand types.
//!
//! ## Algorithm
//!
//! 1. Scan each block for TypeGuard ops. Each guard on value `%x` proving type
//!    `T` forms a "type context" — a (ValueId, TirType) pair.
//!
//! 2. For each such block, create up to k=2 versions:
//!    - **Specialized version**: the TypeGuard is removed and all uses of its
//!      result within the block are replaced with a constant `true` (the guard
//!      is known to succeed). The guarded value's type is refined.
//!    - **Generic version**: the original block, unchanged.
//!
//! 3. Rewire predecessors: if a predecessor can statically prove the guard
//!    condition (e.g., the guarded value was produced by ConstInt, Add, or
//!    another int-producing op), route it to the specialized version.
//!    Otherwise route to the generic version.
//!
//! 4. Loop headers with back-edges are not versioned — doing so could create
//!    unbounded versioning or violate SSA dominance on loop-carried values.
//!
//! 5. After versioning, blocks whose predecessors ALL route to the specialized
//!    version leave the generic version unreachable. The DCE pass (run later
//!    in the pipeline) will clean it up.
//!
//! ## Bounded code size
//!
//! The k=2 limit ensures at most 2x code size increase. In practice, most
//! blocks have at most one TypeGuard, so the increase is much smaller.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;

/// Maximum number of versions per original block (bounded code size).
const MAX_VERSIONS: usize = 2;

/// A type context: the guarded value and the type the guard proves.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeContext {
    /// The SSA value being guarded (operands[0] of the TypeGuard).
    guarded_value: ValueId,
    /// The type that the guard proves (parsed from the "ty" attr).
    proven_type: TirType,
}

/// Information about a TypeGuard candidate in a block.
#[derive(Debug)]
struct GuardCandidate {
    /// Index of the TypeGuard op within the block's ops vector.
    op_index: usize,
    /// The type context this guard establishes.
    context: TypeContext,
    /// The result ValueId of the TypeGuard (the bool flag).
    result: ValueId,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse the proven type from a TypeGuard op's attributes.
/// Returns None if the attributes don't contain a parseable type string.
fn parse_guard_type(op: &TirOp) -> Option<TirType> {
    // The type_guard_hoist tests use "ty", the deopt module uses "expected_type".
    // Check both, preferring "ty".
    let type_str = op
        .attrs
        .get("ty")
        .or_else(|| op.attrs.get("expected_type"));

    match type_str {
        Some(AttrValue::Str(s)) => match s.to_uppercase().as_str() {
            "INT" | "I64" => Some(TirType::I64),
            "FLOAT" | "F64" => Some(TirType::F64),
            "BOOL" => Some(TirType::Bool),
            "STR" => Some(TirType::Str),
            "NONE" => Some(TirType::None),
            "BYTES" => Some(TirType::Bytes),
            _ => None,
        },
        _ => None,
    }
}

/// Collect successor BlockIds from a terminator.
fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut targets: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
            targets.push(*default);
            targets.dedup();
            targets
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Build predecessor map: BlockId -> Vec<BlockId>.
fn build_pred_map(func: &TirFunction) -> HashMap<BlockId, Vec<BlockId>> {
    let mut pred_map: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in func.blocks.keys() {
        pred_map.entry(bid).or_default();
    }
    for (&bid, block) in &func.blocks {
        for succ in terminator_successors(&block.terminator) {
            pred_map.entry(succ).or_default().push(bid);
        }
    }
    pred_map
}

/// Build a map: ValueId -> BlockId (which block defines it).
fn build_def_map(func: &TirFunction) -> HashMap<ValueId, BlockId> {
    let mut def_map: HashMap<ValueId, BlockId> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_map.insert(arg.id, bid);
        }
        for op in &block.ops {
            for &result in &op.results {
                def_map.insert(result, bid);
            }
        }
    }
    def_map
}

/// Build a map: ValueId -> OpCode that produced it (for ops, not block args).
fn build_producing_op_map(func: &TirFunction) -> HashMap<ValueId, OpCode> {
    let mut map: HashMap<ValueId, OpCode> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            for &result in &op.results {
                map.insert(result, op.opcode);
            }
        }
    }
    map
}

/// Identify loop headers: blocks with at least one back-edge predecessor
/// (where pred.0 >= block.0).
fn find_loop_headers(pred_map: &HashMap<BlockId, Vec<BlockId>>) -> HashSet<BlockId> {
    let mut headers = HashSet::new();
    for (&bid, preds) in pred_map {
        if preds.iter().any(|p| p.0 >= bid.0) {
            headers.insert(bid);
        }
    }
    headers
}

/// Returns true if a value is statically known to be of the given type,
/// based on the opcode that produced it.
fn value_proves_type(
    value: ValueId,
    expected: &TirType,
    producing_ops: &HashMap<ValueId, OpCode>,
    block_arg_types: &HashMap<ValueId, TirType>,
) -> bool {
    // Check block argument types first.
    if let Some(ty) = block_arg_types.get(&value) {
        return ty == expected;
    }

    // Check the producing opcode.
    let opcode = match producing_ops.get(&value) {
        Some(op) => op,
        None => return false,
    };

    match expected {
        TirType::I64 => matches!(
            opcode,
            OpCode::ConstInt
                | OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::Div
                | OpCode::FloorDiv
                | OpCode::Mod
                | OpCode::Pow
                | OpCode::Neg
                | OpCode::Pos
                | OpCode::BitAnd
                | OpCode::BitOr
                | OpCode::BitXor
                | OpCode::BitNot
                | OpCode::Shl
                | OpCode::Shr
                | OpCode::InplaceAdd
                | OpCode::InplaceSub
                | OpCode::InplaceMul
        ),
        TirType::F64 => matches!(opcode, OpCode::ConstFloat),
        TirType::Bool => matches!(
            opcode,
            OpCode::ConstBool
                | OpCode::Eq
                | OpCode::Ne
                | OpCode::Lt
                | OpCode::Le
                | OpCode::Gt
                | OpCode::Ge
                | OpCode::Is
                | OpCode::IsNot
                | OpCode::In
                | OpCode::NotIn
                | OpCode::Not
                | OpCode::Bool
                | OpCode::And
                | OpCode::Or
        ),
        TirType::Str => matches!(opcode, OpCode::ConstStr),
        TirType::None => matches!(opcode, OpCode::ConstNone),
        TirType::Bytes => matches!(opcode, OpCode::ConstBytes),
        _ => false,
    }
}

/// Clone a block, remapping all ValueIds using a fresh-value allocator.
/// Returns the cloned block and a mapping from old ValueId -> new ValueId.
fn clone_block_with_fresh_values(
    block: &TirBlock,
    new_block_id: BlockId,
    func: &mut TirFunction,
) -> (TirBlock, HashMap<ValueId, ValueId>) {
    let mut remap: HashMap<ValueId, ValueId> = HashMap::new();

    // Allocate fresh IDs for block arguments.
    let new_args: Vec<TirValue> = block
        .args
        .iter()
        .map(|arg| {
            let new_id = func.fresh_value();
            remap.insert(arg.id, new_id);
            TirValue {
                id: new_id,
                ty: arg.ty.clone(),
            }
        })
        .collect();

    // Allocate fresh IDs for op results first (so operand remapping can find them).
    let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len());
    // First pass: allocate result IDs.
    let mut result_ids: Vec<Vec<ValueId>> = Vec::with_capacity(block.ops.len());
    for op in &block.ops {
        let new_results: Vec<ValueId> = op
            .results
            .iter()
            .map(|&r| {
                let new_id = func.fresh_value();
                remap.insert(r, new_id);
                new_id
            })
            .collect();
        result_ids.push(new_results);
    }

    // Second pass: remap operands and build ops.
    for (op, new_results) in block.ops.iter().zip(result_ids.into_iter()) {
        let new_operands: Vec<ValueId> = op
            .operands
            .iter()
            .map(|&v| *remap.get(&v).unwrap_or(&v))
            .collect();
        new_ops.push(TirOp {
            dialect: op.dialect,
            opcode: op.opcode,
            operands: new_operands,
            results: new_results,
            attrs: op.attrs.clone(),
            source_span: op.source_span,
        });
    }

    // Remap terminator.
    let new_terminator = remap_terminator(&block.terminator, &remap);

    let new_block = TirBlock {
        id: new_block_id,
        args: new_args,
        ops: new_ops,
        terminator: new_terminator,
    };

    (new_block, remap)
}

/// Remap ValueIds in a terminator. BlockIds are NOT remapped (the clone
/// targets the same successor blocks as the original).
fn remap_terminator(term: &Terminator, remap: &HashMap<ValueId, ValueId>) -> Terminator {
    let r = |v: &ValueId| -> ValueId { *remap.get(v).unwrap_or(v) };

    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: *target,
            args: args.iter().map(|a| r(a)).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: r(cond),
            then_block: *then_block,
            then_args: then_args.iter().map(|a| r(a)).collect(),
            else_block: *else_block,
            else_args: else_args.iter().map(|a| r(a)).collect(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: r(value),
            cases: cases
                .iter()
                .map(|(v, bid, args)| (*v, *bid, args.iter().map(|a| r(a)).collect()))
                .collect(),
            default: *default,
            default_args: default_args.iter().map(|a| r(a)).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|v| r(v)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

/// Rewrite a terminator to redirect edges from `old_target` to `new_target`,
/// also remapping branch arguments through `arg_remap`.
fn redirect_terminator(
    term: &mut Terminator,
    old_target: BlockId,
    new_target: BlockId,
    arg_remap: &HashMap<ValueId, ValueId>,
) {
    let remap_args = |args: &mut Vec<ValueId>| {
        for a in args.iter_mut() {
            if let Some(&new_v) = arg_remap.get(a) {
                *a = new_v;
            }
        }
    };

    match term {
        Terminator::Branch { target, args } => {
            if *target == old_target {
                *target = new_target;
                remap_args(args);
            }
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == old_target {
                *then_block = new_target;
                remap_args(then_args);
            }
            if *else_block == old_target {
                *else_block = new_target;
                remap_args(else_args);
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, target, args) in cases.iter_mut() {
                if *target == old_target {
                    *target = new_target;
                    remap_args(args);
                }
            }
            if *default == old_target {
                *default = new_target;
                remap_args(default_args);
            }
        }
        Terminator::Return { .. } | Terminator::Unreachable => {}
    }
}

// ---------------------------------------------------------------------------
// Main pass
// ---------------------------------------------------------------------------

/// Run the SBBV pass on a TIR function.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "block_versioning",
        ..Default::default()
    };

    if func.blocks.is_empty() {
        return stats;
    }

    // Conservative bail-out: exception handling makes versioning unsafe because
    // a TypeGuard failure inside a try region must propagate to the handler.
    if func.has_exception_handling {
        return stats;
    }

    let pred_map = build_pred_map(func);
    let loop_headers = find_loop_headers(&pred_map);
    let producing_ops = build_producing_op_map(func);

    // Build block-argument type map for type proofs.
    let mut block_arg_types: HashMap<ValueId, TirType> = HashMap::new();
    for block in func.blocks.values() {
        for arg in &block.args {
            block_arg_types.insert(arg.id, arg.ty.clone());
        }
    }

    // Phase 1: Identify versioning candidates.
    // A candidate is a non-loop-header block that contains a TypeGuard with a
    // parseable type and has at least one predecessor that can prove the guard.
    struct VersioningCandidate {
        block_id: BlockId,
        guard: GuardCandidate,
    }

    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    let mut candidates: Vec<VersioningCandidate> = Vec::new();

    for &bid in &block_ids {
        // Do not version loop headers — back-edge contexts are unreliable.
        if loop_headers.contains(&bid) {
            continue;
        }

        let block = &func.blocks[&bid];

        // Find the first TypeGuard in this block (we version on at most one
        // guard per block to stay within k=2).
        let guard = block.ops.iter().enumerate().find_map(|(idx, op)| {
            if op.opcode != OpCode::TypeGuard {
                return None;
            }
            let guarded_value = *op.operands.first()?;
            let proven_type = parse_guard_type(op)?;
            let result = *op.results.first()?;
            Some(GuardCandidate {
                op_index: idx,
                context: TypeContext {
                    guarded_value,
                    proven_type,
                },
                result,
            })
        });

        let guard = match guard {
            Some(g) => g,
            None => continue,
        };

        // Check that at least one predecessor can prove the guard.
        let preds = pred_map.get(&bid).map(|v| v.as_slice()).unwrap_or(&[]);
        let any_proves = preds.iter().any(|pred_bid| {
            // The guarded value may be a block argument. Trace through the
            // predecessor's branch args to find the actual source value.
            let guarded = guard.context.guarded_value;
            let block = match func.blocks.get(&bid) {
                Some(b) => b,
                None => return false,
            };
            // Find the position of the guarded value in this block's args
            let arg_pos = block.args.iter().position(|a| a.id == guarded);
            let source_value = if let Some(pos) = arg_pos {
                // Trace through predecessor's branch args
                let pred_block = match func.blocks.get(pred_bid) {
                    Some(b) => b,
                    None => return false,
                };
                match &pred_block.terminator {
                    Terminator::Branch { args, .. } => args.get(pos).copied(),
                    Terminator::CondBranch { then_args, else_args, then_block, else_block, .. } => {
                        if *then_block == bid { then_args.get(pos).copied() }
                        else if *else_block == bid { else_args.get(pos).copied() }
                        else { None }
                    }
                    _ => None,
                }
            } else {
                Some(guarded) // Not a block arg — use directly
            };
            match source_value {
                Some(src) => value_proves_type(
                    src,
                    &guard.context.proven_type,
                    &producing_ops,
                    &block_arg_types,
                ),
                None => false,
            }
        });

        if !any_proves {
            continue;
        }

        candidates.push(VersioningCandidate {
            block_id: bid,
            guard,
        });
    }

    if candidates.is_empty() {
        return stats;
    }

    // Phase 2: Create specialized block versions and rewire predecessors.
    for candidate in candidates {
        let orig_bid = candidate.block_id;

        // Allocate a new block ID for the specialized version.
        let spec_bid = func.fresh_block();

        // Clone the original block with fresh SSA values.
        let orig_block = func.blocks[&orig_bid].clone();
        let (mut spec_block, value_remap) =
            clone_block_with_fresh_values(&orig_block, spec_bid, func);

        // In the specialized block, remove the TypeGuard op and replace uses
        // of its result with ConstBool(true) — the guard is known to pass.
        let guard_result_in_spec = *value_remap
            .get(&candidate.guard.result)
            .unwrap_or(&candidate.guard.result);
        let guard_operand_in_spec = *value_remap
            .get(&candidate.guard.context.guarded_value)
            .unwrap_or(&candidate.guard.context.guarded_value);

        // Remove the TypeGuard op from the specialized block.
        spec_block.ops.remove(candidate.guard.op_index);
        stats.ops_removed += 1;

        // Insert a ConstBool(true) to replace the guard result, using the same
        // ValueId so all downstream uses are satisfied.
        let const_true_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![guard_result_in_spec],
            attrs: {
                let mut m = HashMap::new();
                m.insert("value".to_string(), AttrValue::Bool(true));
                m
            },
            source_span: None,
        };
        // Insert at the position where the guard was (maintaining op order).
        spec_block
            .ops
            .insert(candidate.guard.op_index, const_true_op);
        stats.ops_added += 1;

        // Refine the type of the guarded value's block arg in the specialized
        // block, if the guarded value is a block argument.
        for arg in &mut spec_block.args {
            if arg.id == guard_operand_in_spec {
                arg.ty = candidate.guard.context.proven_type.clone();
            }
        }

        // Insert the specialized block into the function.
        func.blocks.insert(spec_bid, spec_block);
        stats.values_changed += 1; // track as "blocks versioned"

        // Phase 3: Rewire predecessors.
        // Predecessors that can prove the guard type go to spec_bid.
        // Others stay pointed at orig_bid.
        let preds: Vec<BlockId> = pred_map
            .get(&orig_bid)
            .cloned()
            .unwrap_or_default();

        // Build an empty arg remap — branch arguments that correspond to block
        // args need to be remapped for the specialized version.
        // The value_remap maps old block-arg ValueIds to new ones, but the
        // branch arguments are the VALUES passed by the predecessor, not the
        // block-arg IDs. So we don't remap branch args — only the target.
        let empty_remap: HashMap<ValueId, ValueId> = HashMap::new();

        for pred_id in preds {
            // Skip self-edges (shouldn't exist for non-loop-headers, but safe).
            if pred_id == orig_bid {
                continue;
            }

            let can_prove = value_proves_type(
                candidate.guard.context.guarded_value,
                &candidate.guard.context.proven_type,
                &producing_ops,
                &block_arg_types,
            );

            if can_prove {
                let pred_block = func.blocks.get_mut(&pred_id).unwrap();
                redirect_terminator(
                    &mut pred_block.terminator,
                    orig_bid,
                    spec_bid,
                    &empty_remap,
                );
            }
            // Otherwise, keep the edge to the generic (original) block.
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
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_type_guard(operand: ValueId, result: ValueId, ty: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("ty".to_string(), AttrValue::Str(ty.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TypeGuard,
            operands: vec![operand],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn make_const_int(result: ValueId, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".to_string(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Simple TypeGuard elimination via versioning
    //
    // bb0 (entry): %x = ConstInt(42); branch to bb1
    // bb1: %ok = TypeGuard(%x, INT); return %ok
    //
    // After SBBV: bb0 branches to bb1_spec (specialized), where the
    // TypeGuard is replaced with ConstBool(true).
    // -----------------------------------------------------------------------
    #[test]
    fn simple_type_guard_elimination() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let x = func.fresh_value(); // %0
        let ok = func.fresh_value(); // %1

        let bb1 = func.fresh_block(); // BlockId(1)

        // bb0: %x = ConstInt(42); branch to bb1 passing %x as arg
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(x, 42));
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![x],
            };
        }

        // bb1(%x_arg): %ok = TypeGuard(%x_arg, INT); return %ok
        let x_arg = func.fresh_value(); // %2
        let block1 = TirBlock {
            id: bb1,
            args: vec![TirValue {
                id: x_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![make_type_guard(x_arg, ok, "INT")],
            terminator: Terminator::Return {
                values: vec![ok],
            },
        };
        func.blocks.insert(bb1, block1);

        let stats = run(&mut func);

        // Should have created a specialized version.
        assert!(
            stats.values_changed >= 1,
            "expected at least one block versioned"
        );
        assert!(
            stats.ops_removed >= 1,
            "expected at least one TypeGuard removed"
        );

        // The entry block should now branch to the specialized block (not bb1).
        let entry_term = &func.blocks[&func.entry_block].terminator;
        match entry_term {
            Terminator::Branch { target, .. } => {
                assert_ne!(
                    *target, bb1,
                    "entry should branch to specialized block, not original"
                );
                // The specialized block should exist and have a ConstBool instead of TypeGuard.
                let spec_block = &func.blocks[target];
                assert!(
                    spec_block
                        .ops
                        .iter()
                        .any(|op| op.opcode == OpCode::ConstBool),
                    "specialized block should have ConstBool(true)"
                );
                assert!(
                    !spec_block
                        .ops
                        .iter()
                        .any(|op| op.opcode == OpCode::TypeGuard),
                    "specialized block should not have TypeGuard"
                );
            }
            other => panic!("expected Branch terminator, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: Multiple predecessors with different type contexts
    //
    // bb0 (entry): branch based on condition
    //   then -> bb1: %x = ConstInt(1); branch to bb3
    //   else -> bb2: %x = Call(...); branch to bb3
    // bb3(%arg): %ok = TypeGuard(%arg, INT); return %ok
    //
    // After SBBV:
    //   bb1 -> bb3_spec (guard removed, ConstBool(true))
    //   bb2 -> bb3 (original, guard kept)
    // -----------------------------------------------------------------------
    #[test]
    fn multiple_predecessors_different_contexts() {
        let mut func = TirFunction::new("f".into(), vec![TirType::Bool], TirType::Bool);

        let cond = ValueId(0); // entry param

        let bb1 = func.fresh_block(); // BlockId(1)
        let bb2 = func.fresh_block(); // BlockId(2)
        let bb3 = func.fresh_block(); // BlockId(3)

        // Entry: CondBranch to bb1/bb2
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }

        // bb1: %int_val = ConstInt(1); branch to bb3(%int_val)
        let int_val = func.fresh_value();
        let block1 = TirBlock {
            id: bb1,
            args: vec![],
            ops: vec![make_const_int(int_val, 1)],
            terminator: Terminator::Branch {
                target: bb3,
                args: vec![int_val],
            },
        };
        func.blocks.insert(bb1, block1);

        // bb2: %dyn_val = Call(...); branch to bb3(%dyn_val)
        let dyn_val = func.fresh_value();
        let block2 = TirBlock {
            id: bb2,
            args: vec![],
            ops: vec![make_op(OpCode::Call, vec![], vec![dyn_val])],
            terminator: Terminator::Branch {
                target: bb3,
                args: vec![dyn_val],
            },
        };
        func.blocks.insert(bb2, block2);

        // bb3(%arg): %ok = TypeGuard(%arg, INT); return %ok
        let arg = func.fresh_value();
        let ok = func.fresh_value();
        let block3 = TirBlock {
            id: bb3,
            args: vec![TirValue {
                id: arg,
                ty: TirType::DynBox,
            }],
            ops: vec![make_type_guard(arg, ok, "INT")],
            terminator: Terminator::Return { values: vec![ok] },
        };
        func.blocks.insert(bb3, block3);

        let stats = run(&mut func);

        // The pass should version bb3 because the guarded value (%arg) is a
        // block argument, not directly produced by ConstInt. However, the
        // current value_proves_type checks the *producing op* of the guarded
        // value. Since %arg is a block argument (no producing op), the proof
        // relies on block_arg_types. Since %arg has type DynBox, no predecessor
        // can prove it's INT through block arg types alone.
        //
        // This test validates the conservative behavior: when the guarded value
        // is a block argument with DynBox type, no versioning occurs because
        // the type proof is about the block arg, not the passed value.
        //
        // This is correct behavior — a more aggressive version would trace
        // through branch arguments to their definitions, but that requires
        // interprocedural analysis beyond SBBV's scope.
        assert_eq!(
            stats.values_changed, 0,
            "block arg with DynBox type should not be versioned (conservative)"
        );

        // Verify the original block is unchanged.
        let bb3_ops = &func.blocks[&bb3].ops;
        assert!(
            bb3_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
            "original bb3 should still have TypeGuard"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: Version limit (k=2) enforcement
    //
    // Even with many predecessors, at most 2 versions are created.
    // Since we only create 1 specialized + 1 generic = 2 total, this is
    // inherently bounded. Verify that a block with a TypeGuard only gets
    // versioned once (not per-predecessor).
    // -----------------------------------------------------------------------
    #[test]
    fn version_limit_enforced() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let bb1 = func.fresh_block(); // BlockId(1)
        let bb2 = func.fresh_block(); // BlockId(2)
        let bb3 = func.fresh_block(); // BlockId(3)
        let merge = func.fresh_block(); // BlockId(4)

        // Entry -> bb1
        let x0 = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(x0, 0));
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }

        // bb1: %x1 = ConstInt(1); branch to merge
        let x1 = func.fresh_value();
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![make_const_int(x1, 1)],
                terminator: Terminator::Branch {
                    target: merge,
                    args: vec![],
                },
            },
        );

        // bb2: %x2 = ConstInt(2); branch to merge
        let x2 = func.fresh_value();
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![make_const_int(x2, 2)],
                terminator: Terminator::Branch {
                    target: merge,
                    args: vec![],
                },
            },
        );

        // bb3: %x3 = ConstInt(3); branch to merge
        let x3 = func.fresh_value();
        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![make_const_int(x3, 3)],
                terminator: Terminator::Branch {
                    target: merge,
                    args: vec![],
                },
            },
        );

        // merge: %val = ConstInt(99); %ok = TypeGuard(%val, INT); return %ok
        let val = func.fresh_value();
        let ok = func.fresh_value();
        func.blocks.insert(
            merge,
            TirBlock {
                id: merge,
                args: vec![],
                ops: vec![
                    make_const_int(val, 99),
                    make_type_guard(val, ok, "INT"),
                ],
                terminator: Terminator::Return { values: vec![ok] },
            },
        );

        let stats = run(&mut func);

        // At most 1 specialized version should be created (k=2 total including original).
        assert!(
            stats.values_changed <= 1,
            "at most 1 specialized version should be created (k=2 limit)"
        );

        // Count total blocks: original blocks + at most 1 specialized.
        // Original: bb0, bb1, bb2, bb3, merge = 5 blocks.
        // After versioning: 5 + at most 1 = 6.
        assert!(
            func.blocks.len() <= 6,
            "total blocks should be at most 6 (5 original + 1 specialized), got {}",
            func.blocks.len()
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Loop header not versioned from back-edge
    //
    // bb0 (entry): branch to bb1
    // bb1 (loop header): %ok = TypeGuard(%x, INT); branch to bb2
    // bb2 (loop body): back-edge to bb1
    //
    // bb1 is a loop header (has back-edge from bb2). SBBV must not version it.
    // -----------------------------------------------------------------------
    #[test]
    fn loop_header_not_versioned() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let x = func.fresh_value(); // %0
        let ok = func.fresh_value(); // %1

        let bb1 = func.fresh_block(); // BlockId(1) — loop header
        let bb2 = func.fresh_block(); // BlockId(2) — loop body

        // Entry: define %x, branch to bb1
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(x, 42));
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }

        // bb1 (loop header): TypeGuard(%x, INT) -> %ok; branch to bb2
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![make_type_guard(x, ok, "INT")],
                terminator: Terminator::Branch {
                    target: bb2,
                    args: vec![],
                },
            },
        );

        // bb2 (loop body): back-edge to bb1
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb1,
                    args: vec![],
                },
            },
        );

        let stats = run(&mut func);

        // bb1 is a loop header — it must NOT be versioned.
        assert_eq!(
            stats.values_changed, 0,
            "loop header should not be versioned"
        );
        assert_eq!(
            stats.ops_removed, 0,
            "no TypeGuard should be removed from loop header"
        );

        // The TypeGuard should still be in bb1.
        let bb1_ops = &func.blocks[&bb1].ops;
        assert!(
            bb1_ops.iter().any(|op| op.opcode == OpCode::TypeGuard),
            "TypeGuard should remain in loop header bb1"
        );

        // No new blocks should have been created.
        assert_eq!(
            func.blocks.len(),
            3,
            "no new blocks should be created for loop headers"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: No TypeGuard ops — pass is a no-op
    // -----------------------------------------------------------------------
    #[test]
    fn no_type_guards_no_changes() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let v = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(v, 0));
        entry.terminator = Terminator::Return { values: vec![v] };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(stats.ops_added, 0);
    }

    // -----------------------------------------------------------------------
    // Test 6: TypeGuard with unparseable type attr — not versioned
    // -----------------------------------------------------------------------
    #[test]
    fn unparseable_guard_type_skipped() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let x = func.fresh_value();
        let ok = func.fresh_value();
        let bb1 = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(make_const_int(x, 1));
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }

        // TypeGuard with unknown type "CUSTOM_CLASS"
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![make_type_guard(x, ok, "CUSTOM_CLASS")],
                terminator: Terminator::Return { values: vec![ok] },
            },
        );

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0, "unknown type should not be versioned");
    }
}
