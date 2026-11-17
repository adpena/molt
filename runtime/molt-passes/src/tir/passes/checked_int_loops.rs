//! Checked raw integer loops: loop versioning with exact transfer (HF-857).
//!
//! A loop accumulator (`total += i`) of exact int type has no proven bound,
//! so the value-range analysis correctly refuses it a raw carrier: a bare
//! `iadd` would wrap at 2^63. Without this pass the accumulator is a boxed
//! word, and each update pays a tag test, an unbox, a rebox, reference
//! counting and a runtime fallback.
//!
//! The pass versions the loop instead:
//!
//! ```text
//! fast loop L          every selected op is CheckedAdd/CheckedMul:
//!   (r, of) = checked(a, b)
//!   cond_br of, transfer, rest_of_block
//! transfer             exact = add(a, b)    (boxed, BigInt-exact)
//!   br slow_rest        enters L' just after the op's clone
//! slow loop L'         a clone of L with plain boxed arithmetic
//! ```
//!
//! On overflow the iteration continues in the slow loop at the matching
//! point, with the exact result in place of the wrapped one. Nothing is
//! replayed, so the loop body may branch, call, store and raise. The shared
//! SSA updater ([`crate::tir::ssa_update`]) then gives every use the
//! definition that reaches it: the slow loop's first partial iteration reads
//! the fast loop's values, and code after the loop merges both loops. The
//! wrapped result is never observed, because on the overflow path the
//! transfer's exact result is the reaching definition.
//!
//! Selection is an optimistic fixpoint over the loop: an exact-int `Add` or
//! `Mul` is selected when both operands are raw-capable (inline-safe by the
//! representation authority, a constant, a selected result, or a loop phi
//! whose incomings are all raw-capable). The representation plan then makes
//! each selected result and accumulator phi a checked full-range raw carrier
//! (`Repr::RawI64FullDeopt`), and codegen emits a machine add with an
//! overflow branch.
//!
//! The outermost qualifying loop of a nest is versioned whole, so an
//! accumulator carried across an inner loop stays raw in both.
//!
//! Refusals (loops with handler regions, suspension points or exception
//! edges into the loop, loops over the size budget, failed SSA repair) leave
//! the loop unchanged. `MOLT_CHECKED_INT_LOOPS_STATS=1` reports each
//! versioned and refused loop to stderr and as a debug artifact.

use std::collections::BTreeMap;
use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::analysis::{AnalysisManager, LoopForest};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::clone_attrs_without_simple_names;
use crate::tir::dominators::{exception_label_to_block, exception_transfer_target};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    CheckedIntLoopRule, opcode_checked_int_loop_rule_table,
    opcode_is_exception_handler_region_table, opcode_is_state_machine_table,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::value_identity::scalar_value_source;
use crate::tir::passes::value_range::ValueRange;
use crate::tir::ssa_update::{self, RepairRefusal, Variable};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

use super::PassStats;

/// The largest loop, in operations, that the pass clones. Versioning doubles
/// the loop's code; past this size the code growth outweighs the gain.
const MAX_VERSIONED_OPS: usize = 2048;

/// Why a loop was left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// No exact-int add or multiply with raw-capable operands feeds a loop phi.
    NoAccumulator,
    /// The loop holds a try/except region; its handler labels would need a clone.
    HandlerRegion,
    /// The loop holds a suspension point of a generator or coroutine.
    Suspension,
    /// An exception edge enters a block of the loop.
    ExceptionEdgeIntoLoop,
    /// The loop exceeds `MAX_VERSIONED_OPS`.
    TooLarge,
    /// The SSA repair refused the versioned loop.
    Repair(RepairRefusal),
}

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "checked_int_loops",
        ..PassStats::default()
    };
    // Module chunks keep every value boxed (`floor_module_chunk_carriers`).
    if molt_ir::ir::is_module_chunk_function(&func.name) {
        return stats;
    }
    let forest = am.get::<LoopForest>(func).clone();
    if forest.headers.is_empty() {
        return stats;
    }
    let debug = backend_env::var(BackendEnv::MOLT_CHECKED_INT_LOOPS_STATS).as_deref() == Ok("1");
    let mut report: Vec<(BlockId, Result<usize, Refusal>)> = Vec::new();

    // Outermost loops first: a versioned loop's inner loops are versioned
    // with it. A refused loop lets its inner loops try on their own.
    let mut headers = forest.headers.clone();
    headers.sort_by_key(|header| std::cmp::Reverse(forest.bodies[header].len()));
    let mut covered: HashSet<BlockId> = HashSet::default();
    let mut ranges = None;
    for header in headers {
        if covered.contains(&header) {
            continue;
        }
        let body = &forest.bodies[&header];
        let vr = ranges.get_or_insert_with(|| am.get::<ValueRange>(func).clone());
        let outcome = try_version_loop(func, header, body, vr);
        if let Ok(added) = outcome {
            stats.ops_added += added;
            covered.extend(body.iter().copied());
            // Value ranges change with the CFG: recompute them for the next
            // loop instead of reading the cached result of the old function.
            am.invalidate_cfg();
            ranges = None;
        }
        report.push((header, outcome));
    }
    if debug {
        write_report(func, &report);
    }
    stats
}

