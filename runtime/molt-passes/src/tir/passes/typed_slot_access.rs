//! Backend-neutral fixed-layout field access, ownership and presence facts.
//!
//! MemorySSA, MemGVN, DSE, module promotion and lowering share this exact-site
//! authority. Physical extent is not callback freedom; a scalar carrier is not
//! a boxed-slot RC proof. Observed roots never regain pristine write history.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisManager};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{
    build_pred_map, exception_label_to_block, exception_successors, executable_reverse_postorder,
    is_exception_transfer_edge,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    BoxedAllocationLayoutRule, opcode_boxed_allocation_layout_rule_table,
};
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::passes::alias_analysis::{AliasAnalysis, AliasAnalysisResult, MemRegion};
use crate::tir::passes::value_range::ValueRange;
use crate::tir::values::ValueId;

/// A position in the exact TIR stream supplied to `analyze`, not source origin.
pub type AccessSite = (BlockId, usize);

/// Whether the current access has a proven present word and inline backing.
/// This is an exact-site fact, never an opcode or field-shape classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPurity {
    ProvenPure,
    /// Includes plain reads with unknown/missing contents or dictionary backing,
    /// as well as guarded/generic attribute and index protocol operations.
    MayDispatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypedSlotStoreMode {
    /// A pristine receiver and a release-neutral old slot permit initialization
    /// semantics. A heap-valued incoming operand still needs its retain.
    FreshInit,
    /// Both field words are boxed-neutral, and no instance dict was observed.
    DirectNonHeap,
}

/// Physical fixed-layout allocation contract shared with whole-root consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedLayoutAllocation {
    pub result: ValueId,
    payload_bytes: i64,
    reserved_tail_bytes: i64,
    initial_value: OldSlotValue,
}

impl FixedLayoutAllocation {
    /// Every class-shaped payload reserves its trailing u64 for instance_dict.
    /// This is a boxed-word ABI, not the target's native pointer width. No
    /// class-name hint is used to infer additional subclass-specific layout.
    pub fn admits_field_offset(self, offset: i64) -> bool {
        let word = std::mem::size_of::<u64>() as i64;
        offset >= 0
            && offset % word == 0
            && offset.checked_add(word).is_some_and(|end| {
                self.payload_bytes
                    .checked_sub(self.reserved_tail_bytes)
                    .is_some_and(|limit| end <= limit)
            })
    }
}

