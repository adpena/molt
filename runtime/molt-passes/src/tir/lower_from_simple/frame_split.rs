//! Coroutine frame split: the one place a stateful activation's values cross
//! a suspension.
//!
//! The frontend lowers a generator, coroutine or async-generator body the way
//! it lowers any function: plain values plus explicit suspension operations
//! (`state_yield`, `state_transition`, ...) and a resume entry (`state_label`)
//! for each. It places no frame slots for its own temporaries. This pass runs
//! on that source form, before CFG and SSA construction, and makes it
//! re-entrant:
//!
//! 1. Liveness over the source graph gives the names live across every
//!    suspension: in source IR a suspension continues at its resume entry, and
//!    the state dispatch has only its initial-entry edge
//!    (`molt_ir::simple_verify::state_resume_op_edges`).
//! 2. Every entry runs the poll's prologue (the operations before the
//!    dispatch). A live name the prologue defines once needs nothing: a
//!    resume reads its own entry's value. A live name whose one definition is
//!    a constant moves into the prologue: it stays one definition, so SSA
//!    joins nothing for it and facts that need a unique constant (an iterator
//!    pair's selectors) still see one. Every other live name gets its own
//!    frame slot past the frontend's layout, as LLVM gives each spilled value
//!    its own frame field.
//! 3. A name with one definition is stored once, just after that definition:
//!    the slot then holds its value at every suspension the definition
//!    reaches, and a loop that suspends stores nothing for it per iteration.
//!    A name defined more than once is stored at each suspension instead: a
//!    pure suspension stores it just before it returns, and a resume entry
//!    also reached by fall-through (an `await` re-polls from its entry) stores
//!    it on that edge.
//! 4. After the resume entry (its re-entered `try` regions and its frame
//!    publication) each name is reloaded into the same name. SSA construction
//!    then joins the reloaded and the original definitions; the state dispatch
//!    is the resume entry's only other predecessor.
//! 5. Where a spilled name dies (after the operation that reads it last, or
//!    at the head of a block it is dead into), its slot is released: a
//!    `closure_store` of None marked `frame_slot_release`. CPython drops a
//!    frame value at its last use, so a finalizer runs there, not when the
//!    task dies. A slot belongs to one name, so a release where the name is
//!    dead never loses a value a reload needs.
//!
//! This is the switched-resume lowering of LLVM's `coro.split` and the
//! storage-liveness layout of Rust's generator transform, at the SimpleIR/TIR
//! boundary. The split gives `state_switch` its explicit `state_targets` map,
//! so from here on each resume edge leaves the dispatch. The dispatch's `value`
//! stays the frontend layout's byte size, which is where the spill area
//! begins; the split writes the frame's final byte size as its
//! `task_closure_size`. The lift records both ([`STATEFUL_SPILL_BASE_ATTR`],
//! [`STATEFUL_FRAME_SIZE_ATTR`]); type refinement types a spill-slot reload by
//! what the split stores there, and every backend publishes the frame size as
//! the poll's frame-size symbol, which task constructors read. Split IR (a
//! dispatch with an explicit map) passes through unchanged.

use std::collections::{BTreeMap, BTreeSet};

use molt_ir::OpKind;
use molt_ir::simple_verify::state_resume_op_edges;
use molt_ir::tir::cfg::CFG;
use molt_ir::tir::cfg_liveness::{
    SimpleCfgLivenessFacts, analyze_simple_cfg_liveness_facts_with_cfg, liveness_block_successors,
};
use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;
use molt_ir::tir::simple_def_use::{
    SimpleDefinitionFacts, SimpleDefinitionSite, visit_simple_ir_defined_names,
    visit_simple_ir_reads,
};

use crate::ir::OpIR;

/// Function attribute: the activation frame's byte size after the split.
pub const STATEFUL_FRAME_SIZE_ATTR: &str = "stateful_frame_size";

/// Function attribute: the byte offset where the split's spill area begins.
/// Only the split writes a frame slot at or past it.
pub const STATEFUL_SPILL_BASE_ATTR: &str = "stateful_spill_base";

const FRAME_WORD: i64 = 8;

/// One suspension of source IR: the suspend operation, the state it saves and
/// the resume entry it continues at.
#[derive(Debug, Clone, Copy)]
struct Suspension {
    suspend: usize,
    state: i64,
    resume: usize,
}

/// Operations the frontend emits between a resume entry and its frame
/// publication (`frame_context_set`): operand constants and loads.
fn is_frame_publication_operand(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::ConstNone | OpKind::Const | OpKind::ClosureLoad | OpKind::LoadVar | OpKind::Index
    )
}

/// The split frame's spill base and final byte size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FrameLayout {
    pub spill_base: i64,
    pub frame_size: i64,
}

/// The split result: the rewritten operations and the frame layout, or `None`
/// for a function with no state dispatch.
pub(super) struct FrameSplit {
    pub ops: Vec<OpIR>,
    pub layout: Option<FrameLayout>,
}

