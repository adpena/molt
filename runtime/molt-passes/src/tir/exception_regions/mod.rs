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
    compute_state_resume_stacks, is_match_ref_source, iter_ops, match_ref_release_owner,
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