fn try_version_loop(
    func: &mut TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
) -> Result<usize, Refusal> {
    check_loop_shape(func, body)?;
    let selected = select(func, header, body, vr)?;
    let snapshot = func.clone();
    match version(func, body, &selected) {
        Ok(added) => Ok(added),
        Err(refusal) => {
            *func = snapshot;
            Err(refusal)
        }
    }
}

fn check_loop_shape(func: &TirFunction, body: &HashSet<BlockId>) -> Result<(), Refusal> {
    let labels = exception_label_to_block(func);
    let mut ops = 0usize;
    for bid in body {
        let block = &func.blocks[bid];
        ops += block.ops.len();
        for op in &block.ops {
            if opcode_is_exception_handler_region_table(op.opcode) {
                return Err(Refusal::HandlerRegion);
            }
            if opcode_is_state_machine_table(op.opcode) {
                return Err(Refusal::Suspension);
            }
            if exception_transfer_target(op, &labels).is_some_and(|target| body.contains(&target))
            {
                return Err(Refusal::ExceptionEdgeIntoLoop);
            }
        }
        if matches!(block.terminator, Terminator::StateDispatch { .. }) {
            return Err(Refusal::Suspension);
        }
    }
    if body.contains(&func.entry_block) {
        return Err(Refusal::ExceptionEdgeIntoLoop);
    }
    if ops > MAX_VERSIONED_OPS {
        return Err(Refusal::TooLarge);
    }
    Ok(())
}

/// The checked opcode for an exact-int arithmetic opcode.
fn checked_opcode(opcode: OpCode) -> Option<OpCode> {
    match opcode_checked_int_loop_rule_table(opcode) {
        CheckedIntLoopRule::CheckedAdd => Some(OpCode::CheckedAdd),
        CheckedIntLoopRule::CheckedMul => Some(OpCode::CheckedMul),
        CheckedIntLoopRule::None => None,
    }
}

