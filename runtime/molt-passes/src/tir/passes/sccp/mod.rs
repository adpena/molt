//! Sparse Conditional Constant Propagation (SCCP).
//!
//! Propagates constants through the SSA graph, folds constant operations,
//! and eliminates branches with known-constant conditions.
//!
//! The flow-insensitive SSA lattice carries immutable values only. Heap object
//! contents require memory-state and alias custody; they are not SSA constants.

mod eval;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::PassStats;
use super::effects;
use super::reachability::metadata_preserving_reachable_blocks;
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExceptionRegionNestingRole, SccpConstantSeedRule, opcode_exception_region_nesting_role_table,
    opcode_sccp_constant_seed_rule_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use eval::{evaluate_builtin_call, evaluate_op};

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

/// Immutable value constants carried through the flow-insensitive lattice.
///
/// Mutability is excluded by construction, including transitively through
/// tuples. A list/dict snapshot would become stale after mutation through any
/// alias or callback, even when its SSA identity is unchanged. Mutable object
/// producers and their dependent observations therefore remain overdefined.
///
/// Equality denotes exact lattice value identity, not Python `==` or `is`.
/// Float bits preserve signed zero and NaN sign/payload; tuples compare their
/// recursively immutable values without depending on allocation identity.
#[derive(Debug, Clone)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    None,
    /// Recursively immutable tuple (all elements are ConstVal).
    /// Capped at MAX_COMPOUND_ELEMENTS to avoid embedding huge data at compile time.
    /// Shared storage preserves the value DAG instead of expanding nested clones.
    Tuple(Arc<[ConstVal]>),
    /// Compile-time range(start, stop, step). Not materialized as a list,
    /// but supports len() and iteration count propagation.
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
}

impl PartialEq for ConstVal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left.to_bits() == right.to_bits(),
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Str(left), Self::Str(right)) => left == right,
            (Self::None, Self::None) => true,
            (Self::Tuple(left), Self::Tuple(right)) => {
                Arc::ptr_eq(left, right) || left.as_ref() == right.as_ref()
            }
            (
                Self::Range {
                    start: ls,
                    stop: le,
                    step: ld,
                },
                Self::Range {
                    start: rs,
                    stop: re,
                    step: rd,
                },
            ) => (ls, le, ld) == (rs, re, rd),
            _ => false,
        }
    }
}

// Bit-exact floats make equality reflexive even for NaNs.
impl Eq for ConstVal {}

/// Maximum number of elements for compile-time compound value folding.
/// Counts recursive tuple nodes and UTF-8 bytes, not only immediate children.
/// Admission happens before allocation, bounding compiler work and output size.
const MAX_COMPOUND_ELEMENTS: usize = 1000;

impl ConstVal {
    fn materialization_cost(&self) -> Option<usize> {
        let cost = match self {
            Self::Str(value) => value.len().max(1),
            Self::Tuple(elements) => elements.iter().try_fold(1usize, |cost, value| {
                let cost = cost.checked_add(value.materialization_cost()?)?;
                (cost <= MAX_COMPOUND_ELEMENTS).then_some(cost)
            })?,
            _ => 1,
        };
        (cost <= MAX_COMPOUND_ELEMENTS).then_some(cost)
    }
}

/// SCCP computes one immutable result, never a broadcast into result siblings.
/// The generated operation contract owns structural validity for every caller.
fn admits_constant_result(op: &TirOp) -> bool {
    op.has_valid_shape() && op.results.len() == 1
}

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
    let value_types = crate::tir::type_refine::extract_exact_scalar_map(func);
    for block in func.blocks.values() {
        let mut try_depth: u32 = 0;
        for op in &block.ops {
            match opcode_exception_region_nesting_role_table(op.opcode) {
                ExceptionRegionNestingRole::Enter => try_depth += 1,
                ExceptionRegionNestingRole::Exit => try_depth = try_depth.saturating_sub(1),
                ExceptionRegionNestingRole::None => {}
            }
            if try_depth > 0 && effects::op_may_throw_with_types(op, &value_types) {
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
                if !admits_constant_result(op) {
                    // Malformed producers must not seed downstream constants or
                    // branches even when we preserve their own instruction.
                    lattice.insert(res, LatticeValue::Bottom);
                    continue;
                }
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
                let val = seed_constant_lattice_value(op).unwrap_or(LatticeValue::Top);
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
                if !admits_constant_result(op) {
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
                    .or_else(|| evaluate_builtin_call(op, &operand_vals));
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
            if !admits_constant_result(op) {
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
                    // Immutable compounds inform downstream value observations,
                    // but their producers retain runtime allocation/identity.
                    // TIR has no ConstTuple or ConstRange materialization opcode.
                    ConstVal::Tuple(_) | ConstVal::Range { .. } => {}
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
        stats.ops_removed += func
            .retain_blocks(&reachable)
            .expect("SCCP reachability must preserve live block and metadata references");
    }

    stats
}

fn seed_constant_lattice_value(op: &TirOp) -> Option<LatticeValue> {
    if !admits_constant_result(op) {
        return Some(LatticeValue::Bottom);
    }
    let attrs = &op.attrs;
    match opcode_sccp_constant_seed_rule_table(op.opcode) {
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
            Some(AttrValue::Str(v)) if v.len() <= MAX_COMPOUND_ELEMENTS => {
                LatticeValue::Constant(ConstVal::Str(v.clone()))
            }
            Some(AttrValue::Str(_)) => LatticeValue::Bottom,
            _ => match attrs.get("value") {
                Some(AttrValue::Str(v)) if v.len() <= MAX_COMPOUND_ELEMENTS => {
                    LatticeValue::Constant(ConstVal::Str(v.clone()))
                }
                _ => LatticeValue::Bottom,
            },
        }),
        SccpConstantSeedRule::NoneSingleton => Some(LatticeValue::Constant(ConstVal::None)),
    }
}
