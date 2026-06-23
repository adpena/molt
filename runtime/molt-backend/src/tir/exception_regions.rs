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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExceptionRegionToken {
    Labeled(i64),
    Anonymous(ExceptionOpPosition),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExceptionMatchRefRelease {
    pub release: ExceptionOpPosition,
    pub owner: ExceptionRegionToken,
    pub entry_predecessors: Vec<BlockId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExceptionMatchReleaseFact {
    pub value: ValueId,
    pub owner: ExceptionRegionToken,
    pub entry_predecessors: Vec<BlockId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionMatchRefFact {
    pub value: ValueId,
    pub producer: ExceptionOpPosition,
    pub releases: Vec<ExceptionOpPosition>,
    pub release_facts: Vec<ExceptionMatchRefRelease>,
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
    pub release_to_match_facts: BTreeMap<ExceptionOpPosition, Vec<ExceptionMatchReleaseFact>>,
    pub diagnostics: Vec<ExceptionRegionDiagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExceptionPopOwnerStates {
    pub all: BTreeSet<Option<ExceptionRegionToken>>,
    pub by_terminator_pred: BTreeMap<BlockId, BTreeSet<Option<ExceptionRegionToken>>>,
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
    let state_resume_stacks = compute_state_resume_stacks(func, &label_to_block);
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
        let producer_states: Vec<_> =
            path_states_before(func, &label_to_block, &state_resume_stacks, producer)
                .into_iter()
                .collect();
        let owning_tokens: BTreeSet<_> = producer_states
            .iter()
            .filter_map(|state| state.owners.last().copied())
            .collect();
        let unowned_non_finally_reachable = producer_states
            .iter()
            .any(|state| state.owners.is_empty() && state.normal_closures.is_empty());
        if producer_states
            .iter()
            .all(|state| state.owners.is_empty() && state.normal_closures.is_empty())
        {
            // Depth-zero exception reads are observers of pending/global
            // exception state, not handler-owned MatchRefs. They have no
            // handler-region `exception_pop` release boundary; ordinary
            // value/lifetime tracking owns them.
            continue;
        }
        if unowned_non_finally_reachable {
            if source_kind == "exception_last" {
                // `exception_last` is also used by module/function exception-exit
                // cleanup blocks as a public observer of the active exception.
                // Mixed depth-zero and handler-owned reachability at such a site
                // does not make the value a handler MatchRef; ordinary value/drop
                // ownership handles it.
                continue;
            }
            facts.diagnostics.push(ExceptionRegionDiagnostic {
                kind: ExceptionRegionDiagnosticKind::AmbiguousProducerDepth,
                value,
                position: producer,
                message: format!(
                    "exception match ref v{} from {source_kind} is reachable with ambiguous exception-region owners: {:?}",
                    value.0, producer_states
                ),
            });
            facts.match_refs.insert(
                value,
                ExceptionMatchRefFact {
                    value,
                    producer,
                    releases: Vec::new(),
                    release_facts: Vec::new(),
                    source_kind: source_kind.to_string(),
                },
            );
            continue;
        }

        let mut producer_states_by_owner: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut unmapped_non_finally_state_reachable = false;
        for state in &producer_states {
            let Some(owner) = match_ref_release_owner(source_kind, state, &owning_tokens) else {
                if state.owners.is_empty() && state.normal_closures.is_empty() {
                    unmapped_non_finally_state_reachable = true;
                }
                continue;
            };
            producer_states_by_owner
                .entry(owner)
                .or_default()
                .push(state.clone());
        }

        if producer_states_by_owner.is_empty() {
            // Depth-zero exception reads are observers of pending/global
            // exception state, not handler-owned MatchRefs. They have no
            // handler-region `exception_pop` release boundary; ordinary
            // value/lifetime tracking owns them.
            continue;
        }

        if unmapped_non_finally_state_reachable && source_kind != "exception_last" {
            facts.diagnostics.push(ExceptionRegionDiagnostic {
                kind: ExceptionRegionDiagnosticKind::AmbiguousProducerDepth,
                value,
                position: producer,
                message: format!(
                    "exception match ref v{} from {source_kind} is reachable with ambiguous exception-region owners: {:?}",
                    value.0, producer_states
                ),
            });
            facts.match_refs.insert(
                value,
                ExceptionMatchRefFact {
                    value,
                    producer,
                    releases: Vec::new(),
                    release_facts: Vec::new(),
                    source_kind: source_kind.to_string(),
                },
            );
            continue;
        }

        let mut release_positions = BTreeSet::new();
        let mut release_facts = BTreeSet::new();
        let diagnostics_before = facts.diagnostics.len();
        for (owner, owner_states) in producer_states_by_owner {
            let release_candidates = reachable_region_pops(
                func,
                &label_to_block,
                &state_resume_stacks,
                producer,
                owner,
                &owner_states,
            );
            if release_candidates.is_empty() {
                if source_kind == "exception_last" {
                    continue;
                }
                facts.diagnostics.push(ExceptionRegionDiagnostic {
                    kind: ExceptionRegionDiagnosticKind::MatchWithoutReachablePop,
                    value,
                    position: producer,
                    message: format!(
                        "exception match ref v{} from {source_kind} owned by {:?} has no reachable exception_pop",
                        value.0, owner
                    ),
                });
                continue;
            }
            for (release_pos, entry_predecessors) in release_candidates {
                let entry_predecessors: Vec<_> = entry_predecessors.into_iter().collect();
                release_positions.insert(release_pos);
                release_facts.insert(ExceptionMatchRefRelease {
                    release: release_pos,
                    owner,
                    entry_predecessors: entry_predecessors.clone(),
                });
                facts
                    .release_to_match_facts
                    .entry(release_pos)
                    .or_default()
                    .push(ExceptionMatchReleaseFact {
                        value,
                        owner,
                        entry_predecessors,
                    });
            }
        }
        if release_facts.is_empty() && source_kind == "exception_last" {
            continue;
        }

        let releases: Vec<_> = release_positions.into_iter().collect();
        if releases.is_empty() && facts.diagnostics.len() == diagnostics_before {
            continue;
        }
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
                release_facts: release_facts.into_iter().collect(),
                source_kind: source_kind.to_string(),
            },
        );
    }
    for values in facts.release_to_matches.values_mut() {
        values.sort_unstable_by_key(|value| value.0);
        values.dedup();
    }
    for values in facts.release_to_match_facts.values_mut() {
        values.sort_unstable();
        values.dedup();
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

fn op_clears_pending_exception(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy && matches!(original_kind(op), Some("exception_clear"))
}

fn op_normal_fallthrough_reachable(state_before: &ExceptionPathState, op: &TirOp) -> bool {
    !(op.opcode == OpCode::CheckException && state_before.pending_must_transfer)
}

fn terminator_successor_state(
    label_to_block: &BTreeMap<i64, BlockId>,
    target: BlockId,
    state: &ExceptionPathState,
) -> ExceptionPathState {
    if state.pending_must_transfer
        && let Some((&label, _)) = label_to_block.iter().find(|(_, block)| **block == target)
        && let Some(handler_state) = state.enter_handler(label)
    {
        return handler_state;
    }
    state.clone()
}

fn op_exception_successors_with_state(
    label_to_block: &BTreeMap<i64, BlockId>,
    op: &TirOp,
    state: &ExceptionPathState,
) -> Vec<(BlockId, ExceptionPathState)> {
    if !super::dominators::is_exception_transfer_edge(op.opcode) {
        return Vec::new();
    }
    let Some(label) = label_value(op) else {
        return Vec::new();
    };
    let Some(&target) = label_to_block.get(&label) else {
        return Vec::new();
    };
    state
        .enter_handler(label)
        .into_iter()
        .map(|succ_state| (target, succ_state))
        .collect()
}

type ConstIntValues = BTreeMap<ValueId, i64>;

type ExceptionStack = Vec<ExceptionRegionToken>;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ExceptionPathState {
    frames: ExceptionStack,
    owners: ExceptionStack,
    normal_closures: ExceptionStack,
    pending_must_transfer: bool,
}

impl ExceptionPathState {
    fn enter_handler(&self, label: i64) -> Option<Self> {
        let owner = ExceptionRegionToken::Labeled(label);
        let mut next = self.clone();
        let index = next.frames.iter().rposition(|token| *token == owner)?;
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
            if let Some(token) = label_value(op).map(ExceptionRegionToken::Labeled) {
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

fn match_ref_release_owner(
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

type StateResumeStacks = BTreeMap<i64, BTreeSet<ExceptionPathState>>;

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
    state: &ExceptionPathState,
    state_resume_stacks: &StateResumeStacks,
    unknown_state: Option<&ExceptionPathState>,
) -> Vec<(BlockId, ExceptionPathState)> {
    match term {
        Terminator::Branch { target, .. } => {
            vec![(
                *target,
                terminator_successor_state(label_to_block, *target, state),
            )]
        }
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![
            (
                *then_block,
                terminator_successor_state(label_to_block, *then_block, state),
            ),
            (
                *else_block,
                terminator_successor_state(label_to_block, *else_block, state),
            ),
        ],
        Terminator::Switch { cases, default, .. } => {
            let mut successors = Vec::with_capacity(cases.len() + 1);
            successors.extend(cases.iter().map(|(_, target, _)| {
                (
                    *target,
                    terminator_successor_state(label_to_block, *target, state),
                )
            }));
            successors.push((
                *default,
                terminator_successor_state(label_to_block, *default, state),
            ));
            successors
        }
        Terminator::StateDispatch { cases, default, .. } => {
            let mut successors = Vec::with_capacity(cases.len() + 1);
            successors.push((
                *default,
                terminator_successor_state(label_to_block, *default, state),
            ));
            for (state, target, _) in cases {
                if let Some(stacks) = state_resume_stacks.get(state) {
                    successors.extend(stacks.iter().map(|resume_stack| {
                        (
                            *target,
                            terminator_successor_state(label_to_block, *target, resume_stack),
                        )
                    }));
                } else if let Some(fallback_state) = unknown_state {
                    successors.push((
                        *target,
                        terminator_successor_state(label_to_block, *target, fallback_state),
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
        for (succ, succ_state) in
            op_exception_successors_with_state(label_to_block, op, &next_state)
        {
            queue.push_back((succ, 0, succ_state));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state));
        }
    }
    observed
}

fn compute_state_resume_stacks(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
) -> StateResumeStacks {
    let const_int_values = collect_const_int_values(func);
    let mut stacks = StateResumeStacks::new();
    loop {
        let observed =
            collect_state_resume_stacks_once(func, label_to_block, &stacks, &const_int_values);
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

fn reachable_region_pops(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_stacks: &StateResumeStacks,
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
        for (succ, succ_state) in
            op_exception_successors_with_state(label_to_block, op, &next_state)
        {
            queue.push_back((succ, 0, succ_state, None));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state, entry_pred));
        }
    }
    candidates
}

fn path_states_before(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
    state_resume_stacks: &StateResumeStacks,
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
        for (succ, succ_state) in
            op_exception_successors_with_state(label_to_block, op, &next_state)
        {
            queue.push_back((succ, 0, succ_state));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state));
        }
    }
    states
}

pub fn exception_pop_owner_states(
    func: &TirFunction,
    target: ExceptionOpPosition,
) -> ExceptionPopOwnerStates {
    let label_to_block: BTreeMap<_, _> = dominators::exception_label_to_block(func)
        .into_iter()
        .collect();
    let state_resume_stacks = compute_state_resume_stacks(func, &label_to_block);
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
        for (succ, succ_state) in
            op_exception_successors_with_state(&label_to_block, op, &next_state)
        {
            let next_pred = (succ == target.block).then_some(block);
            queue.push_back((succ, 0, succ_state, next_pred));
        }
        if op_normal_fallthrough_reachable(&state, op) {
            queue.push_back((block, op_index + 1, next_state, pred_into_target));
        }
    }
    owners
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

    fn const_int(result: ValueId, value: i64) -> TirOp {
        let mut op = op(OpCode::ConstInt);
        op.results = vec![result];
        op.attrs.insert("value".into(), AttrValue::Int(value));
        op
    }

    fn state_yield(state: i64) -> TirOp {
        let mut op = op(OpCode::StateYield);
        op.attrs.insert("value".into(), AttrValue::Int(state));
        op
    }

    fn state_transition(awaitable: ValueId, slot: ValueId, pending_state: ValueId) -> TirOp {
        let mut op = op(OpCode::StateTransition);
        op.operands = vec![awaitable, slot, pending_state];
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

    fn mixed_exception_exit_observer_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "mixed_exception_exit_observer".into(),
            vec![],
            TirType::None,
        );
        let before_try = func.fresh_block();
        let exit_cleanup = func.fresh_block();
        func.label_id_map.insert(exit_cleanup.0, 3);
        let cond = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: before_try,
            then_args: vec![],
            else_block: exit_cleanup,
            else_args: vec![],
        };
        func.blocks.insert(
            before_try,
            TirBlock {
                id: before_try,
                args: vec![],
                ops: vec![try_start(3), check_exception(3)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            exit_cleanup,
            TirBlock {
                id: exit_cleanup,
                args: vec![],
                ops: vec![original("exception_last", vec![exc])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn mixed_pending_exit_observer_without_pop_function(
        source_kind: &str,
        name: &str,
    ) -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(name.into(), vec![], TirType::None);
        let before_try = func.fresh_block();
        let exit_cleanup = func.fresh_block();
        func.label_id_map.insert(exit_cleanup.0, 3);
        let cond = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: before_try,
            then_args: vec![],
            else_block: exit_cleanup,
            else_args: vec![],
        };
        func.blocks.insert(
            before_try,
            TirBlock {
                id: before_try,
                args: vec![],
                ops: vec![try_start(3), check_exception(3)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            exit_cleanup,
            TirBlock {
                id: exit_cleanup,
                args: vec![],
                ops: vec![original(source_kind, vec![exc])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc)
    }

    fn mixed_exception_pending_exit_observer_without_pop_function() -> (TirFunction, ValueId) {
        mixed_pending_exit_observer_without_pop_function(
            "exception_last_pending",
            "mixed_exception_pending_exit_observer_without_pop",
        )
    }

    fn mixed_finally_pending_exit_observer_without_pop_function() -> (TirFunction, ValueId) {
        mixed_pending_exit_observer_without_pop_function(
            "exception_finally_pending_observer",
            "mixed_finally_pending_exit_observer_without_pop",
        )
    }

    fn same_owner_with_different_outer_prefix_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "same_owner_with_different_outer_prefix".into(),
            vec![],
            TirType::None,
        );
        let direct_inner = func.fresh_block();
        let outer_then_inner = func.fresh_block();
        let handler_merge = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler_merge.0, 20);
        let cond = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: direct_inner,
            then_args: vec![],
            else_block: outer_then_inner,
            else_args: vec![],
        };
        func.blocks.insert(
            direct_inner,
            TirBlock {
                id: direct_inner,
                args: vec![],
                ops: vec![try_start(20), try_end(20)],
                terminator: Terminator::Branch {
                    target: handler_merge,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            outer_then_inner,
            TirBlock {
                id: outer_then_inner,
                args: vec![],
                ops: vec![try_start(10), try_start(20), try_end(20)],
                terminator: Terminator::Branch {
                    target: handler_merge,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            handler_merge,
            TirBlock {
                id: handler_merge,
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

    fn exception_edge_unwinds_to_target_handler_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "exception_edge_unwinds_to_target_handler".into(),
            vec![],
            TirType::None,
        );
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 10);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops =
            vec![try_start(10), try_start(20), check_exception(10)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
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

    fn forced_raise_check_exception_handler_branch_function() -> (TirFunction, ValueId, BlockId) {
        let mut func = TirFunction::new(
            "forced_raise_check_exception_handler_branch".into(),
            vec![],
            TirType::None,
        );
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 57);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops =
            vec![try_start(57), op(OpCode::Raise), check_exception(57)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: handler,
            args: vec![],
        };
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
        (func, exc, handler_pop)
    }

    fn explicit_raise_branch_to_labeled_handler_function() -> (TirFunction, ValueId, BlockId) {
        let mut func = TirFunction::new(
            "explicit_raise_branch_to_labeled_handler".into(),
            vec![],
            TirType::None,
        );
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 61);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops =
            vec![try_start(61), check_exception(61), op(OpCode::Raise)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: handler,
            args: vec![],
        };
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
        (func, exc, handler_pop)
    }

    fn inactive_check_exception_target_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "inactive_check_exception_target".into(),
            vec![],
            TirType::None,
        );
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 73);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![check_exception(73)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
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

    fn finally_cleanup_join_function() -> (TirFunction, ValueId, BlockId) {
        finally_cleanup_join_function_with_source("exception_finally_pending_observer")
    }

    fn finally_cleanup_join_function_with_source(
        source_kind: &str,
    ) -> (TirFunction, ValueId, BlockId) {
        let mut func = TirFunction::new("finally_cleanup_join".into(), vec![], TirType::None);
        let normal = func.fresh_block();
        let cleanup = func.fresh_block();
        let pop = func.fresh_block();
        func.label_id_map.insert(cleanup.0, 20);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(20)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
            target: normal,
            args: vec![],
        };
        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![try_end(20)],
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
                ops: vec![original(source_kind, vec![exc])],
                terminator: Terminator::Branch {
                    target: pop,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            pop,
            TirBlock {
                id: pop,
                args: vec![],
                ops: vec![original("exception_pop", vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        (func, exc, pop)
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
        let initial = func.fresh_block();
        let resume = func.fresh_block();
        let handler = func.fresh_block();
        let match_block = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 94);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
            cases: vec![(197, resume, vec![])],
            default: initial,
            default_args: vec![],
        };
        func.blocks.insert(
            initial,
            TirBlock {
                id: initial,
                args: vec![],
                ops: vec![try_start(94), state_yield(197)],
                terminator: Terminator::Unreachable,
            },
        );
        func.blocks.insert(
            resume,
            TirBlock {
                id: resume,
                args: vec![],
                ops: vec![check_exception(94)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![try_end(94)],
                terminator: Terminator::Branch {
                    target: match_block,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            match_block,
            TirBlock {
                id: match_block,
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

    fn repoll_state_resume_inside_try_function() -> (TirFunction, ValueId) {
        let mut func = TirFunction::new(
            "repoll_state_resume_inside_try".into(),
            vec![],
            TirType::None,
        );
        let initial = func.fresh_block();
        let resume = func.fresh_block();
        let handler = func.fresh_block();
        let handler_pop = func.fresh_block();
        func.label_id_map.insert(handler.0, 46);
        let awaitable = func.fresh_value();
        let slot = func.fresh_value();
        let pending = func.fresh_value();
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
            cases: vec![(47, resume, vec![])],
            default: initial,
            default_args: vec![],
        };
        func.blocks.insert(
            initial,
            TirBlock {
                id: initial,
                args: vec![],
                ops: vec![
                    try_start(46),
                    const_int(slot, 64),
                    const_int(pending, 47),
                    state_transition(awaitable, slot, pending),
                ],
                terminator: Terminator::Branch {
                    target: resume,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            resume,
            TirBlock {
                id: resume,
                args: vec![],
                ops: vec![check_exception(46)],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![try_end(46), original("exception_last", vec![exc])],
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
        assert_eq!(
            facts.release_to_match_facts[&ExceptionOpPosition {
                block: BlockId(3),
                op_index: 0,
            }],
            vec![ExceptionMatchReleaseFact {
                value: exc,
                owner: ExceptionRegionToken::Labeled(4),
                entry_predecessors: vec![BlockId(2)],
            }],
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
    fn exception_region_ignores_owned_exception_last_exit_observer_without_pop() {
        let mut func = TirFunction::new(
            "owned_exception_last_exit_observer".into(),
            vec![],
            TirType::None,
        );
        let cleanup = func.fresh_block();
        func.label_id_map.insert(cleanup.0, 3);
        let exc = func.fresh_value();

        func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(3)];
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![] };
        func.blocks.insert(
            cleanup,
            TirBlock {
                id: cleanup,
                args: vec![],
                ops: vec![original("exception_last", vec![exc])],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert!(!facts.match_refs.contains_key(&exc));
        assert!(facts.release_to_matches.is_empty());
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
    fn exception_region_ignores_mixed_exception_last_exit_observer() {
        let (func, exc) = mixed_exception_exit_observer_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert!(!facts.match_refs.contains_key(&exc));
        assert!(facts.release_to_matches.is_empty());
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_reports_overloaded_pending_observer_without_owner_pop() {
        let (func, exc) = mixed_exception_pending_exit_observer_without_pop_function();

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
        assert!(verify_exception_regions(&func).is_err());
    }

    #[test]
    fn exception_region_ignores_mixed_finally_pending_exit_observer_without_owner_pop() {
        let (func, exc) = mixed_finally_pending_exit_observer_without_pop_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert!(!facts.match_refs.contains_key(&exc));
        assert!(facts.release_to_matches.is_empty());
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_uses_top_region_owner_not_outer_stack_depth() {
        let (func, exc) = same_owner_with_different_outer_prefix_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(4),
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(4),
                op_index: 0,
            }],
            vec![exc],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_exception_edge_unwinds_to_target_handler_owner() {
        let (func, exc) = exception_edge_unwinds_to_target_handler_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(2),
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(2),
                op_index: 0,
            }],
            vec![exc],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_forced_raise_check_exception_has_no_fallthrough_owner() {
        let (func, exc, pop) = forced_raise_check_exception_handler_branch_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: pop,
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_match_facts[&ExceptionOpPosition {
                block: pop,
                op_index: 0,
            }],
            vec![ExceptionMatchReleaseFact {
                value: exc,
                owner: ExceptionRegionToken::Labeled(57),
                entry_predecessors: vec![BlockId(1)],
            }],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_pending_raise_branch_enters_labeled_handler_owner() {
        let (func, exc, pop) = explicit_raise_branch_to_labeled_handler_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: pop,
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_match_facts[&ExceptionOpPosition {
                block: pop,
                op_index: 0,
            }],
            vec![ExceptionMatchReleaseFact {
                value: exc,
                owner: ExceptionRegionToken::Labeled(61),
                entry_predecessors: vec![BlockId(1)],
            }],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_inactive_check_exception_target_does_not_fabricate_owner() {
        let (func, exc) = inactive_check_exception_target_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert!(!facts.match_refs.contains_key(&exc));
        assert!(facts.release_to_matches.is_empty());
        assert!(verify_exception_regions(&func).is_ok());
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
    fn exception_region_ignores_finally_pending_observer_cleanup_join() {
        let (func, exc, _pop) = finally_cleanup_join_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert!(!facts.match_refs.contains_key(&exc));
        assert!(facts.release_to_matches.is_empty());
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_allows_exception_last_finally_cleanup_join() {
        let (func, exc, pop) = finally_cleanup_join_function_with_source("exception_last");

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: pop,
                op_index: 0,
            }]
        );
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
    fn exception_region_state_resume_preserves_suspended_try_depth() {
        let (func, exc) = state_resume_inside_try_function();

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
    fn exception_region_repoll_state_resume_uses_pending_state_depth() {
        let (func, exc) = repoll_state_resume_inside_try_function();

        let facts = compute_exception_region_facts(&func);

        assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
        assert_eq!(
            facts.match_refs[&exc].releases,
            vec![ExceptionOpPosition {
                block: BlockId(4),
                op_index: 0,
            }]
        );
        assert_eq!(
            facts.release_to_matches[&ExceptionOpPosition {
                block: BlockId(4),
                op_index: 0,
            }],
            vec![exc],
        );
        assert!(verify_exception_regions(&func).is_ok());
    }

    #[test]
    fn exception_region_state_resume_stacks_are_bounded_by_lexical_try_token() {
        let mut func = TirFunction::new("state_resume_stack_cycle".into(), vec![], TirType::None);
        let initial = func.fresh_block();
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
            cases: vec![(7, initial, vec![])],
            default: initial,
            default_args: vec![],
        };
        func.blocks.insert(
            initial,
            TirBlock {
                id: initial,
                args: vec![],
                ops: vec![try_start(99), state_yield(7)],
                terminator: Terminator::Branch {
                    target: func.entry_block,
                    args: vec![],
                },
            },
        );

        let label_to_block: BTreeMap<_, _> = dominators::exception_label_to_block(&func)
            .into_iter()
            .collect();
        let stacks = compute_state_resume_stacks(&func, &label_to_block);

        assert_eq!(
            stacks.get(&7).cloned().unwrap_or_default(),
            BTreeSet::from([ExceptionPathState {
                frames: vec![ExceptionRegionToken::Labeled(99)],
                owners: Vec::new(),
                normal_closures: Vec::new(),
                pending_must_transfer: false,
            }]),
            "state-dispatch cycles must not manufacture duplicate lexical exception frames"
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
