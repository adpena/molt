//! Exception-region path-state traversal engine.
//!
//! The CFG walk that computes, for each op position, the set of exception
//! path-states reachable there — the exception-stack frames, handler owners,
//! normal closures, and pending-transfer flag — plus the `state_resume_stacks`
//! generator-resume fixpoint and the reachable `exception_pop` release search.
//! Consumed by [`super`]'s `compute_exception_region_facts`. See the
//! module-level docs on [`super`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::{ExceptionOpPosition, ExceptionPopOwnerStates, ExceptionRegionToken};

pub(super) type AnonymousHandlerDestinations = BTreeMap<ExceptionOpPosition, BTreeSet<i64>>;

pub(super) fn iter_ops(func: &TirFunction) -> Vec<(ExceptionOpPosition, &TirOp)> {
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

pub(super) fn original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

pub(super) fn label_value(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

pub(super) fn is_match_ref_source(kind: &str) -> bool {
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

fn op_clears_pending_exception(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy && matches!(original_kind(op), Some("exception_clear"))
}

fn op_normal_fallthrough_reachable(state_before: &ExceptionPathState, op: &TirOp) -> bool {
    !(op.opcode == OpCode::CheckException && state_before.pending_must_transfer)
}

fn terminator_successor_state(
    label_to_block: &BTreeMap<i64, BlockId>,
    anonymous_destinations: &AnonymousHandlerDestinations,
    target: BlockId,
    state: &ExceptionPathState,
) -> ExceptionPathState {
    if state.pending_must_transfer
        && let Some((&label, _)) = label_to_block.iter().find(|(_, block)| **block == target)
        && let Some(handler_state) = state.enter_handler(label, anonymous_destinations)
    {
        return handler_state;
    }
    state.clone()
}

fn op_exception_successors_with_state(
    label_to_block: &BTreeMap<i64, BlockId>,
    anonymous_destinations: &AnonymousHandlerDestinations,
    op: &TirOp,
    state: &ExceptionPathState,
) -> Vec<(BlockId, ExceptionPathState)> {
    if !dominators::is_exception_transfer_edge(op.opcode) {
        return Vec::new();
    }
    let Some(label) = label_value(op) else {
        return Vec::new();
    };
    let Some(&target) = label_to_block.get(&label) else {
        return Vec::new();
    };
    state
        .enter_handler(label, anonymous_destinations)
        .into_iter()
        .map(|succ_state| (target, succ_state))
        .collect()
}

type ConstIntValues = BTreeMap<ValueId, i64>;

pub(super) type ExceptionStack = Vec<ExceptionRegionToken>;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ExceptionPathState {
    pub(super) frames: ExceptionStack,
    pub(super) owners: ExceptionStack,
    pub(super) normal_closures: ExceptionStack,
    pub(super) pending_must_transfer: bool,
}

impl ExceptionPathState {
    fn enter_handler(
        &self,
        label: i64,
        anonymous_destinations: &AnonymousHandlerDestinations,
    ) -> Option<Self> {
        let mut next = self.clone();
        let index = next.frames.iter().rposition(|token| match token {
            ExceptionRegionToken::Labeled(candidate) => *candidate == label,
            ExceptionRegionToken::Anonymous(owner) => anonymous_destinations
                .get(owner)
                .is_some_and(|destinations| {
                    destinations.len() == 1 && destinations.contains(&label)
                }),
        })?;
        let owner = next.frames[index];
        next.frames.truncate(index);
        if !next.owners.contains(&owner) {
            next.owners.push(owner);
        }
        next.normal_closures.retain(|token| *token != owner);
        next.pending_must_transfer = false;
        Some(next)
    }

    fn after_op(&self, position: ExceptionOpPosition, op: &TirOp) -> Self {
        let mut next = self.clone();
        if op.opcode == OpCode::TryStart {
            let token = label_value(op)
                .map(ExceptionRegionToken::Labeled)
                .unwrap_or(ExceptionRegionToken::Anonymous(position));
            if !next.frames.contains(&token) {
                next.frames.push(token);
            }
            return next;
        }
        if op.opcode == OpCode::TryEnd {
            let token = label_value(op)
                .map(ExceptionRegionToken::Labeled)
                .or_else(|| {
                    next.frames
                        .iter()
                        .rev()
                        .find(|token| matches!(token, ExceptionRegionToken::Anonymous(_)))
                        .copied()
                });
            if let Some(token) = token {
                if let Some(index) = next.frames.iter().rposition(|frame| *frame == token) {
                    next.frames.truncate(index);
                }
                if next.owners.last().copied() != Some(token)
                    && !next.normal_closures.contains(&token)
                {
                    next.normal_closures.push(token);
                }
            }
            return next;
        }
        if is_exception_pop(op) {
            if next.owners.pop().is_none() {
                next.normal_closures.pop();
            }
            return next;
        }
        if op.opcode == OpCode::Raise {
            next.pending_must_transfer = true;
            return next;
        }
        if op_clears_pending_exception(op) {
            next.pending_must_transfer = false;
        }
        next
    }
}

fn current_pop_owner(state: &ExceptionPathState) -> Option<ExceptionRegionToken> {
    state
        .owners
        .last()
        .copied()
        .or_else(|| state.normal_closures.last().copied())
}

pub(super) fn match_ref_release_owner(
    source_kind: &str,
    state: &ExceptionPathState,
    owning_tokens: &BTreeSet<ExceptionRegionToken>,
) -> Option<ExceptionRegionToken> {
    if let Some(owner) = state.owners.last().copied() {
        return Some(owner);
    }
    if !matches!(source_kind, "exception_last" | "exception_last_pending") {
        return None;
    }
    let owner = state.normal_closures.last().copied()?;
    owning_tokens.contains(&owner).then_some(owner)
}

pub(super) type StateResumeStacks = BTreeMap<i64, BTreeSet<ExceptionPathState>>;

fn collect_const_int_values(func: &TirFunction) -> ConstIntValues {
    let mut values = ConstIntValues::new();
    for (_, op) in iter_ops(func) {
        if op.opcode != OpCode::ConstInt {
            continue;
        }
        let (Some(&result), Some(value)) = (op.results.first(), label_value(op)) else {
            continue;
        };
        values.insert(result, value);
    }
    values
}

fn state_id(op: &TirOp, const_int_values: &ConstIntValues) -> Option<i64> {
    if op.opcode == OpCode::StateYield {
        return label_value(op);
    }
    if op.opcode == OpCode::StateTransition
        || op.opcode == OpCode::ChanSendYield
        || op.opcode == OpCode::ChanRecvYield
    {
        return op
            .operands
            .last()
            .and_then(|pending| const_int_values.get(pending))
            .copied();
    }
    None
}

fn terminator_successors_with_state(
    term: &Terminator,
    label_to_block: &BTreeMap<i64, BlockId>,
    anonymous_destinations: &AnonymousHandlerDestinations,
    state: &ExceptionPathState,
    state_resume_stacks: &StateResumeStacks,
    unknown_state: Option<&ExceptionPathState>,
) -> Vec<(BlockId, ExceptionPathState)> {
    match term {
        Terminator::Branch { target, .. } => {
            vec![(
                *target,
                terminator_successor_state(label_to_block, anonymous_destinations, *target, state),
            )]
        }
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![
            (
                *then_block,
                terminator_successor_state(
                    label_to_block,
                    anonymous_destinations,
                    *then_block,
                    state,
                ),
            ),
            (
                *else_block,
                terminator_successor_state(
                    label_to_block,
                    anonymous_destinations,
                    *else_block,
                    state,
                ),
            ),
        ],
        Terminator::Switch { cases, default, .. } => {
            let mut successors = Vec::with_capacity(cases.len() + 1);
            successors.extend(cases.iter().map(|(_, target, _)| {
                (
                    *target,
                    terminator_successor_state(
                        label_to_block,
                        anonymous_destinations,
                        *target,
                        state,
                    ),
                )
            }));
            successors.push((
                *default,
                terminator_successor_state(label_to_block, anonymous_destinations, *default, state),
            ));
            successors
        }
        Terminator::StateDispatch { cases, default, .. } => {
            let mut successors = Vec::with_capacity(cases.len() + 1);
            successors.push((
                *default,
                terminator_successor_state(label_to_block, anonymous_destinations, *default, state),
            ));
            for (state, target, _) in cases {
                if let Some(stacks) = state_resume_stacks.get(state) {
                    successors.extend(stacks.iter().map(|resume_stack| {
                        (
                            *target,
                            terminator_successor_state(
                                label_to_block,
                                anonymous_destinations,
                                *target,
                                resume_stack,
                            ),
                        )
                    }));
                } else if let Some(fallback_state) = unknown_state {
                    successors.push((
                        *target,
                        terminator_successor_state(
                            label_to_block,
                            anonymous_destinations,
                            *target,
                            fallback_state,
                        ),
                    ));
                }
            }
            successors
        }
        Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
    }
}

fn collect_state_resume_stacks_once(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    anonymous_destinations: &AnonymousHandlerDestinations,
    state_resume_stacks: &StateResumeStacks,
    const_int_values: &ConstIntValues,
) -> StateResumeStacks {
    let mut queue = VecDeque::new();
    queue.push_back((func.entry_block, 0usize, ExceptionPathState::default()));
    let mut visited = BTreeSet::new();
    let mut observed = StateResumeStacks::new();
    while let Some((block, op_index, state)) = queue.pop_front() {
        if !visited.insert((block, op_index, state.clone())) {
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        if op_index >= tir_block.ops.len() {
            for (succ, succ_state) in terminator_successors_with_state(
                &tir_block.terminator,
                label_to_block,
                anonymous_destinations,
                &state,
                state_resume_stacks,
                None,
            ) {
                queue.push_back((succ, 0, succ_state));
            }
            continue;
        }
        let op = &tir_block.ops[op_index];
        if let Some(resume_state) = state_id(op, const_int_values) {
            observed
                .entry(resume_state)
                .or_default()
                .insert(state.clone());
        }
        let pos = ExceptionOpPosition { block, op_index };
        let next_state = state.after_op(pos, op);
        for (succ, succ_state) in op_exception_successors_with_state(
            label_to_block,
            anonymous_destinations,
            op,
            &next_state,
        ) {
            queue.push_back((succ, 0, succ_state));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state));
        }
    }
    observed
}

pub(super) fn compute_state_resume_stacks(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    anonymous_destinations: &AnonymousHandlerDestinations,
) -> StateResumeStacks {
    let const_int_values = collect_const_int_values(func);
    let mut stacks = StateResumeStacks::new();
    loop {
        let observed = collect_state_resume_stacks_once(
            func,
            label_to_block,
            anonymous_destinations,
            &stacks,
            &const_int_values,
        );
        let mut changed = false;
        for (state, observed_stacks) in observed {
            let state_stacks = stacks.entry(state).or_default();
            for stack in observed_stacks {
                changed |= state_stacks.insert(stack);
            }
        }
        if !changed {
            return stacks;
        }
    }
}

pub(super) fn reachable_region_pops(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_stacks: &StateResumeStacks,
    anonymous_destinations: &AnonymousHandlerDestinations,
    producer: ExceptionOpPosition,
    owner: ExceptionRegionToken,
    producer_states: &[ExceptionPathState],
) -> BTreeMap<ExceptionOpPosition, BTreeSet<BlockId>> {
    let mut queue = VecDeque::new();
    for state in producer_states
        .iter()
        .filter(|state| current_pop_owner(state) == Some(owner))
    {
        queue.push_back((
            producer.block,
            producer.op_index.saturating_add(1),
            state.clone(),
            None,
        ));
    }
    let mut visited = BTreeSet::new();
    let mut candidates: BTreeMap<ExceptionOpPosition, BTreeSet<BlockId>> = BTreeMap::new();
    while let Some((block, op_index, state, entry_pred)) = queue.pop_front() {
        if !visited.insert((block, op_index, state.clone(), entry_pred)) {
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        if op_index >= tir_block.ops.len() {
            for (succ, succ_state) in terminator_successors_with_state(
                &tir_block.terminator,
                label_to_block,
                anonymous_destinations,
                &state,
                state_resume_stacks,
                Some(&state),
            ) {
                queue.push_back((succ, 0, succ_state, Some(block)));
            }
            continue;
        }
        let op = &tir_block.ops[op_index];
        if is_exception_pop(op) && current_pop_owner(&state) == Some(owner) {
            if let Some(entry_pred) = entry_pred {
                candidates
                    .entry(ExceptionOpPosition { block, op_index })
                    .or_default()
                    .insert(entry_pred);
            } else {
                candidates
                    .entry(ExceptionOpPosition { block, op_index })
                    .or_default();
            }
            continue;
        }
        let pos = ExceptionOpPosition { block, op_index };
        let next_state = state.after_op(pos, op);
        for (succ, succ_state) in op_exception_successors_with_state(
            label_to_block,
            anonymous_destinations,
            op,
            &next_state,
        ) {
            queue.push_back((succ, 0, succ_state, None));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state, entry_pred));
        }
    }
    candidates
}

pub(super) fn path_states_before(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_stacks: &StateResumeStacks,
    anonymous_destinations: &AnonymousHandlerDestinations,
    target: ExceptionOpPosition,
) -> BTreeSet<ExceptionPathState> {
    let mut queue = VecDeque::new();
    queue.push_back((func.entry_block, 0usize, ExceptionPathState::default()));
    let mut visited = BTreeSet::new();
    let mut states = BTreeSet::new();
    while let Some((block, op_index, state)) = queue.pop_front() {
        if !visited.insert((block, op_index, state.clone())) {
            continue;
        }
        if block == target.block && op_index == target.op_index {
            states.insert(state);
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        if op_index >= tir_block.ops.len() {
            for (succ, succ_state) in terminator_successors_with_state(
                &tir_block.terminator,
                label_to_block,
                anonymous_destinations,
                &state,
                state_resume_stacks,
                Some(&state),
            ) {
                queue.push_back((succ, 0, succ_state));
            }
            continue;
        }
        let op = &tir_block.ops[op_index];
        let pos = ExceptionOpPosition { block, op_index };
        let next_state = state.after_op(pos, op);
        for (succ, succ_state) in op_exception_successors_with_state(
            label_to_block,
            anonymous_destinations,
            op,
            &next_state,
        ) {
            queue.push_back((succ, 0, succ_state));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state));
        }
    }
    states
}