/// Fixed boxed-word extent for raw and class-governed allocations. Raw storage
/// owns every payload word; only class-governed storage reserves a dictionary
/// tail. This shape fact does not prove callback-free construction or lifetime.
/// Raw fields start as boxed +0.0; owned class fields start as the immortal
/// missing singleton. Both are release-neutral, but only the latter is empty.
pub fn boxed_allocation_layout(op: &TirOp) -> Option<FixedLayoutAllocation> {
    let word = std::mem::size_of::<u64>() as i64;
    let (operand_count, reserved_tail_bytes, initial_value) =
        match opcode_boxed_allocation_layout_rule_table(op.opcode) {
            BoxedAllocationLayoutRule::RawZeroed => (0, 0, OldSlotValue::BoxedNeutral),
            BoxedAllocationLayoutRule::ClassMissing => (1, word, OldSlotValue::FreshEmpty),
            BoxedAllocationLayoutRule::None => return None,
        };
    let Some(AttrValue::Int(payload_bytes)) = op.attrs.get("value") else {
        return None;
    };
    if op.results.len() != 1
        || op.operands.len() != operand_count
        || *payload_bytes < word
        || *payload_bytes % word != 0
    {
        return None;
    }
    Some(FixedLayoutAllocation {
        result: op.results[0],
        payload_bytes: *payload_bytes,
        reserved_tail_bytes,
        initial_value,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldSlotValue {
    FreshEmpty,
    BoxedNeutral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedSlotStoreFacts {
    pub old_value: OldSlotValue,
    pub incoming_boxed_neutral: bool,
}

impl TypedSlotStoreFacts {
    pub fn lowering_mode(self) -> TypedSlotStoreMode {
        if self.old_value == OldSlotValue::BoxedNeutral && self.incoming_boxed_neutral {
            TypedSlotStoreMode::DirectNonHeap
        } else {
            TypedSlotStoreMode::FreshInit
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypedSlotAccessPlan {
    pub stores: BTreeMap<AccessSite, TypedSlotStoreFacts>,
    pub dead_stores: BTreeSet<AccessSite>,
    /// Initialized inline reads: neither dictionary lookup nor missing-field
    /// fallback can execute Python at these exact sites.
    pub loads: BTreeSet<AccessSite>,
}

impl TypedSlotAccessPlan {
    /// Region at an exact site in the analyzed stream. Only this plan may
    /// discharge load fallback or old-slot/dictionary release paths;
    /// context-free alias and effects queries deliberately remain conservative.
    pub fn region_at(
        &self,
        alias: &AliasAnalysisResult,
        site: AccessSite,
        op: &TirOp,
    ) -> MemRegion {
        if (self.stores.contains_key(&site) && op.plain_typed_slot_store().is_some())
            || self.load_purity_at(site, op) == LoadPurity::ProvenPure
        {
            alias.typed_slot_region(op)
        } else {
            alias.region_of(op)
        }
    }

    pub fn load_purity_at(&self, site: AccessSite, op: &TirOp) -> LoadPurity {
        if self.loads.contains(&site) && op.plain_typed_slot_load().is_some() {
            LoadPurity::ProvenPure
        } else {
            LoadPurity::MayDispatch
        }
    }
}

#[derive(Clone, Copy)]
enum SlotContents {
    Neutral(AccessSite),
    HeapOrUnknown { present: bool },
}

struct PristineObject {
    allocation: FixedLayoutAllocation,
    slots: HashMap<i64, SlotContents>,
    /// Reads preserve backing/presence knowledge, but cannot reestablish
    /// pristine write history or make previously observed stores removable.
    observed: bool,
}

/// Cached-analysis entry point shared by compiler transforms and backend plans.
pub fn for_function(func: &TirFunction, am: &mut AnalysisManager) -> TypedSlotAccessPlan {
    if !has_candidates(func) {
        return TypedSlotAccessPlan::default();
    }
    let alias = am.get::<AliasAnalysis>(func).clone();
    let ranges = am.get::<ValueRange>(func).clone();
    let boxed_neutral = crate::representation_facts::non_heap_boxed_values_for(func, &ranges);
    analyze(func, &alias, &boxed_neutral)
}

/// Standalone consumers share fact preparation. Do not compute value ranges
/// for functions without fixed-layout allocation/access candidates.
pub fn for_alias(func: &TirFunction, alias: &AliasAnalysisResult) -> TypedSlotAccessPlan {
    if !has_candidates(func) {
        return TypedSlotAccessPlan::default();
    }
    let ranges = ValueRange::compute(func);
    let boxed_neutral = crate::representation_facts::non_heap_boxed_values_for(func, &ranges);
    analyze(func, alias, &boxed_neutral)
}

fn has_candidates(func: &TirFunction) -> bool {
    let mut allocation = false;
    let mut access = false;
    for op in func.blocks.values().flat_map(|block| &block.ops) {
        allocation |= boxed_allocation_layout(op).is_some();
        access |= op.plain_typed_slot_store().is_some() || op.plain_typed_slot_load().is_some();
        if allocation && access {
            return true;
        }
    }
    false
}

/// Compute facts from current TIR and its matching alias/boxed-value proofs.
/// There is no backend whitelist, annotation fallback, or reconstructed
/// flow-insensitive slot history.
pub fn analyze(
    func: &TirFunction,
    alias: &AliasAnalysisResult,
    boxed_neutral_values: &HashSet<ValueId>,
) -> TypedSlotAccessPlan {
    let mut plan = TypedSlotAccessPlan::default();
    let predecessors = build_pred_map(func);
    let labels = exception_label_to_block(func);
    let mut exits: HashMap<BlockId, HashMap<ValueId, PristineObject>> = HashMap::new();
    for bid in executable_reverse_postorder(func) {
        let block = &func.blocks[&bid];
        // Carry exact history only through a single, unconditional normal
        // edge. Joins, backedges and mid-block exception transfers cannot mint
        // facts from a predecessor's end state. No iteration or path guessing.
        let mut pristine = predecessors
            .get(&bid)
            .and_then(|preds| {
                let [pred] = preds.as_slice() else {
                    return None;
                };
                let previous = &func.blocks[pred];
                if matches!(previous.terminator, Terminator::Branch { target, .. } if target == bid)
                    && exception_successors(previous, &labels).is_empty()
                {
                    exits.remove(pred)
                } else {
                    None
                }
            })
            .unwrap_or_default();
        for (index, op) in block.ops.iter().enumerate() {
            let site = (block.id, index);
            if is_exception_transfer_edge(op.opcode) {
                // A side exit can observe earlier writes even when the normal
                // path executes no callback. It is not an opcode heap effect.
                // Preserve normal-path presence but never erase those writes
                // using an overwrite that executes only after this edge.
                for object in pristine.values_mut() {
                    object.observed = true;
                }
            }
            let pure_load_root = op.plain_typed_slot_load().and_then(|(target, offset)| {
                let root = alias.root(target);
                let object = pristine.get(&root)?;
                (object.allocation.admits_field_offset(offset)
                    && match object.slots.get(&offset) {
                        Some(SlotContents::Neutral(_)) => true,
                        Some(SlotContents::HeapOrUnknown { present }) => *present,
                        None => object.allocation.initial_value == OldSlotValue::BoxedNeutral,
                    })
                .then_some(root)
            });
            let slot = op
                .plain_typed_slot_store()
                .map(|(target, offset)| (alias.root(target), offset));
            let facts = slot.and_then(|(root, offset)| {
                let object = pristine.get(&root)?;
                if object.observed
                    || !op.results.is_empty()
                    || !object.allocation.admits_field_offset(offset)
                {
                    return None;
                }
                let old_value = match object.slots.get(&offset) {
                    None => object.allocation.initial_value,
                    Some(SlotContents::Neutral(_)) => OldSlotValue::BoxedNeutral,
                    Some(SlotContents::HeapOrUnknown { .. }) => return None,
                };
                Some(TypedSlotStoreFacts {
                    old_value,
                    incoming_boxed_neutral: boxed_neutral_values.contains(&op.operands[1]),
                })
            });

            // Discharge all old-release paths only for a pristine allocation
            // with a proven boxed-neutral old word. The oracle still revokes a
            // different tracked root captured as the incoming stored value.
            if let Some(root) = pure_load_root {
                plan.loads.insert(site);
                pristine.get_mut(&root).unwrap().observed = true;
            } else {
                pristine.retain(|&root, _| {
                    !if facts.is_some() {
                        alias.may_observe_slot_with_boxed_neutral_old_value(op, root)
                    } else {
                        alias.may_observe_slot(op, root)
                    }
                });
            }

            if let Some((root, offset)) = slot {
                if let Some(facts) = facts
                    && let Some(object) = pristine.get_mut(&root)
                {
                    plan.stores.insert(site, facts);
                    let previous = object.slots.insert(
                        offset,
                        if facts.incoming_boxed_neutral {
                            SlotContents::Neutral(site)
                        } else {
                            SlotContents::HeapOrUnknown {
                                present: alias.is_known_present_value(op.operands[1]),
                            }
                        },
                    );
                    // DSE requires a neutral incoming write as well. Never
                    // erase a heap retain or use a releasing store to establish
                    // a new fact: its finalizer could rewrite the field.
                    if facts.incoming_boxed_neutral
                        && let Some(SlotContents::Neutral(previous)) = previous
                    {
                        plan.dead_stores.insert(previous);
                    }
                } else {
                    // Malformed/out-of-extent stores and contradicted init
                    // contracts cannot restore pristine slot or dict history.
                    pristine.remove(&root);
                }
            }

            if let Some(allocation) = boxed_allocation_layout(op) {
                pristine.insert(
                    alias.root(allocation.result),
                    PristineObject {
                        allocation,
                        slots: HashMap::new(),
                        observed: false,
                    },
                );
            }
        }
        exits.insert(bid, pristine);
    }
    plan
}

#[cfg(test)]
mod tests;
