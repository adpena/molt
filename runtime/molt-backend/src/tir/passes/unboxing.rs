//! Unboxing pass: eliminates redundant Box/Unbox pairs.
//!
//! When a value is boxed (`BoxVal`) and ALL consumers unbox it back to the
//! same type (`UnboxVal`), both operations are unnecessary — the original
//! unboxed value can be used directly.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::PassStats;

/// Run the unboxing elimination pass on `func`.
///
/// Algorithm:
/// 1. Build a use-map: for each ValueId, collect all (BlockId, op_index) that use it
/// 2. For each `BoxVal` op, check if ALL uses of its result are `UnboxVal` ops
///    that unbox back to the same type as the original value
/// 3. If so, replace all uses of each UnboxVal result with the original pre-box value,
///    and mark both BoxVal and UnboxVal ops for removal
/// 4. Remove marked ops and return stats
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "unboxing",
        ..Default::default()
    };

    // Step 1: Build use-map. For each ValueId, collect the set of
    // (block_id_index, op_index) pairs where it appears as an operand.
    // We use block id's u32 as key for the outer map to avoid BlockId hashing issues.
    // Actually, we just need: for each ValueId, which ops use it?
    // We'll store (block_id: u32, op_index: usize).
    let block_ids: Vec<u32> = func.blocks.keys().map(|b| b.0).collect();

    // use_map: ValueId -> Vec<(block_u32, op_index)>
    let mut use_map: HashMap<ValueId, Vec<(u32, usize)>> = HashMap::new();

    for &bid_u32 in &block_ids {
        let bid = crate::tir::blocks::BlockId(bid_u32);
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            for operand in &op.operands {
                use_map.entry(*operand).or_default().push((bid_u32, op_idx));
            }
        }
        // Also check terminator uses — these are non-op uses that prevent elimination.
        for v in terminator_values(&block.terminator) {
            use_map.entry(v).or_default().push((bid_u32, usize::MAX));
        }
    }

    // Step 2: Find BoxVal ops and check if all uses are UnboxVal.
    // Collect replacement info: (unbox_result_id -> original_pre_box_id)
    // and ops to remove: (block_u32, op_index).
    let mut replacements: HashMap<ValueId, ValueId> = HashMap::new();
    let mut ops_to_remove: HashSet<(u32, usize)> = HashSet::new();

    for &bid_u32 in &block_ids {
        let bid = crate::tir::blocks::BlockId(bid_u32);
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::BoxVal {
                continue;
            }
            // BoxVal has one operand (value to box) and one result (boxed value).
            if op.operands.len() != 1 || op.results.len() != 1 {
                continue;
            }
            let pre_box_value = op.operands[0];
            let boxed_value = op.results[0];

            // Find all uses of the boxed value.
            let uses = match use_map.get(&boxed_value) {
                Some(u) => u,
                None => {
                    // No uses at all — the BoxVal is dead code. Remove it.
                    ops_to_remove.insert((bid_u32, op_idx));
                    stats.ops_removed += 1;
                    continue;
                }
            };

            // Check that ALL uses are UnboxVal ops (not terminator uses).
            let mut all_unbox = true;
            let mut unbox_ops: Vec<(u32, usize)> = Vec::new();

            for &(use_bid, use_op_idx) in uses {
                if use_op_idx == usize::MAX {
                    // Used in a terminator — can't eliminate.
                    all_unbox = false;
                    break;
                }
                let use_block = &func.blocks[&crate::tir::blocks::BlockId(use_bid)];
                let use_op = &use_block.ops[use_op_idx];
                if use_op.opcode != OpCode::UnboxVal {
                    all_unbox = false;
                    break;
                }
                if use_op.operands.len() != 1 || use_op.results.len() != 1 {
                    all_unbox = false;
                    break;
                }
                unbox_ops.push((use_bid, use_op_idx));
            }

            if !all_unbox || unbox_ops.is_empty() {
                continue;
            }

            // All uses are UnboxVal. Record replacements and mark for removal.
            ops_to_remove.insert((bid_u32, op_idx));
            stats.ops_removed += 1;

            for (ub_bid, ub_idx) in &unbox_ops {
                let ub_block = &func.blocks[&crate::tir::blocks::BlockId(*ub_bid)];
                let ub_op = &ub_block.ops[*ub_idx];
                let unbox_result = ub_op.results[0];
                replacements.insert(unbox_result, pre_box_value);
                ops_to_remove.insert((*ub_bid, *ub_idx));
                stats.ops_removed += 1;
                stats.values_changed += 1;
            }
        }
    }

    if replacements.is_empty() && ops_to_remove.is_empty() {
        return stats;
    }

    // Step 3: Apply replacements — rewrite all operands and terminator args.
    // Resolve transitive replacements (for nested box/unbox).
    let replacements = resolve_transitive(&replacements);

    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            for operand in &mut op.operands {
                if let Some(&replacement) = replacements.get(operand) {
                    *operand = replacement;
                }
            }
        }
        replace_in_terminator(&mut block.terminator, &replacements);
    }

    // Step 4: Remove marked ops (iterate in reverse to preserve indices).
    for block in func.blocks.values_mut() {
        let bid_u32 = block.id.0;
        // Collect indices to remove for this block, sorted descending.
        let mut indices: Vec<usize> = ops_to_remove
            .iter()
            .filter(|(b, _)| *b == bid_u32)
            .map(|(_, idx)| *idx)
            .collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in indices {
            block.ops.remove(idx);
        }
    }

    stats
}