/// The selected arithmetic ops, as (block, op index), in deterministic order.
fn select(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
) -> Result<Vec<(BlockId, usize)>, Refusal> {
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    let is_int = |value: &ValueId| exact.get(value) == Some(&TirType::I64);
    let inline_safe = crate::representation_facts::raw_i64_safe_values_for(func, vr);
    let mut constants: HashSet<ValueId> = HashSet::default();
    let mut arithmetic: Vec<(BlockId, usize, ValueId, [ValueId; 2])> = Vec::new();
    let mut copies: Vec<(ValueId, ValueId)> = Vec::new();
    let mut phis: Vec<(BlockId, usize, ValueId)> = Vec::new();
    for (&bid, block) in &func.blocks {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt && op.results.first().is_some_and(is_int) {
                constants.insert(op.results[0]);
            }
        }
        if !body.contains(&bid) {
            continue;
        }
        for (index, arg) in block.args.iter().enumerate() {
            if is_int(&arg.id) {
                phis.push((bid, index, arg.id));
            }
        }
        for (index, op) in block.ops.iter().enumerate() {
            if checked_opcode(op.opcode).is_some()
                && let ([lhs, rhs], [result]) = (&op.operands[..], &op.results[..])
                && let (lhs, rhs, result) = (*lhs, *rhs, *result)
                && is_int(&lhs)
                && is_int(&rhs)
                && is_int(&result)
                && !inline_safe.contains(&result)
            {
                arithmetic.push((bid, index, result, [lhs, rhs]));
            } else if let Some(source) = scalar_value_source(op)
                && let [result] = op.results[..]
                && is_int(&result)
            {
                copies.push((result, source));
            }
        }
    }
    let incomings = crate::representation_facts::block_arg_incomings_for(func);

    // Optimistic fixpoint: assume every candidate raw-capable, then drop each
    // one with an operand or incoming that is not.
    let mut capable: HashSet<ValueId> = arithmetic.iter().map(|&(_, _, result, _)| result).collect();
    capable.extend(copies.iter().map(|&(result, _)| result));
    capable.extend(phis.iter().map(|&(_, _, phi)| phi));
    let raw = |value: &ValueId, capable: &HashSet<ValueId>| {
        capable.contains(value) || inline_safe.contains(value) || constants.contains(value)
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &(_, _, result, operands) in &arithmetic {
            if capable.contains(&result) && !operands.iter().all(|v| raw(v, &capable)) {
                capable.remove(&result);
                changed = true;
            }
        }
        for &(result, source) in &copies {
            if capable.contains(&result) && !raw(&source, &capable) {
                capable.remove(&result);
                changed = true;
            }
        }
        for &(block, index, phi) in &phis {
            if capable.contains(&phi)
                && !incomings
                    .get(&(block, index))
                    .is_some_and(|srcs| !srcs.is_empty() && srcs.iter().all(|v| raw(v, &capable)))
            {
                capable.remove(&phi);
                changed = true;
            }
        }
    }
    let header_args: Vec<ValueId> = func.blocks[&header].args.iter().map(|arg| arg.id).collect();
    if !header_args.iter().any(|arg| capable.contains(arg)) {
        return Err(Refusal::NoAccumulator);
    }
    let mut selected: Vec<(BlockId, usize)> = arithmetic
        .iter()
        .filter(|&&(_, _, result, _)| capable.contains(&result))
        .map(|&(block, index, _, _)| (block, index))
        .collect();
    if selected.is_empty() {
        return Err(Refusal::NoAccumulator);
    }
    selected.sort_by_key(|&(block, index)| (block.0, index));
    Ok(selected)
}

