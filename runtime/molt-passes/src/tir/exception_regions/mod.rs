//! Backend-neutral ExceptionRegion ownership facts.
//!
//! This analysis is the current backend-neutral authority for handler-owned
//! exception MatchRef release facts. Current TIR still carries several
//! exception-stack operations as `Copy` + `_original_kind`; this analysis
//! recognizes those carriers, computes the path-local match-ref release
//! boundary, feeds pass-manager diagnostics, and drives shared TIR drop
//! insertion on activated targets.
//!
//! The path-state CFG traversal engine that underpins these facts lives in
//! [`path_state`].

use std::collections::{BTreeMap, BTreeSet};

use super::analysis::{Analysis, AnalysisId};
use super::blocks::BlockId;
use super::dominators;
use super::function::TirFunction;
use super::values::ValueId;

use self::path_state::{
    AnonymousHandlerDestinations, StateResumeStacks, compute_state_resume_stacks,
    is_match_ref_source, iter_ops, label_value, lexical_handlers_before, match_ref_release_owner,
    original_kind, path_states_before, reachable_region_pops,
};

mod path_state;

#[cfg(test)]
mod tests;

pub use self::path_state::exception_pop_owner_states;

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

/// Total classification of lexical exception custody at one exact TIR
/// boundary. Absence from the path-state map is a proven unreachable boundary,
/// not a missing handler and not depth zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionBoundaryHandler {
    Unreachable,
    DepthZero,
    Labeled(i64),
    Anonymous {
        owner: ExceptionOpPosition,
        destination: i64,
    },
}

/// Reachable boundaries whose lexical custody or recovered destination is not
/// singular remain explicit fail-closed errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExceptionBoundaryHandlerError {
    AnonymousDestination {
        position: ExceptionOpPosition,
        owner: ExceptionOpPosition,
        destinations: BTreeSet<i64>,
    },
    Ambiguous {
        position: ExceptionOpPosition,
        states: BTreeSet<Option<ExceptionRegionToken>>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExceptionRegionFacts {
    /// Active lexical handler at every reachable op/block-exit boundary.
    /// Multiple entries are preserved so consumers can fail closed rather
    /// than guessing when CFG paths carry different exception stacks.
    lexical_handlers_before: BTreeMap<ExceptionOpPosition, BTreeSet<Option<ExceptionRegionToken>>>,
    /// Concrete label destinations recovered for legacy/hand-built unlabeled
    /// try regions. Canonical frontend regions carry this identity directly;
    /// this map keeps anonymous TIR owners representable without weakening
    /// backend label/cleanup invariants.
    anonymous_handler_destinations: BTreeMap<ExceptionOpPosition, BTreeSet<i64>>,
    pub match_refs: BTreeMap<ValueId, ExceptionMatchRefFact>,
    pub release_to_matches: BTreeMap<ExceptionOpPosition, Vec<ValueId>>,
    pub release_to_match_facts: BTreeMap<ExceptionOpPosition, Vec<ExceptionMatchReleaseFact>>,
    pub diagnostics: Vec<ExceptionRegionDiagnostic>,
}

impl ExceptionRegionFacts {
    /// Classify one op or block-exit boundary from the same path-state authority
    /// that owns exception-region reachability and lexical handler stacks.
    pub fn lexical_handler_before(
        &self,
        position: ExceptionOpPosition,
    ) -> Result<ExceptionBoundaryHandler, ExceptionBoundaryHandlerError> {
        let Some(states) = self.lexical_handlers_before.get(&position) else {
            return Ok(ExceptionBoundaryHandler::Unreachable);
        };
        if states.len() != 1 {
            return Err(ExceptionBoundaryHandlerError::Ambiguous {
                position,
                states: states.clone(),
            });
        }
        match states.iter().next().copied().flatten() {
            None => Ok(ExceptionBoundaryHandler::DepthZero),
            Some(ExceptionRegionToken::Labeled(label)) => {
                Ok(ExceptionBoundaryHandler::Labeled(label))
            }
            Some(ExceptionRegionToken::Anonymous(owner)) => {
                let destinations = self
                    .anonymous_handler_destinations
                    .get(&owner)
                    .cloned()
                    .unwrap_or_default();
                if destinations.len() == 1 {
                    Ok(ExceptionBoundaryHandler::Anonymous {
                        owner,
                        destination: *destinations.iter().next().unwrap(),
                    })
                } else {
                    Err(ExceptionBoundaryHandlerError::AnonymousDestination {
                        position,
                        owner,
                        destinations,
                    })
                }
            }
        }
    }
}

fn observe_anonymous_handler_destinations(
    func: &TirFunction,
    handlers: &BTreeMap<ExceptionOpPosition, BTreeSet<Option<ExceptionRegionToken>>>,
) -> BTreeMap<ExceptionOpPosition, BTreeSet<i64>> {
    let mut destinations: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for (position, states) in handlers {
        if states.len() != 1 {
            continue;
        }
        let Some(ExceptionRegionToken::Anonymous(owner)) = states.iter().next().copied().flatten()
        else {
            continue;
        };
        let Some(block) = func.blocks.get(&position.block) else {
            continue;
        };
        let candidate = block.ops.get(position.op_index).and_then(|op| {
            if dominators::is_exception_transfer_edge(op.opcode) {
                label_value(op)
            } else if op.opcode == super::ops::OpCode::TryEnd && label_value(op).is_none() {
                func.label_id_map.get(&position.block.0).copied()
            } else {
                None
            }
        });
        if let Some(label) = candidate {
            destinations.entry(owner).or_default().insert(label);
        }
    }
    destinations
}

fn exception_region_path_authority(
    func: &TirFunction,
    label_to_block: &BTreeMap<i64, BlockId>,
) -> (
    StateResumeStacks,
    BTreeMap<ExceptionOpPosition, BTreeSet<Option<ExceptionRegionToken>>>,
    AnonymousHandlerDestinations,
) {
    let mut anonymous_destinations = AnonymousHandlerDestinations::new();
    loop {
        let state_resume_stacks =
            compute_state_resume_stacks(func, label_to_block, &anonymous_destinations);
        let handlers = lexical_handlers_before(
            func,
            label_to_block,
            &state_resume_stacks,
            &anonymous_destinations,
        );
        let observed = observe_anonymous_handler_destinations(func, &handlers);
        let mut changed = false;
        for (owner, labels) in observed {
            let destinations = anonymous_destinations.entry(owner).or_default();
            for label in labels {
                changed |= destinations.insert(label);
            }
        }
        if !changed {
            return (state_resume_stacks, handlers, anonymous_destinations);
        }
    }
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
    let (state_resume_stacks, lexical_handlers_before, anonymous_handler_destinations) =
        exception_region_path_authority(func, &label_to_block);
    let mut facts = ExceptionRegionFacts {
        anonymous_handler_destinations: anonymous_handler_destinations.clone(),
        lexical_handlers_before,
        ..ExceptionRegionFacts::default()
    };
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
        let producer_states: Vec<_> = path_states_before(
            func,
            &label_to_block,
            &state_resume_stacks,
            &anonymous_handler_destinations,
            producer,
        )
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
                &anonymous_handler_destinations,
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
