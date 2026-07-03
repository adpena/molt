//! Sparse Conditional Constant Propagation (SCCP).
//!
//! Propagates constants through the SSA graph, folds constant operations,
//! and eliminates branches with known-constant conditions.
//!
//! This is a simplified single-pass forward scan that folds obvious constants.
//! An iterative fixpoint version can replace it later.

mod eval;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use super::PassStats;
use super::effects;
use super::reachability::metadata_preserving_reachable_blocks;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExceptionRegionNestingRole, SccpConstantSeedRule, opcode_exception_region_nesting_role_table,
    opcode_sccp_constant_seed_rule_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::values::ValueId;

use eval::{evaluate_builtin_call, evaluate_method_call, evaluate_op};

/// A value in the constant-propagation lattice.
#[derive(Debug, Clone, PartialEq)]
enum LatticeValue {
    /// Unknown — may still be constant (not yet visited).
    Top,
    /// Known constant value.
    Constant(ConstVal),
    /// Overdefined — definitely not constant.
    Bottom,
}

/// Concrete constant values carried through the lattice.
///
/// # NaN note
/// The derived `PartialEq` for `Float(f64)` uses `f64::eq`, which returns
/// `false` for NaN == NaN. In practice this only affects programs that fold
/// a constant NaN value -- an extremely rare case -- and the worst outcome is
/// a missed constant-fold (the lattice value stays Bottom rather than being
/// collapsed to a constant NaN). A future improvement would be to implement
/// `PartialEq` manually using `f64::to_bits()` for bit-exact NaN comparison.
#[derive(Debug, Clone, PartialEq)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    None,
    /// Compile-time constant list (all elements are ConstVal).
    /// Capped at MAX_COMPOUND_ELEMENTS to avoid embedding huge data at compile time.
    List(Vec<ConstVal>),
    /// Compile-time constant dict (all keys and values are ConstVal).
    /// Capped at MAX_COMPOUND_ELEMENTS entries.
    Dict(Vec<(ConstVal, ConstVal)>),
    /// Compile-time range(start, stop, step). Not materialized as a list,
    /// but supports len() and iteration count propagation.
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
}

/// Maximum number of elements for compile-time compound value folding.
/// Prevents embedding excessively large data structures in the binary.
const MAX_COMPOUND_ELEMENTS: usize = 1000;

/// Build a set of ValueIds that are results of ops inside try regions.
/// When `has_exception_handling` is true, we must not rewrite these ops
/// to constants because the op's execution may transfer control to a
/// handler, and removing the op would change observable behavior.
///
/// "May throw" is sourced from the single op-kind registry oracle
/// (`effects::op_may_throw`, backed by `op_kinds.toml`) rather than a local
/// hand-list — a duplicate list is exactly the drift that mis-classified
/// `Shl`/`Shr`/`Pow` as non-throwing and let SCCP/DCE drop a dead `1 << -1`.
fn build_try_region_results(func: &TirFunction) -> HashSet<ValueId> {
    let mut result_set = HashSet::new();
    for block in func.blocks.values() {
        let mut try_depth: u32 = 0;
        for op in &block.ops {
            match opcode_exception_region_nesting_role_table(op.opcode) {
                ExceptionRegionNestingRole::Enter => try_depth += 1,
                ExceptionRegionNestingRole::Exit => try_depth = try_depth.saturating_sub(1),
                ExceptionRegionNestingRole::None => {}
            }
            if try_depth > 0 && effects::op_may_throw(op) {
                for &r in &op.results {
                    result_set.insert(r);
                }
            }
        }
    }
    result_set
}

