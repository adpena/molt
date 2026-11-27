//! Shared operation-aware Python callback sites for call graph and CallFacts.
//!
//! Effects own callback capability; source roles own direct target identity.
//! Global mutation, exceptions, and deferred poll references are independent.
//! Consumers must rebuild after transforms, especially lifetime finalization.

#[cfg(test)]
use std::collections::BTreeMap;

use super::analysis::AnalysisManager;
use super::blocks::BlockId;
use super::call_targets::{gpu_runtime_result_type_for_op, static_call};
use super::function::TirFunction;
use super::op_kinds_generated::OpcodeEffects;
use super::ops::{OpCode, TirOp};
use super::passes::alias_analysis::AliasAnalysis;
use super::passes::effects::op_effects_with_types;
use super::passes::typed_slot_access::{self, LoadPurity};
use super::passes::value_identity::ScalarFacts;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallSiteTarget {
    /// A compiled function by its typed id (HF-847 slice 1): a direct call or
    /// a proven guarded call ([`static_call`]). Whether the id has a body in
    /// the module is the consumer's question: an extern row is compiled
    /// elsewhere and is opaque to it.
    Direct(molt_ir::FunctionId),
    Opaque,
    /// A fixed GPU helper remains a CallFacts site without a Python call edge.
    RuntimePrimitive,
}

#[derive(Clone, Debug)]
pub struct CallSite {
    pub target: CallSiteTarget,
    pub effects: OpcodeEffects,
}

#[cfg(test)]
#[derive(Default, Debug)]
pub struct FunctionCallSites {
    sites: BTreeMap<(BlockId, usize), CallSite>,
    terminators: BTreeMap<BlockId, CallSite>,
}

#[cfg(test)]
impl FunctionCallSites {
    pub fn for_function(func: &TirFunction) -> Self {
        let mut result = Self::default();
        visit_sites(func, |event| {
            match event {
                CallEvent::Operation { position, site, .. } => {
                    result.sites.insert(position, site);
                }
                CallEvent::Terminator { block, site } => {
                    result.terminators.insert(block, site);
                }
                CallEvent::PollReference(_) => {}
            }
            false
        });
        result
    }

    /// Whether this function has a synchronous Python callback edge.
    /// Fixed runtime primitives remain CallFacts sites without such an edge;
    /// conditional truthiness is covered even when no operation returns a value.
    pub fn may_call_python(&self) -> bool {
        !self.terminators.is_empty()
            || self
                .sites
                .values()
                .any(|site| site.target != CallSiteTarget::RuntimePrimitive)
    }

    pub fn at(&self, position: (BlockId, usize)) -> Option<&CallSite> {
        self.sites.get(&position)
    }

    pub fn terminator_at(&self, block: BlockId) -> Option<&CallSite> {
        self.terminators.get(&block)
    }
}

/// Prove callback freedom without materializing individual call sites.
pub fn may_call_python(func: &TirFunction) -> bool {
    visit_sites(func, |event| match event {
        CallEvent::Operation { site, .. } => site.target != CallSiteTarget::RuntimePrimitive,
        CallEvent::Terminator { .. } => true,
        CallEvent::PollReference(_) => false,
    })
}

pub(crate) enum CallEvent<'a> {
    Operation {
        position: (BlockId, usize),
        op: &'a TirOp,
        site: CallSite,
    },
    Terminator {
        #[cfg(test)]
        block: BlockId,
        #[cfg(test)]
        site: CallSite,
    },
    /// A callable reference for reachability, not a synchronous call.
    PollReference(molt_ir::FunctionId),
}

/// Stream call sites and deferred poll references in operation order.
/// A visitor returning true asks to stop early.
pub(crate) fn visit_sites(func: &TirFunction, mut stop: impl FnMut(CallEvent<'_>) -> bool) -> bool {
    // The shared plan skips alias/range computation without candidates.
    let mut analyses = AnalysisManager::new();
    let slots = typed_slot_access::for_function(func, &mut analyses);
    // Annotation-derived value_types can include overriding subclasses.
    let facts = analyses.cached::<AliasAnalysis>().map_or_else(
        || ScalarFacts::new(func),
        |alias| ScalarFacts::with_types(func, alias.exact_scalar_types()),
    );
    let exact = facts.types();
    for (&bid, block) in &func.blocks {
        for (index, op) in block.ops.iter().enumerate() {
            let position = (bid, index);
            let mut effects = op_effects_with_types(op, exact);
            if op.has_valid_shape()
                && !op.is_async_work_poll()
                && (slots.load_purity_at(position, op) == LoadPurity::ProvenPure
                    || (op.plain_typed_slot_store().is_some()
                        && slots.stores.contains_key(&position)))
            {
                // Store admission proves both old-slot release neutrality
                // and inline backing; a mere offset never suffices.
                effects.may_call_python = false;
            }
            let target = if gpu_runtime_result_type_for_op(op).is_some() {
                Some(CallSiteTarget::RuntimePrimitive)
            } else if !effects.may_call_python {
                None
            } else if let Some(call) = static_call(op) {
                Some(CallSiteTarget::Direct(call.callee))
            } else {
                Some(CallSiteTarget::Opaque)
            };
            if let Some(target) = target
                && stop(CallEvent::Operation {
                    position,
                    op,
                    site: CallSite { target, effects },
                })
            {
                return true;
            }
            if op.opcode == OpCode::AllocTask
                && let Some(poll) = op.callee
                && stop(CallEvent::PollReference(poll))
            {
                return true;
            }
        }
        let effects = super::op_semantics::terminator_effects(&block.terminator, exact);
        if effects.may_call_python
            && stop(CallEvent::Terminator {
                #[cfg(test)]
                block: bid,
                #[cfg(test)]
                site: CallSite {
                    target: CallSiteTarget::Opaque,
                    effects,
                },
            })
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
