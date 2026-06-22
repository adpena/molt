//! Backend-neutral ExceptionRegion ownership facts.
//!
//! This analysis is the current backend-neutral authority for handler-owned
//! exception MatchRef release facts. Current TIR still carries several
//! exception-stack operations as `Copy` + `_original_kind`; this analysis
//! recognizes those carriers, computes the path-local match-ref release
//! boundary, feeds pass-manager diagnostics, and drives shared TIR drop
//! insertion on activated targets.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::analysis::{Analysis, AnalysisId};
use super::blocks::{BlockId, Terminator};
use super::dominators;
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode, TirOp};
use super::values::ValueId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExceptionOpPosition {
    pub block: BlockId,
    pub op_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionMatchRefFact {
    pub value: ValueId,
    pub producer: ExceptionOpPosition,
    pub releases: Vec<ExceptionOpPosition>,
    pub source_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExceptionRegionDiagnosticKind {
    AmbiguousProducerDepth,
    MatchWithoutReachablePop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionRegionDiagnostic {
    pub kind: ExceptionRegionDiagnosticKind,
    pub value: ValueId,
    pub position: ExceptionOpPosition,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExceptionRegionFacts {
    pub match_refs: BTreeMap<ValueId, ExceptionMatchRefFact>,
    pub release_to_matches: BTreeMap<ExceptionOpPosition, Vec<ValueId>>,
    pub diagnostics: Vec<ExceptionRegionDiagnostic>,
}

pub struct ExceptionRegions;

impl Analysis for ExceptionRegions {
    type Result = ExceptionRegionFacts;
    const ID: AnalysisId = AnalysisId::ExceptionRegions;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;

    fn compute(func: &TirFunction) -> Self::Result {
        compute_exception_region_facts(func)
    }
}

pub fn compute_exception_region_facts(func: &TirFunction) -> ExceptionRegionFacts {
    let label_to_block: BTreeMap<_, _> = dominators::exception_label_to_block(func)
        .into_iter()
        .collect();
    let state_resume_depths = compute_state_resume_depths(func, &label_to_block);
    let mut facts = ExceptionRegionFacts::default();
    for (producer, op) in iter_ops(func) {
        let Some(source_kind) = original_kind(op) else {
            continue;
        };
        if !is_match_ref_source(source_kind) {
            continue;
        }
        let Some(&value) = op.results.first() else {
            continue;
        };
        let producer_depths: Vec<_> =
            path_depths_before(func, &label_to_block, &state_resume_depths, producer)
                .into_iter()
                .collect();
        let (release_candidates, producer_depth_ambiguous) = match producer_depths.as_slice() {
            [0] => {
                // Depth-zero exception reads are observers of pending/global
                // exception state, not handler-owned MatchRefs. They have no
                // handler-region `exception_pop` release boundary; ordinary
                // value/lifetime tracking owns them.
                continue;
            }
            [depth] => (
                reachable_region_pops(
                    func,
                    &label_to_block,
                    &state_resume_depths,
                    producer,
                    *depth,
                ),
                false,
            ),
            many if many.len() > 1 => {
                facts.diagnostics.push(ExceptionRegionDiagnostic {
                    kind: ExceptionRegionDiagnosticKind::AmbiguousProducerDepth,
                    value,
                    position: producer,
                    message: format!(
                        "exception match ref v{} from {source_kind} is reachable at multiple exception-region depths: {:?}",
                        value.0,
                        many
                    ),
                });
                (Vec::new(), true)
            }
            _ => (Vec::new(), false),
        };
        let releases = match release_candidates.as_slice() {
            [] if producer_depth_ambiguous => Vec::new(),
            [] => {
                facts.diagnostics.push(ExceptionRegionDiagnostic {
                    kind: ExceptionRegionDiagnosticKind::MatchWithoutReachablePop,
                    value,
                    position: producer,
                    message: format!(
                        "exception match ref v{} from {source_kind} has no reachable exception_pop",
                        value.0
                    ),
                });
                Vec::new()
            }
            many => many.to_vec(),
        };
        for release_pos in releases.iter().copied() {
            facts
                .release_to_matches
                .entry(release_pos)
                .or_default()
                .push(value);
        }
        facts.match_refs.insert(
            value,
            ExceptionMatchRefFact {
                value,
                producer,
                releases,
                source_kind: source_kind.to_string(),
            },
        );
    }
    for values in facts.release_to_matches.values_mut() {
        values.sort_unstable_by_key(|value| value.0);
    }
    facts
}

/// Fail-closed verifier for exception-region ownership facts.
///
/// The analysis computes backend-neutral release boundaries for handler-match
/// references. Diagnostics mean the compiler could otherwise choose a backend
/// local fallback or leak/double-release path, so the pass boundary treats them
/// as hard TIR verification failures.
pub fn verify_exception_regions(func: &TirFunction) -> Result<(), Vec<ExceptionRegionDiagnostic>> {
    let facts = compute_exception_region_facts(func);
    if facts.diagnostics.is_empty() {
        Ok(())
    } else {
        Err(facts.diagnostics)
    }
}

fn iter_ops(func: &TirFunction) -> Vec<(ExceptionOpPosition, &TirOp)> {
    let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
    blocks.sort_unstable_by_key(|block| block.0);
    let mut ops = Vec::new();
    for block in blocks {
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        for (op_index, op) in tir_block.ops.iter().enumerate() {
            ops.push((ExceptionOpPosition { block, op_index }, op));
        }
    }
    ops
}

fn original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

fn label_value(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

fn is_match_ref_source(kind: &str) -> bool {
    matches!(
        kind,
        "exception_last"
            | "exception_last_pending"
            | "exception_active"
            | "exception_current"
            | "exceptiongroup_match"
            | "exceptiongroup_combine"
    )
}

fn is_exception_pop(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy && matches!(original_kind(op), Some("exception_pop"))
}

fn op_exception_successors(label_to_block: &BTreeMap<i64, BlockId>, op: &TirOp) -> Vec<BlockId> {
    if !super::dominators::is_exception_transfer_edge(op.opcode) {
        return Vec::new();
    }
    let Some(label) = label_value(op) else {
        return Vec::new();
    };
    label_to_block.get(&label).copied().into_iter().collect()
}

fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    dominators::terminator_successors(term)
}

fn reachable_region_pops(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_depths: &BTreeMap<i64, BTreeSet<usize>>,
    producer: ExceptionOpPosition,
    depth: usize,
) -> Vec<ExceptionOpPosition> {
    if depth == 0 {
        return Vec::new();
    }
    let mut queue = VecDeque::new();
    queue.push_back((producer.block, producer.op_index.saturating_add(1), depth));
    let mut visited = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    while let Some((block, op_index, path_depth)) = queue.pop_front() {
        if !visited.insert((block, op_index, path_depth)) {
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        if op_index >= tir_block.ops.len() {
            enqueue_terminator_successors(
                &mut queue,
                &tir_block.terminator,
                path_depth,
                StateResumeDepthPolicy::Use(state_resume_depths),
            );
            continue;
        }
        let op = &tir_block.ops[op_index];
        if is_exception_pop(op) && path_depth == depth {
            candidates.insert(ExceptionOpPosition { block, op_index });
            continue;
        }
        let next_depth = depth_after_op(path_depth, op);
        for succ in op_exception_successors(label_to_block, op) {
            queue.push_back((succ, 0, next_depth));
        }
        queue.push_back((block, op_index + 1, next_depth));
    }
    candidates.into_iter().collect()
}

fn path_depths_before(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_depths: &BTreeMap<i64, BTreeSet<usize>>,
    target: ExceptionOpPosition,
) -> BTreeSet<usize> {
    path_depths_before_with_state_policy(
        func,
        label_to_block,
        StateResumeDepthPolicy::Use(state_resume_depths),
        target,
    )
}

fn path_depths_before_with_state_policy(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_policy: StateResumeDepthPolicy<'_>,
    target: ExceptionOpPosition,
) -> BTreeSet<usize> {
    let mut queue = VecDeque::new();
    queue.push_back((func.entry_block, 0usize, 0usize));
    let mut visited = BTreeSet::new();
    let mut depths = BTreeSet::new();
    while let Some((block, op_index, depth)) = queue.pop_front() {
        if !visited.insert((block, op_index, depth)) {
            continue;
        }
        if block == target.block && op_index == target.op_index {
            depths.insert(depth);
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        if op_index >= tir_block.ops.len() {
            enqueue_terminator_successors(&mut queue, &tir_block.terminator, depth, state_policy);
            continue;
        }
        let op = &tir_block.ops[op_index];
        let next_depth = depth_after_op(depth, op);
        for succ in op_exception_successors(label_to_block, op) {
            queue.push_back((succ, 0, next_depth));
        }
        queue.push_back((block, op_index + 1, next_depth));
    }
    depths
}

#[derive(Clone, Copy)]
enum StateResumeDepthPolicy<'a> {
    IgnoreCases,
    Use(&'a BTreeMap<i64, BTreeSet<usize>>),
}

fn enqueue_terminator_successors(
    queue: &mut VecDeque<(BlockId, usize, usize)>,
    term: &Terminator,
    depth: usize,
    state_policy: StateResumeDepthPolicy<'_>,
) {
    match (term, state_policy) {
        (
            Terminator::StateDispatch { cases, default, .. },
            StateResumeDepthPolicy::Use(state_resume_depths),
        ) => {
            queue.push_back((*default, 0, depth));
            for (state_id, target, _) in cases {
                if let Some(depths) = state_resume_depths.get(state_id) {
                    for state_depth in depths {
                        queue.push_back((*target, 0, *state_depth));
                    }
                } else {
                    queue.push_back((*target, 0, depth));
                }
            }
        }
        (Terminator::StateDispatch { default, .. }, StateResumeDepthPolicy::IgnoreCases) => {
            queue.push_back((*default, 0, depth));
        }
        _ => {
            for succ in terminator_successors(term) {
                queue.push_back((succ, 0, depth));
            }
        }
    }
}

fn compute_state_resume_depths(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
) -> BTreeMap<i64, BTreeSet<usize>> {
    let const_ints = const_int_values(func);
    let dispatch_targets = state_dispatch_targets(func);
    let mut depths_by_state: BTreeMap<i64, BTreeSet<usize>> = BTreeMap::new();
    for (position, op) in iter_ops(func) {
        let Some(state_id) = saved_resume_state_id(op, &const_ints) else {
            continue;
        };
        let depths = path_depths_before_with_state_policy(
            func,
            label_to_block,
            StateResumeDepthPolicy::IgnoreCases,
            position,
        );
        if depths.is_empty() {
            continue;
        }
        if let Some(targets) = dispatch_targets.get(&state_id) {
            let entry = depths_by_state.entry(state_id).or_default();
            for target in targets {
                let reopened_depth = leading_try_start_count(func, *target);
                entry.extend(
                    depths
                        .iter()
                        .map(|depth| depth.saturating_sub(reopened_depth)),
                );
            }
        } else {
            depths_by_state.entry(state_id).or_default().extend(depths);
        }
    }
    depths_by_state
}

fn state_dispatch_targets(func: &TirFunction) -> BTreeMap<i64, BTreeSet<BlockId>> {
    let mut targets: BTreeMap<i64, BTreeSet<BlockId>> = BTreeMap::new();
    for block in func.blocks.values() {
        if let Terminator::StateDispatch { cases, .. } = &block.terminator {
            for (state_id, target, _) in cases {
                targets.entry(*state_id).or_default().insert(*target);
            }
        }
    }
    targets
}

fn leading_try_start_count(func: &TirFunction, block: BlockId) -> usize {
    let Some(tir_block) = func.blocks.get(&block) else {
        return 0;
    };
    let mut count = 0;
    for op in &tir_block.ops {
        if op.opcode == OpCode::TryStart {
            count += 1;
            continue;
        }
        if is_resume_entry_depth_neutral_op(op) {
            continue;
        }
        break;
    }
    count
}

fn is_resume_entry_depth_neutral_op(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    matches!(
        original_kind(op),
        None | Some("store_var" | "line" | "trace_enter_slot" | "trace_exit")
    )
}

fn const_int_values(func: &TirFunction) -> BTreeMap<ValueId, i64> {
    let mut values = BTreeMap::new();
    for (_, op) in iter_ops(func) {
        if op.opcode != OpCode::ConstInt {
            continue;
        }
        let Some(&value_id) = op.results.first() else {
            continue;
        };
        if let Some(AttrValue::Int(value)) = op.attrs.get("value") {
            values.insert(value_id, *value);
        }
    }
    values
}

fn saved_resume_state_id(op: &TirOp, const_ints: &BTreeMap<ValueId, i64>) -> Option<i64> {
    match op.opcode {
        OpCode::StateYield => label_value(op),
        OpCode::StateTransition | OpCode::ChanSendYield | OpCode::ChanRecvYield => op
            .operands
            .last()
            .and_then(|value| const_ints.get(value))
            .copied(),
        _ => None,
    }
}

fn depth_after_op(depth: usize, op: &TirOp) -> usize {
    match op.opcode {
        OpCode::TryStart => depth.saturating_add(1),
        // `try_end` closes the handler transfer edge, but the handler-owned
        // exception region is released by the paired runtime `exception_pop`.
        // Treating both as pops double-decrements loops that re-enter a protected
        // region and makes later handler reads appear reachable at depth zero.
        _ if is_exception_pop(op) => depth.saturating_sub(1),
        _ => depth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect, TirOp};
    use crate::tir::types::TirType;

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

    fn original(kind: &str, results: Vec<ValueId>) -> TirOp {
        let mut op = op(OpCode::Copy);
        op.results = results;
        op.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        op
    }

    fn try_start(label: i64) -> TirOp {
        let mut op = op(OpCode::TryStart);
        op.attrs.insert("value".into(), AttrValue::Int(label));
        op
    }

    fn try_end(label: i64) -> TirOp {
        let mut op = op(OpCode::TryEnd);
        op.attrs.insert("value".into(), AttrValue::Int(label));
        op
    }

    fn check_exception(label: i64) -> TirOp {
        let mut op = op(OpCode::CheckException);
        op.attrs.insert("value".into(), AttrValue::Int(label));
        op
    }

    fn const_int(value: i64, out: ValueId) -> TirOp {
        let mut op = op(OpCode::ConstInt);
        op.results = vec![out];
        op.attrs.insert("value".into(), AttrValue::Int(value));
        op
    }

    fn state_transition(operands: Vec<ValueId>, out: ValueId) -> TirOp {
        let mut op = op(OpCode::StateTransition);
        op.operands = operands;
        op.results = vec![out];
        op
    }

    fn state_yield(state: i64, operand: ValueId) -> TirOp {
        let mut op = op(OpCode::StateYield);
        op.operands = vec![operand];
        op.attrs.insert("value".into(), AttrValue::Int(state));
        op
    }

    fn split_cleanup_function() -> TirFunction {
        let mut func = TirFunction::new("split_cleanup".into(), vec![], TirType::None);
        let clean = func.fresh_block();
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 4);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: clean,
            args: vec![],
        };
        func.blocks.insert(
            clean,
            TirBlock {
                id: clean,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![original("exception_last_pending", vec![exc])],
                terminator: Terminator::Branch {
                    target: handler_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_pop,
            TirBlock {
                id: handler_pop,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func
    }

    fn ambiguous_depth_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new("ambiguous_depth".into(), vec![], TirType::None);
        let before_try = func.fresh_block();
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 7);
        let cond = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: before_try,
            then_args: vec![],
            else_block: handler,
            else_args: vec![],
        };
        func.blocks.insert(
            before_try,
            TirBlock {
                id: before_try,
                args: vec![],
                ops: vec![try_start(7)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original("exception_last_pending", vec![exc]),
                    original("exception_pop", vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn body_close_to_normal_exit_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new("body_close_to_normal_exit".into(), vec![], TirType::None);
        let normal_exit = func.fresh_block();
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 17);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(17), try_end(17)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: normal_exit,
            args: vec![],
        };
        func.blocks.insert(
            normal_exit,
            TirBlock {
                id: normal_exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![original("exception_last_pending", vec![exc])],
                terminator: Terminator::Branch {
                    target: handler_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_pop,
            TirBlock {
                id: handler_pop,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn split_exit_pops_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new("split_exit_pops".into(), vec![], TirType::None);
        let handler = func.fresh_block();
        let pop_a = func.fresh_block();
        let pop_b = func.fresh_block();
        func.label_id_map.insert(handler.0, 11);
        let cond = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(11)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![original("exception_last_pending", vec![exc])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: pop_a,
                    then_args: vec![],
                    else_block: pop_b,
                    else_args: vec![],
                },
            },
        );
        for block in [pop_a, pop_b] {
            func.blocks.insert(
                block,
                TirBlock {
                    id: block,
                    args: vec![],
                    ops: vec![original("exception_pop", vec![])],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }
        (func, exc)
    }

    fn depth_zero_observer_after_pop_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "depth_zero_observer_after_pop".into(),
            vec![],
            TirType::None,
        );
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 12);
        let exc = func.fresh_value();
        let late_observer = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(12)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original("exception_last_pending", vec![exc]),
                    original("exception_pop", vec![]),
                    original("exception_last", vec![late_observer]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn loop_reentry_after_try_end_and_exception_pop_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "loop_reentry_after_try_end_and_exception_pop".into(),
            vec![],
            TirType::None,
        );
        let loop_block = func.fresh_block();
        let normal = func.fresh_block();
        let cleanup = func.fresh_block();
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 50);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![op(OpCode::TryStart)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: loop_block,
            args: vec![],
        };
        func.blocks.insert(
            loop_block,
            TirBlock {
                id: loop_block,
                args: vec![],
                ops: vec![try_start(50)],
                terminator: Terminator::Branch {
                    target: normal,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![try_end(50)],
                terminator: Terminator::Branch {
                    target: cleanup,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            cleanup,
            TirBlock {
                id: cleanup,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Branch {
                    target: loop_block,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![try_end(50), original("exception_last_pending", vec![exc])],
                terminator: Terminator::Branch {
                    target: handler_pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_pop,
            TirBlock {
                id: handler_pop,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn state_resume_inside_try_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new("state_resume_inside_try".into(), vec![], TirType::None);
        let first_entry = func.fresh_block();
        let resumed_body = func.fresh_block();
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 85);

        let pending_state = func.fresh_value();
        let awaitable = func.fresh_value();
        let transition_out = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
            cases: vec![(88, resumed_body, vec![])],
            default: first_entry,
            default_args: vec![],
        };
        func.blocks.insert(
            first_entry,
            TirBlock {
                id: first_entry,
                args: vec![],
                ops: vec![try_start(85)],
                terminator: Terminator::Branch {
                    target: resumed_body,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            resumed_body,
            TirBlock {
                id: resumed_body,
                args: vec![],
                ops: vec![
                    const_int(88, pending_state),
                    state_transition(vec![awaitable, pending_state], transition_out),
                    check_exception(85),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original("exception_last_pending", vec![exc]),
                    original("exception_pop", vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn state_yield_reopens_try_on_resume_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "state_yield_reopens_try_on_resume".into(),
            vec![],
            TirType::None,
        );
        let yielding = func.fresh_block();
        let resume = func.fresh_block();
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 99);

        let yielded = func.fresh_value();
        let resume_alias = func.fresh_value();
        let exc = func.fresh_value();
        let mut resume_arg_copy = op(OpCode::Copy);
        resume_arg_copy.operands = vec![yielded];
        resume_arg_copy.results = vec![resume_alias];

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
            cases: vec![(7, resume, vec![])],
            default: yielding,
            default_args: vec![],
        };
        func.blocks.insert(
            yielding,
            TirBlock {
                id: yielding,
                args: vec![],
                ops: vec![try_start(99), state_yield(7, yielded)],
                terminator: Terminator::Unreachable,
            },
        );
        func.blocks.insert(
            resume,
            TirBlock {
                id: resume,
                args: vec![],
                ops: vec![resume_arg_copy, try_start(99), check_exception(99)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![
                    original("exception_last_pending", vec![exc]),
                    original("exception_pop", vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    #[test]
    fn exception_region_pairs_match_ref_with_reachable_handler_pop() {
        let func = split_cleanup_function();
        let exc = ValueId(0);
        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            }]
        );
        assert!(
            !facts.release_to_matches.contains_key(&ExceptionOpPosition {
                block: BlockId(1),
                op_index: 0,
            })
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            }],
            vec![exc],
        );
    }

    #[test]
    fn exception_region_reports_match_without_reachable_pop() {
        let mut func = TirFunction::new("missing_pop".into(), vec![], TirType::None);
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 9);
        let exc = func.fresh_value();
        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(9)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![original("exception_last_pending", vec![exc])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let facts = compute_exception_region_facts(&func);

        assert_eq!(
            facts.diagnostics[0].kind,
            ExceptionRegionDiagnosticKind::MatchWithoutReachablePop,
        );
        assert!(facts.match_refs[&exc].releases.is_empty());
        assert!(verify_exception_regions(&func).is_err());
    }

    #[test]
    fn exception_region_ignores_depth_zero_exception_observer() {
        let mut func = TirFunction::new(
            "depth_zero_exception_observer".into(),
            vec![],
            TirType::None,
        );
        let exc = func.fresh_value();
        func.blocks.get_mut(&func.entry_block).unwrap().ops =
            vec![original("exception_last", vec![exc])];

        let facts = compute_exception_region_facts(&func);

        assert!(facts.match_refs.is_empty());
        assert!(facts.diagnostics.is_empty());
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_reports_ambiguous_producer_depth_without_selecting_release() {
        let (func, exc) = ambiguous_depth_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.match_refs[&exc].releases.is_empty());
        assert!(facts.release_to_matches.is_empty());
        assert_eq!(
            facts
                .diagnostics
                .iter()
                .filter(|diag| diag.kind == ExceptionRegionDiagnosticKind::AmbiguousProducerDepth)
                .count(),
            1,
        );
        assert!(
            facts
                .diagnostics
                .iter()
                .all(|diag| diag.kind != ExceptionRegionDiagnosticKind::MatchWithoutReachablePop),
            "{:?}",
            facts.diagnostics
        );
        assert!(verify_exception_regions(&func).is_err());
    }

    #[test]
    fn exception_region_try_end_does_not_reenter_handler_at_depth_zero() {
        let (func, exc) = body_close_to_normal_exit_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            }],
            vec![exc],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_allows_path_alternative_exit_pops() {
        let (func, exc) = split_exit_pops_function();

        let facts = compute_exception_region_facts(&func);

        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![
                ExceptionOpPosition {
                    block: BlockId(2),
                    op_index: 0,
                },
                ExceptionOpPosition {
                    block: BlockId(3),
                    op_index: 0,
                },
            ]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(2),
                op_index: 0,
            }],
            vec![exc],
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            }],
            vec![exc],
        );
        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_allows_depth_zero_observer_after_handler_pop() {
        let (func, exc) = depth_zero_observer_after_pop_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(1),
                op_index: 1,
            }]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(1),
                op_index: 1,
            }],
            vec![exc],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_loop_reentry_keeps_try_end_and_pop_as_single_close() {
        let (func, exc) = loop_reentry_after_try_end_and_exception_pop_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(5),
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(5),
                op_index: 0,
            }],
            vec![exc],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_seeds_state_dispatch_resume_with_saved_try_depth() {
        let (func, exc) = state_resume_inside_try_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(3),
                op_index: 1,
            }]
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_offsets_state_yield_resume_try_reopen_depth() {
        let (func, exc) = state_yield_reopens_try_on_resume_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(3),
                op_index: 1,
            }]
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_regions_analysis_manager_caches_and_invalidates() {
        let func = split_cleanup_function();
        let mut am = crate::tir::analysis::AnalysisManager::new();

        assert!(!am.is_cached(AnalysisId::ExceptionRegions));
        assert_eq!(am.get::<ExceptionRegions>(&func).match_refs.len(), 1,);
        assert!(am.is_cached(AnalysisId::ExceptionRegions));
        am.invalidate_ops();
        assert!(!am.is_cached(AnalysisId::ExceptionRegions));
        assert_eq!(
            am.get::<ExceptionRegions>(&func).release_to_matches.len(),
            1,
        );
        am.invalidate_cfg();
        assert!(!am.is_cached(AnalysisId::ExceptionRegions));
    }
}