/// Run the SCCP pass on `func`, returning statistics.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "sccp",
        ..Default::default()
    };

    let has_eh = func.has_exception_handling;

    // Phase 1: Build the lattice from all existing ops.
    let mut lattice: HashMap<ValueId, LatticeValue> = HashMap::new();

    // Block arguments are Bottom (parameters / phi-like — not constant).
    for block in func.blocks.values() {
        for arg in &block.args {
            lattice.insert(arg.id, LatticeValue::Bottom);
        }
    }

    // When exception handling is present, mark results of potentially-throwing
    // ops inside try regions as Bottom (unfoldable) so SCCP never rewrites them.
    let try_region_results = if has_eh {
        build_try_region_results(func)
    } else {
        HashSet::new()
    };

    // Collect block ids for deterministic iteration (sorted).
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    // First pass: seed constants from ConstInt/ConstFloat/ConstBool/ConstNone ops,
    // mark everything else as Top initially.
    // Results of potentially-throwing ops inside try regions are forced to Bottom.
    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            for &res in &op.results {
                // Loop-carried values (loop_index_start, loop_index_next, iter_next)
                // must not be folded — they change on each iteration.
                let original_kind = op
                    .attrs
                    .get("_original_kind")
                    .and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .unwrap_or("");
                if matches!(
                    original_kind,
                    "loop_index_start" | "loop_index_next" | "iter_next"
                ) {
                    lattice.insert(res, LatticeValue::Bottom);
                    continue;
                }
                // If this result is inside a try region and may throw, force Bottom.
                if try_region_results.contains(&res) {
                    lattice.insert(res, LatticeValue::Bottom);
                    continue;
                }
                let val =
                    seed_constant_lattice_value(op.opcode, &op.attrs).unwrap_or(LatticeValue::Top);
                lattice.insert(res, val);
            }
        }
    }

    // Phase 2: Forward propagation — try to fold ops with all-constant operands.
    // Iterate until stable (bounded by number of values).
    let mut changed = true;
    while changed {
        changed = false;
        for &bid in &block_ids {
            let block = &func.blocks[&bid];
            for op in &block.ops {
                if op.results.is_empty() {
                    continue;
                }
                // Skip ops that are already resolved as Constant or Bottom.
                let result_id = op.results[0];
                match lattice.get(&result_id) {
                    Some(LatticeValue::Bottom) | Some(LatticeValue::Constant(_)) => continue,
                    _ => {}
                }

                // Gather operand lattice values.
                let operand_vals: Vec<Option<&ConstVal>> = op
                    .operands
                    .iter()
                    .map(|v| match lattice.get(v) {
                        Some(LatticeValue::Constant(c)) => Some(c),
                        _ => None,
                    })
                    .collect();

                // If any operand is Bottom, this result is Bottom.
                let any_bottom = op
                    .operands
                    .iter()
                    .any(|v| matches!(lattice.get(v), Some(LatticeValue::Bottom)));
                if any_bottom {
                    lattice.insert(result_id, LatticeValue::Bottom);
                    changed = true;
                    continue;
                }

                // If any operand is still Top, we can't fold yet.
                if operand_vals.iter().any(|v| v.is_none()) {
                    continue;
                }

                // All operands are Constant — try to evaluate.
                let folded = evaluate_op(op.opcode, &operand_vals)
                    .or_else(|| evaluate_builtin_call(op, &operand_vals))
                    .or_else(|| evaluate_method_call(op, &operand_vals));
                if let Some(result) = folded {
                    lattice.insert(result_id, LatticeValue::Constant(result));
                    changed = true;
                } else {
                    // Can't fold this opcode — mark Bottom.
                    lattice.insert(result_id, LatticeValue::Bottom);
                    changed = true;
                }
            }
        }
    }

    // Phase 3: Rewrite — replace constant-valued ops with ConstXxx ops.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if op.results.is_empty() {
                continue;
            }
            let result_id = op.results[0];
            // Don't rewrite ops that are already constant constructors.
            if opcode_sccp_constant_seed_rule_table(op.opcode) != SccpConstantSeedRule::None {
                continue;
            }
            if let Some(LatticeValue::Constant(cv)) = lattice.get(&result_id) {
                match cv {
                    ConstVal::Int(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("value".into(), AttrValue::Int(*v));
                        op.opcode = OpCode::ConstInt;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Float(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("f_value".into(), AttrValue::Float(*v));
                        op.opcode = OpCode::ConstFloat;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Bool(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("value".into(), AttrValue::Bool(*v));
                        op.opcode = OpCode::ConstBool;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Str(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("s_value".into(), AttrValue::Str(v.clone()));
                        op.opcode = OpCode::ConstStr;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::None => {
                        op.opcode = OpCode::ConstNone;
                        op.operands.clear();
                        op.attrs = AttrDict::new();
                        stats.values_changed += 1;
                    }
                    // Compound types (List, Dict, Range) stay in the lattice for
                    // downstream folding (e.g. len([1,2,3]) → 3) but cannot be
                    // rewritten to a single constant opcode since no ConstList/
                    // ConstDict/ConstRange opcodes exist in TIR.
                    ConstVal::List(_) | ConstVal::Dict(_) | ConstVal::Range { .. } => {}
                }
            }
        }
    }

    // Phase 4: Fold constant conditional branches to unconditional branches.
    // SAFETY: Never fold branches whose targets include a loop header —
    // the loop condition depends on runtime iteration state that SCCP's
    // forward-only lattice cannot model correctly.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        let new_term = match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                // Skip if either branch target is a loop header — folding
                // these would eliminate loop bodies.
                let targets_loop = func
                    .loop_roles
                    .get(then_block)
                    .is_some_and(|r| *r == LoopRole::LoopHeader)
                    || func
                        .loop_roles
                        .get(else_block)
                        .is_some_and(|r| *r == LoopRole::LoopHeader);
                if targets_loop {
                    None
                } else {
                    match lattice.get(cond) {
                        Some(LatticeValue::Constant(ConstVal::Bool(true))) => {
                            Some(Terminator::Branch {
                                target: *then_block,
                                args: then_args.clone(),
                            })
                        }
                        Some(LatticeValue::Constant(ConstVal::Bool(false))) => {
                            Some(Terminator::Branch {
                                target: *else_block,
                                args: else_args.clone(),
                            })
                        }
                        // Python truthiness: nonzero int is truthy
                        Some(LatticeValue::Constant(ConstVal::Int(v))) => {
                            if *v != 0 {
                                Some(Terminator::Branch {
                                    target: *then_block,
                                    args: then_args.clone(),
                                })
                            } else {
                                Some(Terminator::Branch {
                                    target: *else_block,
                                    args: else_args.clone(),
                                })
                            }
                        }
                        Some(LatticeValue::Constant(ConstVal::None)) => Some(Terminator::Branch {
                            target: *else_block,
                            args: else_args.clone(),
                        }),
                        _ => None,
                    }
                } // close else { ... } for targets_loop guard
            }
            _ => None,
        };
        if let Some(term) = new_term {
            block.terminator = term;
            stats.ops_removed += 1; // count branch simplification
        }
    }

    // Phase 5: Eliminate blocks that became unreachable after branch folding.
    // When a CondBranch is folded to a Branch, one successor is no longer
    // reachable from the folded block. If that was the only path to the target,
    // the target and its transitive successors become dead. Leaving dead blocks
    // in the TIR is incorrect because their ops reference values whose
    // definitions may no longer dominate them (the dominance tree changed when
    // the CFG edge was removed). Removing dead blocks prevents downstream
    // verification from reporting false SSA dominance violations.
    if stats.ops_removed > 0 {
        let reachable = metadata_preserving_reachable_blocks(func);
        let dead_blocks: Vec<BlockId> = func
            .blocks
            .keys()
            .copied()
            .filter(|bid| !reachable.contains(bid))
            .collect();
        for bid in &dead_blocks {
            func.blocks.remove(bid);
            func.loop_roles.remove(bid);
            func.loop_pairs.remove(bid);
            func.loop_break_kinds.remove(bid);
            func.loop_cond_blocks.remove(bid);
            func.label_id_map.remove(&bid.0);
        }
        stats.ops_removed += dead_blocks.len();
    }

    stats
}