pub(super) fn split_stateful_frame(
    function_name: &str,
    params: &[String],
    mut ops: Vec<OpIR>,
    observe_async_work: bool,
) -> FrameSplit {
    let Some(switch) = ops.iter().position(|op| op.kind == OpKind::StateSwitch) else {
        return FrameSplit { ops, layout: None };
    };
    let base = ops[switch].value.unwrap_or_else(|| {
        panic!("{function_name}: state_switch must carry the frontend frame layout's byte size")
    });
    assert!(
        base >= 0 && base % FRAME_WORD == 0,
        "{function_name}: frame size {base} is not a nonnegative word multiple"
    );
    if ops[switch].state_targets.is_some() {
        let frame_size = ops[switch].task_closure_size.unwrap_or_else(|| {
            panic!("{function_name}: a split state_switch must carry its frame size")
        });
        return FrameSplit {
            ops,
            layout: Some(FrameLayout {
                spill_base: base,
                frame_size,
            }),
        };
    }
    let suspensions: Vec<Suspension> = state_resume_op_edges(&ops)
        .into_iter()
        .map(|(suspend, resume, state)| Suspension {
            suspend,
            state,
            resume,
        })
        .collect();
    for (index, op) in ops.iter().enumerate() {
        assert!(
            !op.kind.is_suspend() || suspensions.iter().any(|s| s.suspend == index),
            "{function_name}: op#{index} {} has no statically known resume entry",
            op.kind
        );
    }
    for suspension in &suspensions {
        assert!(
            suspension.resume < ops.len() && ops[suspension.resume].kind == OpKind::StateLabel,
            "{function_name}: op#{} resumes at op#{}, which is not a state_label",
            suspension.suspend,
            suspension.resume
        );
    }
    if suspensions.is_empty() {
        ops[switch].state_targets = Some(Vec::new());
        ops[switch].task_closure_size = Some(base);
        return FrameSplit {
            ops,
            layout: Some(FrameLayout {
                spill_base: base,
                frame_size: base,
            }),
        };
    }

    let (live, liveness) = live_across_suspensions(&ops, &suspensions, params, function_name);
    // A name whose one definition is a constant moves into the prologue
    // instead of riding in a slot.
    let definitions = SimpleDefinitionFacts::compute(params, &ops);
    let constant_of = |name: &str| match definitions.unique_definition(name) {
        Some(SimpleDefinitionSite::Operation(index)) if is_rematerializable(&ops[index]) => {
            Some(index)
        }
        _ => None,
    };
    // A name the prologue defines once is defined again by every entry,
    // before the dispatch: a resume reads the value of its own entry.
    let prologue_defined = |name: &str| {
        matches!(
            definitions.unique_definition(name),
            Some(SimpleDefinitionSite::Operation(index)) if index < switch
        )
    };
    let live: BTreeMap<usize, (Vec<String>, Vec<usize>)> = live
        .into_iter()
        .map(|(resume, names)| {
            let (constants, spilled): (Vec<String>, Vec<String>) = names
                .into_iter()
                .filter(|name| !prologue_defined(name))
                .partition(|name| constant_of(name).is_some());
            let constants = constants
                .iter()
                .filter_map(|name| constant_of(name))
                .collect();
            (resume, (spilled, constants))
        })
        .collect();
    let slot_of: BTreeMap<String, i64> = live
        .values()
        .flat_map(|(spilled, _)| spilled.iter().cloned())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let offset = base + FRAME_WORD * i64::try_from(index).expect("frame slot index");
            (name, offset)
        })
        .collect();
    let frame_size = base + FRAME_WORD * i64::try_from(slot_of.len()).expect("frame slot count");

    let variables: BTreeSet<&str> = ops
        .iter()
        .filter(|op| op.kind == OpKind::StoreVar)
        .filter_map(|op| op.var.as_deref())
        .collect();
    let mut names = FreshNames::new(&ops);
    let mut before: BTreeMap<usize, Vec<OpIR>> = BTreeMap::new();
    let mut after: BTreeMap<usize, Vec<OpIR>> = BTreeMap::new();
    // A name with one definition that falls through is stored once, after the
    // definition and the exception checks that follow it.
    let mut stored_at_definition: BTreeSet<&str> = BTreeSet::new();
    let mut after_definition: BTreeMap<usize, Vec<OpIR>> = BTreeMap::new();
    for (name, &slot) in &slot_of {
        let Some(SimpleDefinitionSite::Operation(definition)) = definitions.unique_definition(name)
        else {
            continue;
        };
        let position = past_exception_checks(ops.as_slice(), definition);
        if !falls_through(&ops[position]) || position + 1 >= ops.len() {
            continue;
        }
        stored_at_definition.insert(name.as_str());
        let site = &ops[definition];
        let stores = store_name(site, name, slot, &variables, &mut names);
        after_definition.entry(position).or_default().extend(stores);
    }
    // Where several values die at one point (a handler entered from a
    // suspended call), they go newest first, as CPython unwinds its value
    // stack: the latest definition is released first.
    let mut last_definition: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, op) in ops.iter().enumerate() {
        visit_simple_ir_defined_names(op, |name| {
            if let Some((name, _)) = slot_of.get_key_value(name) {
                last_definition.insert(name.as_str(), index);
            }
        });
    }
    let mut releases: BTreeMap<usize, Vec<OpIR>> = slot_releases(&ops, &liveness, &slot_of)
        .into_iter()
        .map(|(position, released)| {
            let site = &ops[position];
            let mut released: Vec<&str> = released.into_iter().collect();
            released.sort_by_key(|name| std::cmp::Reverse(last_definition.get(name).copied()));
            let stores = released
                .into_iter()
                .map(|name| release_slot(site, slot_of[name]))
                .collect();
            (position, stores)
        })
        .collect();
    let mut by_resume: BTreeMap<usize, Vec<Suspension>> = BTreeMap::new();
    for suspension in &suspensions {
        by_resume
            .entry(suspension.resume)
            .or_default()
            .push(*suspension);
    }
    for (&resume, members) in &by_resume {
        let (spilled, _) = &live[&resume];
        if spilled.is_empty() {
            continue;
        }
        let site = &ops[members[0].suspend];
        let store_ops = |names: &mut FreshNames| {
            let mut out = Vec::new();
            for name in spilled
                .iter()
                .filter(|name| !stored_at_definition.contains(name.as_str()))
            {
                out.extend(store_name(site, name, slot_of[name], &variables, names));
            }
            out
        };
        // Every predecessor of the resume entry other than the dispatch.
        for member in members {
            if !ops[member.suspend].kind.is_repoll() {
                before
                    .entry(member.suspend)
                    .or_default()
                    .extend(store_ops(&mut names));
            }
        }
        if resume > 0 && falls_through(&ops[resume - 1]) {
            before
                .entry(resume)
                .or_default()
                .extend(store_ops(&mut names));
        }
        let label = ops[resume].value;
        if let Some(position) = ops.iter().position(|op| {
            matches!(op.kind, OpKind::Jump | OpKind::Goto | OpKind::BrIf) && op.value == label
        }) {
            panic!(
                "{function_name}: op#{position} branches to resume entry op#{resume}; \
                 resume entries are reached by fall-through or suspension only"
            );
        }
        let defined: Vec<&str> = spilled.iter().map(String::as_str).collect();
        let anchor = reload_anchor(&ops, resume, &defined);
        let reloads = after.entry(anchor).or_default();
        for name in spilled {
            let slot = slot_of[name];
            if variables.contains(name.as_str()) {
                let loaded = names.fresh();
                reloads.push(frame_op(
                    site,
                    OpIR {
                        args: Some(vec!["self".into()]),
                        value: Some(slot),
                        out: Some(loaded.clone()),
                        ..OpIR::new(OpKind::ClosureLoad)
                    },
                ));
                reloads.push(frame_op(
                    site,
                    OpIR {
                        var: Some(name.clone()),
                        args: Some(vec![loaded]),
                        ..OpIR::new(OpKind::StoreVar)
                    },
                ));
            } else {
                reloads.push(frame_op(
                    site,
                    OpIR {
                        args: Some(vec!["self".into()]),
                        value: Some(slot),
                        out: Some(name.clone()),
                        ..OpIR::new(OpKind::ClosureLoad)
                    },
                ));
            }
        }
    }

    // Constants live across a suspension move just before the dispatch.
    let hoisted: BTreeSet<usize> = live
        .values()
        .flat_map(|(_, constants)| constants.iter().copied())
        .filter(|&index| index > switch)
        .collect();
    if !hoisted.is_empty() {
        before
            .entry(switch)
            .or_default()
            .extend(hoisted.iter().map(|&index| ops[index].clone()));
    }
    ops[switch].task_closure_size = Some(frame_size);
    ops[switch].state_targets = Some(
        suspensions
            .iter()
            .map(|suspension| {
                let label = ops[suspension.resume]
                    .value
                    .expect("a state_label carries its state");
                (suspension.state, label)
            })
            .collect(),
    );
    if observe_async_work && !releases.is_empty() {
        preserve_source_latch_polls(&mut ops, &liveness, &releases);
    }
    let mut out = Vec::with_capacity(ops.len() + before.values().map(Vec::len).sum::<usize>());
    for (index, op) in ops.into_iter().enumerate() {
        if let Some(inserted) = before.remove(&index) {
            out.extend(inserted);
        }
        if hoisted.contains(&index) {
            continue;
        }
        out.push(op);
        if let Some(inserted) = after_definition.remove(&index) {
            out.extend(inserted);
        }
        if let Some(inserted) = after.remove(&index) {
            out.extend(inserted);
        }
        if let Some(inserted) = releases.remove(&index) {
            out.extend(inserted);
        }
    }
    FrameSplit {
        ops: out,
        layout: Some(FrameLayout {
            spill_base: base,
            frame_size,
        }),
    }
}

