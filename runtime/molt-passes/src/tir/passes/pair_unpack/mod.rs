//! Pair-loop fusion: a proven two-target loop makes no item tuple
//! (`call_anatomy.md` 4.5a).
//!
//! `for k, v in d.items()`, `enumerate` and `zip` loops reach TIR as
//!
//! ```text
//!   %e, %done = IterNextUnboxed(%it)          ; %e valid only on the not-done edge
//!   CheckException {value: X}
//!   ... CondBranch(%done, exit, body)
//! body:
//!   %k, %v = UnpackSequence(%e) {value: 2}
//!   CheckException {value: X}
//! ```
//!
//! When no Python code can name `%e` and nothing observable runs between the
//! step and the unpack, the two become one runtime call,
//! `%k, %v, %done = IterNextUnpack2(%it)`, and the unpack's error is raised by
//! that call, observed by the step's check. Each fact the proof needs:
//!
//! * **P1** `%e`'s sole use is the unpack (no held pair).
//! * **P2** the unpack has exactly two targets.
//! * **P3** the unpack lies on the not-done continuation: the step's block
//!   ends in a chain of single-entry blocks to the `CondBranch` on `%done`,
//!   and a chain of single-entry blocks leads from its not-done target to the
//!   unpack.
//! * **P4** only `FrameSetLocation` ops naming the location in effect at the
//!   step, and operand-free pending-exception checks, lie between. The step's
//!   own check observes any error first, so the later checks observe nothing in
//!   either program, and an unpack error reports the same source location.
//! * **P5** the step and the unpack are each followed by an operand-free
//!   `CheckException` of the same edge kind and label, so the error reaches the
//!   same handler through the same kind of edge.
//!
//! CPython runs nothing between `FOR_ITER` and `UNPACK_SEQUENCE` for a
//! two-target loop either, so the fused call is the same sequence of observable
//! events: the iterator advances, then the item is unpacked and released.
//! A refused candidate keeps both ops; `TIR_OPT_STATS=1` prints each fused
//! site and each refusal with its reason.

#[cfg(test)]
mod tests;

use molt_ir::deterministic_hash::HashMap;

use molt_ir::backend_environment::{self as backend_env, BackendEnv};

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{exception_label_to_block, is_exception_transfer_edge};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Why a step-then-unpack site keeps its item tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The item has a reader besides the unpack (P1).
    ItemHasOtherUses,
    /// The unpack has this many targets (P2).
    UnpackArity(usize),
    /// The unpack is not on the step's single-entry not-done continuation (P3).
    UnpackNotOnContinuation,
    /// An op that may be observable lies between the step and the unpack (P4).
    ObservableOpBetween(OpCode),
    /// A frame location other than the step's lies between them (P4).
    LocationDiffers,
    /// The step and the unpack are not observed by one handler (P5).
    HandlerDiffers,
}

impl Refusal {
    fn reason(self) -> String {
        match self {
            Self::ItemHasOtherUses => "item_has_other_uses".into(),
            Self::UnpackArity(n) => format!("unpack_arity({n})"),
            Self::UnpackNotOnContinuation => "unpack_not_on_continuation".into(),
            Self::ObservableOpBetween(opcode) => format!("observable_op_between({opcode:?})"),
            Self::LocationDiffers => "location_differs".into(),
            Self::HandlerDiffers => "handler_differs".into(),
        }
    }
}

/// A position in the function: block and op index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct At {
    block: BlockId,
    op: usize,
}

/// A proven site, named by its item: the step defines it, the unpack reads it.
pub(crate) struct Plan {
    step_block: BlockId,
    unpack_block: BlockId,
    item: ValueId,
}

/// The outcome for one candidate, in block order, for the statistics report.
pub(crate) struct SiteOutcome {
    pub(crate) line: Option<i64>,
    pub(crate) result: Result<(), Refusal>,
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "pair_unpack",
        ..Default::default()
    };
    let (plans, outcomes) = analyze(func);
    for plan in &plans {
        apply(func, plan, &mut stats);
    }
    if !outcomes.is_empty() && backend_env::var(BackendEnv::TIR_OPT_STATS).as_deref() == Ok("1") {
        report(&func.name, &outcomes);
    }
    stats
}

