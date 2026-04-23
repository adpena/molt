//! Branchless Boolean Counting Pass.
//!
//! Detects the pattern:
//!
//! ```text
//! block A:
//!   ... CondBranch(cond, then_blk, else_blk)
//!
//! then_blk:
//!   %new = InplaceAdd(%counter, const 1)  // or Add
//!   Branch(merge_blk, [%new])
//!
//! else_blk:
//!   Branch(merge_blk, [%counter])          // no ops, just forward
//! ```
//!
//! And rewrites it to:
//!
//! ```text
//! block A:
//!   %bool_int = CastInt(%cond)             // Bool -> I64 (0 or 1)
//!   %new = Add(%counter, %bool_int)
//!   Branch(merge_blk, [%new])
//! ```
//!
//! This eliminates the branch entirely. The CPU executes a single `iadd`
//! instruction instead of a conditional branch + merge, avoiding pipeline
//! stalls and enabling loop vectorization in downstream backends.
//!
//! The pass only fires when:
//! - The condition value is Bool-typed (guaranteed 0 or 1).
//! - The then-block contains exactly one arithmetic op (Add/InplaceAdd with
//!   a constant 1 operand).
//! - The else-block is empty (no ops, just a branch forwarding the original
//!   counter to the merge block).
//! - Both branches target the same merge block via a single block argument.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Run the branchless boolean counting pass on `func`.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "branchless_count",
        ..Default::default()
    };

    // Phase 1: Build type map from block args and constant ops.
    let mut type_map: HashMap<ValueId, TirType> = HashMap::new();
    let mut const_map: HashMap<ValueId, i64> = HashMap::new();

    for block in func.blocks.values() {
        for arg in &block.args {
            type_map.insert(arg.id, arg.ty.clone());
        }
        for op in &block.ops {
            match op.opcode {
                OpCode::ConstInt => {
                    if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                        for &res in &op.results {
                            const_map.insert(res, *v);
                            type_map.insert(res, TirType::I64);
                        }
                    }
                }
                OpCode::ConstBool => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::Bool);
                    }
                }
                // Bool op (is_truthy) always produces Bool.
                OpCode::Bool => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::Bool);
                    }
                }
                // Comparison ops produce Bool.
                OpCode::Eq
                | OpCode::Ne
                | OpCode::Lt
                | OpCode::Le
                | OpCode::Gt
                | OpCode::Ge
                | OpCode::Is
                | OpCode::IsNot
                | OpCode::In
                | OpCode::NotIn
                | OpCode::Not => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::Bool);
                    }
                }
                OpCode::ConstFloat => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::F64);
                    }
                }
                _ => {}
            }
        }
    }

    // Phase 2: Find and collect candidate transformations.
    // We collect them first to avoid borrow issues during mutation.
    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    let mut rewrites: Vec<Rewrite> = Vec::new();

    for &bid in &block_ids {
        let block = &func.blocks[&bid];

        // Match: CondBranch(cond, then_blk, then_args=[], else_blk, else_args=[])
        let (cond, then_blk, else_blk) = match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } if then_args.is_empty() && else_args.is_empty() => (*cond, *then_block, *else_block),
            _ => continue,
        };

        // Condition must be Bool-typed.
        if !matches!(type_map.get(&cond), Some(TirType::Bool)) {
            continue;
        }

        // Analyze then_blk: must have exactly one Add/InplaceAdd op with const 1,
        // followed by Branch to merge_blk.
        let Some(then_block) = func.blocks.get(&then_blk) else {
            continue;
        };
        if then_block.ops.len() != 1 {
            continue;
        }
        let then_op = &then_block.ops[0];
        if !matches!(then_op.opcode, OpCode::Add | OpCode::InplaceAdd) {
            continue;
        }
        if then_op.operands.len() != 2 || then_op.results.len() != 1 {
            continue;
        }

        // One operand must be const 1.
        let (counter_val, _const_one_val) =
            if const_map.get(&then_op.operands[1]) == Some(&1) {
                (then_op.operands[0], then_op.operands[1])
            } else if const_map.get(&then_op.operands[0]) == Some(&1) {
                (then_op.operands[1], then_op.operands[0])
            } else {
                continue;
            };

        let incremented_val = then_op.results[0];

        // then_blk must branch to merge_blk passing the incremented value.
        let (merge_blk, then_merge_args) = match &then_block.terminator {
            Terminator::Branch { target, args } => (*target, args.clone()),
            _ => continue,
        };
        if then_merge_args.len() != 1 || then_merge_args[0] != incremented_val {
            continue;
        }

        // else_blk must be empty (no ops) and branch to the same merge_blk
        // passing the original counter value.
        let Some(else_block) = func.blocks.get(&else_blk) else {
            continue;
        };
        if !else_block.ops.is_empty() {
            continue;
        }
        let (else_target, else_merge_args) = match &else_block.terminator {
            Terminator::Branch { target, args } => (*target, args.clone()),
            _ => continue,
        };
        if else_target != merge_blk {
            continue;
        }
        if else_merge_args.len() != 1 || else_merge_args[0] != counter_val {
            continue;
        }

        // Merge block must have exactly one block argument (the phi).
        let Some(merge_block) = func.blocks.get(&merge_blk) else {
            continue;
        };
        if merge_block.args.len() != 1 {
            continue;
        }

        // All conditions met. Record the rewrite.
        rewrites.push(Rewrite {
            cond_block: bid,
            then_block_id: then_blk,
            else_block_id: else_blk,
            merge_block_id: merge_blk,
            cond_val: cond,
            counter_val,
            incremented_val,
        });
    }

    // Phase 3: Apply rewrites.
    for rw in rewrites {
        // Allocate fresh ValueIds for the cast and the add result.
        let new_counter = func.fresh_value();

        // Insert two ops at the end of the cond_block:
        //   %bool_as_int = Bool(%cond)  -- cast bool to int (0 or 1)
        //   %new_counter = Add(%counter, %bool_as_int)
        //
        // We use ConstInt 0/1 semantics: Bool in TIR is already 0 or 1 at the
        // machine level. We emit a BoxVal (Bool -> I64 widening) that downstream
        // lowering recognizes as a zero-cost reinterpret.
        //
        // Actually, the cleanest approach: emit an Add where one operand is the
        // Bool cond directly. The type_refine pass and backend both handle
        // Bool-as-integer in arithmetic context. But to be precise, we cast
        // via a dedicated op. Since TIR doesn't have a CastBoolToInt op, we
        // use the existing Bool opcode which is identity on Bool values, then
        // rely on the fact that Add(I64, Bool) is valid in TIR (Bool is numeric).
        //
        // Simplest correct approach: emit Add(counter, cond) directly.
        // Bool is numeric (TirType::is_numeric() returns true for Bool), and
        // the backend treats Bool as 0/1 in arithmetic context. No cast needed.

        let add_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![rw.counter_val, rw.cond_val],
            results: vec![new_counter],
            attrs: AttrDict::new(),
            source_span: None,
        };

        let cond_block = func.blocks.get_mut(&rw.cond_block).unwrap();
        cond_block.ops.push(add_op);

        // Replace the CondBranch with a direct Branch to merge_blk.
        cond_block.terminator = Terminator::Branch {
            target: rw.merge_block_id,
            args: vec![new_counter],
        };

        // Remove the now-dead then and else blocks (unless they have other
        // predecessors, which they won't in this diamond pattern).
        func.blocks.remove(&rw.then_block_id);
        // Only remove else_block if it's distinct from merge and has no other use.
        // In the pattern we matched, else_block is a dedicated forwarding block.
        if rw.else_block_id != rw.merge_block_id {
            func.blocks.remove(&rw.else_block_id);
        }

        stats.values_changed += 1;
        stats.ops_removed += 1; // removed the InplaceAdd/Add in then_block
        stats.ops_added += 1; // added the Add in cond_block
    }

    stats
}

