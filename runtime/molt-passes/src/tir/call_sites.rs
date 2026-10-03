//! Shared operation-aware Python callback sites for call graph and CallFacts.
//!
//! Effects own callback capability; source roles own direct target identity.
//! Global mutation, exceptions, and deferred poll references are independent.
//! Consumers must rebuild after transforms, especially lifetime finalization.

use std::collections::BTreeMap;

use super::analysis::AnalysisManager;
use super::blocks::BlockId;
use super::call_targets::{direct_call_symbol_for_op, gpu_runtime_result_type_for_op};
use super::function::TirFunction;
use super::op_kinds_generated::OpcodeEffects;
use super::passes::effects::op_effects_with_types;
use super::passes::typed_slot_access::{self, LoadPurity};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallSiteTarget {
    /// Source-proven symbol; module membership is resolved by the consumer.
    Direct(String),
    Opaque,
    /// A fixed GPU helper remains a CallFacts site without a Python call edge.
    RuntimePrimitive,
}

#[derive(Clone, Debug)]
pub struct CallSite {
    pub target: CallSiteTarget,
    pub effects: OpcodeEffects,
}

#[derive(Default, Debug)]
pub struct FunctionCallSites {
    sites: BTreeMap<(BlockId, usize), CallSite>,
    terminators: BTreeMap<BlockId, CallSite>,
}

impl FunctionCallSites {
    pub fn for_function(func: &TirFunction) -> Self {
        // Annotation-derived value_types can include overriding subclasses.
        let exact = super::type_refine::extract_exact_scalar_map(func);
        // The shared plan skips alias/range computation without candidates.
        let slots = typed_slot_access::for_function(func, &mut AnalysisManager::new());
        let mut sites = BTreeMap::new();
        let mut terminators = BTreeMap::new();
        for (&bid, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                let position = (bid, index);
                let mut effects = op_effects_with_types(op, &exact);
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
                    CallSiteTarget::RuntimePrimitive
                } else if !effects.may_call_python {
                    continue;
                } else if let Some(name) = direct_call_symbol_for_op(op) {
                    CallSiteTarget::Direct(name.to_owned())
                } else {
                    CallSiteTarget::Opaque
                };
                sites.insert(position, CallSite { target, effects });
            }
            let effects = super::op_semantics::terminator_effects(&block.terminator, &exact);
            if effects.may_call_python {
                terminators.insert(
                    bid,
                    CallSite {
                        target: CallSiteTarget::Opaque,
                        effects,
                    },
                );
            }
        }
        Self { sites, terminators }
    }

    pub fn at(&self, position: (BlockId, usize)) -> Option<&CallSite> {
        self.sites.get(&position)
    }

    pub fn terminator_at(&self, block: BlockId) -> Option<&CallSite> {
        self.terminators.get(&block)
    }
}

#[cfg(test)]
mod tests;