/// Liveness over the source graph, in which each suspension continues at its
/// resume entry.
struct SourceLiveness {
    cfg: CFG,
    facts: SimpleCfgLivenessFacts,
}

/// The names live on entry to each resume entry, in name order, excluding
/// invocation parameters: the poll's `self` is the activation itself.
fn live_across_suspensions(
    ops: &[OpIR],
    suspensions: &[Suspension],
    params: &[String],
    function_name: &str,
) -> (BTreeMap<usize, Vec<String>>, SourceLiveness) {
    let cfg = CFG::build(ops);
    let block_of = |op: usize| {
        cfg.blocks
            .iter()
            .find(|block| block.start_op <= op && op < block.end_op)
            .map(|block| block.id)
            .unwrap_or_else(|| panic!("{function_name}: op#{op} is outside the CFG"))
    };
    for suspension in suspensions {
        assert_eq!(
            cfg.blocks[block_of(suspension.resume)].start_op,
            suspension.resume,
            "{function_name}: resume entry op#{} must lead its block",
            suspension.resume
        );
    }
    let facts = analyze_simple_cfg_liveness_facts_with_cfg(ops, &cfg);
    let excluded: BTreeSet<&str> = params
        .iter()
        .map(String::as_str)
        .chain(["none", "self"])
        .collect();
    let live = suspensions
        .iter()
        .map(|suspension| {
            let block = block_of(suspension.resume);
            let mut names: Vec<String> = facts
                .names
                .materialize(&facts.live_in_by_block[block])
                .into_iter()
                .filter(|name| !excluded.contains(name.as_str()))
                .collect();
            names.sort();
            (suspension.resume, names)
        })
        .collect();
    (live, SourceLiveness { cfg, facts })
}