/// Resolve transitive replacement chains: if A -> B and B -> C, then A -> C.
fn resolve_transitive(replacements: &HashMap<ValueId, ValueId>) -> HashMap<ValueId, ValueId> {
    let mut resolved = HashMap::with_capacity(replacements.len());
    for (&from, &to) in replacements {
        let mut current = to;
        let mut seen = HashSet::new();
        seen.insert(from);
        while let Some(&next) = replacements.get(&current) {
            if !seen.insert(current) {
                break; // cycle guard
            }
            current = next;
        }
        resolved.insert(from, current);
    }
    resolved
}

/// Collect all ValueIds used in a terminator.
fn terminator_values(term: &Terminator) -> Vec<ValueId> {
    match term {
        Terminator::Branch { args, .. } => args.clone(),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            let mut v = vec![*cond];
            v.extend_from_slice(then_args);
            v.extend_from_slice(else_args);
            v
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            let mut v = vec![*value];
            for (_, _, args) in cases {
                v.extend_from_slice(args);
            }
            v.extend_from_slice(default_args);
            v
        }
        Terminator::Return { values } => values.clone(),
        Terminator::Unreachable => vec![],
    }
}

/// Replace ValueIds in a terminator according to the replacement map.
fn replace_in_terminator(term: &mut Terminator, replacements: &HashMap<ValueId, ValueId>) {
    let replace = |v: &mut ValueId| {
        if let Some(&r) = replacements.get(v) {
            *v = r;
        }
    };

    match term {
        Terminator::Branch { args, .. } => {
            for a in args {
                replace(a);
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            replace(cond);
            for a in then_args {
                replace(a);
            }
            for a in else_args {
                replace(a);
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            replace(value);
            for (_, _, args) in cases {
                for a in args {
                    replace(a);
                }
            }
            for a in default_args {
                replace(a);
            }
        }
        Terminator::Return { values } => {
            for v in values {
                replace(v);
            }
        }
        Terminator::Unreachable => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use crate::tir::verify::verify_function;

    /// Helper: create a BoxVal op.
    fn box_op(operand: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BoxVal,
            operands: vec![operand],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Helper: create an UnboxVal op.
    fn unbox_op(operand: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::UnboxVal,
            operands: vec![operand],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Helper: create a ConstInt op.
    fn const_int_op(result: ValueId, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    /// Helper: create an Add op.
    fn add_op(lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    // Test 1: Simple pair elimination — Box followed by single Unbox, both removed.
    #[test]
    fn simple_box_unbox_pair_eliminated() {
        // func @test() -> i64
        //   %0 = const_int 42
        //   %1 = box %0          <- should be removed
        //   %2 = unbox %1        <- should be removed, %2 replaced by %0
        //   return %2
        let mut func = TirFunction::new("test".into(), vec![], TirType::I64);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_op(v0, 42));
        entry.ops.push(box_op(v0, v1));
        entry.ops.push(unbox_op(v1, v2));
        entry.terminator = Terminator::Return { values: vec![v2] };

        // Verify valid before pass.
        assert!(
            verify_function(&func).is_ok(),
            "pre-pass verification failed"
        );

        let stats = run(&mut func);

        // Both box and unbox should be removed.
        assert_eq!(stats.ops_removed, 2, "expected 2 ops removed");
        assert_eq!(stats.values_changed, 1, "expected 1 value changed");

        // The entry block should have only the const_int op left.
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 1, "expected 1 op remaining");
        assert_eq!(entry.ops[0].opcode, OpCode::ConstInt);

        // The return should now use v0 directly.
        if let Terminator::Return { values } = &entry.terminator {
            assert_eq!(values, &[v0], "return should use original value");
        } else {
            panic!("expected Return terminator");
        }

        // Verify valid after pass.
        assert!(
            verify_function(&func).is_ok(),
            "post-pass verification failed: {:?}",
            verify_function(&func).err()
        );
    }

    // Test 2: Multiple consumers all unbox — Box with 3 Unbox consumers, all removed.
    #[test]
    fn multiple_unbox_consumers_all_eliminated() {
        // func @test() -> i64
        //   %0 = const_int 10
        //   %1 = box %0
        //   %2 = unbox %1
        //   %3 = unbox %1
        //   %4 = unbox %1
        //   %5 = add %2, %3
        //   %6 = add %5, %4
        //   return %6
        let mut func = TirFunction::new("test".into(), vec![], TirType::I64);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;
        let v3 = ValueId(func.next_value);
        func.next_value += 1;
        let v4 = ValueId(func.next_value);
        func.next_value += 1;
        let v5 = ValueId(func.next_value);
        func.next_value += 1;
        let v6 = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_op(v0, 10));
        entry.ops.push(box_op(v0, v1));
        entry.ops.push(unbox_op(v1, v2));
        entry.ops.push(unbox_op(v1, v3));
        entry.ops.push(unbox_op(v1, v4));
        entry.ops.push(add_op(v2, v3, v5));
        entry.ops.push(add_op(v5, v4, v6));
        entry.terminator = Terminator::Return { values: vec![v6] };

        assert!(
            verify_function(&func).is_ok(),
            "pre-pass verification failed"
        );

        let stats = run(&mut func);

        // 1 BoxVal + 3 UnboxVal = 4 ops removed.
        assert_eq!(stats.ops_removed, 4, "expected 4 ops removed");
        assert_eq!(stats.values_changed, 3, "expected 3 values changed");

        // Remaining: const_int, add, add = 3 ops.
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 3, "expected 3 ops remaining");

        // The add ops should now use v0 instead of v2/v3/v4.
        let add1 = &entry.ops[1];
        assert_eq!(add1.opcode, OpCode::Add);
        assert_eq!(add1.operands, vec![v0, v0], "add should use original value");

        assert!(
            verify_function(&func).is_ok(),
            "post-pass verification failed: {:?}",
            verify_function(&func).err()
        );
    }

    // Test 3: Mixed consumers — Box with one Unbox and one non-Unbox use, NOT removed.
    #[test]
    fn mixed_consumers_not_eliminated() {
        // func @test() -> DynBox
        //   %0 = const_int 10
        //   %1 = box %0
        //   %2 = unbox %1       <- can't remove because %1 also used by add
        //   %3 = add %1, %1     <- uses boxed value directly
        //   return %3
        let mut func = TirFunction::new("test".into(), vec![], TirType::DynBox);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;
        let v3 = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_op(v0, 10));
        entry.ops.push(box_op(v0, v1));
        entry.ops.push(unbox_op(v1, v2));
        entry.ops.push(add_op(v1, v1, v3));
        entry.terminator = Terminator::Return { values: vec![v3] };

        assert!(
            verify_function(&func).is_ok(),
            "pre-pass verification failed"
        );

        let stats = run(&mut func);

        // Nothing should be removed — mixed consumers.
        assert_eq!(stats.ops_removed, 0, "expected 0 ops removed");
        assert_eq!(stats.values_changed, 0, "expected 0 values changed");

        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 4, "all ops should remain");

        assert!(
            verify_function(&func).is_ok(),
            "post-pass verification failed"
        );
    }

    // Test 4: No BoxVal ops — function without boxing, no changes.
    #[test]
    fn no_box_ops_no_changes() {
        // func @add(i64, i64) -> i64
        //   %2 = add %0, %1
        //   return %2
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(add_op(ValueId(0), ValueId(1), v2));
        entry.terminator = Terminator::Return { values: vec![v2] };

        assert!(
            verify_function(&func).is_ok(),
            "pre-pass verification failed"
        );

        let stats = run(&mut func);

        assert_eq!(stats.ops_removed, 0);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.name, "unboxing");

        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 1);

        assert!(
            verify_function(&func).is_ok(),
            "post-pass verification failed"
        );
    }

    // Test 5: Nested Box/Unbox — Box(Unbox(Box(x))), only inner pair eliminated.
    #[test]
    fn nested_box_unbox_inner_pair_eliminated() {
        // func @test() -> DynBox
        //   %0 = const_int 5
        //   %1 = box %0          <- inner box
        //   %2 = unbox %1        <- inner unbox (only use of %1) -> pair eliminated
        //   %3 = box %2          <- outer box
        //   return %3            <- %3 used in terminator, outer box NOT eliminated
        //
        // After pass:
        //   %0 = const_int 5
        //   %3 = box %0          <- operand rewritten from %2 to %0
        //   return %3
        let mut func = TirFunction::new("test".into(), vec![], TirType::DynBox);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;
        let v3 = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_op(v0, 5));
        entry.ops.push(box_op(v0, v1)); // inner box
        entry.ops.push(unbox_op(v1, v2)); // inner unbox
        entry.ops.push(box_op(v2, v3)); // outer box
        entry.terminator = Terminator::Return { values: vec![v3] };

        assert!(
            verify_function(&func).is_ok(),
            "pre-pass verification failed"
        );

        let stats = run(&mut func);

        // Inner pair (box %0 -> %1, unbox %1 -> %2) should be eliminated.
        // Outer box stays because its result is used in the terminator.
        assert_eq!(stats.ops_removed, 2, "expected inner pair removed");

        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 2, "expected const_int + outer box");
        assert_eq!(entry.ops[0].opcode, OpCode::ConstInt);
        assert_eq!(entry.ops[1].opcode, OpCode::BoxVal);
        // Outer box should now take v0 directly (since v2 was replaced by v0).
        assert_eq!(
            entry.ops[1].operands,
            vec![v0],
            "outer box should use original value"
        );

        assert!(
            verify_function(&func).is_ok(),
            "post-pass verification failed: {:?}",
            verify_function(&func).err()
        );
    }
}
