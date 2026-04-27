//! Copy Propagation Pass for TIR.
//!
//! Eliminates redundant `Copy` operations by replacing uses of the copy
//! result with the original source value.  This is a critical deforestation
//! enabler: frontend desugaring, TIR lifting, and other passes produce
//! chains of `a = Copy(b); c = Copy(a)` that waste register pressure and
//! create unnecessary SSA values.
//!
//! After copy propagation, `c` directly references `b`, and the intermediate
//! `a` becomes dead (cleaned up by DCE).
//!
//! Safety conditions:
//! 1. The Copy must have exactly one operand and one result.
//! 2. The Copy must not have special attributes (e.g., `_original_kind`,
//!    `fused`) that carry semantic meaning beyond the value copy.
//! 3. Block arguments are never replaced (they represent phi merges).

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

/// Returns `true` if a Copy op is a pure value copy with no semantic
/// attributes that must be preserved (fused tags, original_kind, etc.).
#[inline]
fn is_pure_copy(attrs: &crate::tir::ops::AttrDict) -> bool {
    attrs.is_empty()
}

/// Build a copy chain map: for every pure Copy(src) → dst, map dst → src.
/// Transitively resolves chains: if a → b → c, then a → c.
fn build_copy_map(func: &crate::tir::function::TirFunction) -> HashMap<ValueId, ValueId> {
    let mut copy_of: HashMap<ValueId, ValueId> = HashMap::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Copy
                && op.operands.len() == 1
                && op.results.len() == 1
                && is_pure_copy(&op.attrs)
            {
                copy_of.insert(op.results[0], op.operands[0]);
            }
        }
    }

    // Resolve transitive chains: if dst → mid → src, flatten to dst → src.
    // Iterate until fixpoint (max 20 rounds to avoid pathological cases).
    for _ in 0..20 {
        let mut changed = false;
        let keys: Vec<ValueId> = copy_of.keys().copied().collect();
        for k in keys {
            let v = copy_of[&k];
            if let Some(&deeper) = copy_of.get(&v)
                && deeper != v {
                    copy_of.insert(k, deeper);
                    changed = true;
                }
        }
        if !changed {
            break;
        }
    }

    copy_of
}

/// Resolve a value through the copy map to its root source.
#[inline]
fn resolve(val: ValueId, copy_map: &HashMap<ValueId, ValueId>) -> ValueId {
    copy_map.get(&val).copied().unwrap_or(val)
}

/// Replace all uses of copy results with their root source values.
///
/// Iterates to fixpoint: after replacement, some Copy ops may become
/// identity copies (src == dst) which are cleaned up by DCE.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "copy_prop",
        ..Default::default()
    };

    let copy_map = build_copy_map(func);
    if copy_map.is_empty() {
        return stats;
    }

    // Replace all operand references in ops and terminators.
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            for operand in &mut op.operands {
                let resolved = resolve(*operand, &copy_map);
                if resolved != *operand {
                    *operand = resolved;
                    stats.values_changed += 1;
                }
            }
        }

        // Replace in terminators.
        match &mut block.terminator {
            Terminator::Branch { args, .. } => {
                for arg in args.iter_mut() {
                    let resolved = resolve(*arg, &copy_map);
                    if resolved != *arg {
                        *arg = resolved;
                        stats.values_changed += 1;
                    }
                }
            }
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                let resolved_cond = resolve(*cond, &copy_map);
                if resolved_cond != *cond {
                    *cond = resolved_cond;
                    stats.values_changed += 1;
                }
                for arg in then_args.iter_mut() {
                    let resolved = resolve(*arg, &copy_map);
                    if resolved != *arg {
                        *arg = resolved;
                        stats.values_changed += 1;
                    }
                }
                for arg in else_args.iter_mut() {
                    let resolved = resolve(*arg, &copy_map);
                    if resolved != *arg {
                        *arg = resolved;
                        stats.values_changed += 1;
                    }
                }
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                let resolved_val = resolve(*value, &copy_map);
                if resolved_val != *value {
                    *value = resolved_val;
                    stats.values_changed += 1;
                }
                for (_, _, args) in cases.iter_mut() {
                    for arg in args.iter_mut() {
                        let resolved = resolve(*arg, &copy_map);
                        if resolved != *arg {
                            *arg = resolved;
                            stats.values_changed += 1;
                        }
                    }
                }
                for arg in default_args.iter_mut() {
                    let resolved = resolve(*arg, &copy_map);
                    if resolved != *arg {
                        *arg = resolved;
                        stats.values_changed += 1;
                    }
                }
            }
            Terminator::Return { values } => {
                for val in values.iter_mut() {
                    let resolved = resolve(*val, &copy_map);
                    if resolved != *val {
                        *val = resolved;
                        stats.values_changed += 1;
                    }
                }
            }
            Terminator::Unreachable => {}
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
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

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

    // -----------------------------------------------------------------------
    // Test 1: simple copy chain a→b→c collapses to a→c
    // -----------------------------------------------------------------------
    #[test]
    fn simple_copy_chain() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let a = func.fresh_value();
        let b = func.fresh_value();
        let c = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![a]));
        entry.ops.push(make_op(OpCode::Copy, vec![a], vec![b]));
        entry.ops.push(make_op(OpCode::Copy, vec![b], vec![c]));
        entry.terminator = Terminator::Return { values: vec![c] };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        // The return should reference `a` directly, not `c`.
        match &func.blocks[&func.entry_block].terminator {
            Terminator::Return { values } => assert_eq!(values[0], a),
            _ => panic!("expected Return"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: copy with attrs (fused, _original_kind) is NOT propagated
    // -----------------------------------------------------------------------
    #[test]
    fn attributed_copy_not_propagated() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let a = func.fresh_value();
        let b = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![a]));
        // Copy with a fused attribute — must NOT be propagated.
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![a],
            results: vec![b],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("fused".into(), AttrValue::Str("sum".into()));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![b] };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);

        // The return should still reference `b`, not `a`.
        match &func.blocks[&func.entry_block].terminator {
            Terminator::Return { values } => assert_eq!(values[0], b),
            _ => panic!("expected Return"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: operand replacement in non-Copy ops
    // -----------------------------------------------------------------------
    #[test]
    fn operand_replacement() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let param = ValueId(0);
        let copy_val = func.fresh_value();
        let add_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Copy, vec![param], vec![copy_val]));
        entry.ops.push(make_op(
            OpCode::Add,
            vec![copy_val, copy_val],
            vec![add_result],
        ));
        entry.terminator = Terminator::Return {
            values: vec![add_result],
        };

        let stats = run(&mut func);
        // Both operands of Add should now reference `param`.
        assert!(stats.values_changed >= 2);

        let add_op = &func.blocks[&func.entry_block].ops[1];
        assert_eq!(add_op.operands[0], param);
        assert_eq!(add_op.operands[1], param);
    }

    // -----------------------------------------------------------------------
    // Test 4: no copies → no changes
    // -----------------------------------------------------------------------
    #[test]
    fn no_copies_no_changes() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::I64);
        let a = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(OpCode::ConstInt, vec![], vec![a]));
        entry.terminator = Terminator::Return { values: vec![a] };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
    }
}