/// Where each spilled name dies, by the operation its release follows: the
/// operation that reads it last, when control falls from it, and the head of
/// a block the name is dead into although a predecessor holds it live (after
/// a resume entry's reload anchor, so a resumed frame is published first).
/// A head that defines the name or does not fall through gets no release;
/// the slot then keeps the value until it is stored again.
fn slot_releases<'a>(
    ops: &[OpIR],
    liveness: &SourceLiveness,
    slot_of: &'a BTreeMap<String, i64>,
) -> BTreeMap<usize, BTreeSet<&'a str>> {
    let facts = &liveness.facts;
    let spilled: BTreeMap<u32, &'a str> = slot_of
        .keys()
        .filter_map(|name| facts.names.id(name).map(|id| (id, name.as_str())))
        .collect();
    let defines = |op: &OpIR, name: &str| {
        let mut found = false;
        visit_simple_ir_defined_names(op, |defined| found |= defined == name);
        found
    };
    let mut releases: BTreeMap<usize, BTreeSet<&'a str>> = BTreeMap::new();
    for block in 0..facts.block_count() {
        facts.visit_block_backward(ops, block, |index, live_after| {
            if !falls_through(&ops[index]) {
                return;
            }
            visit_simple_ir_reads(&ops[index], |read| {
                if let Some(id) = facts.names.id(read.name)
                    && let Some(&name) = spilled.get(&id)
                    && !live_after.contains(id)
                    && !defines(&ops[index], name)
                {
                    releases
                        .entry(past_exception_checks(ops, index))
                        .or_default()
                        .insert(name);
                }
            });
        });
    }
    let successors = liveness_block_successors(&liveness.cfg);
    for (predecessor, targets) in successors.iter().enumerate() {
        for &block in targets {
            let (start, end) = facts.block_ops[block];
            if start == end {
                continue;
            }
            for (&id, &name) in &spilled {
                if !facts.live_out_by_block[predecessor].contains(id)
                    || facts.live_in_by_block[block].contains(id)
                {
                    continue;
                }
                let position = if ops[start].kind == OpKind::StateLabel {
                    reload_anchor(ops, start, &[])
                } else if falls_through(&ops[start]) && !defines(&ops[start], name) {
                    start
                } else {
                    continue;
                };
                releases
                    .entry(past_exception_checks(ops, position))
                    .or_default()
                    .insert(name);
            }
        }
    }
    releases
}

/// Preserve the source poll before a dying spill is released. Its finalizer
/// can run Python, so a post-split latch walk cannot cross the release. The
/// source CFG still identifies the exact successful check-to-continue edge.
fn preserve_source_latch_polls(
    ops: &mut [OpIR],
    liveness: &SourceLiveness,
    releases: &BTreeMap<usize, Vec<OpIR>>,
) {
    use molt_ir::tir::dominators::{SimpleExecutionDominance, SimpleProgramPoint};
    let mut execution = None;
    let cfg = &liveness.cfg;
    for &check in releases.keys() {
        let latch = check + 1;
        if ops[check].kind != OpKind::CheckException
            || ops[check].exception_edge != Some(ExceptionEdgeKind::Unwind)
            || ops[check].value.is_none()
            || ops
                .get(latch)
                .is_none_or(|op| op.kind != OpKind::LoopContinue)
        {
            continue;
        }
        let block = liveness.facts.block_for_op(latch);
        let [header] = cfg.successors[block].as_slice() else {
            continue;
        };
        if ops[cfg.blocks[*header].start_op].kind != OpKind::LoopStart {
            continue;
        }
        let points = execution.get_or_insert_with(|| SimpleExecutionDominance::compute(cfg, ops));
        if points.operation_is_reachable(latch)
            && points.definition_available(
                SimpleDefinitionSite::Operation(check),
                SimpleProgramPoint::Before(latch),
            )
        {
            ops[check].kind = OpKind::AsyncWorkPoll;
        }
    }
}