/// Every candidate's outcome and the plans of the proven ones. A plan names
/// its ops by value identity, so applying one never moves another.
pub(crate) fn analyze(func: &TirFunction) -> (Vec<Plan>, Vec<SiteOutcome>) {
    let uses = use_counts(func);
    let entries = entry_counts(func);
    let mut readers: HashMap<ValueId, At> = HashMap::default();
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|block| block.0);
    for &block in &block_ids {
        for (op, tir_op) in func.blocks[&block].ops.iter().enumerate() {
            if tir_op.opcode == OpCode::UnpackSequence
                && let Some(&source) = tir_op.operands.first()
            {
                readers.insert(source, At { block, op });
            }
        }
    }
    let mut plans = Vec::new();
    let mut outcomes = Vec::new();
    for &block in &block_ids {
        for (op, tir_op) in func.blocks[&block].ops.iter().enumerate() {
            if tir_op.opcode != OpCode::IterNextUnboxed || tir_op.results.len() != 2 {
                continue;
            }
            let item = tir_op.results[0];
            let Some(&unpack) = readers.get(&item) else {
                continue;
            };
            let step = At { block, op };
            let result = prove(func, &uses, &entries, step, unpack);
            outcomes.push(SiteOutcome {
                line: match tir_op.attrs.get("_source_line") {
                    Some(AttrValue::Int(line)) => Some(*line),
                    _ => None,
                },
                result,
            });
            if result.is_ok() {
                plans.push(Plan {
                    step_block: block,
                    unpack_block: unpack.block,
                    item,
                });
            }
        }
    }
    (plans, outcomes)
}

fn prove(
    func: &TirFunction,
    uses: &HashMap<ValueId, usize>,
    entries: &HashMap<BlockId, usize>,
    step: At,
    unpack: At,
) -> Result<(), Refusal> {
    let step_op = &func.blocks[&step.block].ops[step.op];
    let unpack_op = &func.blocks[&unpack.block].ops[unpack.op];
    let [item, done] = [step_op.results[0], step_op.results[1]];
    // P1: the unpack reads the item once and nothing else reads it.
    if uses.get(&item).copied() != Some(1) || unpack_op.operands.len() != 1 {
        return Err(Refusal::ItemHasOtherUses);
    }
    // P2.
    if unpack_op.results.len() != 2 {
        return Err(Refusal::UnpackArity(unpack_op.results.len()));
    }
    // P5, first half: the step's own observation follows it.
    let step_block = &func.blocks[&step.block];
    let step_label =
        observation_label(step_block.ops.get(step.op + 1)).ok_or(Refusal::HandlerDiffers)?;
    let location = location_before(&step_block.ops[..step.op]);
    // P3/P4 on the step's side: the rest of its block, then single-entry
    // blocks, to the CondBranch on `done`.
    check_between(&step_block.ops[step.op + 2..], location)?;
    let mut block = step.block;
    let body = loop {
        match &func.blocks[&block].terminator {
            Terminator::Branch { target, args } if args.is_empty() => {
                block = enter(func, entries, *target, location)?;
            }
            Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                else_args,
                ..
            } if *cond == done && then_block != else_block && else_args.is_empty() => {
                break *else_block;
            }
            _ => return Err(Refusal::UnpackNotOnContinuation),
        }
    };
    // From the not-done target, single-entry blocks to the unpack.
    block = body;
    if entries.get(&block).copied() != Some(1) {
        return Err(Refusal::UnpackNotOnContinuation);
    }
    loop {
        let ops = &func.blocks[&block].ops;
        if block == unpack.block {
            check_between(&ops[..unpack.op], location)?;
            break;
        }
        check_between(ops, location)?;
        match &func.blocks[&block].terminator {
            Terminator::Branch { target, args } if args.is_empty() => {
                if entries.get(target).copied() != Some(1) || *target == body {
                    return Err(Refusal::UnpackNotOnContinuation);
                }
                block = *target;
            }
            _ => return Err(Refusal::UnpackNotOnContinuation),
        }
    }
    // P5, second half: the unpack's observation names the step's handler.
    let unpack_label = observation_label(func.blocks[&unpack.block].ops.get(unpack.op + 1));
    if unpack_label != Some(step_label) {
        return Err(Refusal::HandlerDiffers);
    }
    Ok(())
}

/// Enter a block on the step's side of the branch: it must have one entry and
/// hold only ops that `check_between` admits.
fn enter(
    func: &TirFunction,
    entries: &HashMap<BlockId, usize>,
    target: BlockId,
    location: Option<i64>,
) -> Result<BlockId, Refusal> {
    if entries.get(&target).copied() != Some(1) {
        return Err(Refusal::UnpackNotOnContinuation);
    }
    check_between(&func.blocks[&target].ops, location)?;
    Ok(target)
}

