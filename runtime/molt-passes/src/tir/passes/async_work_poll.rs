//! Final placement of Python asynchronous-work/eval-breaker observations.
//!
//! The frontend's universal `CheckException` observations carry the exact
//! exceptional successor for their lexical region. This pass marks that
//! existing authority at every generated call-return site and every canonical
//! loop backedge. `check_exception_elim` must preserve marked observations.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ir::{FunctionIR, OpIR};
use crate::tir::analysis::{AnalysisManager, LoopForest};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::cfg::CFG;
use crate::tir::exception_regions::{
    ExceptionBoundaryHandler, ExceptionOpPosition, ExceptionRegionFacts, ExceptionRegions,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_requires_async_work_poll_after_table, simpleir_kind_is_call_graph_user_call,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::PassStats;
use super::check_exception_elim::classify::{
    const_int_values, op_clears_pending_exception, op_may_raise,
};

fn is_call_return_poll(op: &TirOp) -> bool {
    opcode_requires_async_work_poll_after_table(op.opcode)
        || (op.opcode == OpCode::Copy
            && matches!(
                op.attrs.get("_original_kind"),
                Some(AttrValue::Str(kind)) if simpleir_kind_is_call_graph_user_call(kind)
            ))
}

fn mark_poll(op: &mut TirOp) -> bool {
    op.mark_async_work_poll()
}

fn check_label(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::CheckException {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

const FINALLY_PENDING_OBSERVER: &str = "exception_finally_pending_observer";

fn is_deferred_finally_observer(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && matches!(
            op.attrs.get("_original_kind"),
            Some(AttrValue::Str(kind)) if kind == FINALLY_PENDING_OBSERVER
        )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PostCallObservation {
    Check(BlockId, usize),
    DeferredFinally(BlockId, usize),
}

/// Locate the frontend-authored exception observation for a call boundary.
///
/// Optimization and CFG construction may separate a call from its original
/// payload-bearing `CheckException` or split the observation into a unique
/// unconditional successor block. Traverse only operations the canonical
/// check-elimination oracle proves cannot raise or clear pending state, plus
/// unconditional fallthrough. Never cross another call, lexical transfer,
/// conditional edge, or cycle and incorrectly let one later check service two
/// semantic boundaries.
fn post_call_observation(
    func: &TirFunction,
    block_id: BlockId,
    call_index: usize,
    target: Option<i64>,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
) -> Option<PostCallObservation> {
    let mut current = block_id;
    let mut start = call_index + 1;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let block = func.blocks.get(&current)?;
        for (index, op) in block.ops.iter().enumerate().skip(start) {
            if op.opcode == OpCode::CheckException {
                if op.is_async_work_poll() && check_label(op).is_none() {
                    return Some(PostCallObservation::Check(current, index));
                }
                return check_label(op)
                    .filter(|label| target.is_none() || target == Some(*label))
                    .map(|_| PostCallObservation::Check(current, index));
            }
            if is_deferred_finally_observer(op) {
                return Some(PostCallObservation::DeferredFinally(current, index));
            }
            if crate::tir::dominators::is_exception_transfer_edge(op.opcode)
                || op_clears_pending_exception(op)
                || op_may_raise(value_types, const_ints, op)
            {
                return None;
            }
        }
        let Terminator::Branch { target, .. } = &block.terminator else {
            return None;
        };
        if predecessors.get(target).map(Vec::as_slice) != Some(&[current]) {
            return None;
        }
        current = *target;
        start = 0;
    }
}

/// Find the payload-bearing observation that services a loop backedge when
/// non-raising bookkeeping follows it in the latch block.
fn latch_check_site(
    func: &TirFunction,
    latch: BlockId,
    target: Option<i64>,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
) -> Option<(BlockId, usize)> {
    let mut current = latch;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let block = func.blocks.get(&current)?;
        for (index, op) in block.ops.iter().enumerate().rev() {
            if op.opcode == OpCode::CheckException {
                return check_label(op)
                    .filter(|label| target.is_none() || target == Some(*label))
                    .map(|_| (current, index));
            }
            if crate::tir::dominators::is_exception_transfer_edge(op.opcode)
                || op_clears_pending_exception(op)
                || op_may_raise(value_types, const_ints, op)
            {
                return None;
            }
        }
        let [predecessor] = predecessors.get(&current)?.as_slice() else {
            return None;
        };
        let Terminator::Branch { target, .. } = &func.blocks.get(predecessor)?.terminator else {
            return None;
        };
        if *target != current {
            return None;
        }
        current = *predecessor;
    }
}

/// Resolve a reachable insertion boundary. The outer `Option` is reachability;
/// the inner `Option` is depth zero versus a labeled lexical handler.
fn reachable_lexical_handler(
    facts: &ExceptionRegionFacts,
    function_name: &str,
    position: ExceptionOpPosition,
) -> Option<Option<i64>> {
    match facts.lexical_handler_before(position) {
        Ok(ExceptionBoundaryHandler::Unreachable) => None,
        Ok(ExceptionBoundaryHandler::DepthZero) => Some(None),
        Ok(ExceptionBoundaryHandler::Labeled(label)) => Some(Some(label)),
        Ok(ExceptionBoundaryHandler::Anonymous { destination, .. }) => Some(Some(destination)),
        Err(error) => {
            panic!(
                "async-work poll boundary in function {function_name:?} has invalid lexical custody: {error:?}"
            )
        }
    }
}

#[derive(Debug)]
struct PollPlacementPlan {
    call_sites: Vec<(BlockId, usize, Option<i64>)>,
    latch_sites: Vec<(BlockId, BTreeSet<BlockId>, Option<i64>)>,
    predecessors: HashMap<BlockId, Vec<BlockId>>,
    value_types: HashMap<ValueId, TirType>,
    const_ints: HashMap<ValueId, i64>,
}

fn placement_plan(func: &TirFunction, am: &mut AnalysisManager) -> PollPlacementPlan {
    let loops = am.get::<LoopForest>(func).clone();
    let region_facts = am.get::<ExceptionRegions>(func).clone();
    let predecessors = crate::tir::dominators::build_pred_map(func);
    let const_ints = const_int_values(func);
    let value_types = func.value_types.clone();
    let mut latches = BTreeSet::new();
    for header in loops.headers {
        let Some(body) = loops.bodies.get(&header) else {
            continue;
        };
        for &block_id in body {
            let Some(block) = func.blocks.get(&block_id) else {
                continue;
            };
            let mut reaches_header = false;
            block
                .terminator
                .for_each_edge(|target, _| reaches_header |= target == header);
            if reaches_header {
                latches.insert((block_id, header));
            }
        }
    }

    let call_sites = func
        .blocks
        .iter()
        .flat_map(|(&block_id, block)| {
            block
                .ops
                .iter()
                .enumerate()
                .filter_map(|(index, op)| {
                    if !is_call_return_poll(op) {
                        return None;
                    }
                    let position = ExceptionOpPosition {
                        block: block_id,
                        op_index: index + 1,
                    };
                    let target = reachable_lexical_handler(&region_facts, &func.name, position)?;
                    Some((block_id, index, target))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut latch_sites_by_block: BTreeMap<_, (BTreeSet<_>, Option<i64>)> = BTreeMap::new();
    for (latch, header) in latches {
        let position = ExceptionOpPosition {
            block: latch,
            op_index: func.blocks[&latch].ops.len(),
        };
        let Some(target) = reachable_lexical_handler(&region_facts, &func.name, position) else {
            continue;
        };
        let entry = latch_sites_by_block
            .entry(latch)
            .or_insert_with(|| (BTreeSet::new(), target));
        assert_eq!(
            entry.1, target,
            "one latch boundary cannot have header-dependent exception custody"
        );
        entry.0.insert(header);
    }
    let latch_sites = latch_sites_by_block
        .into_iter()
        .map(|(latch, (headers, target))| (latch, headers, target))
        .collect();

    PollPlacementPlan {
        call_sites,
        latch_sites,
        predecessors,
        value_types,
        const_ints,
    }
}

fn observation_is_marked(func: &TirFunction, observation: PostCallObservation) -> bool {
    let (block, index) = match observation {
        PostCallObservation::Check(block, index)
        | PostCallObservation::DeferredFinally(block, index) => (block, index),
    };
    func.blocks[&block].ops[index].is_async_work_poll()
}

/// Whether every target-required call and loop boundary already owns its final
/// marked observation. Module transforms consume only this post-pipeline form;
/// refusing unprepared input before mutation keeps SSA payload construction in
/// the pre-SSA lowering authority.
pub(crate) fn is_materialized(func: &TirFunction) -> bool {
    let mut analyses = AnalysisManager::new();
    let plan = placement_plan(func, &mut analyses);
    let calls_ready = plan.call_sites.iter().all(|(block, index, target)| {
        post_call_observation(
            func,
            *block,
            *index,
            *target,
            &plan.predecessors,
            &plan.value_types,
            &plan.const_ints,
        )
        .is_some_and(|observation| observation_is_marked(func, observation))
    });
    calls_ready
        && plan.latch_sites.iter().all(|(latch, _, target)| {
            latch_check_site(
                func,
                *latch,
                *target,
                &plan.predecessors,
                &plan.value_types,
                &plan.const_ints,
            )
            .is_some_and(|(block, index)| func.blocks[&block].ops[index].is_async_work_poll())
        })
}

/// Observations whose asynchronous-work role belongs exclusively to one loop.
/// A shared call-return or surviving-loop role cannot be retired. Observation
/// sites may precede their latch in a unique fallthrough predecessor block.
/// These facts authorize clearing the marker, never deleting the synchronous
/// exception check or its payload-bearing edge.
pub(crate) fn loop_only_poll_sites(
    func: &TirFunction,
    retired_header: BlockId,
) -> BTreeSet<(BlockId, usize)> {
    let mut analyses = AnalysisManager::new();
    let plan = placement_plan(func, &mut analyses);
    let call_observations: BTreeSet<_> = plan
        .call_sites
        .iter()
        .filter_map(|(block, index, target)| {
            match post_call_observation(
                func,
                *block,
                *index,
                *target,
                &plan.predecessors,
                &plan.value_types,
                &plan.const_ints,
            ) {
                Some(PostCallObservation::Check(block, index)) => Some((block, index)),
                _ => None,
            }
        })
        .collect();
    let mut observation_headers: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for (latch, headers, target) in &plan.latch_sites {
        if let Some(site) = latch_check_site(
            func,
            *latch,
            *target,
            &plan.predecessors,
            &plan.value_types,
            &plan.const_ints,
        ) {
            observation_headers
                .entry(site)
                .or_default()
                .extend(headers.iter().copied());
        }
    }
    observation_headers
        .into_iter()
        .filter(|(site, headers)| {
            func.blocks[&site.0].ops[site.1].is_async_work_poll()
                && !call_observations.contains(site)
                && headers.len() == 1
                && headers.contains(&retired_header)
        })
        .map(|(site, _)| site)
        .collect()
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct PreSsaMaterialization {
    pub(crate) markers_changed: usize,
    pub(crate) transfers_inserted: usize,
}

fn latch_insertion_index(ir: &FunctionIR, cfg: &CFG, latch: BlockId) -> usize {
    let block = cfg
        .blocks
        .get(latch.0 as usize)
        .unwrap_or_else(|| panic!("async-work latch {latch} has no SimpleIR block"));
    if block.start_op == block.end_op {
        return block.end_op;
    }
    let last = block.end_op - 1;
    if crate::tir::is_structural(&ir.ops[last].kind) {
        last
    } else {
        block.end_op
    }
}

/// Materialize every missing asynchronous-work observation in SimpleIR before
/// SSA conversion. This is the sole insertion authority: SSA already owns the
/// handler environment and therefore authors payload operands exactly once.
///
/// A call inside generated `finally` arbitration is different: its pending
/// exception is intentionally consumed by `exception_finally_pending_observer`
/// so replacement and `__context__` chaining can complete. Such a boundary gets
/// a branchless poll; every other missing boundary gets an explicit transfer.
///
/// `preview` is a placement-only lift whose source indices are physical
/// positions in this exact `ir` stream. Durable transported source provenance
/// is restored by the owning lowerer after materialization.
pub(crate) fn materialize_before_ssa(
    ir: &mut FunctionIR,
    preview: &mut TirFunction,
) -> PreSsaMaterialization {
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    enum Target {
        Handler(i64),
        FunctionExit,
    }

    let mut am = AnalysisManager::new();
    let plan = placement_plan(preview, &mut am);
    let simple_cfg = CFG::build(&ir.ops);
    let mut insertions: BTreeMap<usize, Target> = BTreeMap::new();
    let mut markers_changed = 0;

    let mut record = |index: usize, target: Target| {
        if let Some(previous) = insertions.insert(index, target) {
            assert_eq!(
                previous, target,
                "one async-work boundary cannot have conflicting exception custody"
            );
        }
    };

    for (block, index, target) in &plan.call_sites {
        match post_call_observation(
            preview,
            *block,
            *index,
            *target,
            &plan.predecessors,
            &plan.value_types,
            &plan.const_ints,
        ) {
            Some(PostCallObservation::Check(..)) => {}
            Some(PostCallObservation::DeferredFinally(observer_block, observer_index)) => {
                let prepared_index = preview.blocks[&observer_block].ops[observer_index]
                    .source_op_index()
                    .expect("deferred-finally observer lost its prepared SimpleIR position");
                assert_eq!(
                    ir.ops[prepared_index].kind, FINALLY_PENDING_OBSERVER,
                    "deferred-finally observer source kind drifted before SSA materialization"
                );
                if !ir.ops[prepared_index].async_work_poll {
                    ir.ops[prepared_index].async_work_poll = true;
                    let observer =
                        &mut preview.blocks.get_mut(&observer_block).unwrap().ops[observer_index];
                    assert!(
                        mark_poll(observer),
                        "SimpleIR and preview TIR async-work markers must change together"
                    );
                    markers_changed += 1;
                }
            }
            None => {
                let prepared_index = preview.blocks[block].ops[*index]
                    .source_op_index()
                    .expect("async-work call lost its prepared SimpleIR position");
                record(
                    prepared_index + 1,
                    target.map_or(Target::FunctionExit, Target::Handler),
                );
            }
        }
    }
    for (latch, _, target) in &plan.latch_sites {
        if latch_check_site(
            preview,
            *latch,
            *target,
            &plan.predecessors,
            &plan.value_types,
            &plan.const_ints,
        )
        .is_none()
        {
            record(
                latch_insertion_index(ir, &simple_cfg, *latch),
                target.map_or(Target::FunctionExit, Target::Handler),
            );
        }
    }

    if insertions.is_empty() {
        return PreSsaMaterialization {
            markers_changed,
            transfers_inserted: 0,
        };
    }
    let function_exit = insertions
        .values()
        .any(|target| *target == Target::FunctionExit)
        .then(|| crate::tir::clone_support::LabelAllocator::for_simple_ir(ir).fresh());
    let insertion_count = insertions.len();
    let original_ops = std::mem::take(&mut ir.ops);
    let original_len = original_ops.len();
    let mut insertion_iter = insertions.into_iter().peekable();
    let mut materialized_ops = Vec::with_capacity(
        original_len + insertion_count + if function_exit.is_some() { 2 } else { 0 },
    );
    let materialized_poll = |target| OpIR {
        kind: "async_work_poll".into(),
        value: match target {
            Target::Handler(label) => Some(label),
            Target::FunctionExit => function_exit,
        },
        ..OpIR::default()
    };
    for (index, op) in original_ops.into_iter().enumerate() {
        if insertion_iter.peek().is_some_and(|(at, _)| *at == index) {
            let (_, target) = insertion_iter.next().expect("peeked insertion");
            materialized_ops.push(materialized_poll(target));
        }
        materialized_ops.push(op);
    }
    if insertion_iter
        .peek()
        .is_some_and(|(at, _)| *at == original_len)
    {
        let (_, target) = insertion_iter.next().expect("peeked terminal insertion");
        materialized_ops.push(materialized_poll(target));
    }
    assert!(
        insertion_iter.next().is_none(),
        "async-work insertion index exceeds SimpleIR length"
    );
    if let Some(label) = function_exit {
        materialized_ops.push(OpIR {
            kind: "label".into(),
            value: Some(label),
            ..OpIR::default()
        });
        materialized_ops.push(OpIR {
            kind: "ret_void".into(),
            ..OpIR::default()
        });
    }
    ir.ops = materialized_ops;
    PreSsaMaterialization {
        markers_changed,
        transfers_inserted: insertion_count,
    }
}

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "async_work_poll",
        ..Default::default()
    };

    let plan = placement_plan(func, am);
    for (block_id, index, target) in plan.call_sites {
        let observation = post_call_observation(
            func,
            block_id,
            index,
            target,
            &plan.predecessors,
            &plan.value_types,
            &plan.const_ints,
        );
        if let Some(PostCallObservation::Check(existing_block, existing_index)) = observation {
            let block = func.blocks.get_mut(&existing_block).unwrap();
            stats.attrs_changed += usize::from(mark_poll(&mut block.ops[existing_index]));
            continue;
        }
        if let Some(PostCallObservation::DeferredFinally(observer_block, observer_index)) =
            observation
        {
            let observer = &mut func.blocks.get_mut(&observer_block).unwrap().ops[observer_index];
            stats.attrs_changed += usize::from(mark_poll(observer));
            continue;
        }
        panic!(
            "async-work post-call {block_id} op#{index} in function {:?} has no pre-SSA-authored exception transfer; materialize async-work boundaries before SSA conversion",
            func.name
        );
    }

    for (latch, _headers, target) in plan.latch_sites {
        let existing_site = latch_check_site(
            func,
            latch,
            target,
            &plan.predecessors,
            &plan.value_types,
            &plan.const_ints,
        );
        if let Some((existing_block, existing_index)) = existing_site {
            let check = &mut func.blocks.get_mut(&existing_block).unwrap().ops[existing_index];
            stats.attrs_changed += usize::from(mark_poll(check));
            continue;
        }
        panic!(
            "async-work loop latch {latch} in function {:?} has no pre-SSA-authored exception transfer; materialize async-work boundaries before SSA conversion",
            func.name
        );
    }

    if stats.total_changes() != 0 {
        func.has_exception_handling = true;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    fn op(opcode: OpCode) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn labeled_op(opcode: OpCode, label: i64) -> TirOp {
        let mut op = op(opcode);
        op.attrs.insert("value".into(), AttrValue::Int(label));
        op
    }

    fn check(label: i64) -> TirOp {
        labeled_op(OpCode::CheckException, label)
    }

    fn exception_pop() -> TirOp {
        let mut op = op(OpCode::Copy);
        op.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("exception_pop".into()),
        );
        op
    }

    fn deferred_finally_payload_ir() -> FunctionIR {
        FunctionIR {
            name: "deferred_finally_payload".into(),
            params: vec![
                "__molt_closure__".into(),
                "self".into(),
                "exception_stack_token".into(),
                "exception_stack_depth".into(),
            ],
            ops: vec![
                OpIR {
                    kind: "try_start".into(),
                    value: Some(93),
                    ..Default::default()
                },
                OpIR {
                    kind: "try_start".into(),
                    value: Some(96),
                    ..Default::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    value: Some(96),
                    ..Default::default()
                },
                OpIR {
                    kind: "jump".into(),
                    value: Some(97),
                    ..Default::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(96),
                    ..Default::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    value: Some(96),
                    ..Default::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(97),
                    ..Default::default()
                },
                OpIR {
                    kind: "call".into(),
                    s_value: Some("cleanup".into()),
                    out: Some("cleanup_result".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: FINALLY_PENDING_OBSERVER.into(),
                    out: Some("cleanup_pending".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "const_none".into(),
                    out: Some("none".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "is".into(),
                    args: Some(vec!["cleanup_pending".into(), "none".into()]),
                    out: Some("cleanup_succeeded".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "if".into(),
                    args: Some(vec!["cleanup_succeeded".into()]),
                    ..Default::default()
                },
                OpIR {
                    kind: "const_none".into(),
                    out: Some("selected_pending".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "else".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "copy_var".into(),
                    var: Some("cleanup_pending".into()),
                    out: Some("selected_pending".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "end_if".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(93),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(93),
                    ..Default::default()
                },
                OpIR {
                    kind: "build_tuple".into(),
                    args: Some(vec![
                        "__molt_closure__".into(),
                        "self".into(),
                        "exception_stack_token".into(),
                        "exception_stack_depth".into(),
                    ]),
                    out: Some("payload".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["payload".into()]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn deferred_finally_poll_preserves_one_observer_and_four_value_handler_payload() {
        let mut ir = deferred_finally_payload_ir();

        let mut preview = crate::tir::lower_from_simple::lower_to_tir(&ir);
        let materialized = materialize_before_ssa(&mut ir, &mut preview);
        assert_eq!(materialized.markers_changed, 1);
        assert_eq!(
            materialized.transfers_inserted, 0,
            "the existing observer must avoid a second SSA lowering"
        );
        assert_eq!(ir.ops[7].kind, "call");
        assert_eq!(ir.ops[8].kind, FINALLY_PENDING_OBSERVER);
        assert!(ir.ops[8].async_work_poll);

        let observer = preview
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| is_deferred_finally_observer(op))
            .expect("existing finally-pending observer");
        assert!(observer.is_async_work_poll());
        let call_block = preview
            .blocks
            .values()
            .find(|block| block.ops.iter().any(|op| op.opcode == OpCode::Call))
            .expect("cleanup call block");
        assert!(
            call_block
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::CheckException),
            "the poll must continue through finally arbitration instead of branching directly"
        );
        let handler = preview
            .label_id_map
            .iter()
            .find_map(|(block, label)| (*label == 93).then_some(BlockId(*block)))
            .expect("outer handler block");
        assert_eq!(
            preview.blocks[&handler].args.len(),
            4,
            "SSA must remain the sole authority for the four-value handler payload"
        );

        let round_trip = crate::tir::lower_to_simple::lower_to_simple_ir(&preview);
        let observer = round_trip
            .iter()
            .find(|op| op.kind == FINALLY_PENDING_OBSERVER)
            .expect("observer survives TIR to SimpleIR");
        assert!(observer.async_work_poll, "semantic marker must round-trip");
    }

    #[test]
    fn target_lowering_separates_finally_observer_position_from_durable_provenance() {
        let mut ir = deferred_finally_payload_ir();
        let observer_index = ir
            .ops
            .iter()
            .position(|op| op.kind == FINALLY_PENDING_OBSERVER)
            .expect("fixture observer");
        ir.ops[observer_index].source_op_idx = Some(0);
        assert_ne!(ir.ops[0].kind, FINALLY_PENDING_OBSERVER);

        let tir = crate::tir::lower_from_simple::lower_to_tir_for_target(
            &ir,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        let observer = tir
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| is_deferred_finally_observer(op))
            .expect("existing finally-pending observer");
        assert!(observer.is_async_work_poll());
        assert_eq!(observer.source_op_index(), Some(0));

        let round_trip = crate::tir::lower_to_simple::lower_to_simple_ir(&tir);
        let observer = round_trip
            .iter()
            .find(|op| op.kind == FINALLY_PENDING_OBSERVER)
            .expect("observer survives target-aware TIR round trip");
        assert!(observer.async_work_poll);
        assert_eq!(observer.source_op_idx, Some(0));
        assert!(
            round_trip
                .iter()
                .filter(|op| op.kind != FINALLY_PENDING_OBSERVER)
                .all(|op| !op.async_work_poll),
            "stale provenance must not mark an unrelated current-stream operation"
        );
    }

    #[test]
    fn target_lowering_places_call_poll_after_pre_ssa_loop_rewrite() {
        let call = OpIR {
            kind: "call".into(),
            s_value: Some("work".into()),
            out: Some("result".into()),
            source_op_idx: Some(0),
            ..OpIR::default()
        };
        let ir = FunctionIR {
            name: "rewritten_loop_call".into(),
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("initial".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_index_start".into(),
                    args: Some(vec!["initial".into()]),
                    out: Some("index".into()),
                    ..OpIR::default()
                },
                call,
                OpIR {
                    kind: "loop_index_next".into(),
                    args: Some(vec!["index".into()]),
                    out: Some("index".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };

        let tir = crate::tir::lower_from_simple::lower_to_tir_for_target(
            &ir,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        let round_trip = crate::tir::lower_to_simple::lower_to_simple_ir(&tir);
        let call_index = round_trip
            .iter()
            .position(|op| op.kind == "call")
            .expect("call survives target-aware lowering");
        assert_eq!(round_trip[call_index].source_op_idx, Some(0));
        assert_eq!(round_trip[call_index + 1].kind, "async_work_poll");
    }

    #[test]
    fn loop_latch_poll_is_materialized_before_ssa_with_handler_payload() {
        let mut ir = FunctionIR {
            name: "payload_loop".into(),
            params: vec!["payload".into()],
            ops: vec![
                OpIR {
                    kind: "try_start".into(),
                    value: Some(40),
                    ..Default::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "copy_var".into(),
                    var: Some("payload".into()),
                    out: Some("iteration_value".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "try_end".into(),
                    value: Some(40),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..Default::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(40),
                    ..Default::default()
                },
                OpIR {
                    kind: "copy_var".into(),
                    var: Some("payload".into()),
                    out: Some("handler_value".into()),
                    ..Default::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut preview = crate::tir::lower_from_simple::lower_to_tir(&ir);
        let materialized = materialize_before_ssa(&mut ir, &mut preview);
        assert_eq!(materialized.markers_changed, 0);
        assert_eq!(materialized.transfers_inserted, 1);
        let inserted = ir
            .ops
            .windows(2)
            .find(|ops| ops[1].kind == "loop_continue")
            .map(|ops| &ops[0])
            .expect("poll immediately before loop latch transfer");
        assert_eq!(inserted.kind, "async_work_poll");
        assert_eq!(inserted.value, Some(40));

        let mut lowered = crate::tir::lower_from_simple::lower_to_tir(&ir);
        let poll = lowered
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.is_async_work_poll())
            .expect("marked latch transfer");
        assert_eq!(poll.operands.len(), 1);
        let handler = lowered
            .label_id_map
            .iter()
            .find_map(|(block, label)| (*label == 40).then_some(BlockId(*block)))
            .expect("payload handler");
        assert_eq!(lowered.blocks[&handler].args.len(), 1);
        run(&mut lowered, &mut AnalysisManager::new());
        crate::tir::verify::verify_function(&lowered)
            .expect("payload-bearing latch poll must remain valid SSA");
    }

    #[test]
    fn generated_call_returns_and_loop_backedges_share_one_poll_marker() {
        let mut func = TirFunction::new("polls".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = BlockId(1);
        func.next_block = 2;
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.get_mut(&header).unwrap().ops = vec![op(OpCode::Call), check(70)];
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![check(70)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.attrs_changed, 2);
        for block in func.blocks.values() {
            for check in block
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::CheckException)
            {
                assert!(check.is_async_work_poll());
            }
        }
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert_eq!(
            simple
                .iter()
                .filter(|op| op.kind == "async_work_poll")
                .count(),
            2,
            "the canonical wire spelling must preserve both generated sites"
        );
    }

    #[test]
    fn refcount_bookkeeping_preserves_the_ssa_authored_poll_payload() {
        let mut func = TirFunction::new(
            "call_cleanup_poll".into(),
            vec![TirType::DynBox],
            TirType::None,
        );
        let entry = func.entry_block;
        let handler = func.fresh_block();
        let handler_arg = func.fresh_value();
        func.value_types.insert(handler_arg, TirType::DynBox);
        func.label_id_map.insert(handler.0, 70);
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let mut start = labeled_op(OpCode::TryStart, 70);
        start.operands.push(ValueId(0));
        let mut original_check = check(70);
        original_check.operands.push(ValueId(0));
        let mut call = op(OpCode::Call);
        call.operands.push(ValueId(0));
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            start,
            call,
            original_check,
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::DecRef,
                operands: vec![ValueId(0)],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            },
        ];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.ops_added, 0, "must reuse the SSA-authored edge");
        assert_eq!(stats.attrs_changed, 1);
        assert_eq!(func.blocks[&entry].ops.len(), 4);
        let poll = &func.blocks[&entry].ops[2];
        assert!(poll.is_async_work_poll());
        assert_eq!(poll.operands, [ValueId(0)]);
        crate::tir::verify::verify_function(&func)
            .expect("payload-preserving poll must remain well formed");
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        let poll_index = simple
            .iter()
            .position(|op| op.kind == "async_work_poll" && op.value == Some(70))
            .expect("lowered async-work poll");
        let handler_slot = format!("_bb{}_arg0", handler.0);
        assert!(
            simple[..poll_index].iter().any(|op| {
                op.kind == "store_var" && op.var.as_deref() == Some(handler_slot.as_str())
            }),
            "lowering must materialize the preserved payload before the poll: {simple:?}"
        );
    }

    #[test]
    fn poll_lookup_never_crosses_a_second_call() {
        let mut func = TirFunction::new("two_call_barrier".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let mut first = op(OpCode::Call);
        first.operands.push(ValueId(0));
        let mut second = op(OpCode::Call);
        second.operands.push(ValueId(0));
        func.blocks.get_mut(&entry).unwrap().ops = vec![first, second, check(70)];
        let value_types = func.value_types.clone();
        let const_ints = const_int_values(&func);
        let predecessors = crate::tir::dominators::build_pred_map(&func);
        assert_eq!(
            post_call_observation(
                &func,
                entry,
                0,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            None,
            "a later call is a hard semantic boundary"
        );
        assert_eq!(
            post_call_observation(
                &func,
                entry,
                1,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            Some(PostCallObservation::Check(entry, 2))
        );
    }

    #[test]
    fn poll_lookup_follows_only_unique_empty_fallthrough_blocks() {
        let mut func = TirFunction::new("split_call_boundary".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let observer = func.fresh_block();
        func.blocks.get_mut(&entry).unwrap().ops = vec![op(OpCode::Call)];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: observer,
            args: vec![],
        };
        func.blocks.insert(
            observer,
            TirBlock {
                id: observer,
                args: vec![],
                ops: vec![check(70)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let value_types = func.value_types.clone();
        let const_ints = const_int_values(&func);
        let predecessors = crate::tir::dominators::build_pred_map(&func);
        assert_eq!(
            post_call_observation(
                &func,
                entry,
                0,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            Some(PostCallObservation::Check(observer, 0))
        );

        func.blocks
            .get_mut(&observer)
            .unwrap()
            .ops
            .insert(0, op(OpCode::Call));
        assert_eq!(
            post_call_observation(
                &func,
                entry,
                0,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            None,
            "a call in the successor is a hard boundary"
        );

        let other = func.fresh_block();
        func.blocks.insert(
            other,
            TirBlock {
                id: other,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: observer,
                    args: vec![],
                },
            },
        );
        func.blocks.get_mut(&observer).unwrap().ops.remove(0);
        let predecessors = crate::tir::dominators::build_pred_map(&func);
        assert_eq!(
            post_call_observation(
                &func,
                entry,
                0,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            None,
            "a shared successor observation cannot belong to one incoming call boundary"
        );
    }

    #[test]
    fn latch_lookup_reuses_payload_check_before_nonraising_transport_suffix() {
        let mut func =
            TirFunction::new("latch_suffix".into(), vec![TirType::DynBox], TirType::None);
        let latch = func.entry_block;
        let mut transport = op(OpCode::Copy);
        transport
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("store_var".into()));
        let source = ValueId(0);
        let copied = func.fresh_value();
        transport.operands = vec![source];
        transport.results = vec![copied];
        func.blocks.get_mut(&latch).unwrap().ops = vec![check(70), transport];
        func.blocks.get_mut(&latch).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![source],
        };
        let predecessors = crate::tir::dominators::build_pred_map(&func);
        let value_types = func.value_types.clone();
        let const_ints = const_int_values(&func);
        assert_eq!(
            latch_check_site(
                &func,
                latch,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            Some((latch, 0))
        );

        func.blocks
            .get_mut(&latch)
            .unwrap()
            .ops
            .push(op(OpCode::Call));
        assert_eq!(
            latch_check_site(
                &func,
                latch,
                Some(70),
                &predecessors,
                &value_types,
                &const_ints,
            ),
            None,
            "a raising call after the check is a hard latch boundary"
        );
    }

    #[test]
    #[should_panic(expected = "has no pre-SSA-authored exception transfer")]
    fn post_ssa_loop_without_transfer_fails_closed() {
        let mut func = TirFunction::new("synthetic_loop".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = BlockId(1);
        func.next_block = 2;
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
    }

    #[test]
    fn nested_try_call_uses_inner_lexical_handler_not_a_later_outer_check() {
        let mut func = TirFunction::new("nested_try".into(), vec![], TirType::None);
        let entry = func.entry_block;
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            labeled_op(OpCode::TryStart, 10),
            labeled_op(OpCode::TryStart, 20),
            op(OpCode::Call),
            check(20),
            check(10),
        ];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

        run(&mut func, &mut AnalysisManager::new());
        let checks: Vec<_> = func.blocks[&entry]
            .ops
            .iter()
            .skip(3)
            .filter_map(check_label)
            .collect();
        assert_eq!(checks[0], 20, "poll must target the inner lexical handler");
        assert_eq!(checks[1..], [10]);
    }

    #[test]
    fn same_block_try_transition_routes_each_call_from_its_exact_boundary() {
        let mut func = TirFunction::new("try_transition".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let exit = func.fresh_block();
        func.label_id_map.insert(exit.0, 31);
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            labeled_op(OpCode::TryStart, 30),
            op(OpCode::Call),
            check(30),
            labeled_op(OpCode::TryEnd, 30),
            op(OpCode::Call),
            check(31),
        ];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        let polls: Vec<_> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.is_async_work_poll())
            .filter_map(check_label)
            .collect();
        assert_eq!(polls.len(), 2);
        assert_eq!(polls[0], 30);
        assert_eq!(polls[1], 31, "depth-zero call must use the function exit");
    }

    #[test]
    fn anonymous_try_call_uses_its_recovered_handler_destination() {
        let mut func = TirFunction::new("anonymous_try_call".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 73);
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            op(OpCode::TryStart),
            op(OpCode::Call),
            check(73),
            op(OpCode::TryEnd),
        ];
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![op(OpCode::TryEnd)],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        let poll = func.blocks[&entry]
            .ops
            .iter()
            .find(|op| op.is_async_work_poll())
            .expect("anonymous-region call poll");
        assert_eq!(check_label(poll), Some(73));
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert!(crate::tir::lower_to_simple::validate_labels(&simple));
        assert!(
            simple
                .iter()
                .any(|op| { op.kind == "async_work_poll" && op.value == Some(73) })
        );
    }

    #[test]
    fn anonymous_try_loop_latch_uses_its_recovered_handler_destination() {
        let mut func = TirFunction::new("anonymous_try_loop".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = func.fresh_block();
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 74);
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.get_mut(&header).unwrap().ops = vec![op(OpCode::TryStart), check(74)];
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![check(74)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![op(OpCode::TryEnd)],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        assert_eq!(
            func.blocks[&latch].ops.last().and_then(check_label),
            Some(74)
        );
        assert!(func.blocks[&latch].ops.last().unwrap().is_async_work_poll());
    }

    #[test]
    fn nested_anonymous_try_calls_keep_inner_and_outer_destinations() {
        let mut func = TirFunction::new("nested_anonymous_try".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let outer_handler = func.fresh_block();
        let inner_handler = func.fresh_block();
        func.label_id_map.insert(outer_handler.0, 80);
        func.label_id_map.insert(inner_handler.0, 81);
        func.blocks.get_mut(&entry).unwrap().ops = vec![
            op(OpCode::TryStart),
            check(80),
            op(OpCode::Call),
            check(80),
            op(OpCode::TryStart),
            check(81),
            op(OpCode::Call),
            check(81),
            op(OpCode::TryEnd),
            op(OpCode::Call),
            check(80),
            op(OpCode::TryEnd),
        ];
        for (handler, label) in [(outer_handler, 80), (inner_handler, 81)] {
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![labeled_op(OpCode::TryEnd, label)],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }

        run(&mut func, &mut AnalysisManager::new());
        let polls: Vec<_> = func.blocks[&entry]
            .ops
            .iter()
            .filter(|op| op.is_async_work_poll())
            .filter_map(check_label)
            .collect();
        assert_eq!(polls, [80, 81, 80]);
    }

    #[test]
    fn loop_backedge_inside_try_uses_the_active_handler() {
        let mut func = TirFunction::new("try_loop".into(), vec![], TirType::None);
        let header = func.entry_block;
        let latch = func.fresh_block();
        func.blocks.get_mut(&header).unwrap().ops = vec![labeled_op(OpCode::TryStart, 40)];
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: latch,
            args: vec![],
        };
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![check(40)],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        assert_eq!(func.blocks[&latch].ops.len(), 1);
        assert_eq!(check_label(&func.blocks[&latch].ops[0]), Some(40));
    }

    #[test]
    fn one_latch_for_nested_loop_headers_gets_one_poll() {
        use crate::tir::values::ValueId;

        let mut func = TirFunction::new("multi_header_latch".into(), vec![], TirType::None);
        let outer = func.entry_block;
        let inner = func.fresh_block();
        let latch = func.fresh_block();
        func.blocks.get_mut(&outer).unwrap().ops = vec![labeled_op(OpCode::TryStart, 50)];
        func.blocks.get_mut(&outer).unwrap().terminator = Terminator::Branch {
            target: inner,
            args: vec![],
        };
        func.blocks.insert(
            inner,
            TirBlock {
                id: inner,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: latch,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![check(50)],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: inner,
                    then_args: vec![],
                    else_block: outer,
                    else_args: vec![],
                },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        let polls = func.blocks[&latch]
            .ops
            .iter()
            .filter(|op| op.is_async_work_poll())
            .count();
        assert_eq!(polls, 1, "one insertion boundary must have one poll");
        assert_eq!(check_label(&func.blocks[&latch].ops[0]), Some(50));
        assert!(loop_only_poll_sites(&func, outer).is_empty());
        assert!(loop_only_poll_sites(&func, inner).is_empty());
    }

    #[test]
    fn nonlocal_inner_unwinds_preserve_one_outer_handler_at_loop_join() {
        use crate::tir::values::ValueId;

        let mut func = TirFunction::new("nonlocal_inner_unwinds".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let header = func.fresh_block();
        let first_try = func.fresh_block();
        let second_try = func.fresh_block();
        let cond = ValueId(0);
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.get_mut(&entry).unwrap().ops = vec![labeled_op(OpCode::TryStart, 22)];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![op(OpCode::Call), check(22)],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: first_try,
                    then_args: vec![],
                    else_block: second_try,
                    else_args: vec![],
                },
            },
        );
        for (block, label) in [(first_try, 23), (second_try, 28)] {
            func.blocks.insert(
                block,
                TirBlock {
                    id: block,
                    args: vec![],
                    ops: vec![
                        labeled_op(OpCode::TryStart, label),
                        exception_pop(),
                        check(22),
                    ],
                    terminator: Terminator::Branch {
                        target: header,
                        args: vec![],
                    },
                },
            );
        }

        let facts = crate::tir::exception_regions::compute_exception_region_facts(&func);
        assert_eq!(
            facts.lexical_handler_before(ExceptionOpPosition {
                block: header,
                op_index: 1,
            }),
            Ok(ExceptionBoundaryHandler::Labeled(22)),
            "normal continue/break/return unwinds must not leak an inner try frame into the loop join"
        );
        run(&mut func, &mut AnalysisManager::new());
        let poll = func.blocks[&header]
            .ops
            .iter()
            .find(|op| op.is_async_work_poll())
            .expect("post-call poll");
        assert_eq!(check_label(poll), Some(22));
    }

    #[test]
    fn depth_zero_exit_lowers_as_value_return_for_non_none_function() {
        let mut func = TirFunction::new("value_function".into(), vec![], TirType::I64);
        let entry = func.entry_block;
        let exit = func.fresh_block();
        func.label_id_map.insert(exit.0, 1);
        let value = func.fresh_value();
        func.value_types.insert(value, TirType::I64);
        let mut constant = labeled_op(OpCode::ConstInt, 7);
        constant.results.push(value);
        func.blocks.get_mut(&entry).unwrap().ops = vec![op(OpCode::Call), check(1), constant];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return {
            values: vec![value],
        };
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        run(&mut func, &mut AnalysisManager::new());
        let simple = crate::tir::lower_to_simple::lower_to_simple_ir(&func);
        assert!(simple.iter().any(|op| op.kind == "async_work_poll"));
        assert!(simple.iter().any(|op| op.kind == "label"));
        assert!(
            simple.iter().any(|op| op.kind == "ret"),
            "the ordinary value-return path must remain typed"
        );
        assert!(
            simple.iter().any(|op| op.kind == "ret_void"),
            "an empty TIR Return must lower to the existing return-with-pending backend sentinel"
        );
    }

    #[test]
    fn unreachable_block_call_is_not_a_poll_site() {
        let mut func = TirFunction::new("dead_call_block".into(), vec![], TirType::None);
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let dead = func.fresh_block();
        func.blocks.insert(
            dead,
            TirBlock {
                id: dead,
                args: vec![],
                ops: vec![op(OpCode::Call)],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert_eq!(func.blocks[&dead].ops.len(), 1);
        assert!(!func.has_exception_handling);
    }

    #[test]
    fn unreachable_in_block_post_call_boundary_is_not_a_poll_site() {
        let mut func = TirFunction::new("dead_post_transfer_call".into(), vec![], TirType::None);
        let entry = func.entry_block;
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 91);
        func.blocks.get_mut(&entry).unwrap().ops =
            vec![op(OpCode::Raise), check(91), op(OpCode::Call)];
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert_eq!(func.blocks[&entry].ops.len(), 3);
        assert!(
            !func.blocks[&entry]
                .ops
                .iter()
                .any(TirOp::is_async_work_poll)
        );
    }

    #[test]
    fn unreachable_loop_latch_is_not_a_poll_site() {
        let mut func = TirFunction::new("dead_loop".into(), vec![], TirType::None);
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        let header = func.fresh_block();
        let latch = func.fresh_block();
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: latch,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            latch,
            TirBlock {
                id: latch,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert!(func.blocks[&latch].ops.is_empty());
        assert!(!func.has_exception_handling);
    }
}