/// The last of the exception checks that follow `index`, or `index` itself.
/// A frame slot transfer goes after them: the checks consume the pending
/// exception of the operation they follow, and a release may run a finalizer.
fn past_exception_checks(ops: &[OpIR], index: usize) -> usize {
    let mut position = index;
    while position + 1 < ops.len()
        && matches!(
            ops[position + 1].kind,
            OpKind::CheckException | OpKind::AsyncWorkPoll
        )
    {
        position += 1;
    }
    position
}

/// Release a dead value's frame slot.
fn release_slot(site: &OpIR, slot: i64) -> OpIR {
    frame_op(
        site,
        OpIR {
            args: Some(vec!["self".into(), "none".into()]),
            value: Some(slot),
            frame_slot_release: Some(true),
            ..OpIR::new(OpKind::ClosureStore)
        },
    )
}

/// A constant the resume defines again: it reads nothing and owns no heap
/// object, so a second definition is the same value at no cost.
fn is_rematerializable(op: &OpIR) -> bool {
    matches!(
        op.kind,
        OpKind::Const | OpKind::ConstNone | OpKind::ConstBool | OpKind::ConstFloat
    ) && op.args.as_ref().is_none_or(Vec::is_empty)
}

/// Whether control falls from `op` into the operation that follows it.
fn falls_through(op: &OpIR) -> bool {
    !(op.kind.is_terminator()
        || (op.kind.is_suspend() && !op.kind.is_repoll())
        || matches!(op.kind, OpKind::Jump | OpKind::Goto))
}

/// Where the reloads of a resume entry go: after the entry's re-entered `try`
/// regions and, when it reads none of the reloaded names, after the frame
/// publication the frontend emits there, so a resumed frame is published
/// before any reload can release a value.
fn reload_anchor(ops: &[OpIR], resume: usize, defined: &[&str]) -> usize {
    let mut anchor = resume;
    while anchor + 1 < ops.len() && ops[anchor + 1].kind == OpKind::TryStart {
        anchor += 1;
    }
    let mut publication = anchor + 1;
    while publication < ops.len() && is_frame_publication_operand(ops[publication].kind) {
        publication += 1;
    }
    if publication < ops.len() && ops[publication].kind == OpKind::FrameContextSet {
        let defined: BTreeSet<&str> = defined.iter().copied().collect();
        let mut reads_defined = false;
        for op in &ops[anchor + 1..=publication] {
            visit_simple_ir_reads(op, |read| {
                reads_defined |= defined.contains(read.name);
            });
        }
        if !reads_defined {
            anchor = publication;
        }
    }
    anchor
}

/// Store `name`'s current value into its frame slot. A variable is read
/// through its binding first.
fn store_name(
    site: &OpIR,
    name: &str,
    slot: i64,
    variables: &BTreeSet<&str>,
    names: &mut FreshNames,
) -> Vec<OpIR> {
    let mut out = Vec::new();
    let value = if variables.contains(name) {
        let loaded = names.fresh();
        out.push(frame_op(
            site,
            OpIR {
                var: Some(name.to_owned()),
                out: Some(loaded.clone()),
                ..OpIR::new(OpKind::LoadVar)
            },
        ));
        loaded
    } else {
        name.to_owned()
    };
    out.push(frame_op(
        site,
        OpIR {
            args: Some(vec!["self".into(), value]),
            value: Some(slot),
            ..OpIR::new(OpKind::ClosureStore)
        },
    ));
    out
}

/// A frame slot transfer: it cannot raise, and it carries the suspension's
/// source site.
fn frame_op(site: &OpIR, mut op: OpIR) -> OpIR {
    op.exception_source = Some(ExceptionEdgeKind::Unwind);
    op.inherit_source_site_from(site);
    op
}

/// Names no operation of the function defines or reads.
struct FreshNames {
    taken: BTreeSet<String>,
    next: usize,
}

impl FreshNames {
    fn new(ops: &[OpIR]) -> Self {
        let mut taken = BTreeSet::new();
        for op in ops {
            taken.extend(op.out.iter().cloned());
            taken.extend(op.var.iter().cloned());
            taken.extend(op.args.iter().flatten().cloned());
        }
        Self { taken, next: 0 }
    }