/// Compute the active lexical exception-region owner at every reachable op
/// boundary in one CFG traversal. `op_index == block.ops.len()` is the block
/// exit/terminator boundary, which makes this the shared authority for both
/// in-block observations and backedge observations.
pub(super) fn lexical_handlers_before(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_stacks: &StateResumeStacks,
    anonymous_destinations: &AnonymousHandlerDestinations,
) -> BTreeMap<ExceptionOpPosition, BTreeSet<Option<ExceptionRegionToken>>> {
    let mut queue = VecDeque::new();
    queue.push_back((func.entry_block, 0usize, ExceptionPathState::default()));
    let mut visited = BTreeSet::new();
    let mut handlers: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    while let Some((block, op_index, state)) = queue.pop_front() {
        if !visited.insert((block, op_index, state.clone())) {
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        handlers
            .entry(ExceptionOpPosition { block, op_index })
            .or_default()
            .insert(state.frames.last().copied());
        if op_index >= tir_block.ops.len() {
            for (succ, succ_state) in terminator_successors_with_state(
                &tir_block.terminator,
                label_to_block,
                anonymous_destinations,
                &state,
                state_resume_stacks,
                Some(&state),
            ) {
                queue.push_back((succ, 0, succ_state));
            }
            continue;
        }
        let op = &tir_block.ops[op_index];
        let pos = ExceptionOpPosition { block, op_index };
        let next_state = state.after_op(pos, op);
        for (succ, succ_state) in op_exception_successors_with_state(
            label_to_block,
            anonymous_destinations,
            op,
            &next_state,
        ) {
            queue.push_back((succ, 0, succ_state));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state));
        }
    }
    handlers
}