/// P4: every op between the step and the unpack is a `FrameSetLocation` that
/// restates the location in effect at the step, or an operand-free check of
/// the pending exception that polls no async work. The step's own check has
/// already observed any error, so such a check observes nothing.
fn check_between(ops: &[TirOp], location: Option<i64>) -> Result<(), Refusal> {
    for op in ops {
        match op.opcode {
            OpCode::FrameSetLocation => {
                if location.is_none() || frame_location(op) != location {
                    return Err(Refusal::LocationDiffers);
                }
            }
            OpCode::CheckException
                if op.operands.is_empty() && !op.attrs.contains_key("async_work_poll") => {}
            opcode => return Err(Refusal::ObservableOpBetween(opcode)),
        }
    }
    Ok(())
}

/// The location the last `FrameSetLocation` among `ops` sets.
fn location_before(ops: &[TirOp]) -> Option<i64> {
    ops.iter()
        .rev()
        .find(|op| op.opcode == OpCode::FrameSetLocation)
        .and_then(frame_location)
}

fn frame_location(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(location)) => Some(*location),
        _ => None,
    }
}

/// The handler identity of an operand-free `CheckException`: its label, its
/// edge kind and the location it materializes.
fn observation_label(op: Option<&TirOp>) -> Option<(i64, Option<i64>, Option<i64>)> {
    let op = op?;
    if op.opcode != OpCode::CheckException
        || !op.operands.is_empty()
        || op.attrs.contains_key("async_work_poll")
    {
        return None;
    }
    let edge = |key: &str| match op.attrs.get(key) {
        Some(AttrValue::Int(value)) => Some(*value),
        _ => None,
    };
    Some((edge("value")?, edge("exception_edge"), op.frame_location()))
}

/// How many times each value is read: op operands and terminator values.
fn use_counts(func: &TirFunction) -> HashMap<ValueId, usize> {
    let mut uses: HashMap<ValueId, usize> = HashMap::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            for &operand in &op.operands {
                *uses.entry(operand).or_default() += 1;
            }
        }
        block
            .terminator
            .for_each_value(|value| *uses.entry(value).or_default() += 1);
    }
    uses
}

/// How many control arcs enter each block: terminator edges and exception
/// transfer edges, as the ownership lattice counts them for the not-done region.
fn entry_counts(func: &TirFunction) -> HashMap<BlockId, usize> {
    let labels = exception_label_to_block(func);
    let mut entries: HashMap<BlockId, usize> = HashMap::default();
    for block in func.blocks.values() {
        for target in block.terminator.successors() {
            *entries.entry(target).or_default() += 1;
        }
        for op in &block.ops {
            if is_exception_transfer_edge(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = labels.get(label)
            {
                *entries.entry(target).or_default() += 1;
            }
        }
    }
    entries
}

/// Fuse one proven site: the step becomes `IterNextUnpack2` with the unpack's
/// results and the step's done flag; the unpack and its observation go.
fn apply(func: &mut TirFunction, plan: &Plan, stats: &mut PassStats) {
    let unpack_block = func
        .blocks
        .get_mut(&plan.unpack_block)
        .expect("unpack block");
    let at = unpack_block
        .ops
        .iter()
        .position(|op| op.opcode == OpCode::UnpackSequence && op.operands == [plan.item])
        .expect("proven unpack");
    let unpack = unpack_block.ops.remove(at);
    let observation = unpack_block.ops.remove(at);
    debug_assert_eq!(observation.opcode, OpCode::CheckException);
    let step = func
        .blocks
        .get_mut(&plan.step_block)
        .expect("step block")
        .ops
        .iter_mut()
        .find(|op| op.opcode == OpCode::IterNextUnboxed && op.results.first() == Some(&plan.item))
        .expect("proven step");
    let done = step.results[1];
    step.opcode = OpCode::IterNextUnpack2;
    step.results = vec![unpack.results[0], unpack.results[1], done];
    let done_name = step.attrs.remove("_simple_result_1");
    step.attrs.remove("_simple_result_0");
    for (index, key) in ["_simple_result_0", "_simple_result_1"]
        .into_iter()
        .enumerate()
    {
        if let Some(name) = unpack.attrs.get(key) {
            step.attrs
                .insert(format!("_simple_result_{index}"), name.clone());
        }
    }
    if let Some(name) = done_name {
        step.attrs.insert("_simple_result_2".into(), name);
    }
    func.value_types.remove(&plan.item);
    stats.ops_removed += 2;
    stats.values_changed += 1;
}

fn report(function: &str, outcomes: &[SiteOutcome]) {
    let mut text = String::new();
    for outcome in outcomes {
        let line = outcome
            .line
            .map_or_else(|| "?".to_string(), |line| line.to_string());
        match outcome.result {
            Ok(()) => text.push_str(&format!("[pair_unpack] {function} line {line}: fused\n")),
            Err(refusal) => text.push_str(&format!(
                "[pair_unpack] {function} line {line}: refused {}\n",
                refusal.reason()
            )),
        }
    }
    eprint!("{text}");
}