    fn fresh(&mut self) -> String {
        loop {
            let name = format!("frame_split.{}", self.next);
            self.next += 1;
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The oracle is the activation's resume contract, written out by hand for
    //! each program: what must be in which frame slot when the poll returns at
    //! a suspension, and which names a resume entry must define again.
    use super::*;

    fn op(kind: OpKind) -> OpIR {
        OpIR::new(kind)
    }

    fn konst(out: &str, value: i64) -> OpIR {
        OpIR {
            out: Some(out.into()),
            value: Some(value),
            ..op(OpKind::Const)
        }
    }

    fn switch(frame: i64) -> OpIR {
        OpIR {
            value: Some(frame),
            ..op(OpKind::StateSwitch)
        }
    }

    fn yield_(value: &str, state: i64) -> OpIR {
        OpIR {
            args: Some(vec![value.into()]),
            value: Some(state),
            out: Some("none".into()),
            ..op(OpKind::StateYield)
        }
    }

    fn label(state: i64) -> OpIR {
        OpIR {
            value: Some(state),
            ..op(OpKind::StateLabel)
        }
    }

    fn ret(value: &str) -> OpIR {
        OpIR {
            args: Some(vec![value.into()]),
            ..op(OpKind::Ret)
        }
    }

    fn add(out: &str, left: &str, right: &str) -> OpIR {
        OpIR {
            args: Some(vec![left.into(), right.into()]),
            out: Some(out.into()),
            ..op(OpKind::Add)
        }
    }

    /// (kind, args, value, out, var) of each operation.
    fn shape(
        ops: &[OpIR],
    ) -> Vec<(
        OpKind,
        Vec<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
    )> {
        ops.iter()
            .map(|op| {
                (
                    op.kind,
                    op.args.clone().unwrap_or_default(),
                    op.value,
                    op.out.clone(),
                    op.var.clone(),
                )
            })
            .collect()
    }

    fn split(ops: Vec<OpIR>) -> FrameSplit {
        split_stateful_frame("poll", &["self".into()], ops, true)
    }

    #[test]
    fn a_value_used_after_a_resume_rides_in_a_slot_past_the_layout() {
        let out = split(vec![
            switch(16),
            konst("c", 5),
            add("a", "c", "c"),
            yield_("a", 1),
            label(1),
            ret("a"),
        ]);
        assert_eq!(out.layout.map(|layout| layout.frame_size), Some(24));
        let s = |x: &str| x.to_string();
        assert_eq!(
            shape(&out.ops),
            vec![
                (OpKind::StateSwitch, vec![], Some(16), None, None),
                (OpKind::Const, vec![], Some(5), Some(s("c")), None),
                (OpKind::Add, vec![s("c"), s("c")], None, Some(s("a")), None),
                (
                    OpKind::ClosureStore,
                    vec![s("self"), s("a")],
                    Some(16),
                    None,
                    None
                ),
                (
                    OpKind::StateYield,
                    vec![s("a")],
                    Some(1),
                    Some(s("none")),
                    None
                ),
                (OpKind::StateLabel, vec![], Some(1), None, None),
                (
                    OpKind::ClosureLoad,
                    vec![s("self")],
                    Some(16),
                    Some(s("a")),
                    None
                ),
                (OpKind::Ret, vec![s("a")], None, None, None),
            ]
        );
    }

    #[test]
    fn the_split_dispatch_maps_each_state_and_carries_both_sizes() {
        let out = split(vec![
            switch(16),
            konst("c", 5),
            add("a", "c", "c"),
            yield_("a", 1),
            label(1),
            ret("a"),
        ]);
        let dispatch = &out.ops[0];
        assert_eq!(dispatch.state_targets, Some(vec![(1, 1)]));
        assert_eq!(
            (dispatch.value, dispatch.task_closure_size),
            (Some(16), Some(24))
        );
        assert_eq!(
            out.layout,
            Some(FrameLayout {
                spill_base: 16,
                frame_size: 24
            })
        );
    }

    #[test]
    fn a_value_keeps_one_slot_and_is_stored_once_after_its_definition() {
        // `a` crosses both suspensions, `b` only the second: each has one
        // slot, and each is stored once, where it is defined.
        let out = split(vec![
            switch(8),
            konst("one", 1),
            add("a", "one", "one"),
            yield_("a", 1),
            label(1),
            add("b", "a", "a"),
            yield_("b", 2),
            label(2),
            add("c", "a", "b"),
            ret("c"),
        ]);
        assert_eq!(out.layout.map(|layout| layout.frame_size), Some(24));
        let stores: Vec<_> = out
            .ops
            .iter()
            .filter(|op| op.kind == OpKind::ClosureStore && !is_release(op))
            .map(|op| (op.args.clone().unwrap()[1].clone(), op.value.unwrap()))
            .collect();
        assert_eq!(stores, vec![("a".into(), 8), ("b".into(), 16)]);
        // `c` reads both for the last time: both slots are released there.
        let c_at = out
            .ops
            .iter()
            .position(|op| op.out.as_deref() == Some("c"))
            .unwrap();
        let released: Vec<_> = out.ops[c_at + 1..]
            .iter()
            .take_while(|op| is_release(op))
            .map(|op| op.value.unwrap())
            .collect();
        // `b` is the newer value, so its slot is released first, as CPython
        // unwinds its value stack newest first.
        assert_eq!(released, vec![16, 8]);
        let loads: Vec<_> = out
            .ops
            .iter()
            .filter(|op| op.kind == OpKind::ClosureLoad)
            .map(|op| (op.out.clone().unwrap(), op.value.unwrap()))
            .collect();
        assert_eq!(
            loads,
            vec![("a".into(), 8), ("a".into(), 8), ("b".into(), 16)]
        );
    }

    #[test]
    fn a_variable_crosses_through_its_own_binding_and_parameters_never_do() {
        let out = split(vec![
            switch(8),
            OpIR {
                var: Some("x".into()),
                args: Some(vec!["self".into()]),
                ..op(OpKind::StoreVar)
            },
            konst("k", 0),
            yield_("k", 1),
            label(1),
            OpIR {
                var: Some("x".into()),
                out: Some("y".into()),
                ..op(OpKind::LoadVar)
            },
            ret("y"),
        ]);
        assert_eq!(out.layout.map(|layout| layout.frame_size), Some(16));
        let kinds: Vec<_> = out.ops.iter().map(|op| op.kind).collect();
        assert_eq!(
            kinds,
            vec![
                OpKind::StateSwitch,
                OpKind::StoreVar,
                OpKind::LoadVar,
                OpKind::ClosureStore,
                OpKind::Const,
                OpKind::StateYield,
                OpKind::StateLabel,
                OpKind::ClosureLoad,
                OpKind::StoreVar,
                OpKind::LoadVar,
                OpKind::ClosureStore,
                OpKind::Ret,
            ]
        );
        assert!(
            is_release(&out.ops[10]),
            "the last read of `x` releases its slot"
        );
        assert_eq!(out.ops[8].var.as_deref(), Some("x"));
        assert!(
            out.ops
                .iter()
                .filter(|op| op.kind == OpKind::ClosureStore)
                .all(|op| op.args.as_ref().unwrap()[1] != "self")
        );
    }

    fn plain_label(id: i64) -> OpIR {
        OpIR {
            value: Some(id),
            ..op(OpKind::Label)
        }
    }

    fn jump(id: i64) -> OpIR {
        OpIR {
            value: Some(id),
            ..op(OpKind::Jump)
        }
    }

    fn store_var(var: &str, value: &str) -> OpIR {
        OpIR {
            var: Some(var.into()),
            args: Some(vec![value.into()]),
            ..op(OpKind::StoreVar)
        }
    }

    fn is_release(op: &OpIR) -> bool {
        op.kind == OpKind::ClosureStore && op.frame_slot_release == Some(true)
    }

    /// (operation index, slot) of each frame store that saves a value, in
    /// order; releases are not counted.
    fn stores(ops: &[OpIR]) -> Vec<(usize, i64)> {
        ops.iter()
            .enumerate()
            .filter(|(_, op)| op.kind == OpKind::ClosureStore && !is_release(op))
            .map(|(index, op)| (index, op.value.unwrap()))
            .collect()
    }

    #[test]
    fn a_loop_that_suspends_stores_a_value_defined_before_it_once() {
        // `it` is defined once, before the loop; the yield inside the loop
        // must not store it again on every iteration.
        let out = split(vec![
            switch(8),
            konst("one", 1),
            add("it", "one", "one"),
            plain_label(50),
            add("v", "it", "one"),
            yield_("v", 1),
            label(1),
            jump(50),
        ]);
        let stored = stores(&out.ops);
        assert_eq!(stored.len(), 1, "{:?}", shape(&out.ops));
        let (at, slot) = stored[0];
        assert_eq!(slot, 8);
        assert_eq!(out.ops[at - 1].out.as_deref(), Some("it"));
        assert_eq!(out.ops[at + 1].kind, OpKind::Label);
    }

    #[test]
    fn a_name_defined_twice_is_stored_at_each_suspension() {
        // `k` is assigned before the loop and again in it: its slot must hold
        // the value of the latest assignment when the frame suspends.
        let out = split(vec![
            switch(8),
            konst("zero", 0),
            store_var("k", "zero"),
            plain_label(50),
            OpIR {
                var: Some("k".into()),
                out: Some("k0".into()),
                ..op(OpKind::LoadVar)
            },
            add("k1", "k0", "k0"),
            store_var("k", "k1"),
            yield_("zero", 1),
            label(1),
            jump(50),
        ]);
        let yield_at = out
            .ops
            .iter()
            .position(|op| op.kind == OpKind::StateYield)
            .unwrap();
        assert_eq!(stores(&out.ops), vec![(yield_at - 1, 8)]);
        assert_eq!(out.ops[yield_at - 2].kind, OpKind::LoadVar);
        assert_eq!(out.ops[yield_at - 2].var.as_deref(), Some("k"));
        // Between its read and its next assignment `k` is dead: its slot is
        // released right after the read.
        let read = out
            .ops
            .iter()
            .position(|op| op.out.as_deref() == Some("k0"))
            .unwrap();
        assert!(is_release(&out.ops[read + 1]));
        assert_eq!(out.ops[read + 1].value, Some(8));
    }

    #[test]
    fn a_suspended_loop_services_its_authored_poll_before_releasing_a_dead_slot() {
        for (exit, observation) in [
            (OpKind::LoopContinue, OpKind::AsyncWorkPoll),
            (OpKind::LoopBreak, OpKind::CheckException),
        ] {
            let check = OpIR {
                value: Some(99),
                exception_edge: Some(ExceptionEdgeKind::Unwind),
                ..op(OpKind::CheckException)
            };
            let out = split(vec![
                switch(8),
                konst("k", 1),
                op(OpKind::LoopStart),
                add("spill", "k", "k"),
                yield_("k", 1),
                label(1),
                add("last_read", "spill", "k"),
                check,
                op(exit),
                op(OpKind::LoopEnd),
                plain_label(99),
                ret("k"),
            ]);
            let pos = out
                .ops
                .iter()
                .position(|op| op.kind == observation && op.value == Some(99))
                .unwrap();
            assert_eq!(out.ops[pos].exception_edge, Some(ExceptionEdgeKind::Unwind));
            assert!(
                is_release(&out.ops[pos + 1]),
                "slot finalization follows the original observation"
            );
            assert_eq!(out.ops[pos + 2].kind, exit);
        }
    }

    #[test]
    fn a_value_is_released_after_its_last_read() {
        // `a` crosses the yield and is read once after it: its slot is
        // released after that read, before the frame returns. CPython drops
        // the value there, so a finalizer runs there.
        let out = split(vec![
            switch(8),
            konst("one", 1),
            add("a", "one", "one"),
            yield_("one", 1),
            label(1),
            add("b", "a", "one"),
            yield_("b", 2),
            label(2),
            ret("one"),
        ]);
        let b_at = out
            .ops
            .iter()
            .position(|op| op.out.as_deref() == Some("b"))
            .unwrap();
        let release = &out.ops[b_at + 1];
        assert!(is_release(release), "{:?}", shape(&out.ops));
        assert_eq!(release.value, Some(8));
        assert_eq!(
            release.args.as_deref(),
            Some(&["self".to_string(), "none".to_string()][..])
        );
        // `a` is stored once, after its definition; `b` crosses nothing.
        assert_eq!(stores(&out.ops), vec![(3, 8)]);
    }

    #[test]
    fn a_value_dead_into_a_branch_is_released_at_its_head() {
        // `a` is read only on the `then` arm; the `else` arm releases it.
        let out = split(vec![
            switch(8),
            konst("one", 1),
            add("a", "one", "one"),
            yield_("one", 1),
            label(1),
            OpIR {
                args: Some(vec!["one".into()]),
                ..op(OpKind::If)
            },
            add("b", "a", "one"),
            ret("b"),
            op(OpKind::Else),
            ret("one"),
            op(OpKind::EndIf),
        ]);
        let else_at = out
            .ops
            .iter()
            .position(|op| op.kind == OpKind::Else)
            .unwrap();
        assert!(is_release(&out.ops[else_at + 1]), "{:?}", shape(&out.ops));
        assert_eq!(out.ops[else_at + 1].value, Some(8));
    }

    #[test]
    fn a_constant_live_across_a_suspension_moves_into_the_prologue() {
        // `one` is read after the resume. It needs no slot and no second
        // definition: it moves before the dispatch, which every entry runs.
        let out = split(vec![
            switch(8),
            konst("one", 1),
            konst("k", 7),
            yield_("k", 1),
            label(1),
            add("x", "one", "one"),
            ret("x"),
        ]);
        assert_eq!(
            out.layout.map(|layout| layout.frame_size),
            Some(8),
            "a constant needs no slot"
        );
        assert!(out.ops.iter().all(|op| op.kind != OpKind::ClosureStore));
        assert_eq!(out.ops[0].kind, OpKind::Const);
        assert_eq!(out.ops[0].out.as_deref(), Some("one"));
        assert_eq!(out.ops[1].kind, OpKind::StateSwitch);
        assert_eq!(
            out.ops
                .iter()
                .filter(|op| op.out.as_deref() == Some("one"))
                .count(),
            1,
            "one definition"
        );
    }

    #[test]
    fn a_prologue_value_is_defined_again_by_every_entry() {
        // `depth` is defined before the dispatch, so every entry defines it
        // again: no slot, no store, no reload.
        let out = split(vec![
            OpIR {
                out: Some("depth".into()),
                ..op(OpKind::ExceptionStackDepth)
            },
            switch(8),
            konst("k", 7),
            yield_("k", 1),
            label(1),
            ret("depth"),
        ]);
        assert_eq!(out.layout.map(|layout| layout.frame_size), Some(8));
        assert!(
            out.ops
                .iter()
                .all(|op| !matches!(op.kind, OpKind::ClosureStore | OpKind::ClosureLoad))
        );
    }

    #[test]
    fn a_lowered_dispatch_is_already_split() {
        let ops = vec![
            OpIR {
                state_targets: Some(vec![(1, 1)]),
                task_closure_size: Some(48),
                ..switch(40)
            },
            konst("a", 1),
            yield_("a", 1),
            label(1),
            ret("a"),
        ];
        let out = split(ops.clone());
        assert_eq!(out.layout.map(|layout| layout.frame_size), Some(48));
        assert_eq!(shape(&out.ops), shape(&ops));
    }

    #[test]
    fn a_function_without_dispatch_has_no_frame() {
        let ops = vec![konst("a", 1), ret("a")];
        let out = split(ops.clone());
        assert_eq!(out.layout.map(|layout| layout.frame_size), None);
        assert_eq!(shape(&out.ops), shape(&ops));
    }
}