fn seed_constant_lattice_value(opcode: OpCode, attrs: &AttrDict) -> Option<LatticeValue> {
    match opcode_sccp_constant_seed_rule_table(opcode) {
        SccpConstantSeedRule::None => None,
        SccpConstantSeedRule::IntAttr => Some(match attrs.get("value") {
            Some(AttrValue::Int(v)) => LatticeValue::Constant(ConstVal::Int(*v)),
            _ => LatticeValue::Bottom,
        }),
        SccpConstantSeedRule::FloatAttr => Some(match attrs.get("f_value") {
            Some(AttrValue::Float(v)) => LatticeValue::Constant(ConstVal::Float(*v)),
            _ => LatticeValue::Bottom,
        }),
        SccpConstantSeedRule::BoolAttr => Some(match attrs.get("value") {
            Some(AttrValue::Bool(v)) => LatticeValue::Constant(ConstVal::Bool(*v)),
            _ => LatticeValue::Bottom,
        }),
        SccpConstantSeedRule::StrAttr => Some(match attrs.get("s_value") {
            Some(AttrValue::Str(v)) => LatticeValue::Constant(ConstVal::Str(v.clone())),
            _ => match attrs.get("value") {
                Some(AttrValue::Str(v)) => LatticeValue::Constant(ConstVal::Str(v.clone())),
                _ => LatticeValue::Bottom,
            },
        }),
        SccpConstantSeedRule::NoneSingleton => Some(LatticeValue::Constant(ConstVal::None)),
    }
}