pub fn exception_pop_owner_states(
    func: &TirFunction,
    target: ExceptionOpPosition,
) -> ExceptionPopOwnerStates {
    let label_to_block: BTreeMap<_, _> = dominators::exception_label_to_block(func)
        .into_iter()
        .collect();
    let (state_resume_stacks, _, anonymous_destinations) =
        super::exception_region_path_authority(func, &label_to_block);
    let mut queue = VecDeque::new();
    queue.push_back((
        func.entry_block,
        0usize,
        ExceptionPathState::default(),
        None,
    ));
    let mut visited = BTreeSet::new();
    let mut owners = ExceptionPopOwnerStates::default();
    while let Some((block, op_index, state, pred_into_target)) = queue.pop_front() {
        if !visited.insert((block, op_index, state.clone(), pred_into_target)) {
            continue;
        }
        if block == target.block && op_index == target.op_index {
            let owner = current_pop_owner(&state);
            owners.all.insert(owner);
            if let Some(pred) = pred_into_target {
                owners
                    .by_terminator_pred
                    .entry(pred)
                    .or_default()
                    .insert(owner);
            }
            continue;
        }
        let Some(tir_block) = func.blocks.get(&block) else {
            continue;
        };
        if op_index >= tir_block.ops.len() {
            for (succ, succ_state) in terminator_successors_with_state(
                &tir_block.terminator,
                &label_to_block,
                &anonymous_destinations,
                &state,
                &state_resume_stacks,
                Some(&state),
            ) {
                let next_pred = (succ == target.block).then_some(block);
                queue.push_back((succ, 0, succ_state, next_pred));
            }
            continue;
        }
        let op = &tir_block.ops[op_index];
        let pos = ExceptionOpPosition { block, op_index };
        let next_state = state.after_op(pos, op);
        for (succ, succ_state) in op_exception_successors_with_state(
            &label_to_block,
            &anonymous_destinations,
            op,
            &next_state,
        ) {
            let next_pred = (succ == target.block).then_some(block);
            queue.push_back((succ, 0, succ_state, next_pred));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state, pred_into_target));
        }
    }
    owners
}