/// Clone the loop, check the selected ops, wire each overflow to the slow
/// loop and repair SSA. Returns the number of operations added.
fn version(
    func: &mut TirFunction,
    body: &HashSet<BlockId>,
    selected: &[(BlockId, usize)],
) -> Result<usize, Refusal> {
    let mut blocks: Vec<BlockId> = body.iter().copied().collect();
    blocks.sort_by_key(|block| block.0);

    // 1. The slow loop: fresh ids for every block and every value it defines.
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::default();
    for &bid in &blocks {
        block_map.insert(bid, func.fresh_block());
    }
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::default();
    let mut defined: Vec<(ValueId, TirType)> = Vec::new();
    for &bid in &blocks {
        let block = &func.blocks[&bid];
        let block_values: Vec<(ValueId, TirType)> = block
            .args
            .iter()
            .map(|arg| (arg.id, arg.ty.clone()))
            .chain(block.ops.iter().flat_map(|op| {
                op.results.iter().map(|&result| {
                    let ty = func.value_types.get(&result).cloned().unwrap_or(TirType::DynBox);
                    (result, ty)
                })
            }))
            .collect();
        for (value, ty) in block_values {
            let fresh = func.fresh_value();
            if let Some(known) = func.value_types.get(&value).cloned() {
                func.value_types.insert(fresh, known);
            }
            value_map.insert(value, fresh);
            defined.push((value, ty));
        }
    }
    let map_value = |value: ValueId| value_map.get(&value).copied().unwrap_or(value);
    let mut added = 0usize;
    for &bid in &blocks {
        let source = &func.blocks[&bid];
        let mut terminator = source.terminator.clone();
        terminator.for_each_value_mut(|value| *value = map_value(*value));
        terminator.for_each_edge_mut(|target, _| {
            if let Some(&cloned) = block_map.get(target) {
                *target = cloned;
            }
        });
        let clone = TirBlock {
            id: block_map[&bid],
            args: source
                .args
                .iter()
                .map(|arg| TirValue {
                    id: map_value(arg.id),
                    ty: arg.ty.clone(),
                })
                .collect(),
            ops: source
                .ops
                .iter()
                .map(|op| TirOp {
                    operands: op.operands.iter().map(|&v| map_value(v)).collect(),
                    results: op.results.iter().map(|&v| map_value(v)).collect(),
                    attrs: clone_attrs_without_simple_names(&op.attrs),
                    ..op.clone()
                })
                .collect(),
            terminator,
        };
        added += clone.ops.len();
        func.blocks.insert(clone.id, clone);
    }

    // 2. Check each selected op; split its block in both loops after it.
    let mut by_block: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for &(block, index) in selected {
        by_block.entry(block.0).or_default().push(index);
    }
    let mut variables: HashMap<ValueId, Vec<ValueId>> = defined
        .iter()
        .map(|(value, _)| (*value, vec![*value, value_map[value]]))
        .collect();
    for (block, mut indices) in by_block {
        let fast = BlockId(block);
        let slow = block_map[&fast];
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for index in indices {
            let original = func.blocks[&fast].ops[index].clone();
            let result = original.results[0];
            let flag = func.fresh_value();
            func.value_types.insert(flag, TirType::Bool);
            let exact = func.fresh_value();
            func.value_types.insert(exact, TirType::I64);
            let fast_rest = func.fresh_block();
            let slow_rest = func.fresh_block();
            let transfer = func.fresh_block();

            let fast_block = func.blocks.get_mut(&fast).expect("selected block exists");
            let rest_ops = fast_block.ops.split_off(index + 1);
            let checked = &mut fast_block.ops[index];
            checked.opcode = checked_opcode(original.opcode).expect("selected op is arithmetic");
            checked.results.push(flag);
            let rest_terminator = std::mem::replace(
                &mut fast_block.terminator,
                Terminator::CondBranch {
                    cond: flag,
                    then_block: transfer,
                    then_args: vec![],
                    else_block: fast_rest,
                    else_args: vec![],
                },
            );
            func.blocks.insert(
                fast_rest,
                TirBlock {
                    id: fast_rest,
                    args: vec![],
                    ops: rest_ops,
                    terminator: rest_terminator,
                },
            );
            for cond_block in func.loop_cond_blocks.values_mut() {
                if *cond_block == fast {
                    *cond_block = fast_rest;
                }
            }

            let slow_block = func.blocks.get_mut(&slow).expect("cloned block exists");
            let slow_ops = slow_block.ops.split_off(index + 1);
            let slow_terminator = std::mem::replace(
                &mut slow_block.terminator,
                Terminator::Branch {
                    target: slow_rest,
                    args: vec![],
                },
            );
            func.blocks.insert(
                slow_rest,
                TirBlock {
                    id: slow_rest,
                    args: vec![],
                    ops: slow_ops,
                    terminator: slow_terminator,
                },
            );

            func.blocks.insert(
                transfer,
                TirBlock {
                    id: transfer,
                    args: vec![],
                    ops: vec![TirOp {
                        results: vec![exact],
                        attrs: clone_attrs_without_simple_names(&original.attrs),
                        ..original.clone()
                    }],
                    terminator: Terminator::Branch {
                        target: slow_rest,
                        args: vec![],
                    },
                },
            );
            variables
                .get_mut(&result)
                .expect("selected result is a loop value")
                .push(exact);
            added += 1;
        }
    }

    // 3. Every use reads the definition that reaches it.
    let types: HashMap<ValueId, TirType> = defined.into_iter().collect();
    let mut repair: Vec<Variable> = variables
        .into_iter()
        .map(|(value, names)| Variable {
            ty: types[&value].clone(),
            names,
        })
        .collect();
    repair.sort_by_key(|variable| variable.names[0].0);
    ssa_update::repair(func, &repair).map_err(Refusal::Repair)?;
    Ok(added)
}

fn write_report(func: &TirFunction, report: &[(BlockId, Result<usize, Refusal>)]) {
    let versioned = report.iter().filter(|(_, outcome)| outcome.is_ok()).count();
    let mut text = format!(
        "[checked_int_loops] func '{}': {} versioned, {} refused\n",
        func.name,
        versioned,
        report.len() - versioned
    );
    for (header, outcome) in report {
        match outcome {
            Ok(added) => text.push_str(&format!(
                "  versioned loop @ block {}: {added} ops added\n",
                header.0
            )),
            Err(refusal) => {
                text.push_str(&format!("  refused loop @ block {}: {refusal:?}\n", header.0))
            }
        }
    }
    eprint!("{text}");
    let sanitized: String = func
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("checked_int_loops/{sanitized}.txt"),
        text,
    );
}

#[cfg(test)]
mod tests;