/// A recorded rewrite candidate.
struct Rewrite {
    cond_block: BlockId,
    then_block_id: BlockId,
    else_block_id: BlockId,
    merge_block_id: BlockId,
    cond_val: ValueId,
    counter_val: ValueId,
    #[allow(dead_code)]
    incremented_val: ValueId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{TirBlock, Terminator};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    /// Build a function that models:
    ///   count = 0
    ///   if cond: count += 1
    ///   return count
    ///
    /// TIR blocks:
    ///   bb0(cond: Bool):
    ///     %1 = ConstInt(0)        // counter
    ///     %2 = ConstInt(1)        // increment
    ///     CondBranch(%0, bb1, [], bb2, [])
    ///   bb1:                       // then
    ///     %3 = Add(%1, %2)
    ///     Branch(bb3, [%3])
    ///   bb2:                       // else
    ///     Branch(bb3, [%1])
    ///   bb3(%4: I64):              // merge
    ///     Return(%4)
    fn make_bool_counting_func() -> TirFunction {
        let mut func = TirFunction::new("test_count".into(), vec![TirType::Bool], TirType::I64);
        // Entry block param: %0 = Bool
        // func.next_value == 1 after constructor

        let const_zero_id = ValueId(1);
        let const_one_id = ValueId(2);
        let add_result_id = ValueId(3);
        let merge_arg_id = ValueId(4);
        func.next_value = 5;

        let then_id = func.fresh_block(); // bb1
        let else_id = func.fresh_block(); // bb2
        let merge_id = func.fresh_block(); // bb3

        // bb0: entry
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![const_zero_id],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(0));
                        m
                    },
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![const_one_id],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(1));
                        m
                    },
                    source_span: None,
                },
            ];
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0), // Bool param
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            };
        }

        // bb1: then block - Add(counter, 1) -> Branch(merge)
        func.blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Add,
                    operands: vec![const_zero_id, const_one_id],
                    results: vec![add_result_id],
                    attrs: AttrDict::new(),
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: merge_id,
                    args: vec![add_result_id],
                },
            },
        );

        // bb2: else block - Branch(merge, [counter])
        func.blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: merge_id,
                    args: vec![const_zero_id],
                },
            },
        );

        // bb3: merge block
        func.blocks.insert(
            merge_id,
            TirBlock {
                id: merge_id,
                args: vec![TirValue {
                    id: merge_arg_id,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![merge_arg_id],
                },
            },
        );

        func
    }

    #[test]
    fn branchless_count_fuses_bool_increment() {
        let mut func = make_bool_counting_func();
        assert_eq!(func.blocks.len(), 4); // bb0, bb1, bb2, bb3

        let stats = run(&mut func);

        // Should have fused the pattern.
        assert_eq!(stats.values_changed, 1, "should report one value changed");

        // then and else blocks should be removed.
        assert_eq!(func.blocks.len(), 2, "should have bb0 and bb3 only");

        // Entry block should now have: ConstInt(0), ConstInt(1), Add(counter, cond)
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 3, "entry should have 3 ops");
        assert_eq!(entry.ops[2].opcode, OpCode::Add);
        // The Add operands should be: counter (%1) and cond (%0)
        assert_eq!(entry.ops[2].operands[0], ValueId(1)); // counter (const 0)
        assert_eq!(entry.ops[2].operands[1], ValueId(0)); // cond (Bool param)

        // Terminator should be Branch to merge.
        assert!(matches!(entry.terminator, Terminator::Branch { .. }));
    }

    #[test]
    fn branchless_count_skips_non_bool_cond() {
        let mut func = make_bool_counting_func();
        // Change the param type from Bool to I64.
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.args[0].ty = TirType::I64;
        func.param_types = vec![TirType::I64];

        let stats = run(&mut func);

        // Should NOT fuse — cond is I64, not Bool.
        assert_eq!(stats.values_changed, 0);
        assert_eq!(func.blocks.len(), 4);
    }

    #[test]
    fn branchless_count_skips_multi_op_then_block() {
        let mut func = make_bool_counting_func();
        // Add a second op to the then block.
        let then_id = BlockId(1);
        let then_block = func.blocks.get_mut(&then_id).unwrap();
        then_block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(99)],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(42));
                m
            },
            source_span: None,
        });

        let stats = run(&mut func);

        // Should NOT fuse — then block has 2 ops.
        assert_eq!(stats.values_changed, 0);
        assert_eq!(func.blocks.len(), 4);
    }

    #[test]
    fn branchless_count_skips_non_unit_increment() {
        let mut func = make_bool_counting_func();
        // Change the ConstInt(1) to ConstInt(2).
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        if let Some(AttrValue::Int(v)) = entry.ops[1].attrs.get_mut("value") {
            *v = 2;
        }

        let stats = run(&mut func);

        // Should NOT fuse — increment is 2, not 1.
        assert_eq!(stats.values_changed, 0);
        assert_eq!(func.blocks.len(), 4);
    }

    #[test]
    fn branchless_count_handles_inplace_add() {
        let mut func = make_bool_counting_func();
        // Change the Add to InplaceAdd in the then block.
        let then_id = BlockId(1);
        let then_block = func.blocks.get_mut(&then_id).unwrap();
        then_block.ops[0].opcode = OpCode::InplaceAdd;

        let stats = run(&mut func);

        // Should still fuse — InplaceAdd is recognized.
        assert_eq!(stats.values_changed, 1);
        assert_eq!(func.blocks.len(), 2);
    }

    #[test]
    fn branchless_count_works_with_comparison_cond() {
        // Test that comparison results (which are Bool) also trigger fusion.
        let mut func = TirFunction::new(
            "test_cmp_count".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        // %0 = param a (I64), %1 = param b (I64)
        let cmp_result = ValueId(2);
        let counter_val = ValueId(3);
        let const_one = ValueId(4);
        let add_result = ValueId(5);
        let merge_arg = ValueId(6);
        func.next_value = 7;

        let then_id = func.fresh_block();
        let else_id = func.fresh_block();
        let merge_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                // %2 = Lt(%0, %1) -> Bool
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Lt,
                    operands: vec![ValueId(0), ValueId(1)],
                    results: vec![cmp_result],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
                // %3 = ConstInt(0)
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![counter_val],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(0));
                        m
                    },
                    source_span: None,
                },
                // %4 = ConstInt(1)
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![const_one],
                    attrs: {
                        let mut m = AttrDict::new();
                        m.insert("value".into(), AttrValue::Int(1));
                        m
                    },
                    source_span: None,
                },
            ];
            entry.terminator = Terminator::CondBranch {
                cond: cmp_result,
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            };
        }

        func.blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Add,
                    operands: vec![counter_val, const_one],
                    results: vec![add_result],
                    attrs: AttrDict::new(),
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: merge_id,
                    args: vec![add_result],
                },
            },
        );

        func.blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: merge_id,
                    args: vec![counter_val],
                },
            },
        );

        func.blocks.insert(
            merge_id,
            TirBlock {
                id: merge_id,
                args: vec![TirValue {
                    id: merge_arg,
                    ty: TirType::I64,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![merge_arg],
                },
            },
        );

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 1, "comparison-cond pattern should fuse");
        assert_eq!(func.blocks.len(), 2);
    }
}
