use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use crate::ir::{FunctionIR, OpIR};
use crate::repr::{ContainerKind, ContainerStorageFact, ContainerStorageKind, Repr, ScalarKind};
use crate::tir::function::TirFunction;
use crate::tir::lir::{LirRepr, LirValue};
use crate::tir::lower_from_simple::lower_to_tir;
use crate::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction;
use crate::tir::lower_to_simple::SimpleValueNames;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::type_refine::refine_types;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

mod value_repr;

pub(crate) use value_repr::raw_i64_carrier_values_for;
#[cfg(test)]
pub(crate) use value_repr::raw_i64_safe_values_for;
pub use value_repr::{repr_by_value_for, value_range_for};

const PLAN_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PLAN_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone)]
struct PlanHasher(u64);

impl Default for PlanHasher {
    fn default() -> Self {
        Self(PLAN_HASH_OFFSET)
    }
}

impl PlanHasher {
    #[inline]
    fn mix_byte(&mut self, value: u8) {
        self.0 ^= u64::from(value);
        self.0 = self.0.wrapping_mul(PLAN_HASH_PRIME);
    }

    #[inline]
    fn mix_u64(&mut self, value: u64) {
        self.0 ^= value;
        self.0 = self.0.wrapping_mul(PLAN_HASH_PRIME);
    }
}

impl Hasher for PlanHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut offset = 0;
        while offset + 8 <= bytes.len() {
            let lane = u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .expect("eight-byte hash chunk"),
            );
            self.mix_u64(lane);
            offset += 8;
        }
        while offset < bytes.len() {
            self.mix_byte(bytes[offset]);
            offset += 1;
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.mix_byte(value);
    }

    fn write_u16(&mut self, value: u16) {
        self.mix_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.mix_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.mix_u64(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.mix_u64(value as u64);
    }
}

type PlanBuildHasher = BuildHasherDefault<PlanHasher>;
type PlanHashMap<K, V> = HashMap<K, V, PlanBuildHasher>;
type PlanHashSet<T> = HashSet<T, PlanBuildHasher>;

fn plan_hash_map<K, V>(capacity: usize) -> PlanHashMap<K, V> {
    HashMap::with_capacity_and_hasher(capacity, PlanBuildHasher::default())
}

fn plan_hash_set<T>(capacity: usize) -> PlanHashSet<T> {
    HashSet::with_capacity_and_hasher(capacity, PlanBuildHasher::default())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NameId(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScalarFactId(usize);

#[derive(Default)]
struct FunctionNameIndex<'a> {
    ids_by_name: PlanHashMap<&'a str, NameId>,
    names: Vec<&'a str>,
}

impl<'a> FunctionNameIndex<'a> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            ids_by_name: plan_hash_map(capacity),
            names: Vec::with_capacity(capacity),
        }
    }

    fn intern(&mut self, name: &'a str) -> NameId {
        if let Some(&id) = self.ids_by_name.get(name) {
            return id;
        }
        let id = NameId(self.names.len());
        self.names.push(name);
        self.ids_by_name.insert(name, id);
        id
    }

    fn get(&self, name: &str) -> Option<NameId> {
        self.ids_by_name.get(name).copied()
    }

    fn len(&self) -> usize {
        self.names.len()
    }
}

struct NameMarkSet {
    marks: Vec<u32>,
    epoch: u32,
}

impl NameMarkSet {
    fn new(name_count: usize) -> Self {
        Self {
            marks: vec![0; name_count],
            epoch: 1,
        }
    }

    fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.marks.fill(0);
            self.epoch = 1;
        }
    }

    #[inline]
    fn insert(&mut self, id: NameId) {
        self.marks[id.0] = self.epoch;
    }

    #[inline]
    fn contains(&self, id: NameId) -> bool {
        self.marks[id.0] == self.epoch
    }
}

struct NameWorkSet {
    marks: NameMarkSet,
    ids: Vec<NameId>,
}

impl NameWorkSet {
    fn new(name_count: usize) -> Self {
        Self {
            marks: NameMarkSet::new(name_count),
            ids: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.marks.clear();
        self.ids.clear();
    }

    fn insert(&mut self, id: NameId) -> bool {
        if self.marks.contains(id) {
            return false;
        }
        self.marks.insert(id);
        self.ids.push(id);
        true
    }

    fn contains(&self, id: NameId) -> bool {
        self.marks.contains(id)
    }

    fn iter(&self) -> impl Iterator<Item = NameId> + '_ {
        self.ids.iter().copied()
    }
}

#[derive(Clone, Copy)]
struct StoreVarEdge<'a> {
    target: &'a str,
    source: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct AliasEdge<'a> {
    out: &'a str,
    source: &'a str,
}

struct FunctionFactIndex<'a> {
    stores: Vec<StoreVarEdge<'a>>,
    aliases: Vec<AliasEdge<'a>>,
    output_ops: Vec<&'a OpIR>,
    data_ops: Vec<&'a OpIR>,
    store_index_ops: Vec<&'a OpIR>,
    sentinel_outputs: PlanHashSet<String>,
    delete_targets: PlanHashSet<String>,
}

impl<'a> FunctionFactIndex<'a> {
    fn for_function(func_ir: &'a FunctionIR) -> Self {
        let mut stores = Vec::with_capacity(func_ir.ops.len() / 4 + 1);
        let mut aliases = Vec::with_capacity(func_ir.ops.len() / 4 + 1);
        let mut output_ops = Vec::with_capacity(func_ir.ops.len());
        let mut data_ops = Vec::with_capacity(func_ir.ops.len());
        let mut store_index_ops = Vec::with_capacity(func_ir.ops.len() / 8 + 1);
        let mut sentinel_outputs = plan_hash_set(func_ir.ops.len() / 16 + 1);
        let mut delete_targets = plan_hash_set(func_ir.ops.len() / 8 + 1);

        for op in &func_ir.ops {
            if let Some(target) = store_var_target_name(op) {
                stores.push(StoreVarEdge {
                    target,
                    source: store_var_source_name(op),
                });
            }
            if op.kind == "delete_var"
                && let Some(target) = op.var.as_ref().or(op.out.as_ref())
            {
                delete_targets.insert(target.clone());
            }
            if let Some(out) = op.out.as_deref() {
                output_ops.push(op);
                if op.kind == "missing" {
                    sentinel_outputs.insert(out.to_string());
                }
                if !matches!(op.kind.as_str(), "store_var" | "delete_var") {
                    data_ops.push(op);
                }
                if let Some(source) = alias_source_name(op) {
                    aliases.push(AliasEdge { out, source });
                }
            }
            if op.kind == "store_index" {
                store_index_ops.push(op);
            }
        }

        Self {
            stores,
            aliases,
            output_ops,
            data_ops,
            store_index_ops,
            sentinel_outputs,
            delete_targets,
        }
    }

    fn has_scalar_alias_or_store_edges(&self) -> bool {
        !self.stores.is_empty() || !self.aliases.is_empty()
    }

    fn has_container_storage_edges(&self) -> bool {
        !self.stores.is_empty() || !self.aliases.is_empty() || !self.store_index_ops.is_empty()
    }

    fn needs_indexed_name_graph(&self) -> bool {
        self.has_scalar_alias_or_store_edges() || self.has_container_storage_edges()
    }
}

#[derive(Clone, Copy)]
struct IndexedStoreVarEdge {
    target: NameId,
    source: Option<NameId>,
}

#[derive(Clone, Copy)]
struct IndexedAliasEdge {
    out: NameId,
    source: NameId,
}

struct IndexedAliasGroup {
    out: NameId,
    sources: Vec<NameId>,
}

struct IndexedFunctionFactIndex<'a> {
    names: FunctionNameIndex<'a>,
    stores: Vec<IndexedStoreVarEdge>,
    alias_groups: Vec<IndexedAliasGroup>,
    alias_sources: Vec<bool>,
    alias_outputs: Vec<bool>,
}

impl<'a> IndexedFunctionFactIndex<'a> {
    fn for_function_facts(fact_index: &FunctionFactIndex<'a>) -> Self {
        let mut names = FunctionNameIndex::with_capacity(
            fact_index
                .stores
                .len()
                .saturating_mul(2)
                .saturating_add(fact_index.aliases.len().saturating_mul(2))
                .saturating_add(fact_index.data_ops.len().saturating_mul(3))
                .saturating_add(16),
        );
        let mut stores = Vec::with_capacity(fact_index.stores.len());
        for edge in &fact_index.stores {
            let target = names.intern(edge.target);
            let source = edge.source.map(|source| names.intern(source));
            stores.push(IndexedStoreVarEdge { target, source });
        }
        let mut aliases = Vec::with_capacity(fact_index.aliases.len());
        for edge in &fact_index.aliases {
            let out = names.intern(edge.out);
            let source = names.intern(edge.source);
            aliases.push(IndexedAliasEdge { out, source });
        }
        for op in &fact_index.data_ops {
            if let Some(out) = op.out.as_deref() {
                names.intern(out);
            }
            if let Some(var) = op.var.as_deref() {
                names.intern(var);
            }
            if let Some(args) = &op.args {
                for arg in args {
                    names.intern(arg);
                }
            }
        }
        for op in &fact_index.store_index_ops {
            if let Some(out) = op.out.as_deref() {
                names.intern(out);
            }
            if let Some(args) = &op.args {
                for arg in args {
                    names.intern(arg);
                }
            }
        }

        let mut alias_slots_by_out: Vec<Option<usize>> = vec![None; names.len()];
        let mut alias_groups: Vec<IndexedAliasGroup> = Vec::with_capacity(aliases.len());
        for edge in aliases {
            if let Some(slot) = alias_slots_by_out[edge.out.0] {
                alias_groups[slot].sources.push(edge.source);
            } else {
                let slot = alias_groups.len();
                alias_slots_by_out[edge.out.0] = Some(slot);
                alias_groups.push(IndexedAliasGroup {
                    out: edge.out,
                    sources: vec![edge.source],
                });
            }
        }
        let mut alias_sources = vec![false; names.len()];
        let mut alias_outputs = vec![false; names.len()];
        for group in &alias_groups {
            alias_outputs[group.out.0] = true;
            for source in &group.sources {
                alias_sources[source.0] = true;
            }
        }
        Self {
            names,
            stores,
            alias_groups,
            alias_sources,
            alias_outputs,
        }
    }
}

#[derive(Clone, Copy)]
enum StoreTargetMergeSource {
    Missing,
    Pending,
    Proven(NameId),
}

enum StoreTargetState {
    Proven(NameId),
    Pending(Option<NameId>),
    UnknownRelevant,
    UnknownIrrelevant,
}

fn unknown_store_target_state(relevant: impl FnOnce() -> bool) -> StoreTargetState {
    if relevant() {
        StoreTargetState::UnknownRelevant
    } else {
        StoreTargetState::UnknownIrrelevant
    }
}

fn merge_store_target_state(
    state: &mut StoreTargetState,
    source: StoreTargetMergeSource,
    relevant: impl Fn() -> bool,
    sources_match: impl Fn(NameId, NameId) -> bool,
) {
    match source {
        StoreTargetMergeSource::Missing => {
            *state = unknown_store_target_state(relevant);
        }
        StoreTargetMergeSource::Pending => match state {
            StoreTargetState::Proven(existing) => {
                *state = StoreTargetState::Pending(Some(*existing));
            }
            StoreTargetState::Pending(_) => {}
            StoreTargetState::UnknownRelevant | StoreTargetState::UnknownIrrelevant => {}
        },
        StoreTargetMergeSource::Proven(source) => match state {
            StoreTargetState::Proven(existing) => {
                if !sources_match(*existing, source) {
                    *state = unknown_store_target_state(relevant);
                }
            }
            StoreTargetState::Pending(known_source) => {
                if let Some(existing) = *known_source
                    && !sources_match(existing, source)
                {
                    *state = unknown_store_target_state(relevant);
                    return;
                }
                *known_source = Some(source);
            }
            StoreTargetState::UnknownRelevant | StoreTargetState::UnknownIrrelevant => {}
        },
    }
}

fn initial_store_target_state(
    source: StoreTargetMergeSource,
    relevant: impl FnOnce() -> bool,
) -> StoreTargetState {
    match source {
        StoreTargetMergeSource::Missing => unknown_store_target_state(relevant),
        StoreTargetMergeSource::Pending => {
            if relevant() {
                StoreTargetState::Pending(None)
            } else {
                StoreTargetState::UnknownIrrelevant
            }
        }
        StoreTargetMergeSource::Proven(source) => StoreTargetState::Proven(source),
    }
}

struct IndexedStoreTargetFacts {
    entries: Vec<(NameId, Option<NameId>)>,
    pending_entries: Vec<NameId>,
    none_targets: NameMarkSet,
    pending_targets: NameMarkSet,
}

impl IndexedStoreTargetFacts {
    fn new(name_count: usize, entry_capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(entry_capacity),
            pending_entries: Vec::with_capacity(entry_capacity),
            none_targets: NameMarkSet::new(name_count),
            pending_targets: NameMarkSet::new(name_count),
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.pending_entries.clear();
        self.none_targets.clear();
        self.pending_targets.clear();
    }

    fn target_is_none(&self, target: NameId) -> bool {
        self.none_targets.contains(target)
    }

    fn target_is_pending(&self, target: NameId) -> bool {
        self.pending_targets.contains(target)
    }
}

struct IndexedStoreTargetScratch {
    slots_by_name: Vec<Option<usize>>,
    states: Vec<(NameId, StoreTargetState)>,
}

impl IndexedStoreTargetScratch {
    fn new(name_count: usize, state_capacity: usize) -> Self {
        Self {
            slots_by_name: vec![None; name_count],
            states: Vec::with_capacity(state_capacity),
        }
    }

    fn merge(
        &mut self,
        target: NameId,
        source: StoreTargetMergeSource,
        relevant: impl Fn() -> bool,
        sources_match: impl Fn(NameId, NameId) -> bool,
    ) {
        if let Some(slot) = self.slots_by_name[target.0] {
            merge_store_target_state(&mut self.states[slot].1, source, relevant, sources_match);
            return;
        }
        let slot = self.states.len();
        self.slots_by_name[target.0] = Some(slot);
        self.states
            .push((target, initial_store_target_state(source, relevant)));
    }

    fn finish_into(&mut self, facts: &mut IndexedStoreTargetFacts) {
        facts.clear();
        for (target, state) in self.states.drain(..) {
            self.slots_by_name[target.0] = None;
            match state {
                StoreTargetState::Proven(fact) => facts.entries.push((target, Some(fact))),
                StoreTargetState::Pending(_) => {
                    facts.pending_targets.insert(target);
                    facts.pending_entries.push(target);
                }
                StoreTargetState::UnknownRelevant => {
                    facts.none_targets.insert(target);
                    facts.entries.push((target, None));
                }
                StoreTargetState::UnknownIrrelevant => {}
            }
        }
    }
}

struct IndexedScalarFacts {
    fact_ids_by_name: Vec<Option<ScalarFactId>>,
    facts: Vec<ScalarRepresentationFact>,
    ids_by_fact: PlanHashMap<ScalarRepresentationFact, ScalarFactId>,
    conflicted: Vec<bool>,
    weak: Vec<bool>,
}

impl IndexedScalarFacts {
    fn from_plan(plan: &ScalarRepresentationPlan, index: &IndexedFunctionFactIndex<'_>) -> Self {
        let mut indexed = Self {
            fact_ids_by_name: Vec::with_capacity(index.names.len()),
            facts: Vec::new(),
            ids_by_fact: plan_hash_map(plan.facts_by_name.len().saturating_add(1)),
            conflicted: Vec::with_capacity(index.names.len()),
            weak: Vec::with_capacity(index.names.len()),
        };
        for name in &index.names.names {
            let fact_id = plan
                .facts_by_name
                .get(*name)
                .cloned()
                .map(|fact| indexed.intern_fact(fact));
            indexed
                .conflicted
                .push(plan.conflicted_names.contains(*name));
            indexed
                .weak
                .push(fact_id.is_some() && plan.weak_fact_names.contains(*name));
            indexed.fact_ids_by_name.push(fact_id);
        }
        indexed
    }

    fn intern_fact(&mut self, fact: ScalarRepresentationFact) -> ScalarFactId {
        if let Some(&id) = self.ids_by_fact.get(&fact) {
            return id;
        }
        let id = ScalarFactId(self.facts.len());
        self.facts.push(fact.clone());
        self.ids_by_fact.insert(fact, id);
        id
    }

    fn fact_id(&self, id: NameId) -> Option<ScalarFactId> {
        self.fact_ids_by_name.get(id.0).and_then(|fact| *fact)
    }

    fn fact(&self, id: ScalarFactId) -> &ScalarRepresentationFact {
        &self.facts[id.0]
    }

    fn contains(&self, id: NameId) -> bool {
        self.fact_id(id).is_some()
    }

    fn conflict(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.weak[id.0] = false;
        self.fact_ids_by_name[id.0] = None;
        self.conflicted[id.0] = true;
        true
    }

    fn clear_fact(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.weak[id.0] = false;
        self.fact_ids_by_name[id.0].take().is_some()
    }

    fn insert_graph_fact_id(&mut self, id: NameId, fact_id: ScalarFactId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        if let Some(existing) = self.fact_ids_by_name[id.0] {
            if existing == fact_id {
                let was_weak = self.weak[id.0];
                self.weak[id.0] = false;
                return was_weak;
            }
            if self.weak[id.0] {
                self.fact_ids_by_name[id.0] = Some(fact_id);
                self.weak[id.0] = false;
                return true;
            }
            let existing_is_top = self.fact(existing).is_dynbox_top();
            let incoming_is_top = self.fact(fact_id).is_dynbox_top();
            if existing_is_top {
                return false;
            }
            if incoming_is_top {
                self.fact_ids_by_name[id.0] = Some(fact_id);
                self.weak[id.0] = false;
                return true;
            }
            self.fact_ids_by_name[id.0] = None;
            self.weak[id.0] = false;
            self.conflicted[id.0] = true;
            return true;
        }
        self.fact_ids_by_name[id.0] = Some(fact_id);
        self.weak[id.0] = false;
        true
    }

    fn sync_to_plan(
        self,
        plan: &mut ScalarRepresentationPlan,
        index: &IndexedFunctionFactIndex<'_>,
    ) {
        for (slot, name) in index.names.names.iter().enumerate() {
            if self.conflicted[slot] {
                plan.facts_by_name.remove(*name);
                plan.weak_fact_names.remove(*name);
                plan.conflicted_names.insert((*name).to_string());
                continue;
            }
            match self.fact_ids_by_name[slot] {
                Some(fact_id) => {
                    plan.facts_by_name
                        .insert((*name).to_string(), self.fact(fact_id).clone());
                    if self.weak[slot] {
                        plan.weak_fact_names.insert((*name).to_string());
                    } else {
                        plan.weak_fact_names.remove(*name);
                    }
                }
                None => {
                    plan.facts_by_name.remove(*name);
                    plan.weak_fact_names.remove(*name);
                }
            }
        }
    }
}

struct IndexedContainerFacts {
    facts: Vec<Option<ContainerStorageFact>>,
    conflicted: Vec<bool>,
}

impl IndexedContainerFacts {
    fn from_plan(plan: &ScalarRepresentationPlan, index: &IndexedFunctionFactIndex<'_>) -> Self {
        let mut facts = Vec::with_capacity(index.names.len());
        let mut conflicted = Vec::with_capacity(index.names.len());
        for name in &index.names.names {
            facts.push(plan.container_storage_by_name.get(*name).cloned());
            conflicted.push(plan.container_storage_conflicted_names.contains(*name));
        }
        Self { facts, conflicted }
    }

    fn get(&self, id: NameId) -> Option<&ContainerStorageFact> {
        self.facts.get(id.0).and_then(Option::as_ref)
    }

    fn contains(&self, id: NameId) -> bool {
        self.get(id).is_some()
    }

    fn kind(&self, id: NameId) -> Option<ContainerStorageKind> {
        self.get(id).map(|fact| fact.kind)
    }

    fn conflict(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.facts[id.0] = None;
        self.conflicted[id.0] = true;
        true
    }

    fn clear_fact(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.facts[id.0].take().is_some()
    }

    fn insert(&mut self, id: NameId, fact: ContainerStorageFact) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        if let Some(existing) = self.facts[id.0].as_ref() {
            if existing != &fact {
                self.facts[id.0] = None;
                self.conflicted[id.0] = true;
                return true;
            }
            return false;
        }
        self.facts[id.0] = Some(fact);
        true
    }

    fn sync_to_plan(
        self,
        plan: &mut ScalarRepresentationPlan,
        index: &IndexedFunctionFactIndex<'_>,
    ) {
        for (slot, name) in index.names.names.iter().enumerate() {
            if self.conflicted[slot] {
                plan.container_storage_by_name.remove(*name);
                plan.container_storage_conflicted_names
                    .insert((*name).to_string());
                continue;
            }
            match self.facts[slot].clone() {
                Some(fact) => {
                    plan.container_storage_by_name
                        .insert((*name).to_string(), fact);
                }
                None => {
                    plan.container_storage_by_name.remove(*name);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct I64Interval {
    min: i64,
    max: i64,
}

impl I64Interval {
    fn singleton(value: i64) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    fn from_i128_bounds(min: i128, max: i128) -> Option<Self> {
        if min > max || min < i64::MIN as i128 || max > i64::MAX as i128 {
            return None;
        }
        Some(Self {
            min: min as i64,
            max: max as i64,
        })
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Widening join (abstract-interpretation `∇`): like [`union`], but any
    /// bound that would *grow* (a lower bound moving down, an upper bound moving
    /// up) is pushed straight to the `i64` extremum instead of inching outward.
    /// This guarantees the interval fixpoint terminates even when a value is
    /// updated self-referentially without a loop back-edge — e.g. a fully
    /// UNROLLED accumulator `total = total + k` repeated in straight-line code,
    /// where every clone writes the same SimpleIR variable, so each fixpoint
    /// pass widens the variable's range by another step and a plain monotone
    /// `union` never converges (an infinite-loop compile hang). Widening to
    /// ±i64 is conservative-correct: an unbounded interval simply means the
    /// value is not proven to fit the inline-int representation, so codegen
    /// falls back to the BigInt-correct boxed path (bug #15 floor) — never a
    /// miscompile.
    ///
    /// [`union`]: I64Interval::union
    fn widen(self, other: Self) -> Self {
        Self {
            min: if other.min < self.min {
                i64::MIN
            } else {
                self.min
            },
            max: if other.max > self.max {
                i64::MAX
            } else {
                self.max
            },
        }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Self::from_i128_bounds(
            self.min as i128 + other.min as i128,
            self.max as i128 + other.max as i128,
        )
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Self::from_i128_bounds(
            self.min as i128 - other.max as i128,
            self.max as i128 - other.min as i128,
        )
    }

    fn singleton_value(self) -> Option<i64> {
        (self.min == self.max).then_some(self.min)
    }
}

/// A typed representation fact for a name in the legacy SimpleIR namespace.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ScalarRepresentationFact {
    pub(crate) ty: TirType,
    pub(crate) repr: LirRepr,
}

impl ScalarRepresentationFact {
    fn is_dynbox_top(&self) -> bool {
        matches!(self.ty, TirType::DynBox) && self.repr == LirRepr::DynBox
    }

    fn scalar_kind(&self) -> Option<ScalarKind> {
        match (&self.ty, self.repr) {
            (TirType::I64, LirRepr::I64) => Some(ScalarKind::Int),
            (TirType::Bool, LirRepr::Bool1) => Some(ScalarKind::Bool),
            (TirType::F64, LirRepr::F64) => Some(ScalarKind::Float),
            (TirType::Str, _) => Some(ScalarKind::Str),
            (TirType::None, _) => Some(ScalarKind::NoneValue),
            _ => None,
        }
    }

    fn container_kind(&self) -> Option<ContainerKind> {
        match &self.ty {
            TirType::List(_) => Some(ContainerKind::List),
            TirType::Dict(_, _) => Some(ContainerKind::Dict),
            TirType::Set(_) => Some(ContainerKind::Set),
            TirType::Tuple(_) => Some(ContainerKind::Tuple),
            TirType::Str => Some(ContainerKind::Str),
            _ => None,
        }
    }
}

/// A backend's read-only view of final typed representation facts.
///
/// This is built from the exact `FunctionIR` that a backend is about to lower,
/// after module-level TIR roundtrip and post-TIR SimpleIR rewrites have already
/// run. It deliberately does not trust transport hints (`fast_int`,
/// `fast_float`, or `type_hint`) as representation authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScalarRepresentationPlan {
    facts_by_name: PlanHashMap<String, ScalarRepresentationFact>,
    conflicted_names: PlanHashSet<String>,
    /// Names whose current fact came from a SYNTHETIC (canonical fallback)
    /// name — `_v{N}` / `_bb{N}_arg{I}` from the internal re-lift. Weak facts
    /// yield to explicit-stream-name facts instead of conflicting them out
    /// (see [`Self::insert_lir_value_weak`]).
    weak_fact_names: PlanHashSet<String>,
    non_scalar_names: PlanHashSet<String>,
    integer_family_names: PlanHashSet<String>,
    container_storage_by_name: PlanHashMap<String, ContainerStorageFact>,
    container_storage_conflicted_names: PlanHashSet<String>,
    container_storage_ops: PlanHashMap<usize, ContainerStorageFact>,
    /// The representation lattice element per SimpleIR name — the single source
    /// of truth for native scalar-carrier classification. Integer names floor to
    /// [`Repr::MaybeBigInt`] and can be raised to [`Repr::RawI64Safe`] or
    /// [`Repr::RawI64FullDeopt`]. Bool/F64 names floor to boxed
    /// [`Repr::DynBox`] in this name-keyed native authority and are raised to
    /// [`Repr::Bool`] / [`Repr::FloatUnboxed`] only by the explicit raw-carrier
    /// eligibility filters. The `primary_names.*` sets are views over this map;
    /// see [`Self::primary_name_sets`].
    repr_by_name: PlanHashMap<String, Repr>,
    scalar_slot_exclusion_unsafe: PlanHashSet<String>,
    scalar_store_targets_by_kind: BTreeMap<ScalarKind, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScalarPrimaryNameSets {
    /// Native raw-i64 carrier names: `RawI64Safe ∪ RawI64FullDeopt`.
    pub int: BTreeSet<String>,
    /// Inline-int47-safe raw carriers. This is the name-keyed seed for
    /// value-keyed `RawI64Safe` propagation.
    pub int_inline_safe: BTreeSet<String>,
    /// Full-i64 checked-overflow raw carriers from overflow-peel loops.
    pub int_full_deopt: BTreeSet<String>,
    pub bool_: BTreeSet<String>,
    pub float: BTreeSet<String>,
}

/// Per-function representation facts consumed by the LLVM backend.
///
/// The LLVM backend lowers `TirFunction` (SSA `ValueId`s) directly. This struct
/// carries the same value-keyed representation proof the WASM/LIR path consumes:
/// every backend decision here is derived from the post-pipeline TIR function
/// being lowered.
///
/// This makes the LLVM backend consume the identical typed facts the
/// native/WASM/Luau backends consume, rather than treating `TirType::I64` as an
/// exact-i64 carrier (which it is not — `type_refine` assigns `add(I64, I64) ->
/// I64` with no overflow proof, so unbounded integer arithmetic must stay
/// boxed/runtime-backed until a range proof exists).
#[derive(Clone, Debug, Default)]
#[cfg(feature = "llvm")]
pub struct LlvmReprFacts {
    /// The representation lattice element per TIR `ValueId`: the value-keyed
    /// source of truth. Every value floors to [`Repr::default_for`] of its refined
    /// `TirType`; inline-int47 carriers raise to [`Repr::RawI64Safe`], while
    /// checked overflow-peel carriers raise to [`Repr::RawI64FullDeopt`].
    ///
    /// The raw-i64 tiers are seeded from value-range and checked-op proofs, then
    /// propagated across TIR SSA identity edges: through `Copy` chains and block
    /// arguments (phis). Dataflow propagation is what lets the backend keep
    /// unproven accumulators boxed (`MaybeBigInt`/`DynBox`) while preserving
    /// proven raw carriers.
    pub repr_by_value: HashMap<ValueId, Repr>,
}

#[cfg(feature = "llvm")]
impl LlvmReprFacts {
    /// Build the LLVM representation facts from the post-pipeline TIR function
    /// the LLVM backend is about to lower.
    pub fn build(tir_func: &TirFunction) -> Self {
        let vr = value_range_for(tir_func);
        let repr_by_value = repr_by_value_for(tir_func, Some(&vr));
        Self { repr_by_value }
    }

    /// Whether the value `id` is an inline-int47-safe raw i64 carrier: the
    /// `{RawI64Safe}` view over `repr_by_value`.
    pub fn is_inline_safe_int(&self, id: ValueId) -> bool {
        self.repr_by_value
            .get(&id)
            .is_some_and(|repr| repr.is_raw_i64_safe())
    }

    /// Whether the value `id` is the full-range checked-overflow raw i64 tier.
    pub fn is_full_deopt_int(&self, id: ValueId) -> bool {
        self.repr_by_value
            .get(&id)
            .is_some_and(|repr| repr.is_raw_i64_full_deopt())
    }

    /// Whether the value `id` is any bare-i64 carrier. Box sites must still
    /// distinguish inline-safe and full-deopt tiers.
    pub fn is_raw_int_carrier(&self, id: ValueId) -> bool {
        self.repr_by_value
            .get(&id)
            .is_some_and(|repr| repr.is_raw_i64_carrier())
    }

    /// The **effective** parameter carrier types `tir_func`'s callers must
    /// coerce arguments to, per this function's own value-range proof.
    ///
    /// A declared `int` parameter (`TirType::I64`) is a *semantic* int with no
    /// representation proof attached; it is sound as a raw-i64 carrier only when
    /// the value-range analysis proves its entire range fits the 47-bit inline
    /// payload (`is_inline_safe_int`). An unproven `int` param can receive a
    /// heap BigInt and therefore MUST be carried `DynBox` (NaN-boxed) across the
    /// call boundary: the caller passes the boxed value unchanged and the callee
    /// body uses it boxed. This is the parameter-ABI twin of
    /// [`FunctionLowering::effective_block_arg_type`] (which makes the same
    /// decision for loop-carried phis), and the SAME `repr_by_value` proof feeds
    /// both, so the caller's coercion target and the callee's entry-param carrier
    /// can never disagree. Non-`I64` declared types pass through unchanged.
    pub fn effective_param_types(&self, tir_func: &TirFunction) -> Vec<TirType> {
        let entry_args = &tir_func.blocks[&tir_func.entry_block].args;
        tir_func
            .param_types
            .iter()
            .enumerate()
            .map(|(i, declared)| {
                let proven_safe = entry_args
                    .get(i)
                    .is_some_and(|arg| self.is_inline_safe_int(arg.id));
                if matches!(declared, TirType::I64) && !proven_safe {
                    TirType::DynBox
                } else {
                    declared.clone()
                }
            })
            .collect()
    }
}

impl ScalarRepresentationPlan {
    fn with_capacity(op_count: usize) -> Self {
        let name_capacity = op_count.saturating_mul(2).max(16);
        Self {
            facts_by_name: plan_hash_map(name_capacity),
            conflicted_names: plan_hash_set(op_count / 4 + 1),
            weak_fact_names: plan_hash_set(name_capacity),
            non_scalar_names: plan_hash_set(op_count / 4 + 1),
            integer_family_names: plan_hash_set(name_capacity),
            container_storage_by_name: plan_hash_map(op_count / 2 + 1),
            container_storage_conflicted_names: plan_hash_set(op_count / 8 + 1),
            container_storage_ops: plan_hash_map(op_count / 8 + 1),
            repr_by_name: plan_hash_map(name_capacity),
            scalar_slot_exclusion_unsafe: plan_hash_set(op_count / 4 + 1),
            scalar_store_targets_by_kind: BTreeMap::new(),
        }
    }

    pub fn for_function_ir(func_ir: &FunctionIR) -> Self {
        if is_cold_module_chunk_function(&func_ir.name) {
            return Self::with_capacity(func_ir.ops.len());
        }

        let fact_index = FunctionFactIndex::for_function(func_ir);
        let indexed_fact_index = fact_index
            .needs_indexed_name_graph()
            .then(|| IndexedFunctionFactIndex::for_function_facts(&fact_index));
        let mut tir_func = lower_to_tir(func_ir);
        refine_types(&mut tir_func);
        let names = SimpleValueNames::for_function(&tir_func);
        // Fact extraction uses the semantic type floor because this call builds
        // the name-keyed scalar plan that later feeds `repr_by_value`; consuming
        // the proven value map here would make the analysis circular.
        let lir_func = lower_function_to_lir_for_repr_fact_extraction(&tir_func);

        let mut plan = Self::with_capacity(func_ir.ops.len());
        plan.seed_container_storage_from_tir(&tir_func, &names);
        let mut block_ids: Vec<_> = lir_func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|block_id| block_id.0);
        for block_id in block_ids {
            let block = &lir_func.blocks[&block_id];
            for (index, arg) in block.args.iter().enumerate() {
                // Both arg-name forms are synthetic fallbacks (a re-lift
                // renumbers ValueIds and BlockIds), so they insert WEAK facts:
                // an explicit stream name carried by a `_simple_out` /
                // `_simple_result_N` override must win over a colliding
                // canonical name, not be conflicted out by it.
                plan.insert_lir_value_weak(names.value_name(arg.id), arg);
                plan.insert_lir_value_weak(names.block_arg_slot(block.id, index), arg);
            }
            for op in &block.ops {
                let checked_i64_arithmetic = matches!(
                    op.tir_op.attrs.get("lir.checked_overflow"),
                    Some(AttrValue::Bool(true))
                );
                for (index, result) in op.result_values.iter().enumerate() {
                    if checked_i64_arithmetic && index == 0 {
                        // The checked-overflow result's repr is loop-carried-
                        // ambiguous; register it WEAK under the canonical
                        // `_v{id}` name so it never displaces or conflicts out
                        // the strong `_simple_out` fact inserted below (a weak
                        // insert under the override name would collide with the
                        // block-arg weak insert above and blacklist the carrier).
                        // The collision resolver in `SimpleValueNames` keeps a
                        // checked-overflow result's emitted name equal to this
                        // canonical name whenever it is free, so the int-carrier
                        // lookup still resolves; on the rare collision the strong
                        // `_simple_out` fact (keyed on the emitted name) carries it.
                        plan.insert_lir_value_weak(
                            SimpleValueNames::canonical_value_name(result.id),
                            result,
                        );
                    } else if names.has_override(result.id) {
                        plan.insert_lir_value(names.value_name(result.id), result);
                    } else {
                        plan.insert_lir_value_weak(names.value_name(result.id), result);
                    }
                }
                if op.result_values.len() == 1
                    && let Some(AttrValue::Str(simple_out)) = op.tir_op.attrs.get("_simple_out")
                    && let Some(result) = op.result_values.first()
                {
                    plan.insert_lir_value(simple_out.clone(), result);
                }
            }
        }
        // Restore the true container kind for constructor outputs before alias
        // propagation, so a `set`/`dict`/`list`/`tuple` built by `set_new`/etc.
        // (lifted to a type-aliasing `OpCode::Copy` passthrough) is not mistyped
        // as its first element — the root of the membership-dispatch miscompile.
        plan.seed_container_constructor_facts(func_ir);
        if fact_index.has_scalar_alias_or_store_edges()
            && let Some(indexed_fact_index) = indexed_fact_index.as_ref()
        {
            plan.propagate_simple_aliases(indexed_fact_index);
        }
        plan.propagate_integer_family(func_ir, &fact_index);
        if fact_index.has_container_storage_edges()
            && let Some(indexed_fact_index) = indexed_fact_index.as_ref()
        {
            plan.propagate_container_storage(&fact_index, indexed_fact_index);
        }
        plan.mark_container_storage_ops(func_ir);
        plan.scalar_slot_exclusion_unsafe = plan.compute_scalar_slot_exclusion_unsafe(func_ir);
        plan.scalar_store_targets_by_kind = plan.compute_scalar_store_targets(&fact_index);
        plan.seed_repr_by_name(func_ir, &fact_index);
        plan
    }

    /// Compute the integer/bool/float raw-carrier sets and translate all native
    /// name-keyed scalar carrier tiers into the `repr_by_name` source of truth.
    /// Integer names floor to `MaybeBigInt` and are raised by range/overflow-peel
    /// proofs. Bool/F64 names floor to boxed `DynBox` here and are raised only
    /// by the raw-bool/raw-f64 eligibility filters, so semantic type facts alone
    /// cannot authorize unboxed native storage.
    fn seed_repr_by_name(&mut self, func_ir: &FunctionIR, fact_index: &FunctionFactIndex<'_>) {
        let primary = self.compute_primary_name_sets(func_ir, fact_index);
        let mut repr_by_name =
            plan_hash_map(self.facts_by_name.len().saturating_add(primary.int.len()));
        for (name, fact) in &self.facts_by_name {
            repr_by_name.insert(name.clone(), Self::name_keyed_repr_floor(&fact.ty));
        }
        // Raise inline-safe first, then full-deopt so the more precise checked
        // overflow tier wins when a name appears in both through alias/store
        // propagation.
        for name in &primary.int_inline_safe {
            repr_by_name.insert(name.clone(), Repr::RawI64Safe);
        }
        for name in &primary.int_full_deopt {
            repr_by_name.insert(name.clone(), Repr::RawI64FullDeopt);
        }
        for name in &primary.bool_ {
            repr_by_name.insert(name.clone(), Repr::Bool);
        }
        for name in &primary.float {
            repr_by_name.insert(name.clone(), Repr::FloatUnboxed);
        }
        self.repr_by_name = repr_by_name;
    }

    fn name_keyed_repr_floor(ty: &TirType) -> Repr {
        match ty {
            TirType::Bool | TirType::F64 => Repr::DynBox,
            _ => Repr::default_for(ty),
        }
    }

    pub fn scalar_name_sets(
        &self,
    ) -> (
        BTreeSet<String>,
        BTreeSet<String>,
        BTreeSet<String>,
        BTreeSet<String>,
        BTreeSet<String>,
    ) {
        let mut int_like = BTreeSet::new();
        let mut bool_like = BTreeSet::new();
        let mut float_like = BTreeSet::new();
        let mut str_like = BTreeSet::new();
        let mut none_like = BTreeSet::new();
        for (name, fact) in &self.facts_by_name {
            match fact.scalar_kind() {
                Some(ScalarKind::Int) => {
                    int_like.insert(name.clone());
                }
                Some(ScalarKind::Bool) => {
                    bool_like.insert(name.clone());
                }
                Some(ScalarKind::Float) => {
                    float_like.insert(name.clone());
                }
                Some(ScalarKind::Str) => {
                    str_like.insert(name.clone());
                }
                Some(ScalarKind::NoneValue) => {
                    none_like.insert(name.clone());
                }
                None => {}
            }
        }
        (int_like, bool_like, float_like, str_like, none_like)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn integer_family_names(&self) -> BTreeSet<String> {
        self.integer_family_names.iter().cloned().collect()
    }

    /// The raw-primary carrier sets, as a **view** over the representation
    /// lattice. `int` is the native raw-i64 carrier union; `int_inline_safe` and
    /// `int_full_deopt` expose the two name-keyed tiers separately so box sites
    /// cannot confuse the inline-int47 proof with the overflow-peel proof. Bool
    /// and float are the raw 0/1 and unboxed-f64 views over the same
    /// `repr_by_name` authority.
    #[cfg(any(feature = "native-backend", feature = "llvm", test))]
    pub fn primary_name_sets(&self) -> ScalarPrimaryNameSets {
        let int_inline_safe = self.int_carrier_names();
        let int_full_deopt = self.int_full_deopt_names();
        let mut int = int_inline_safe.clone();
        int.extend(int_full_deopt.iter().cloned());
        let bool_ = self.bool_carrier_names();
        let float = self.float_unboxed_names();
        ScalarPrimaryNameSets {
            int,
            int_inline_safe,
            int_full_deopt,
            bool_,
            float,
        }
    }

    /// Inline-int47 raw-i64 carrier names — the `{RawI64Safe}` view over
    /// `repr_by_name`. Use [`Self::int_raw_carrier_names`] or
    /// [`Self::is_raw_int_carrier_name`] when a native consumer means "any raw
    /// i64 storage"; box sites must distinguish this tier from
    /// [`Self::int_full_deopt_names`].
    pub fn int_carrier_names(&self) -> BTreeSet<String> {
        self.repr_by_name
            .iter()
            .filter(|(_, repr)| repr.is_raw_i64_safe())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Full-i64 checked-overflow carrier names — the `{RawI64FullDeopt}` view
    /// over `repr_by_name`.
    pub fn int_full_deopt_names(&self) -> BTreeSet<String> {
        self.repr_by_name
            .iter()
            .filter(|(_, repr)| repr.is_raw_i64_full_deopt())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// All native raw-i64 carrier names, independent of box-site tier.
    pub fn int_raw_carrier_names(&self) -> BTreeSet<String> {
        self.repr_by_name
            .iter()
            .filter(|(_, repr)| repr.is_raw_i64_carrier())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Raw-bool carrier names — the `{Bool}` view over `repr_by_name`.
    pub fn bool_carrier_names(&self) -> BTreeSet<String> {
        self.repr_by_name
            .iter()
            .filter(|(_, repr)| repr.is_bool_carrier())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Raw-f64 carrier names — the `{FloatUnboxed}` view over `repr_by_name`.
    pub fn float_unboxed_names(&self) -> BTreeSet<String> {
        self.repr_by_name
            .iter()
            .filter(|(_, repr)| repr.is_float_unboxed())
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Name-keyed inline-int47 predicate for native lowering.
    pub fn is_inline_safe_int_name(&self, name: &str) -> bool {
        self.repr_by_name
            .get(name)
            .is_some_and(|repr| repr.is_raw_i64_safe())
    }

    /// Name-keyed full-i64 checked-overflow predicate for native lowering.
    pub fn is_full_deopt_int_name(&self, name: &str) -> bool {
        self.repr_by_name
            .get(name)
            .is_some_and(|repr| repr.is_raw_i64_full_deopt())
    }

    /// Name-keyed raw-i64 carrier predicate for native lowering. Box-site code
    /// must still distinguish [`Self::is_inline_safe_int_name`] from
    /// [`Self::is_full_deopt_int_name`].
    pub fn is_raw_int_carrier_name(&self, name: &str) -> bool {
        self.repr_by_name
            .get(name)
            .is_some_and(|repr| repr.is_raw_i64_carrier())
    }

    /// Name-keyed raw-bool carrier predicate for native lowering.
    pub fn is_bool_unboxed(&self, name: &str) -> bool {
        self.repr_by_name
            .get(name)
            .is_some_and(|repr| repr.is_bool_carrier())
    }

    /// Name-keyed raw-f64 carrier predicate for native lowering.
    pub fn is_float_unboxed(&self, name: &str) -> bool {
        self.repr_by_name
            .get(name)
            .is_some_and(|repr| repr.is_float_unboxed())
    }

    #[cfg(any(feature = "native-backend", test))]
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub fn scalar_slot_exclusion_unsafe(&self) -> BTreeSet<String> {
        self.scalar_slot_exclusion_unsafe.iter().cloned().collect()
    }

    #[cfg(any(feature = "native-backend", test))]
    pub fn scalar_store_targets(&self, kind: ScalarKind) -> BTreeSet<String> {
        self.scalar_store_targets_by_kind
            .get(&kind)
            .cloned()
            .unwrap_or_default()
    }

    pub fn op_scalar_lane(&self, op: &OpIR) -> Option<ScalarKind> {
        self.infer_scalar_lane(op)
    }

    pub(crate) fn name_container_storage_kind(&self, name: &str) -> Option<ContainerStorageKind> {
        self.container_storage_by_name
            .get(name)
            .map(|fact| fact.kind)
    }

    pub fn op_has_container_storage(
        &self,
        op_index: usize,
        op: &OpIR,
        kind: ContainerStorageKind,
    ) -> bool {
        op.args
            .as_ref()
            .and_then(|args| args.first())
            .is_some_and(|name| self.name_container_storage_kind(name) == Some(kind))
            && self
                .container_storage_ops
                .get(&op_index)
                .is_some_and(|fact| fact.kind == kind)
    }

    pub fn op_prefers_integer_runtime_lane(&self, op: &OpIR) -> bool {
        matches!(
            op.kind.as_str(),
            "add"
                | "inplace_add"
                | "sub"
                | "inplace_sub"
                | "mul"
                | "inplace_mul"
                | "floordiv"
                | "inplace_floordiv"
                | "mod"
                | "mod_"
                | "inplace_mod"
                | "bit_and"
                | "inplace_bit_and"
                | "bit_or"
                | "inplace_bit_or"
                | "bit_xor"
                | "inplace_bit_xor"
                | "lshift"
                | "inplace_lshift"
                | "rshift"
                | "inplace_rshift"
                | "shl"
                | "shr"
                | "neg"
                | "pos"
                | "abs"
                | "builtin_abs"
                | "invert"
        ) && op.args.as_ref().is_some_and(|args| {
            !args.is_empty() && args.iter().all(|arg| self.name_is_integer_family(arg))
        })
    }

    pub fn op_index_key_is_integer_family(&self, op: &OpIR) -> bool {
        matches!(op.kind.as_str(), "index" | "store_index" | "dict_set")
            && op.args.as_ref().is_some_and(|args| {
                args.get(1)
                    .is_some_and(|key| self.name_is_integer_family(key))
            })
    }

    fn insert_lir_value(&mut self, name: String, value: &LirValue) {
        let fact = ScalarRepresentationFact {
            ty: value.ty.clone(),
            repr: value.repr,
        };
        if fact.is_dynbox_top() {
            self.insert_weak_fact(name, fact);
        } else {
            self.insert_fact(name, fact);
        }
    }

    /// Insert a fact under a SYNTHETIC (canonical fallback) name — `_v{N}` /
    /// `_bb{N}_arg{I}` from the plan's internal re-lift, whose numbering does
    /// NOT match the emitted stream's. Weak facts never displace or conflict
    /// out an explicit-stream-name (strong) fact: a canonical name colliding
    /// with a different value's explicit name previously blacklisted BOTH,
    /// silently demoting the explicitly-named value to the boxed lane (found
    /// via the overflow_peel flag chain, where `_v43`-style emitted names
    /// collided with the re-lift's canonical `_v43`).
    fn insert_lir_value_weak(&mut self, name: String, value: &LirValue) {
        let fact = ScalarRepresentationFact {
            ty: value.ty.clone(),
            repr: value.repr,
        };
        self.insert_weak_fact(name, fact);
    }

    fn insert_weak_fact(&mut self, name: String, fact: ScalarRepresentationFact) {
        if self.conflicted_names.contains(&name) {
            return;
        }
        if let Some(existing) = self.facts_by_name.get(&name) {
            if existing != &fact && self.weak_fact_names.contains(&name) {
                // Two genuinely ambiguous weak facts: blacklist (the old
                // conservative behavior).
                self.facts_by_name.remove(&name);
                self.weak_fact_names.remove(&name);
                self.conflicted_names.insert(name);
            }
            // A strong fact stands regardless of weak disagreement.
            return;
        }
        self.weak_fact_names.insert(name.clone());
        self.facts_by_name.insert(name, fact);
    }

    fn insert_fact(&mut self, name: String, fact: ScalarRepresentationFact) -> bool {
        if self.conflicted_names.contains(&name) {
            return false;
        }
        if let Some(existing) = self.facts_by_name.get(&name) {
            if existing != &fact {
                // A strong (explicit stream name) fact REPLACES a weak
                // (canonical fallback) one — the explicit name is the
                // stream's source of truth.
                if self.weak_fact_names.remove(&name) {
                    self.facts_by_name.insert(name, fact);
                    return true;
                }
                self.facts_by_name.remove(&name);
                self.conflicted_names.insert(name);
                return true;
            }
            // Equal fact: upgrade weak → strong so a later disagreeing weak
            // insert cannot blacklist it.
            self.weak_fact_names.remove(&name);
            return false;
        }
        self.facts_by_name.insert(name, fact);
        true
    }

    fn insert_container_storage_fact(&mut self, name: String, fact: ContainerStorageFact) -> bool {
        if self.container_storage_conflicted_names.contains(&name) {
            return false;
        }
        if let Some(existing) = self.container_storage_by_name.get(&name) {
            if existing != &fact {
                self.container_storage_by_name.remove(&name);
                self.container_storage_conflicted_names.insert(name);
                return true;
            }
            return false;
        }
        self.container_storage_by_name.insert(name, fact);
        true
    }

    fn seed_container_storage_from_tir(
        &mut self,
        tir_func: &TirFunction,
        names: &SimpleValueNames,
    ) {
        let storage_by_value = tir_container_storage_facts(tir_func);
        let mut block_ids: Vec<_> = tir_func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|block_id| block_id.0);
        for block_id in block_ids {
            let block = &tir_func.blocks[&block_id];
            for (index, arg) in block.args.iter().enumerate() {
                if let Some(fact) = storage_by_value.get(&arg.id) {
                    self.insert_container_storage_fact(names.value_name(arg.id), fact.clone());
                    self.insert_container_storage_fact(
                        names.block_arg_slot(block.id, index),
                        fact.clone(),
                    );
                }
            }
            for op in &block.ops {
                for result in &op.results {
                    if let Some(fact) = storage_by_value.get(result) {
                        self.insert_container_storage_fact(names.value_name(*result), fact.clone());
                    }
                }
                if op.results.len() == 1
                    && let Some(AttrValue::Str(simple_out)) = op.attrs.get("_simple_out")
                    && let Some(result) = op.results.first()
                    && let Some(fact) = storage_by_value.get(result)
                {
                    self.insert_container_storage_fact(simple_out.clone(), fact.clone());
                }
            }
        }
    }

    /// Seed each container-constructor op's output with its true container
    /// [`TirType`], overriding the type inferred via the TIR lift.
    ///
    /// The frontend/native container constructors (`list_new`, `dict_new`,
    /// `set_new`, `tuple_new`, `frozenset_new`, and the list/tuple conversion
    /// variants) have no dedicated TIR `OpCode`; `ssa::kind_to_opcode` lifts them
    /// to the `OpCode::Copy` passthrough fallback. Copy's type rule aliases the
    /// result to its first operand — which for a constructor is one *element*
    /// (e.g. the first `str` of a `set`), not the container. That mistyping makes
    /// [`Self::name_container_kind`] report the element type, so a `contains`
    /// dispatch (`function_compiler.rs`) calls the wrong specialized intrinsic on
    /// the container — e.g. `molt_str_contains` on a `set`/`dict`, which reads the
    /// container's bytes as a string and faults (a P0 SIGSEGV), or `molt_len_str`
    /// on a `set` from the `len` dispatch.
    ///
    /// The container kind is unambiguous from the constructor op kind itself, so
    /// this restores it directly from the SimpleIR stream. Operating on the plan
    /// facts (not the TIR `value_types`) keeps the fix free of the generator
    /// poll-tuple / `frozenset` *return-type* contracts that the TIR types feed:
    /// the plan facts exist solely for backend lane/dispatch selection, where
    /// `frozenset` correctly probes through the shared set hash path
    /// (`molt_set_contains` reads set/frozenset by the same layout) and an
    /// unknown-arity tuple is the right "is a tuple" answer.
    fn seed_container_constructor_facts(&mut self, func_ir: &FunctionIR) {
        for op in &func_ir.ops {
            let Some(out) = op.out.as_deref() else {
                continue;
            };
            let Some(ty) = container_constructor_result_ty(op.kind.as_str()) else {
                continue;
            };
            // The constructor's container kind is authoritative; force it over
            // any (mistyped) LIR-derived fact and clear the conflict/weak markers
            // so a later alias/weak insert cannot blacklist or displace it.
            self.conflicted_names.remove(out);
            self.weak_fact_names.remove(out);
            self.facts_by_name.insert(
                out.to_string(),
                ScalarRepresentationFact {
                    ty,
                    repr: LirRepr::DynBox,
                },
            );
        }
    }

    fn propagate_simple_aliases(&mut self, index: &IndexedFunctionFactIndex<'_>) {
        let mut facts = IndexedScalarFacts::from_plan(self, index);
        let mut store_target_scratch =
            IndexedStoreTargetScratch::new(index.names.len(), index.stores.len());
        let mut store_target_facts =
            IndexedStoreTargetFacts::new(index.names.len(), index.stores.len());
        let mut blocked = NameWorkSet::new(index.names.len());
        let mut changed = true;
        while changed {
            changed = false;
            blocked.clear();
            Self::indexed_store_target_facts(
                index,
                &facts,
                &mut store_target_scratch,
                &mut store_target_facts,
            );
            for (target, fact) in &store_target_facts.entries {
                if fact.is_none() {
                    changed |= facts.conflict(*target);
                }
            }
            for target in &store_target_facts.pending_entries {
                blocked.insert(*target);
            }
            Self::collect_pending_scalar_alias_outputs(&store_target_facts, index, &mut blocked);
            for target in blocked.iter() {
                changed |= facts.clear_fact(target);
            }
            let store_changed =
                Self::propagate_indexed_store_targets(&mut facts, &store_target_facts, &blocked);
            changed |= store_changed;
            changed |= Self::propagate_indexed_alias_groups(
                &mut facts,
                &store_target_facts,
                index,
                &blocked,
            );
        }
        facts.sync_to_plan(self, index);
    }

    fn collect_pending_scalar_alias_outputs(
        store_target_facts: &IndexedStoreTargetFacts,
        index: &IndexedFunctionFactIndex<'_>,
        blocked: &mut NameWorkSet,
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            for group in &index.alias_groups {
                if store_target_facts.target_is_pending(group.out) {
                    changed |= blocked.insert(group.out);
                    continue;
                }
                for source in &group.sources {
                    if store_target_facts.target_is_pending(*source) || blocked.contains(*source) {
                        changed |= blocked.insert(group.out);
                        break;
                    }
                }
            }
        }
    }

    fn collect_pending_container_storage_alias_outputs(
        store_target_facts: &IndexedStoreTargetFacts,
        index: &IndexedFunctionFactIndex<'_>,
        blocked: &mut NameWorkSet,
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            for group in &index.alias_groups {
                if store_target_facts.target_is_pending(group.out) {
                    changed |= blocked.insert(group.out);
                    continue;
                }
                for source in &group.sources {
                    if store_target_facts.target_is_pending(*source) || blocked.contains(*source) {
                        changed |= blocked.insert(group.out);
                        break;
                    }
                }
            }
        }
    }

    fn propagate_indexed_alias_groups(
        facts: &mut IndexedScalarFacts,
        store_target_facts: &IndexedStoreTargetFacts,
        index: &IndexedFunctionFactIndex<'_>,
        blocked: &NameWorkSet,
    ) -> bool {
        let mut changed = false;
        for group in &index.alias_groups {
            if blocked.contains(group.out) {
                continue;
            }
            if store_target_facts.target_is_none(group.out) {
                changed |= facts.conflict(group.out);
                continue;
            }
            if store_target_facts.target_is_pending(group.out) {
                changed |= facts.clear_fact(group.out);
                continue;
            }
            let mut joined: Option<ScalarFactId> = None;
            let mut unknown_or_conflict = false;
            let mut pending = false;
            for source in &group.sources {
                if store_target_facts.target_is_none(*source) || facts.conflicted[source.0] {
                    unknown_or_conflict = true;
                    break;
                }
                if store_target_facts.target_is_pending(*source) {
                    pending = true;
                    break;
                }
                let Some(fact_id) = facts.fact_id(*source) else {
                    continue;
                };
                match joined {
                    Some(existing) if existing != fact_id => {
                        unknown_or_conflict = true;
                        break;
                    }
                    Some(_) => {}
                    None => joined = Some(fact_id),
                }
            }
            if unknown_or_conflict {
                changed |= facts.conflict(group.out);
                continue;
            }
            if pending {
                changed |= facts.clear_fact(group.out);
                continue;
            }
            if let Some(fact_id) = joined {
                changed |= facts.insert_graph_fact_id(group.out, fact_id);
            }
        }
        changed
    }

    fn indexed_store_target_facts(
        index: &IndexedFunctionFactIndex<'_>,
        facts: &IndexedScalarFacts,
        scratch: &mut IndexedStoreTargetScratch,
        out: &mut IndexedStoreTargetFacts,
    ) {
        for edge in &index.stores {
            let source = match edge.source {
                Some(source) if facts.fact_id(source).is_some() => {
                    StoreTargetMergeSource::Proven(source)
                }
                Some(_) => StoreTargetMergeSource::Pending,
                None => StoreTargetMergeSource::Missing,
            };
            let relevant = || {
                facts.contains(edge.target)
                    || index.alias_sources[edge.target.0]
                    || index.alias_outputs[edge.target.0]
            };
            scratch.merge(edge.target, source, relevant, |lhs, rhs| {
                facts.fact_id(lhs) == facts.fact_id(rhs)
            });
        }
        scratch.finish_into(out);
    }

    fn propagate_indexed_store_targets(
        facts: &mut IndexedScalarFacts,
        facts_by_target: &IndexedStoreTargetFacts,
        blocked: &NameWorkSet,
    ) -> bool {
        let mut changed = false;
        for (target, source) in &facts_by_target.entries {
            if blocked.contains(*target) {
                continue;
            }
            let Some(source) = *source else {
                continue;
            };
            let Some(fact_id) = facts.fact_id(source) else {
                continue;
            };
            if facts.fact_id(*target) != Some(fact_id) {
                changed |= facts.insert_graph_fact_id(*target, fact_id);
            }
        }
        changed
    }

    fn propagate_container_storage(
        &mut self,
        fact_index: &FunctionFactIndex<'_>,
        index: &IndexedFunctionFactIndex<'_>,
    ) {
        let mut facts = IndexedContainerFacts::from_plan(self, index);
        let mut store_target_scratch =
            IndexedStoreTargetScratch::new(index.names.len(), index.stores.len());
        let mut store_target_facts =
            IndexedStoreTargetFacts::new(index.names.len(), index.stores.len());
        let mut blocked = NameWorkSet::new(index.names.len());
        let mut changed = true;
        while changed {
            changed = false;
            blocked.clear();
            Self::indexed_container_storage_store_target_facts(
                index,
                &facts,
                &mut store_target_scratch,
                &mut store_target_facts,
            );
            for (target, fact) in &store_target_facts.entries {
                if fact.is_none() {
                    changed |= facts.conflict(*target);
                }
            }
            for target in &store_target_facts.pending_entries {
                blocked.insert(*target);
            }
            Self::collect_pending_container_storage_alias_outputs(
                &store_target_facts,
                index,
                &mut blocked,
            );
            for target in blocked.iter() {
                changed |= facts.clear_fact(target);
            }
            let store_changed = Self::propagate_indexed_container_storage_store_targets(
                &mut facts,
                &store_target_facts,
                &blocked,
            );
            changed |= store_changed;
            let alias_changed = Self::propagate_indexed_container_storage_alias_groups(
                &mut facts,
                &store_target_facts,
                index,
                &blocked,
            );
            changed |= alias_changed;
            for op in &fact_index.store_index_ops {
                let Some(args) = op.args.as_ref() else {
                    continue;
                };
                let Some(container) = args.first() else {
                    continue;
                };
                let Some(container_id) = index.names.get(container) else {
                    continue;
                };
                if facts.kind(container_id) != Some(ContainerStorageKind::FlatListInt) {
                    continue;
                }
                let value_preserves_flat_int = args.get(2).is_some_and(|value| {
                    self.name_scalar_kind(value) == Some(ScalarKind::Int)
                        || self.name_is_integer_family(value)
                });
                if value_preserves_flat_int {
                    if let Some(out) = op.out.as_ref()
                        && let Some(out_id) = index.names.get(out)
                        && let Some(fact) = facts.get(container_id).cloned()
                        && facts.insert(out_id, fact)
                    {
                        changed = true;
                    }
                } else {
                    changed |= facts.conflict(container_id);
                    if let Some(out) = op.out.as_ref()
                        && let Some(out_id) = index.names.get(out)
                    {
                        changed |= facts.conflict(out_id);
                    }
                }
            }
        }
        facts.sync_to_plan(self, index);
    }

    fn propagate_indexed_container_storage_alias_groups(
        facts: &mut IndexedContainerFacts,
        store_target_facts: &IndexedStoreTargetFacts,
        index: &IndexedFunctionFactIndex<'_>,
        blocked: &NameWorkSet,
    ) -> bool {
        let mut changed = false;
        for group in &index.alias_groups {
            if blocked.contains(group.out) {
                continue;
            }
            if store_target_facts.target_is_none(group.out) {
                changed |= facts.conflict(group.out);
                continue;
            }
            if store_target_facts.target_is_pending(group.out) {
                changed |= facts.clear_fact(group.out);
                continue;
            }
            let mut joined: Option<ContainerStorageFact> = None;
            let mut unknown_or_conflict = false;
            let mut pending = false;
            for source in &group.sources {
                if store_target_facts.target_is_none(*source) || facts.conflicted[source.0] {
                    unknown_or_conflict = true;
                    break;
                }
                if store_target_facts.target_is_pending(*source) {
                    pending = true;
                    break;
                }
                let Some(fact) = facts.get(*source).cloned() else {
                    continue;
                };
                match joined.as_ref() {
                    Some(existing) if existing != &fact => {
                        unknown_or_conflict = true;
                        break;
                    }
                    Some(_) => {}
                    None => joined = Some(fact),
                }
            }
            if unknown_or_conflict {
                changed |= facts.conflict(group.out);
                continue;
            }
            if pending {
                changed |= facts.clear_fact(group.out);
                continue;
            }
            if let Some(fact) = joined {
                changed |= facts.insert(group.out, fact);
            }
        }
        changed
    }

    fn indexed_container_storage_store_target_facts(
        index: &IndexedFunctionFactIndex<'_>,
        facts: &IndexedContainerFacts,
        scratch: &mut IndexedStoreTargetScratch,
        out: &mut IndexedStoreTargetFacts,
    ) {
        for edge in &index.stores {
            let source = match edge.source {
                Some(source) if facts.contains(source) => StoreTargetMergeSource::Proven(source),
                Some(_) => StoreTargetMergeSource::Pending,
                None => StoreTargetMergeSource::Missing,
            };
            let relevant = || {
                facts.contains(edge.target)
                    || index.alias_sources[edge.target.0]
                    || index.alias_outputs[edge.target.0]
            };
            scratch.merge(edge.target, source, relevant, |lhs, rhs| {
                facts.get(lhs) == facts.get(rhs)
            });
        }
        scratch.finish_into(out);
    }

    fn propagate_indexed_container_storage_store_targets(
        facts: &mut IndexedContainerFacts,
        facts_by_target: &IndexedStoreTargetFacts,
        blocked: &NameWorkSet,
    ) -> bool {
        let mut changed = false;
        for (target, source) in &facts_by_target.entries {
            if blocked.contains(*target) {
                continue;
            }
            let Some(source) = *source else {
                continue;
            };
            let Some(fact) = facts.get(source).cloned() else {
                continue;
            };
            if facts.get(*target) != Some(&fact) {
                changed |= facts.insert(*target, fact);
            }
        }
        changed
    }

    fn mark_container_storage_ops(&mut self, func_ir: &FunctionIR) {
        self.container_storage_ops.clear();
        for (op_index, op) in func_ir.ops.iter().enumerate() {
            if !matches!(op.kind.as_str(), "index" | "store_index" | "dict_set") {
                continue;
            }
            let Some(container) = op.args.as_ref().and_then(|args| args.first()) else {
                continue;
            };
            if let Some(fact) = self.container_storage_by_name.get(container) {
                self.container_storage_ops.insert(op_index, fact.clone());
            }
        }
    }

    fn propagate_integer_family(
        &mut self,
        func_ir: &FunctionIR,
        fact_index: &FunctionFactIndex<'_>,
    ) {
        self.non_scalar_names = self.non_scalar_simple_outputs(func_ir);
        self.integer_family_names.extend(
            self.facts_by_name
                .iter()
                .filter(|(name, fact)| {
                    !self.non_scalar_names.contains(*name)
                        && (matches!(fact.ty, TirType::I64) && fact.repr == LirRepr::I64
                            || matches!(fact.ty, TirType::BigInt))
                })
                .map(|(name, _)| name.clone()),
        );

        let mut changed = true;
        while changed {
            changed = false;
            changed |= self.propagate_integer_store_targets(fact_index);
            for op in &fact_index.data_ops {
                let Some(out) = op.out.as_ref() else {
                    continue;
                };
                if self.integer_family_names.contains(out) {
                    continue;
                }
                let inserted = if let Some(source) = alias_source_name(op) {
                    self.integer_family_names.contains(source)
                } else if integer_only_result_op(op.kind.as_str()) {
                    true
                } else if integer_arithmetic_result_op(op.kind.as_str()) {
                    op.args.as_ref().is_some_and(|args| {
                        !args.is_empty() && args.iter().all(|arg| self.name_is_integer_family(arg))
                    })
                } else {
                    false
                };
                if inserted {
                    self.integer_family_names.insert(out.clone());
                    changed = true;
                }
            }
        }
    }

    fn non_scalar_simple_outputs(&self, func_ir: &FunctionIR) -> PlanHashSet<String> {
        let mut names = plan_hash_set(func_ir.ops.len() / 4 + 1);
        for op in &func_ir.ops {
            if let Some(out) = op.out.as_deref()
                && simple_op_produces_non_scalar_value(op.kind.as_str())
            {
                names.insert(out.to_string());
            }
        }
        names
    }

    fn propagate_integer_store_targets(&mut self, fact_index: &FunctionFactIndex<'_>) -> bool {
        let mut targets: PlanHashMap<&str, bool> =
            plan_hash_map(fact_index.stores.len().saturating_add(1));
        for edge in &fact_index.stores {
            let source_is_integer = edge
                .source
                .is_some_and(|source| self.integer_family_names.contains(source));
            targets
                .entry(edge.target)
                .and_modify(|all_sources_integer| {
                    *all_sources_integer &= source_is_integer;
                })
                .or_insert(source_is_integer);
        }

        let mut changed = false;
        for (target, all_sources_integer) in targets {
            if all_sources_integer && self.integer_family_names.insert(target.to_string()) {
                changed = true;
            }
        }
        changed
    }

    pub fn name_scalar_kind(&self, name: &str) -> Option<ScalarKind> {
        if self.non_scalar_names.contains(name) {
            return None;
        }
        self.facts_by_name
            .get(name)
            .and_then(ScalarRepresentationFact::scalar_kind)
    }

    pub fn name_container_kind(&self, name: &str) -> Option<ContainerKind> {
        self.facts_by_name
            .get(name)
            .and_then(ScalarRepresentationFact::container_kind)
    }

    pub fn op_container_kind(&self, op: &OpIR) -> Option<ContainerKind> {
        op.args
            .as_ref()
            .and_then(|args| args.first())
            .and_then(|name| self.name_container_kind(name))
    }

    pub fn op_has_container_kind(&self, op: &OpIR, kind: ContainerKind) -> bool {
        self.op_container_kind(op) == Some(kind)
    }

    pub fn name_is_integer_family(&self, name: &str) -> bool {
        if self.non_scalar_names.contains(name) {
            return false;
        }
        self.integer_family_names.contains(name)
            || self.name_scalar_kind(name) == Some(ScalarKind::Bool)
    }

    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub fn op_args_are_integer_family(&self, op: &OpIR) -> bool {
        op.args.as_ref().is_some_and(|args| {
            !args.is_empty() && args.iter().all(|arg| self.name_is_integer_family(arg))
        })
    }

    fn name_has_scalar_kind(&self, name: &str, kind: ScalarKind) -> bool {
        self.name_scalar_kind(name) == Some(kind)
    }

    fn compute_scalar_store_targets(
        &self,
        fact_index: &FunctionFactIndex<'_>,
    ) -> BTreeMap<ScalarKind, BTreeSet<String>> {
        let mut targets = BTreeMap::new();
        for kind in [
            ScalarKind::Int,
            ScalarKind::Bool,
            ScalarKind::Float,
            ScalarKind::Str,
        ] {
            targets.insert(kind, self.scalar_lane_store_target_names(fact_index, kind));
        }
        targets
    }

    fn scalar_lane_store_target_names(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        lane: ScalarKind,
    ) -> BTreeSet<String> {
        let mut lane_outputs = BTreeSet::new();
        let mut changed = true;
        while changed {
            changed = propagate_store_var_targets_in(fact_index, &mut lane_outputs);
            for op in &fact_index.data_ops {
                let Some(out) = op.out.as_ref() else {
                    continue;
                };
                let inferred_lane = self.infer_scalar_lane_with_overrides(op, lane, &lane_outputs);
                let first_arg_is_lane = op
                    .args
                    .as_ref()
                    .and_then(|args| args.first())
                    .is_some_and(|src| lane_outputs.contains(src));
                let var_source_is_lane = op
                    .var
                    .as_ref()
                    .is_some_and(|src| lane_outputs.contains(src));
                let is_lane_alias = matches!(
                    op.kind.as_str(),
                    "copy_var" | "copy" | "load_var" | "identity_alias" | "binding_alias"
                ) && first_arg_is_lane
                    || matches!(op.kind.as_str(), "copy_var" | "load_var") && var_source_is_lane;

                if (inferred_lane == Some(lane) || is_lane_alias)
                    && lane_outputs.insert(out.clone())
                {
                    changed = true;
                }
            }
        }
        store_var_targets_all_sources_in(fact_index, &lane_outputs)
    }

    fn compute_primary_name_sets(
        &self,
        func_ir: &FunctionIR,
        fact_index: &FunctionFactIndex<'_>,
    ) -> ScalarPrimaryNameSets {
        if is_cold_module_chunk_function(&func_ir.name) {
            return ScalarPrimaryNameSets::default();
        }

        let (int_like, bool_like, float_like, str_like, _) = self.scalar_name_sets();
        let param_name_set: BTreeSet<&str> = func_ir.params.iter().map(String::as_str).collect();
        let (int_inline_safe, int_full_deopt) = self.compute_int_primary_name_tiers(
            func_ir,
            fact_index,
            &param_name_set,
            &int_like,
            &bool_like,
            &float_like,
        );
        let mut int_primary = int_inline_safe.clone();
        int_primary.extend(int_full_deopt.iter().cloned());
        let bool_primary = self.compute_bool_primary_names(
            fact_index,
            &param_name_set,
            &int_primary,
            &bool_like,
            &int_like,
            &float_like,
            &str_like,
        );
        let float_primary =
            self.compute_float_primary_names(fact_index, &param_name_set, &int_like, &float_like);

        ScalarPrimaryNameSets {
            int: int_primary,
            int_inline_safe,
            int_full_deopt,
            bool_: bool_primary,
            float: float_primary,
        }
    }

    fn compute_int_primary_name_tiers(
        &self,
        func_ir: &FunctionIR,
        fact_index: &FunctionFactIndex<'_>,
        param_name_set: &BTreeSet<&str>,
        int_like: &BTreeSet<String>,
        bool_like: &BTreeSet<String>,
        float_like: &BTreeSet<String>,
    ) -> (BTreeSet<String>, BTreeSet<String>) {
        let bounded_i64_names = compute_i64_interval_facts(func_ir);
        let int_unsafe_outputs: BTreeSet<String> = fact_index
            .output_ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_ref()?;
                let is_safe_int_op = matches!(
                    op.kind.as_str(),
                    "const"
                        | "loop_index_start"
                        | "loop_index_next"
                        | "len"
                        | "gpu_thread_id"
                        | "gpu_block_id"
                        | "gpu_block_dim"
                        | "gpu_grid_dim"
                        | "bit_and"
                        | "bit_or"
                        | "bit_xor"
                        | "inplace_bit_and"
                        | "inplace_bit_or"
                        | "inplace_bit_xor"
                        | "invert"
                        | "copy"
                        | "copy_var"
                        | "load_var"
                        | "identity_alias"
                        | "binding_alias"
                        | "store_var"
                );
                let range_safe_arithmetic = bounded_i64_names.contains_key(out)
                    && matches!(
                        op.kind.as_str(),
                        "add" | "inplace_add" | "sub" | "inplace_sub"
                    );
                (!is_safe_int_op && !range_safe_arithmetic && int_like.contains(out))
                    .then(|| out.clone())
            })
            .collect();
        let bounded_i64_name_set: BTreeSet<String> = bounded_i64_names.keys().cloned().collect();
        let vars_with_non_int_defs =
            self.vars_with_non_int_defs(fact_index, int_like, bool_like, &bounded_i64_name_set);
        let passes_filter = |name: &str| {
            (int_like.contains(name) || bounded_i64_names.contains_key(name))
                && !param_name_set.contains(name)
                && !int_unsafe_outputs.contains(name)
                && !vars_with_non_int_defs.contains(name)
                && !fact_index.sentinel_outputs.contains(name)
                && !fact_index.delete_targets.contains(name)
                && !float_like.contains(name)
        };
        let mut candidates: PlanHashSet<String> =
            plan_hash_set(bounded_i64_names.len().saturating_add(16));
        for name in bounded_store_load_loop_seed_names(func_ir, &bounded_i64_names) {
            if passes_filter(&name) {
                candidates.insert(name);
            }
        }
        // OSC admission for `overflow_peel`'d loops: the {slot → load_var →
        // checked_add/checked_mul → slot} carrier cycle is raw-i64-admissible
        // AS A UNIT — no interval proof is needed (or possible: the accumulator
        // is genuinely unbounded; that is the point of the peel). The
        // `checked_add`/`checked_mul` contract supplies the wrap-safety the
        // interval chain cannot: the result is a true i64 with a hardware
        // overflow flag, and the peel's CFG gates every consumption of a
        // wrapped value.
        let checked_loop_members =
            checked_loop_seed_names(func_ir, &bounded_i64_names, &passes_filter);
        candidates.extend(checked_loop_members.iter().cloned());
        let mut changed = true;
        while changed {
            changed = false;
            for target in
                store_var_targets_all_sources_where(fact_index, |src| candidates.contains(src))
            {
                if passes_filter(&target) && candidates.insert(target) {
                    changed = true;
                }
            }
            for op in &fact_index.data_ops {
                let Some(out) = op.out.as_ref() else {
                    continue;
                };
                if candidates.contains(out) || !passes_filter(out) {
                    continue;
                }
                if op_produces_raw_i64_for_int_primary(op, &candidates, &bounded_i64_names)
                    && candidates.insert(out.clone())
                {
                    changed = true;
                }
            }
        }
        let full_deopt =
            self.propagate_full_deopt_name_tier(fact_index, &candidates, checked_loop_members);
        let inline_safe = candidates
            .into_iter()
            .filter(|name| !full_deopt.contains(name))
            .collect();
        (inline_safe, full_deopt)
    }

    fn vars_with_non_int_defs(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        int_like: &BTreeSet<String>,
        bool_like: &BTreeSet<String>,
        extra_int_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut non_int = BTreeSet::new();
        for edge in &fact_index.stores {
            let source_is_int = edge.source.is_some_and(|s| {
                !fact_index.sentinel_outputs.contains(s)
                    && (int_like.contains(s) || bool_like.contains(s) || extra_int_like.contains(s))
            });
            if !source_is_int {
                non_int.insert(edge.target.to_string());
            }
        }
        for op in &fact_index.output_ops {
            if let Some(out) = op.out.as_ref() {
                let lane = self.infer_scalar_lane(op);
                let proven_int = matches!(lane, Some(ScalarKind::Int) | Some(ScalarKind::Bool))
                    || extra_int_like.contains(out);
                if !proven_int && int_like.contains(out) {
                    non_int.insert(out.clone());
                }
            }
        }
        non_int
    }

    fn propagate_full_deopt_name_tier(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        raw_candidates: &PlanHashSet<String>,
        seed: BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut full_deopt: BTreeSet<String> = seed
            .into_iter()
            .filter(|name| raw_candidates.contains(name))
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for target in
                store_var_targets_all_sources_where(fact_index, |src| full_deopt.contains(src))
            {
                if raw_candidates.contains(&target) && full_deopt.insert(target) {
                    changed = true;
                }
            }
            for op in &fact_index.data_ops {
                let Some(out) = op.out.as_ref() else {
                    continue;
                };
                if full_deopt.contains(out) || !raw_candidates.contains(out) {
                    continue;
                }
                let first_source = op.var.as_deref().or_else(|| {
                    op.args
                        .as_ref()
                        .and_then(|args| args.first().map(String::as_str))
                });
                let propagates_full_deopt = matches!(
                    op.kind.as_str(),
                    "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias"
                ) && first_source
                    .is_some_and(|source| full_deopt.contains(source));
                if propagates_full_deopt && full_deopt.insert(out.clone()) {
                    changed = true;
                }
            }
        }
        full_deopt
    }

    fn compute_bool_primary_names(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        param_name_set: &BTreeSet<&str>,
        int_primary: &BTreeSet<String>,
        bool_like: &BTreeSet<String>,
        int_like: &BTreeSet<String>,
        float_like: &BTreeSet<String>,
        str_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let bool_unsafe_outputs: BTreeSet<String> = fact_index
            .output_ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_ref()?;
                let is_safe_bool_op = op.kind == "store_var"
                    || self.op_produces_raw_bool_for_bool_primary(op, bool_like, int_primary);
                (!is_safe_bool_op && bool_like.contains(out)).then(|| out.clone())
            })
            .collect();
        let vars_with_non_bool_defs =
            self.vars_with_non_bool_defs(fact_index, bool_like, int_primary);
        let passes_filter = |name: &str| {
            bool_like.contains(name)
                && !param_name_set.contains(name)
                && !bool_unsafe_outputs.contains(name)
                && !vars_with_non_bool_defs.contains(name)
                && !fact_index.sentinel_outputs.contains(name)
                && !fact_index.delete_targets.contains(name)
                && !int_like.contains(name)
                && !float_like.contains(name)
                && !str_like.contains(name)
                && !self.scalar_slot_exclusion_unsafe.contains(name)
        };
        let mut candidates = BTreeSet::new();
        let mut changed = true;
        while changed {
            changed = false;
            for target in store_var_targets_all_sources_in(fact_index, &candidates) {
                if passes_filter(&target) && candidates.insert(target) {
                    changed = true;
                }
            }
            for op in &fact_index.data_ops {
                let Some(out) = op.out.as_ref() else {
                    continue;
                };
                if candidates.contains(out) || !passes_filter(out) {
                    continue;
                }
                if self.op_produces_raw_bool_for_bool_primary(op, &candidates, int_primary)
                    && candidates.insert(out.clone())
                {
                    changed = true;
                }
            }
        }
        candidates
    }

    fn vars_with_non_bool_defs(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        bool_like: &BTreeSet<String>,
        int_primary: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut non_bool = BTreeSet::new();
        for edge in &fact_index.stores {
            let source_is_bool = edge
                .source
                .is_some_and(|s| bool_like.contains(s) && !fact_index.sentinel_outputs.contains(s));
            if !source_is_bool {
                non_bool.insert(edge.target.to_string());
            }
        }
        for op in &fact_index.output_ops {
            if let Some(out) = op.out.as_ref() {
                let lane = self.infer_scalar_lane(op);
                let raw_bool_output =
                    self.op_produces_raw_bool_for_bool_primary(op, bool_like, int_primary);
                if lane != Some(ScalarKind::Bool) && !raw_bool_output && bool_like.contains(out) {
                    non_bool.insert(out.clone());
                }
            }
        }
        non_bool
    }

    fn op_produces_raw_bool_for_bool_primary(
        &self,
        op: &OpIR,
        candidates: &BTreeSet<String>,
        int_primary: &BTreeSet<String>,
    ) -> bool {
        let first_source = || {
            op.var.as_deref().or_else(|| {
                op.args
                    .as_ref()
                    .and_then(|args| args.first().map(String::as_str))
            })
        };
        match op.kind.as_str() {
            "const_bool" | "lt" | "le" | "gt" | "ge" | "eq" | "ne" | "string_eq" | "is" | "not"
            | "bool" | "cast_bool" | "builtin_bool" => true,
            // checked_add/checked_mul's `out` is the overflow flag — defined
            // raw 0/1 on both native lanes (hardware `of` widened, or constant
            // false on the boxed fallback).
            "checked_add" | "checked_mul" => true,
            // Python `and`/`or` are value-selects, but a value-select over two
            // raw 0/1 bools IS the boolean op — the native arms emit a raw
            // band/bor lane when both operands are bool-primary, so the result
            // is a raw producer exactly when both inputs are.
            "and" | "or" => op
                .args
                .as_ref()
                .is_some_and(|args| args.len() >= 2 && args.iter().all(|a| candidates.contains(a))),
            "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" => {
                first_source().is_some_and(|s| candidates.contains(s))
            }
            "index" => {
                self.op_has_container_kind(op, ContainerKind::List)
                    && op
                        .args
                        .as_ref()
                        .and_then(|args| args.get(1))
                        .is_some_and(|idx| int_primary.contains(idx))
            }
            _ => false,
        }
    }

    fn compute_float_primary_names(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        param_name_set: &BTreeSet<&str>,
        int_like: &BTreeSet<String>,
        float_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let float_unsafe_outputs: BTreeSet<String> = fact_index
            .output_ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_ref()?;
                let is_safe_float_op = matches!(
                    op.kind.as_str(),
                    "const_float"
                        | "add"
                        | "sub"
                        | "mul"
                        | "div"
                        | "inplace_add"
                        | "inplace_sub"
                        | "inplace_mul"
                        | "inplace_div"
                        | "neg"
                        | "unary_neg"
                        | "copy_var"
                        | "load_var"
                        | "identity_alias"
                        | "binding_alias"
                        | "store_var"
                        | "float_from_obj"
                );
                (!is_safe_float_op && float_like.contains(out)).then(|| out.clone())
            })
            .collect();
        let vars_with_non_float_defs = self.vars_with_non_float_defs(fact_index, float_like);
        float_like
            .iter()
            .filter(|name| {
                !param_name_set.contains(name.as_str())
                    && !float_unsafe_outputs.contains(*name)
                    && !int_like.contains(*name)
                    && !vars_with_non_float_defs.contains(*name)
                    && !fact_index.sentinel_outputs.contains(name.as_str())
                    && !fact_index.delete_targets.contains(name.as_str())
            })
            .cloned()
            .collect()
    }

    fn vars_with_non_float_defs(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        float_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut non_float = BTreeSet::new();
        for edge in &fact_index.stores {
            let source_is_float = edge.source.is_some_and(|s| {
                float_like.contains(s) && !fact_index.sentinel_outputs.contains(s)
            });
            if !source_is_float {
                non_float.insert(edge.target.to_string());
            }
        }
        for op in &fact_index.output_ops {
            if let Some(out) = op.out.as_ref() {
                let lane = self.infer_scalar_lane(op);
                if lane != Some(ScalarKind::Float) && float_like.contains(out) {
                    non_float.insert(out.clone());
                }
            }
        }
        non_float
    }

    fn compute_scalar_slot_exclusion_unsafe(&self, func_ir: &FunctionIR) -> PlanHashSet<String> {
        let mut unsafe_set = plan_hash_set(func_ir.ops.len() / 4 + 1);
        for (op_index, op) in func_ir.ops.iter().enumerate() {
            match op.kind.as_str() {
                "call"
                | "call_method"
                | "call_builtin"
                | "call_function_value"
                | "call_super"
                | "call_kw"
                | "call_star"
                | "call_ex"
                | "bytearray_fill_range" => {
                    self.collect_scalar_args(op, &mut unsafe_set);
                }
                "store_attr" | "store_global" | "store_name" => {
                    self.collect_scalar_args(op, &mut unsafe_set);
                }
                "store_index" => {
                    let has_flat_int_storage = self.op_has_container_storage(
                        op_index,
                        op,
                        ContainerStorageKind::FlatListInt,
                    );
                    if !has_flat_int_storage
                        && let Some(args) = &op.args
                        && let Some(val_name) = args.get(2)
                        && self.name_is_slot_scalar(val_name)
                    {
                        unsafe_set.insert(val_name.clone());
                    }
                }
                "ret" => {
                    if let Some(var) = &op.var
                        && self.name_is_slot_scalar(var)
                    {
                        unsafe_set.insert(var.clone());
                    }
                    self.collect_scalar_args(op, &mut unsafe_set);
                }
                "inc_ref" | "dec_ref" | "borrow" | "release" => {
                    self.collect_scalar_args(op, &mut unsafe_set);
                    if let Some(var) = &op.var
                        && self.name_is_slot_scalar(var)
                    {
                        unsafe_set.insert(var.clone());
                    }
                }
                "state_yield" | "chan_send_yield" | "chan_recv_yield" => {
                    self.collect_scalar_args(op, &mut unsafe_set);
                    if let Some(var) = &op.var
                        && self.name_is_slot_scalar(var)
                    {
                        unsafe_set.insert(var.clone());
                    }
                }
                "delete_var" => {
                    if let Some(target) = op.var.as_ref().or(op.out.as_ref())
                        && self.name_is_slot_scalar(target)
                    {
                        unsafe_set.insert(target.clone());
                    }
                }
                _ => {}
            }
        }
        unsafe_set
    }

    fn collect_scalar_args(&self, op: &OpIR, into: &mut PlanHashSet<String>) {
        if let Some(args) = &op.args {
            for arg in args {
                if self.name_is_slot_scalar(arg) {
                    into.insert(arg.clone());
                }
            }
        }
    }

    fn name_is_slot_scalar(&self, name: &str) -> bool {
        matches!(
            self.facts_by_name
                .get(name)
                .and_then(ScalarRepresentationFact::scalar_kind),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float)
        )
    }

    fn infer_scalar_lane(&self, op: &OpIR) -> Option<ScalarKind> {
        self.infer_scalar_lane_with_overrides(op, ScalarKind::NoneValue, &BTreeSet::new())
    }

    // `pub(crate)` for stable cross-CGU linkage. This is referenced from another
    // codegen unit (a `pub(crate)` caller inlined into `luau_lower`) under the
    // multi-CGU dev/debug profile; a private `fn` here gets ThinLTO-internalized
    // (an `.llvm.<hash>` local symbol) and the cross-CGU reference then fails to
    // link. Declaring the real (external) linkage requirement keeps the dev,
    // release-fast, and debug profiles all linkable.
    pub(crate) fn infer_scalar_lane_with_overrides(
        &self,
        op: &OpIR,
        override_kind: ScalarKind,
        override_names: &BTreeSet<String>,
    ) -> Option<ScalarKind> {
        let first_source = || {
            op.var.as_deref().or_else(|| {
                op.args
                    .as_ref()
                    .and_then(|args| args.first())
                    .map(String::as_str)
            })
        };
        let args = op.args.as_deref().unwrap_or(&[]);
        let args_all =
            |pred: &dyn Fn(&str) -> bool| !args.is_empty() && args.iter().all(|arg| pred(arg));
        let args_any = |pred: &dyn Fn(&str) -> bool| args.iter().any(|arg| pred(arg));
        let has_kind = |name: &str, kind| {
            self.name_has_scalar_kind(name, kind)
                || (override_kind == kind && override_names.contains(name))
        };
        let is_float = |name: &str| has_kind(name, ScalarKind::Float);
        let is_str = |name: &str| has_kind(name, ScalarKind::Str);
        let is_int =
            |name: &str| has_kind(name, ScalarKind::Int) || has_kind(name, ScalarKind::Bool);
        match op.kind.as_str() {
            "const" | "loop_index_start" | "loop_index_next" | "len" => Some(ScalarKind::Int),
            "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" => {
                Some(ScalarKind::Int)
            }
            "const_bool" => Some(ScalarKind::Bool),
            "const_float" => Some(ScalarKind::Float),
            "const_str" => Some(ScalarKind::Str),
            "float_from_obj" => Some(ScalarKind::Float),
            "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" => first_source()
                .and_then(|src| {
                    if has_kind(src, ScalarKind::Int) {
                        Some(ScalarKind::Int)
                    } else if has_kind(src, ScalarKind::Bool) {
                        Some(ScalarKind::Bool)
                    } else if has_kind(src, ScalarKind::Float) {
                        Some(ScalarKind::Float)
                    } else if has_kind(src, ScalarKind::Str) {
                        Some(ScalarKind::Str)
                    } else {
                        None
                    }
                }),
            "lt" | "le" | "gt" | "ge" | "eq" | "ne" | "is" => Some(ScalarKind::Bool),
            "bool" | "cast_bool" | "builtin_bool" | "is_truthy" | "not" => {
                first_source().and_then(|src| {
                    if has_kind(src, ScalarKind::Bool) {
                        Some(ScalarKind::Bool)
                    } else if is_int(src) {
                        Some(ScalarKind::Int)
                    } else {
                        None
                    }
                })
            }
            "if" => first_source().and_then(|src| {
                if has_kind(src, ScalarKind::Bool) {
                    Some(ScalarKind::Bool)
                } else if is_int(src) {
                    Some(ScalarKind::Int)
                } else {
                    None
                }
            }),
            "add" | "inplace_add" => {
                if args_all(&is_str) {
                    Some(ScalarKind::Str)
                } else if args_all(&is_float)
                    || (args_any(&is_float) && args.iter().all(|arg| is_float(arg) || is_int(arg)))
                {
                    Some(ScalarKind::Float)
                } else if args_all(&is_int) {
                    Some(ScalarKind::Int)
                } else {
                    None
                }
            }
            "sub" | "mul" | "inplace_sub" | "inplace_mul" | "floordiv" | "mod" | "mod_"
            | "inplace_floordiv" | "inplace_mod" | "bit_and" | "bit_or" | "bit_xor" | "bitand"
            | "bitor" | "bitxor" | "inplace_bit_and" | "inplace_bit_or" | "inplace_bit_xor" => {
                if args_all(&is_float)
                    || (args_any(&is_float) && args.iter().all(|arg| is_float(arg) || is_int(arg)))
                {
                    Some(ScalarKind::Float)
                } else if args_all(&is_int) {
                    Some(ScalarKind::Int)
                } else {
                    None
                }
            }
            "pow" | "inplace_pow" => {
                if (args.len() >= 2 && is_float(&args[1]))
                    || args_all(&is_float)
                    || (args_any(&is_float) && args.iter().all(|arg| is_float(arg) || is_int(arg)))
                {
                    Some(ScalarKind::Float)
                } else {
                    None
                }
            }
            "div" | "inplace_div" => {
                if args_all(&is_float)
                    || (args_any(&is_float) && args.iter().all(|arg| is_float(arg) || is_int(arg)))
                {
                    Some(ScalarKind::Float)
                } else {
                    None
                }
            }
            "lshift" | "rshift" | "shl" | "shr" | "inplace_lshift" | "inplace_rshift" => {
                if args_all(&is_int) {
                    Some(ScalarKind::Int)
                } else {
                    None
                }
            }
            "neg" | "pos" | "abs" | "builtin_abs" => first_source().and_then(|src| {
                if has_kind(src, ScalarKind::Float) {
                    Some(ScalarKind::Float)
                } else if is_int(src) {
                    Some(ScalarKind::Int)
                } else {
                    None
                }
            }),
            "invert" => first_source()
                .filter(|src| is_int(src))
                .map(|_| ScalarKind::Int),
            "index" => op.out.as_deref().and_then(|out| self.name_scalar_kind(out)),
            _ => None,
        }
    }
}

fn store_var_targets_all_sources_in(
    fact_index: &FunctionFactIndex<'_>,
    proven_outputs: &BTreeSet<String>,
) -> BTreeSet<String> {
    store_var_targets_all_sources_where(fact_index, |src| proven_outputs.contains(src))
}

fn store_var_targets_all_sources_where(
    fact_index: &FunctionFactIndex<'_>,
    mut source_proven: impl FnMut(&str) -> bool,
) -> BTreeSet<String> {
    let mut targets: PlanHashMap<&str, bool> =
        plan_hash_map(fact_index.stores.len().saturating_add(1));
    for edge in &fact_index.stores {
        let source_proven = edge.source.is_some_and(&mut source_proven);
        targets
            .entry(edge.target)
            .and_modify(|all_sources_proven| *all_sources_proven &= source_proven)
            .or_insert(source_proven);
    }
    targets
        .into_iter()
        .filter(|&(_, all_sources_proven)| all_sources_proven)
        .map(|(target, _)| target.to_string())
        .collect()
}

fn propagate_store_var_targets_in(
    fact_index: &FunctionFactIndex<'_>,
    proven_outputs: &mut BTreeSet<String>,
) -> bool {
    let mut changed = false;
    for target in store_var_targets_all_sources_in(fact_index, proven_outputs) {
        if proven_outputs.insert(target) {
            changed = true;
        }
    }
    changed
}

fn flat_list_int_storage_fact() -> ContainerStorageFact {
    ContainerStorageFact {
        kind: ContainerStorageKind::FlatListInt,
        elem_ty: TirType::I64,
    }
}

fn tir_container_storage_facts(tir_func: &TirFunction) -> HashMap<ValueId, ContainerStorageFact> {
    let mut facts = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        let mut block_ids: Vec<_> = tir_func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|block_id| block_id.0);
        for block_id in block_ids {
            let block = &tir_func.blocks[&block_id];
            for op in &block.ops {
                if tir_op_original_kind(op) == Some("list_int_new") {
                    for result in &op.results {
                        changed |= insert_value_storage_fact(
                            &mut facts,
                            *result,
                            flat_list_int_storage_fact(),
                        );
                    }
                    continue;
                }
                if op.is_plain_value_copy()
                    && let Some(source) = op.operands.first()
                    && let Some(result) = op.results.first()
                    && let Some(fact) = facts.get(source).cloned()
                {
                    changed |= insert_value_storage_fact(&mut facts, *result, fact);
                }
            }
        }
    }
    facts
}

fn insert_value_storage_fact(
    facts: &mut HashMap<ValueId, ContainerStorageFact>,
    value: ValueId,
    fact: ContainerStorageFact,
) -> bool {
    if facts.get(&value) == Some(&fact) {
        return false;
    }
    facts.insert(value, fact);
    true
}

fn tir_op_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

fn op_produces_raw_i64_for_int_primary(
    op: &OpIR,
    candidates: &PlanHashSet<String>,
    bounded_i64_names: &PlanHashMap<String, I64Interval>,
) -> bool {
    let first_source = || {
        op.var.as_deref().or_else(|| {
            op.args
                .as_ref()
                .and_then(|args| args.first().map(String::as_str))
        })
    };
    match op.kind.as_str() {
        "const" | "loop_index_start" | "loop_index_next" | "len" | "gpu_thread_id"
        | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" => true,
        "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" | "pos"
        | "invert" => first_source().is_some_and(|s| candidates.contains(s)),
        "bit_and" | "bit_or" | "bit_xor" | "inplace_bit_and" | "inplace_bit_or"
        | "inplace_bit_xor" => op
            .args
            .as_ref()
            .is_some_and(|args| args.len() >= 2 && args.iter().all(|a| candidates.contains(a))),
        "add" | "inplace_add" | "sub" | "inplace_sub" => {
            op.out
                .as_deref()
                .is_some_and(|out| bounded_i64_names.contains_key(out))
                && op.args.as_ref().is_some_and(|args| {
                    args.len() >= 2 && args.iter().all(|arg| candidates.contains(arg))
                })
        }
        _ => false,
    }
}

/// Number of monotone `union` fixpoint passes allowed before the interval
/// analysis switches to a WIDENING join. A legitimately-converging program
/// stabilises in a handful of passes; anything still growing past this bound is
/// a self-referential accumulator (e.g. a fully-unrolled `total += i`) whose
/// range is unbounded, so we widen it to ±i64 (→ BigInt-correct boxed path)
/// rather than loop forever. The bound guarantees termination on ANY input.
const INTERVAL_WIDEN_AFTER_PASSES: u32 = 8;

fn compute_i64_interval_facts(func_ir: &FunctionIR) -> PlanHashMap<String, I64Interval> {
    let mut intervals = plan_hash_map(func_ir.ops.len());
    let loop_backedge_updates = loop_backedge_update_names(func_ir);
    let mut changed = true;
    let mut pass = 0u32;
    while changed {
        // After the union budget is spent, widen every join so any still-growing
        // interval saturates to ±i64 in one more step and the fixpoint
        // terminates. Widening is sound (a strict superset of the union result).
        let widen = pass >= INTERVAL_WIDEN_AFTER_PASSES;
        changed = false;
        for op in &func_ir.ops {
            if let Some(out) = op.out.as_deref()
                && let Some(interval) =
                    interval_for_simple_op(op, &intervals, &loop_backedge_updates)
            {
                changed |= insert_interval(&mut intervals, out, interval, widen);
            }
        }
        changed |= propagate_store_target_intervals(func_ir, &mut intervals, widen);
        changed |= propagate_counted_loop_intervals(func_ir, &mut intervals, widen);
        pass = pass.saturating_add(1);
    }
    intervals
}

fn insert_interval(
    intervals: &mut PlanHashMap<String, I64Interval>,
    name: &str,
    interval: I64Interval,
    widen: bool,
) -> bool {
    match intervals.get(name).copied() {
        Some(existing) => {
            let joined = if widen {
                existing.widen(interval)
            } else {
                existing.union(interval)
            };
            if joined == existing {
                false
            } else {
                intervals.insert(name.to_string(), joined);
                true
            }
        }
        None => {
            intervals.insert(name.to_string(), interval);
            true
        }
    }
}

fn interval_for_simple_op(
    op: &OpIR,
    intervals: &PlanHashMap<String, I64Interval>,
    loop_backedge_updates: &PlanHashSet<String>,
) -> Option<I64Interval> {
    if op
        .out
        .as_ref()
        .is_some_and(|out| loop_backedge_updates.contains(out))
        && matches!(
            op.kind.as_str(),
            "add" | "inplace_add" | "sub" | "inplace_sub"
        )
    {
        return None;
    }
    match op.kind.as_str() {
        "const" => op.value.map(I64Interval::singleton),
        "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" | "pos"
        | "loop_index_start" | "loop_index_next" => interval_for_first_source(op, intervals),
        "add" | "inplace_add" => interval_for_binary_args(op, intervals, I64Interval::checked_add),
        "sub" | "inplace_sub" => interval_for_binary_args(op, intervals, I64Interval::checked_sub),
        _ => None,
    }
}

fn interval_for_first_source(
    op: &OpIR,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<I64Interval> {
    let source = op.var.as_deref().or_else(|| {
        op.args
            .as_ref()
            .and_then(|args| args.first().map(String::as_str))
    })?;
    intervals.get(source).copied()
}

fn interval_for_binary_args(
    op: &OpIR,
    intervals: &PlanHashMap<String, I64Interval>,
    combine: fn(I64Interval, I64Interval) -> Option<I64Interval>,
) -> Option<I64Interval> {
    let args = op.args.as_ref()?;
    let lhs = intervals.get(args.first()?)?;
    let rhs = intervals.get(args.get(1)?)?;
    combine(*lhs, *rhs)
}

fn propagate_store_target_intervals(
    func_ir: &FunctionIR,
    intervals: &mut PlanHashMap<String, I64Interval>,
    widen: bool,
) -> bool {
    let mut targets: PlanHashMap<String, Option<I64Interval>> =
        plan_hash_map(func_ir.ops.len() / 4 + 1);
    for op in &func_ir.ops {
        let Some(target) = store_var_target_name(op) else {
            continue;
        };
        let source_interval =
            store_var_source_name(op).and_then(|source| intervals.get(source).copied());
        targets
            .entry(target.to_string())
            .and_modify(|existing| {
                *existing = match (*existing, source_interval) {
                    (Some(lhs), Some(rhs)) => Some(lhs.union(rhs)),
                    _ => None,
                };
            })
            .or_insert(source_interval);
    }

    let mut changed = false;
    for (target, interval) in targets {
        if let Some(interval) = interval {
            changed |= insert_interval(intervals, &target, interval, widen);
        }
    }
    changed
}

fn propagate_counted_loop_intervals(
    func_ir: &FunctionIR,
    intervals: &mut PlanHashMap<String, I64Interval>,
    widen: bool,
) -> bool {
    let mut changed = false;
    for (start, end) in loop_regions(&func_ir.ops) {
        if let Some(proof) = loop_index_interval_proof(func_ir, start, end, intervals) {
            for (name, interval) in proof.names {
                changed |= insert_interval(intervals, &name, interval, widen);
            }
        }
        if let Some(proof) = store_load_loop_interval_proof(func_ir, start, end, intervals) {
            for (name, interval) in proof.names {
                changed |= insert_interval(intervals, &name, interval, widen);
            }
        }
    }
    changed
}

struct CountedLoopIntervalProof {
    names: Vec<(String, I64Interval)>,
}

fn loop_regions(ops: &[OpIR]) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => stack.push(idx),
            "loop_end" => {
                if let Some(start) = stack.pop() {
                    regions.push((start, idx));
                }
            }
            _ => {}
        }
    }
    regions
}

fn loop_index_interval_proof(
    func_ir: &FunctionIR,
    start: usize,
    end: usize,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<CountedLoopIntervalProof> {
    let ops = &func_ir.ops;
    let (iv_start_idx, iv_name, init_name) = ((start + 1)..end).find_map(|idx| {
        let op = &ops[idx];
        (op.kind == "loop_index_start")
            .then(|| Some((idx, op.out.clone()?, op.args.as_ref()?.first()?.clone())))?
    })?;
    let init = resolve_interval_before(func_ir, iv_start_idx, &init_name, intervals, 0)?
        .singleton_value()?;
    let predicate = counted_loop_continue_predicate(func_ir, start, end, &iv_name, intervals)?;
    let (next_idx, next_name, step) =
        counted_loop_update(func_ir, start, end, &iv_name, intervals)?;
    let (iv_interval, next_interval) =
        bounded_loop_intervals(init, predicate.bound, step, predicate.op)?;
    if next_idx <= iv_start_idx {
        return None;
    }
    Some(CountedLoopIntervalProof {
        names: vec![(iv_name, iv_interval), (next_name, next_interval)],
    })
}

fn store_load_loop_interval_proof(
    func_ir: &FunctionIR,
    start: usize,
    end: usize,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<CountedLoopIntervalProof> {
    let ops = &func_ir.ops;
    for load_idx in (start + 1)..end {
        let load = &ops[load_idx];
        if load.kind != "load_var" {
            continue;
        }
        let slot_name = load.var.as_ref()?;
        let iv_name = load.out.as_ref()?;
        let init = resolve_store_slot_interval_before(func_ir, start, slot_name, intervals)?
            .singleton_value()?;
        let predicate = counted_loop_continue_predicate(func_ir, start, end, iv_name, intervals)?;
        let (store_idx, next_name, step) =
            store_load_loop_update(func_ir, start, end, slot_name, iv_name, intervals)?;
        if store_idx <= load_idx {
            continue;
        }
        let (iv_interval, next_interval) =
            bounded_loop_intervals(init, predicate.bound, step, predicate.op)?;
        let slot_interval = iv_interval.union(next_interval);
        let mut names = vec![
            (slot_name.clone(), slot_interval),
            (iv_name.clone(), iv_interval),
            (next_name, next_interval),
        ];
        for op in &ops[start + 1..end] {
            if op.kind == "load_var"
                && op.var.as_deref() == Some(slot_name.as_str())
                && let Some(out) = op.out.as_ref()
            {
                names.push((out.clone(), slot_interval));
            }
        }
        return Some(CountedLoopIntervalProof { names });
    }
    None
}

#[derive(Clone, Copy)]
enum LoopCompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

struct LoopContinuePredicate {
    op: LoopCompareOp,
    bound: i64,
}

fn counted_loop_continue_predicate(
    func_ir: &FunctionIR,
    start: usize,
    end: usize,
    iv_name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<LoopContinuePredicate> {
    let ops = &func_ir.ops;
    for break_idx in (start + 1)..end {
        let break_op = &ops[break_idx];
        if !matches!(
            break_op.kind.as_str(),
            "loop_break_if_false" | "loop_break_if_true"
        ) {
            continue;
        }
        let cond_name = break_op.args.as_ref()?.first()?;
        let compare_idx = (start + 1..break_idx).rev().find(|idx| {
            ops[*idx].out.as_ref() == Some(cond_name)
                && matches!(ops[*idx].kind.as_str(), "lt" | "le" | "gt" | "ge")
        })?;
        let compare = &ops[compare_idx];
        let args = compare.args.as_ref()?;
        let lhs = args.first()?;
        let rhs = args.get(1)?;
        let mut op = match compare.kind.as_str() {
            "lt" => LoopCompareOp::Lt,
            "le" => LoopCompareOp::Le,
            "gt" => LoopCompareOp::Gt,
            "ge" => LoopCompareOp::Ge,
            _ => return None,
        };
        let bound_name = if lhs == iv_name {
            rhs
        } else if rhs == iv_name {
            op = flip_compare_op(op);
            lhs
        } else {
            continue;
        };
        if break_op.kind == "loop_break_if_true" {
            op = invert_compare_op(op);
        }
        let bound = resolve_interval_before(func_ir, compare_idx, bound_name, intervals, 0)?
            .singleton_value()?;
        return Some(LoopContinuePredicate { op, bound });
    }
    None
}

fn flip_compare_op(op: LoopCompareOp) -> LoopCompareOp {
    match op {
        LoopCompareOp::Lt => LoopCompareOp::Gt,
        LoopCompareOp::Le => LoopCompareOp::Ge,
        LoopCompareOp::Gt => LoopCompareOp::Lt,
        LoopCompareOp::Ge => LoopCompareOp::Le,
    }
}

fn invert_compare_op(op: LoopCompareOp) -> LoopCompareOp {
    match op {
        LoopCompareOp::Lt => LoopCompareOp::Ge,
        LoopCompareOp::Le => LoopCompareOp::Gt,
        LoopCompareOp::Gt => LoopCompareOp::Le,
        LoopCompareOp::Ge => LoopCompareOp::Lt,
    }
}

fn counted_loop_update(
    func_ir: &FunctionIR,
    start: usize,
    end: usize,
    iv_name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<(usize, String, i64)> {
    let ops = &func_ir.ops;
    for idx in (start + 1..end).rev() {
        let op = &ops[idx];
        if op.kind != "loop_index_next" || op.out.as_deref() != Some(iv_name) {
            continue;
        }
        let next_name = op.args.as_ref()?.first()?.clone();
        let update_idx = (start + 1..idx)
            .rev()
            .find(|candidate| ops[*candidate].out.as_deref() == Some(next_name.as_str()))?;
        let step = induction_update_step(func_ir, update_idx, iv_name, intervals)?;
        return Some((idx, next_name, step));
    }
    None
}

fn store_load_loop_update(
    func_ir: &FunctionIR,
    start: usize,
    end: usize,
    slot_name: &str,
    iv_name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<(usize, String, i64)> {
    let ops = &func_ir.ops;
    for idx in (start + 1..end).rev() {
        let op = &ops[idx];
        if store_var_target_name(op) != Some(slot_name) {
            continue;
        }
        let next_name = store_var_source_name(op)?.to_string();
        let (_update_idx, update_name, step) = induction_update_for_name_before(
            func_ir, start, idx, &next_name, iv_name, intervals, 0,
        )?;
        return Some((idx, update_name, step));
    }
    None
}

fn induction_update_for_name_before(
    func_ir: &FunctionIR,
    start: usize,
    before_idx: usize,
    name: &str,
    iv_name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
    depth: usize,
) -> Option<(usize, String, i64)> {
    if depth > 8 {
        return None;
    }
    let ops = &func_ir.ops;
    for update_idx in (start + 1..before_idx).rev() {
        let op = &ops[update_idx];
        if op.out.as_deref() != Some(name) {
            continue;
        }
        if let Some(step) = induction_update_step(func_ir, update_idx, iv_name, intervals) {
            return Some((update_idx, name.to_string(), step));
        }
        if let Some(source) = alias_source_name(op) {
            return induction_update_for_name_before(
                func_ir,
                start,
                update_idx,
                source,
                iv_name,
                intervals,
                depth + 1,
            );
        }
        return None;
    }
    None
}

fn induction_update_step(
    func_ir: &FunctionIR,
    update_idx: usize,
    iv_name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<i64> {
    let op = &func_ir.ops[update_idx];
    let args = op.args.as_ref()?;
    let lhs = args.first()?;
    let rhs = args.get(1)?;
    match op.kind.as_str() {
        "add" | "inplace_add" => {
            if lhs == iv_name {
                resolve_interval_before(func_ir, update_idx, rhs, intervals, 0)?.singleton_value()
            } else if rhs == iv_name {
                resolve_interval_before(func_ir, update_idx, lhs, intervals, 0)?.singleton_value()
            } else {
                None
            }
        }
        "sub" | "inplace_sub" => {
            if lhs == iv_name {
                let step = resolve_interval_before(func_ir, update_idx, rhs, intervals, 0)?
                    .singleton_value()?;
                step.checked_neg()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn bounded_loop_intervals(
    init: i64,
    bound: i64,
    step: i64,
    op: LoopCompareOp,
) -> Option<(I64Interval, I64Interval)> {
    if step == 0 {
        return None;
    }
    let init = init as i128;
    let bound = bound as i128;
    let step = step as i128;
    let body_edge = match (step > 0, op) {
        (true, LoopCompareOp::Lt) => bound.checked_sub(1)?,
        (true, LoopCompareOp::Le) => bound,
        (false, LoopCompareOp::Gt) => bound.checked_add(1)?,
        (false, LoopCompareOp::Ge) => bound,
        _ => return None,
    };
    let next_edge = body_edge.checked_add(step)?;
    let init_next = init.checked_add(step)?;
    let iv_min = init.min(body_edge);
    let iv_max = init.max(body_edge);
    let next_min = init.min(init_next).min(next_edge);
    let next_max = init.max(init_next).max(next_edge);
    Some((
        I64Interval::from_i128_bounds(iv_min, iv_max)?,
        I64Interval::from_i128_bounds(next_min, next_max)?,
    ))
}

fn resolve_interval_before(
    func_ir: &FunctionIR,
    before_idx: usize,
    name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
    depth: usize,
) -> Option<I64Interval> {
    if depth > 8 {
        return None;
    }
    if let Some(interval) = intervals.get(name).copied() {
        return Some(interval);
    }
    for idx in (0..before_idx).rev() {
        let op = &func_ir.ops[idx];
        if store_var_target_name(op) == Some(name) {
            let source = store_var_source_name(op)?;
            return resolve_interval_before(func_ir, idx, source, intervals, depth + 1);
        }
        if op.out.as_deref() != Some(name) {
            continue;
        }
        if let Some(interval) = interval_for_simple_op(op, intervals, &plan_hash_set(0)) {
            return Some(interval);
        }
        match op.kind.as_str() {
            "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" | "pos" => {
                let source = op.var.as_deref().or_else(|| {
                    op.args
                        .as_ref()
                        .and_then(|args| args.first().map(String::as_str))
                })?;
                return resolve_interval_before(func_ir, idx, source, intervals, depth + 1);
            }
            _ => return None,
        }
    }
    None
}

fn resolve_store_slot_interval_before(
    func_ir: &FunctionIR,
    before_idx: usize,
    slot_name: &str,
    intervals: &PlanHashMap<String, I64Interval>,
) -> Option<I64Interval> {
    for idx in (0..before_idx).rev() {
        let op = &func_ir.ops[idx];
        if store_var_target_name(op) == Some(slot_name) {
            let source = store_var_source_name(op)?;
            return resolve_interval_before(func_ir, idx, source, intervals, 0);
        }
    }
    None
}

fn loop_backedge_update_names(func_ir: &FunctionIR) -> PlanHashSet<String> {
    let mut names = plan_hash_set(func_ir.ops.len() / 8 + 1);
    let ops = &func_ir.ops;
    for (start, end) in loop_regions(&func_ir.ops) {
        for idx in start + 1..end {
            let op = &ops[idx];
            if op.kind == "loop_index_next"
                && let Some(source) = op.args.as_ref().and_then(|args| args.first())
            {
                names.insert(source.clone());
            }
            if op.kind != "store_var" {
                continue;
            }
            let Some(slot_name) = store_var_target_name(op) else {
                continue;
            };
            let Some(source) = store_var_source_name(op) else {
                continue;
            };
            let Some((update_idx, update_name)) =
                alias_resolved_output_before(func_ir, start, idx, source, 0)
            else {
                continue;
            };
            let update = &ops[update_idx];
            if !matches!(
                update.kind.as_str(),
                "add" | "inplace_add" | "sub" | "inplace_sub"
            ) {
                continue;
            }
            let args = update.args.as_deref().unwrap_or(&[]);
            let updates_slot_load = args.iter().any(|arg| {
                (start + 1..update_idx).rev().any(|load_idx| {
                    let load = &ops[load_idx];
                    load.kind == "load_var"
                        && load.out.as_deref() == Some(arg.as_str())
                        && load.var.as_deref() == Some(slot_name)
                })
            });
            if updates_slot_load {
                names.insert(update_name);
            }
        }
    }
    names
}

fn alias_resolved_output_before(
    func_ir: &FunctionIR,
    start: usize,
    before_idx: usize,
    name: &str,
    depth: usize,
) -> Option<(usize, String)> {
    if depth > 8 {
        return None;
    }
    let ops = &func_ir.ops;
    for idx in (start + 1..before_idx).rev() {
        let op = &ops[idx];
        if op.out.as_deref() != Some(name) {
            continue;
        }
        if let Some(source) = alias_source_name(op) {
            return alias_resolved_output_before(func_ir, start, idx, source, depth + 1);
        }
        return Some((idx, name.to_string()));
    }
    None
}

/// OSC admission for `overflow_peel`'d loops (the name-keyed native chain).
///
/// The peeled fast loop's carrier cycle is
/// `slot --load_var--> arg --checked_add--> sum --store_var--> slot`, plus
/// the `prev_*` snapshot slots (`slot --load_var--> val --store_var--> slot`).
/// The cycle is raw-i64-admissible AS A UNIT: `checked_add`'s contract — a
/// true wrapping-i64 sum with a hardware overflow flag, every wrapped value
/// CFG-gated by the peel — makes the raw carrier exact for every reachable
/// value, which the interval chain cannot prove (the accumulator is genuinely
/// unbounded; that is the point of the peel).
///
/// Greatest-fixpoint: optimistically admit every `checked_add` sum, every
/// `load_var` result, and every `store_var` slot, then iteratively STRIP any
/// member whose defs don't conform —
///
/// * a sum whose arg is neither an in-set member nor interval-bounded,
/// * a load whose slot was stripped,
/// * a slot with ANY store from a non-member, non-bounded source,
/// * anything failing the int-primary `passes_filter` (type/poison gates).
///
/// Fail-closed: a questionable member invalidates its dependents, so the
/// boxed slow-loop clone (whose plain `add` sums are not members) and the
/// exit-merge slot (fed by both loops) strip out exactly as required, while
/// the fast loop's accumulator, IV, and `prev_*` snapshot slots survive.
fn checked_loop_seed_names(
    func_ir: &FunctionIR,
    bounded_i64_names: &PlanHashMap<String, I64Interval>,
    passes_filter: &dyn Fn(&str) -> bool,
) -> BTreeSet<String> {
    // No checked_add/checked_mul ops → nothing to admit (the common case, zero
    // cost). Both produce a wrapping-i64 result with a hardware overflow flag,
    // CFG-gated by the peel, so both are raw-i64-admissible as a carrier unit.
    if !func_ir
        .ops
        .iter()
        .any(|op| op.kind == "checked_add" || op.kind == "checked_mul")
    {
        return BTreeSet::new();
    }

    // Structure maps over the SimpleIR.
    let mut sum_args: BTreeMap<&str, Vec<&str>> = BTreeMap::new(); // sum var → args
    let mut load_slot: BTreeMap<&str, &str> = BTreeMap::new(); // load out → slot
    let mut slot_sources: BTreeMap<&str, Vec<&str>> = BTreeMap::new(); // slot → store sources
    for op in &func_ir.ops {
        match op.kind.as_str() {
            // Both checked ops have the same carrier shape: result `var` from
            // two operand `args` (the wrapping result; the overflow flag is a
            // separate result not relevant to raw-carrier admission).
            "checked_add" | "checked_mul" => {
                if let (Some(var), Some(args)) = (op.var.as_deref(), op.args.as_ref())
                    && args.len() == 2
                {
                    sum_args.insert(var, args.iter().map(String::as_str).collect());
                }
            }
            "load_var" => {
                if let (Some(out), Some(slot)) = (op.out.as_deref(), op.var.as_deref()) {
                    load_slot.insert(out, slot);
                }
            }
            "store_var" => {
                let target = op.var.as_deref().or(op.out.as_deref());
                let source = op
                    .args
                    .as_ref()
                    .and_then(|args| args.first().map(String::as_str));
                if let (Some(target), Some(source)) = (target, source) {
                    slot_sources.entry(target).or_default().push(source);
                }
            }
            _ => {}
        }
    }

    // Optimistic membership: every sum, load result, and stored slot.
    let mut members: BTreeSet<&str> = BTreeSet::new();
    members.extend(sum_args.keys().copied());
    members.extend(load_slot.keys().copied());
    members.extend(slot_sources.keys().copied());

    // Strip to the greatest conforming fixpoint.
    let mut changed = true;
    while changed {
        changed = false;
        let conforming_source = |s: &str, members: &BTreeSet<&str>| {
            members.contains(s) || bounded_i64_names.contains_key(s)
        };
        let snapshot: Vec<&str> = members.iter().copied().collect();
        for m in snapshot {
            let ok = if let Some(args) = sum_args.get(m) {
                args.iter().all(|a| conforming_source(a, &members))
            } else if let Some(slot) = load_slot.get(m) {
                members.contains(slot)
            } else if let Some(sources) = slot_sources.get(m) {
                sources.iter().all(|s| conforming_source(s, &members))
            } else {
                false
            };
            if !(ok && passes_filter(m)) && members.remove(m) {
                changed = true;
            }
        }
    }

    members.into_iter().map(str::to_string).collect()
}

fn bounded_store_load_loop_seed_names(
    func_ir: &FunctionIR,
    bounded_i64_names: &PlanHashMap<String, I64Interval>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (start, end) in loop_regions(&func_ir.ops) {
        let Some(proof) = store_load_loop_interval_proof(func_ir, start, end, bounded_i64_names)
        else {
            continue;
        };
        let Some((slot_name, _)) = proof.names.first() else {
            continue;
        };
        if store_slot_initial_source_is_raw_seed(func_ir, start, slot_name, bounded_i64_names) {
            names.insert(slot_name.clone());
        }
    }
    names
}

fn store_slot_initial_source_is_raw_seed(
    func_ir: &FunctionIR,
    before_idx: usize,
    slot_name: &str,
    bounded_i64_names: &PlanHashMap<String, I64Interval>,
) -> bool {
    for idx in (0..before_idx).rev() {
        let op = &func_ir.ops[idx];
        if store_var_target_name(op) == Some(slot_name) {
            return store_var_source_name(op).is_some_and(|source| {
                name_is_structural_raw_i64_before(func_ir, idx, source, bounded_i64_names, 0)
            });
        }
    }
    false
}

fn name_is_structural_raw_i64_before(
    func_ir: &FunctionIR,
    before_idx: usize,
    name: &str,
    bounded_i64_names: &PlanHashMap<String, I64Interval>,
    depth: usize,
) -> bool {
    if depth > 8 || !bounded_i64_names.contains_key(name) {
        return false;
    }
    for idx in (0..before_idx).rev() {
        let op = &func_ir.ops[idx];
        if store_var_target_name(op) == Some(name) {
            return store_var_source_name(op).is_some_and(|source| {
                name_is_structural_raw_i64_before(
                    func_ir,
                    idx,
                    source,
                    bounded_i64_names,
                    depth + 1,
                )
            });
        }
        if op.out.as_deref() != Some(name) {
            continue;
        }
        return match op.kind.as_str() {
            "const" | "loop_index_start" | "loop_index_next" | "len" | "gpu_thread_id"
            | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" => true,
            "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" | "pos" => {
                let source = op.var.as_deref().or_else(|| {
                    op.args
                        .as_ref()
                        .and_then(|args| args.first().map(String::as_str))
                });
                source.is_some_and(|source| {
                    name_is_structural_raw_i64_before(
                        func_ir,
                        idx,
                        source,
                        bounded_i64_names,
                        depth + 1,
                    )
                })
            }
            "add" | "inplace_add" | "sub" | "inplace_sub" => op.args.as_ref().is_some_and(|args| {
                args.len() >= 2
                    && args.iter().all(|arg| {
                        name_is_structural_raw_i64_before(
                            func_ir,
                            idx,
                            arg,
                            bounded_i64_names,
                            depth + 1,
                        )
                    })
            }),
            _ => false,
        };
    }
    false
}

fn is_cold_module_chunk_function(name: &str) -> bool {
    name.contains("__molt_module_chunk_")
}

fn integer_arithmetic_result_op(kind: &str) -> bool {
    matches!(
        kind,
        "add"
            | "inplace_add"
            | "sub"
            | "inplace_sub"
            | "mul"
            | "inplace_mul"
            | "floordiv"
            | "inplace_floordiv"
            | "mod"
            | "mod_"
            | "inplace_mod"
    )
}

fn integer_only_result_op(kind: &str) -> bool {
    matches!(
        kind,
        "bit_and"
            | "inplace_bit_and"
            | "bit_or"
            | "inplace_bit_or"
            | "bit_xor"
            | "inplace_bit_xor"
            | "bitand"
            | "bitor"
            | "bitxor"
            | "lshift"
            | "inplace_lshift"
            | "rshift"
            | "inplace_rshift"
            | "shl"
            | "shr"
            | "neg"
            | "pos"
            | "abs"
            | "builtin_abs"
            | "invert"
    )
}

fn simple_op_produces_non_scalar_value(kind: &str) -> bool {
    matches!(
        kind,
        "async_sleep"
            | "asyncgen_new"
            | "bound_method_new"
            | "build_dict"
            | "build_list"
            | "builtin_type"
            | "buffer2d_new"
            | "callargs_new"
            | "cancel_token_new"
            | "chan_new"
            | "class_new"
            | "classmethod_new"
            | "code_new"
            | "dataclass_new"
            | "dataclass_new_values"
            | "dict_new"
            | "exception_new"
            | "exception_new_builtin"
            | "exception_new_builtin_empty"
            | "exception_new_builtin_one"
            | "exception_new_from_class"
            | "frozenset_new"
            | "func_new"
            | "func_new_closure"
            | "io_wait_new"
            | "list_from_range"
            | "list_int_new"
            | "list_new"
            | "lock_new"
            | "memoryview_new"
            | "module_new"
            | "object_new"
            | "promise_new"
            | "property_new"
            | "range_new"
            | "rlock_new"
            | "set_new"
            | "slice_new"
            | "socket_new"
            | "staticmethod_new"
            | "stream_new"
            | "string_split_field"
            | "super_new"
            | "task_new"
            | "tuple_from_list"
            | "tuple_new"
            | "ws_wait_new"
    )
}

fn alias_source_name(op: &OpIR) -> Option<&str> {
    match op.kind.as_str() {
        "copy" | "copy_var" | "load_var" | "identity_alias" | "binding_alias" => {
            op.var.as_deref().or_else(|| {
                op.args
                    .as_ref()
                    .and_then(|args| args.first().map(String::as_str))
            })
        }
        _ => None,
    }
}

/// The container [`TirType`] produced by a SimpleIR container-constructor op
/// kind, or `None` for any non-constructor kind.
///
/// These are the frontend/native container constructors that
/// [`ssa::kind_to_opcode`](crate::tir::ssa) lifts to the `OpCode::Copy`
/// passthrough (no dedicated opcode); see [`ScalarRepresentationPlan::
/// seed_container_constructor_facts`] for why the plan must override the
/// resulting element-aliased type with the true container kind. Element/key
/// types are intentionally `DynBox` and tuples are unknown-arity: the plan's
/// container facts drive lane/dispatch selection, which needs only the kind.
fn container_constructor_result_ty(kind: &str) -> Option<TirType> {
    let dynbox = || Box::new(TirType::DynBox);
    match kind {
        // List builders (variadic elements, typed-int list, runtime fill, range
        // materialization, `.copy()`) — all produce `list`; the element type is
        // not tracked here. NOTE: `list_index_range` is deliberately absent — it
        // is `list.index(value, start, end)`, which returns the int index, not a
        // list (frontend `type_hint="int"`).
        "list_new" | "list_int_new" | "list_fill_new" | "list_from_range" | "list_copy" => {
            Some(TirType::List(dynbox()))
        }
        // Dict builders. Keys/values not tracked here.
        "dict_new" | "dict_from_obj" => Some(TirType::Dict(dynbox(), dynbox())),
        // Set / frozenset builders. molt has no distinct frozenset container
        // kind; both probe through the shared set hash layout, so both type
        // `Set` for dispatch (`molt_set_contains` handles set + frozenset).
        "set_new" | "frozenset_new" => Some(TirType::Set(dynbox())),
        // Tuple builders. The element types/arity are not needed for container
        // dispatch, so an unknown-arity tuple is the canonical "is a tuple" fact.
        "tuple_new" | "tuple_from_list" => Some(TirType::Tuple(Vec::new())),
        _ => None,
    }
}

fn store_var_target_name(op: &OpIR) -> Option<&str> {
    if matches!(op.kind.as_str(), "store_var" | "delete_var") {
        op.var.as_deref().or(op.out.as_deref())
    } else {
        None
    }
}

fn store_var_source_name(op: &OpIR) -> Option<&str> {
    if op.kind == "delete_var" {
        return None;
    }
    op.args
        .as_ref()
        .and_then(|args| args.first().map(String::as_str))
}

/// Test fixtures shared across modules (the wasm fast-lane gate test in
/// `wasm.rs` consumes the peeled-loop shape too).
#[cfg(test)]
pub(crate) mod test_fixtures {
    use crate::ir::{FunctionIR, OpIR};

    pub(crate) fn op(kind: &str, out: Option<&str>, var: Option<&str>, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: out.map(str::to_string),
            var: var.map(str::to_string),
            args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
            ..OpIR::default()
        }
    }

    pub(crate) fn op_v(
        kind: &str,
        out: Option<&str>,
        var: Option<&str>,
        args: &[&str],
        value: i64,
    ) -> OpIR {
        OpIR {
            value: Some(value),
            ..op(kind, out, var, args)
        }
    }

    pub(crate) fn function(
        name: &str,
        params: &[&str],
        param_types: Option<Vec<&str>>,
        ops: Vec<OpIR>,
    ) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            params: params.iter().map(|param| param.to_string()).collect(),
            ops,
            param_types: param_types.map(|types| types.into_iter().map(str::to_string).collect()),
            source_file: None,
            is_extern: false,
        }
    }

    /// The EXACT post-`overflow_peel` SimpleIR shape (captured live from
    /// `tmp/peel_sum.py`'s `compute` — fast structured loop with two
    /// `checked_add`s + carried overflow flag + `prev_*` snapshot slots,
    /// post-loop dispatch, generic boxed slow loop, exit-arg merge).
    pub(crate) fn peeled_compute_func_ir() -> FunctionIR {
        function(
            "peel_compute",
            &["n"],
            None,
            vec![
                op_v("const", Some("v106"), None, &[], 0),
                op_v("const", Some("v108"), None, &[], 0),
                op_v("const", Some("v117"), None, &[], 1),
                op_v("const_bool", Some("_v43"), None, &[], 0),
                op("store_var", None, Some("_bb1_arg0"), &["v108"]),
                op("store_var", None, Some("_bb1_arg1"), &["v106"]),
                op("store_var", None, Some("_bb1_arg2"), &["_v43"]),
                op("store_var", None, Some("_bb1_arg3"), &["v108"]),
                op("store_var", None, Some("_bb1_arg4"), &["v106"]),
                op_v("jump", None, None, &[], 15),
                op_v("label", None, None, &[], 15),
                op("loop_start", None, None, &[]),
                op("load_var", Some("_v16"), Some("_bb1_arg0"), &[]),
                op("load_var", Some("_v17"), Some("_bb1_arg1"), &[]),
                op("load_var", Some("_v40"), Some("_bb1_arg2"), &[]),
                op("load_var", Some("_v41"), Some("_bb1_arg3"), &[]),
                op("load_var", Some("_v42"), Some("_bb1_arg4"), &[]),
                op("lt", Some("v111"), None, &["_v16", "n"]),
                op("not", Some("_v44"), None, &["_v40"]),
                op("and", Some("_v45"), None, &["v111", "_v44"]),
                op("loop_break_if_false", None, None, &["_v45"]),
                op("checked_add", Some("_v47"), Some("_v22"), &["_v17", "_v16"]),
                op("checked_add", Some("_v46"), Some("_v25"), &["_v16", "v117"]),
                op("or", Some("_v48"), None, &["_v46", "_v47"]),
                op("store_var", None, Some("_bb1_arg0"), &["_v25"]),
                op("store_var", None, Some("_bb1_arg1"), &["_v22"]),
                op("store_var", None, Some("_bb1_arg2"), &["_v48"]),
                op("store_var", None, Some("_bb1_arg3"), &["_v16"]),
                op("store_var", None, Some("_bb1_arg4"), &["_v17"]),
                op("loop_continue", None, None, &[]),
                op("loop_end", None, None, &[]),
                op_v("jump", None, None, &[], 19),
                op_v("label", None, None, &[], 19),
                op_v("br_if", None, None, &["_v40"], 20),
                op("store_var", None, Some("_bb5_arg0"), &["_v17"]),
                op_v("jump", None, None, &[], 17),
                op_v("label", None, None, &[], 20),
                op("store_var", None, Some("_bb7_arg0"), &["_v41"]),
                op("store_var", None, Some("_bb7_arg1"), &["_v42"]),
                op_v("jump", None, None, &[], 18),
                op_v("label", None, None, &[], 18),
                op("load_var", Some("_v29"), Some("_bb7_arg0"), &[]),
                op("load_var", Some("_v30"), Some("_bb7_arg1"), &[]),
                op_v("jump", None, None, &[], 21),
                op_v("label", None, None, &[], 21),
                op("lt", Some("v111"), None, &["_v29", "n"]),
                op_v("br_if", None, None, &["v111"], 16),
                op("store_var", None, Some("_bb5_arg0"), &["_v30"]),
                op_v("jump", None, None, &[], 17),
                op_v("label", None, None, &[], 17),
                op("load_var", Some("_v51"), Some("_bb5_arg0"), &[]),
                op("ret", None, Some("_v51"), &["_v51"]),
                op_v("label", None, None, &[], 16),
                op("add", Some("v114"), None, &["_v30", "_v29"]),
                op("add", Some("v118"), None, &["_v29", "v117"]),
                op("store_var", None, Some("_bb7_arg0"), &["v118"]),
                op("store_var", None, Some("_bb7_arg1"), &["v114"]),
                op_v("jump", None, None, &[], 18),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(kind: &str, out: Option<&str>, var: Option<&str>, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            out: out.map(str::to_string),
            var: var.map(str::to_string),
            args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
            ..OpIR::default()
        }
    }

    fn function(
        name: &str,
        params: &[&str],
        param_types: Option<Vec<&str>>,
        ops: Vec<OpIR>,
    ) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            params: params.iter().map(|param| param.to_string()).collect(),
            ops,
            param_types: param_types.map(|types| types.into_iter().map(str::to_string).collect()),
            source_file: None,
            is_extern: false,
        }
    }

    fn const_int(out: &str, value: i64) -> OpIR {
        OpIR {
            kind: "const".to_string(),
            out: Some(out.to_string()),
            value: Some(value),
            ..OpIR::default()
        }
    }

    fn const_bool(out: &str, value: bool) -> OpIR {
        OpIR {
            kind: "const_bool".to_string(),
            out: Some(out.to_string()),
            value: Some(i64::from(value)),
            ..OpIR::default()
        }
    }

    fn const_float(out: &str, value: f64) -> OpIR {
        OpIR {
            kind: "const_float".to_string(),
            out: Some(out.to_string()),
            f_value: Some(value),
            ..OpIR::default()
        }
    }

    #[test]
    fn dynbox_i64_fact_is_not_a_scalar_integer() {
        let mut plan = ScalarRepresentationPlan::default();
        plan.insert_fact(
            "boxed_word".to_string(),
            ScalarRepresentationFact {
                ty: TirType::I64,
                repr: LirRepr::DynBox,
            },
        );
        let func = function("empty", &[], None, vec![]);
        let fact_index = FunctionFactIndex::for_function(&func);
        plan.propagate_integer_family(&func, &fact_index);

        let (int_like, _, _, _, _) = plan.scalar_name_sets();

        assert!(!int_like.contains("boxed_word"));
        assert!(!plan.integer_family_names().contains("boxed_word"));
    }

    #[test]
    fn container_kind_comes_from_structured_tir_types() {
        let func = function(
            "typed_containers",
            &["xs", "d", "s", "t", "text"],
            Some(vec![
                "list[int]",
                "dict[str, int]",
                "set[bool]",
                "tuple[int, str]",
                "str",
            ]),
            vec![op("ret", None, None, &["xs"])],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.name_container_kind("xs"), Some(ContainerKind::List));
        assert_eq!(plan.name_container_kind("d"), Some(ContainerKind::Dict));
        assert_eq!(plan.name_container_kind("s"), Some(ContainerKind::Set));
        assert_eq!(plan.name_container_kind("t"), Some(ContainerKind::Tuple));
        assert_eq!(plan.name_container_kind("text"), Some(ContainerKind::Str));
    }

    #[test]
    fn container_transport_metadata_does_not_seed_container_kind() {
        let mut index = op("index", Some("item"), None, &["xs", "i"]);
        index.container_type = Some("list".to_string());
        index.type_hint = Some("list".to_string());
        let func = function("transport_only", &["xs", "i"], None, vec![index]);
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.name_container_kind("xs"), None);
        assert_eq!(plan.name_container_kind("item"), None);
    }

    #[test]
    fn flat_list_storage_requires_structural_producer() {
        let mut index = op("index", Some("item"), None, &["xs", "i"]);
        index.container_type = Some("list".to_string());
        let func = function(
            "transport_only_storage",
            &["xs", "i"],
            None,
            vec![index.clone()],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.name_container_storage_kind("xs"), None);
        assert!(!plan.op_has_container_storage(0, &index, ContainerStorageKind::FlatListInt));
    }

    #[test]
    fn list_int_new_seeds_flat_storage_and_aliases() {
        let list_new = op("list_int_new", Some("xs"), None, &[]);
        let copy = op("copy", Some("ys"), None, &["xs"]);
        let store = op("store_var", None, Some("slot"), &["ys"]);
        let load = op("load_var", Some("zs"), Some("slot"), &[]);
        let index = op("index", Some("item"), None, &["zs", "i"]);
        let func = function(
            "storage_aliases",
            &["i"],
            Some(vec!["int"]),
            vec![list_new, copy, store, load, index.clone()],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(
            plan.name_container_storage_kind("xs"),
            Some(ContainerStorageKind::FlatListInt)
        );
        assert_eq!(
            plan.name_container_storage_kind("ys"),
            Some(ContainerStorageKind::FlatListInt)
        );
        assert_eq!(
            plan.name_container_storage_kind("slot"),
            Some(ContainerStorageKind::FlatListInt)
        );
        assert_eq!(
            plan.name_container_storage_kind("zs"),
            Some(ContainerStorageKind::FlatListInt)
        );
        assert!(plan.op_has_container_storage(4, &index, ContainerStorageKind::FlatListInt));
    }

    #[test]
    fn non_int_store_index_conflicts_flat_list_storage() {
        let list_new = op("list_int_new", Some("xs"), None, &[]);
        let idx = const_int("i", 0);
        let value = const_float("f", 1.25);
        let store = op("store_index", Some("ys"), None, &["xs", "i", "f"]);
        let index = op("index", Some("item"), None, &["ys", "i"]);
        let func = function(
            "flat_storage_non_int_write",
            &[],
            None,
            vec![list_new, idx, value, store.clone(), index.clone()],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.name_container_storage_kind("xs"), None);
        assert_eq!(plan.name_container_storage_kind("ys"), None);
        assert!(!plan.op_has_container_storage(3, &store, ContainerStorageKind::FlatListInt));
        assert!(!plan.op_has_container_storage(4, &index, ContainerStorageKind::FlatListInt));
    }

    #[test]
    fn semantic_list_bool_index_does_not_authorize_raw_bool_primary() {
        let index = op("index", Some("item"), None, &["items", "idx"]);
        let func = function(
            "typed_list_bool_index",
            &["items", "idx"],
            Some(vec!["list[bool]", "int"]),
            vec![index],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (_, bool_like, _, _, _) = plan.scalar_name_sets();
        let primary = plan.primary_name_sets();

        assert!(
            bool_like.contains("item"),
            "semantic list[bool] indexing should refine the element type"
        );
        assert!(
            !primary.bool_.contains("item"),
            "semantic element type alone must not prove native raw-bool carrier codegen"
        );
        assert!(
            !plan.is_bool_unboxed("item"),
            "native raw-bool predicate must derive from repr_by_name eligibility, not semantic type"
        );
    }

    #[test]
    fn index_result_lane_comes_from_element_fact_not_key() {
        let index = op("index", Some("item"), None, &["items", "idx"]);
        let func = function(
            "typed_list_int_index",
            &["items", "idx"],
            Some(vec!["list[int]", "int"]),
            vec![index.clone()],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();
        let primary = plan.primary_name_sets();

        assert_eq!(plan.op_scalar_lane(&index), Some(ScalarKind::Int));
        assert!(plan.op_index_key_is_integer_family(&index));
        assert!(int_like.contains("item"));
        assert!(
            !primary.int.contains("item"),
            "generic index results are boxed transport unless lowering proves a raw element carrier"
        );
    }

    #[test]
    fn ord_at_result_is_integer_family_from_tir_not_transport_hints() {
        let mut ord_at = op("ord_at", Some("code"), None, &["text", "idx"]);
        ord_at.type_hint = Some("list".to_string());
        ord_at.container_type = Some("list".to_string());
        ord_at.fast_int = Some(true);
        let add = op("add", Some("shifted"), None, &["code", "bias"]);
        let func = function(
            "ord_at_representation",
            &[],
            None,
            vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("text".to_string()),
                    s_value: Some("AéZ".to_string()),
                    ..OpIR::default()
                },
                const_int("idx", 1),
                ord_at,
                const_int("bias", 1),
                add.clone(),
            ],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();

        assert!(
            int_like.contains("code"),
            "ord_at result must be proven by first-class typed TIR/LIR lowering"
        );
        assert!(
            plan.name_is_integer_family("shifted"),
            "downstream arithmetic must consume ord_at's structural integer-family fact"
        );
        assert_eq!(plan.name_container_kind("code"), None);
        assert_eq!(plan.op_scalar_lane(&add), Some(ScalarKind::Int));
        assert!(
            plan.integer_family_names().contains("code"),
            "legacy result metadata must not be required for ord_at integer-family propagation"
        );
    }

    #[test]
    fn generic_index_does_not_promote_result_from_integer_key() {
        let index = op(
            "index",
            Some("object_type_tag"),
            None,
            &["__molt_split_frame", "__molt_split_frame_index"],
        );
        let func = function(
            "split_frame_index",
            &[],
            None,
            vec![
                op("list_new", Some("__molt_split_frame"), None, &[]),
                const_int("__molt_split_frame_index", 0),
                index.clone(),
                op(
                    "builtin_type",
                    Some("object_type"),
                    None,
                    &["object_type_tag"],
                ),
            ],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();
        let primary = plan.primary_name_sets();

        assert_eq!(plan.op_scalar_lane(&index), None);
        assert!(plan.op_index_key_is_integer_family(&index));
        assert!(!int_like.contains("object_type_tag"));
        assert!(!primary.int.contains("object_type_tag"));
    }

    #[test]
    fn alias_group_unknown_loop_header_source_terminates_without_promotion() {
        let func = function(
            "alias_group_unknown_loop_header_source",
            &[],
            None,
            vec![
                const_int("zero", 0),
                op("const_none", Some("none"), None, &[]),
                const_int("one", 1),
                op("store_var", None, Some("_bb2_arg0"), &["zero"]),
                op("store_var", None, Some("_bb2_arg0"), &["none"]),
                op("load_var", Some("_v19"), Some("_bb2_arg0"), &[]),
                op("add", Some("next"), None, &["one", "one"]),
                op("store_var", None, Some("_v19"), &["next"]),
                op("copy_var", Some("after"), None, &["_v19"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();

        assert!(
            !int_like.contains("_v19"),
            "ambiguous loop-header alias/store join must not re-promote _v19"
        );
        assert!(
            !int_like.contains("after"),
            "aliases fed by an ambiguous loop-header source must stay unpromoted"
        );
    }

    #[test]
    fn pending_store_target_dominates_same_name_alias_output() {
        let func = function(
            "pending_store_target_dominates_same_name_alias_output",
            &[],
            None,
            vec![
                const_int("one", 1),
                op("copy_var", Some("slot"), None, &["one"]),
                op("store_var", None, Some("slot"), &["unproven_source"]),
                op("copy_var", Some("after"), None, &["slot"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();

        assert!(
            !int_like.contains("slot"),
            "pending store target must prevent same-name alias output reinsertion"
        );
        assert!(
            !int_like.contains("after"),
            "aliases from a pending store target must not inherit stale facts"
        );
    }

    #[test]
    fn pending_store_target_remains_relevant_for_same_name_alias_output() {
        let func = function(
            "pending_store_target_remains_relevant_for_same_name_alias_output",
            &[],
            None,
            vec![
                const_int("one", 1),
                op("copy_var", Some("slot"), None, &["one"]),
                op("store_var", None, Some("slot"), &["unproven_source"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();

        assert!(
            !int_like.contains("slot"),
            "same-name alias output must keep a pending store target relevant"
        );
    }

    #[test]
    fn pending_alias_source_blocks_store_target_reinsert_loop() {
        let func = function(
            "pending_alias_source_blocks_store_target_reinsert_loop",
            &[],
            None,
            vec![
                const_int("one", 1),
                op("store_var", None, Some("loop_slot"), &["unproven_source"]),
                op("load_var", Some("iv"), Some("loop_slot"), &[]),
                op("store_var", None, Some("iv"), &["one"]),
                op("copy_var", Some("after"), None, &["iv"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, _, _, _) = plan.scalar_name_sets();

        assert!(
            !int_like.contains("iv"),
            "a name defined by both a pending load alias and a store target must not oscillate back to int"
        );
        assert!(
            !int_like.contains("after"),
            "aliases fed by the blocked name must not inherit a stale fact"
        );
    }

    #[test]
    fn iter_next_done_flag_uses_fused_bool_fact_not_index_fast_int_hint() {
        let mut done_index = const_int("done_index", 1);
        done_index.fast_int = Some(true);
        let mut done = op("index", Some("done_flag"), None, &["pair", "done_index"]);
        done.fast_int = Some(true);
        let mut value_index = const_int("value_index", 0);
        value_index.fast_int = Some(true);
        let mut value = op("index", Some("next_value"), None, &["pair", "value_index"]);
        value.fast_int = Some(true);
        let func = function(
            "iter_next_done_flag",
            &["items"],
            None,
            vec![
                op("iter", Some("iter_obj"), None, &["items"]),
                op("iter_next", Some("pair"), None, &["iter_obj"]),
                done_index,
                done.clone(),
                value_index,
                value,
                op("loop_break_if_true", None, None, &["done_flag"]),
            ],
        );
        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, bool_like, _, _, _) = plan.scalar_name_sets();
        let primary = plan.primary_name_sets();

        assert!(
            bool_like.contains("done_flag"),
            "fused iter_next done flag must retain its bool fact under the original SimpleIR name"
        );
        assert!(
            !int_like.contains("done_flag"),
            "index fast_int metadata cannot override the fused done flag's bool type"
        );
        assert_eq!(plan.op_scalar_lane(&done), Some(ScalarKind::Bool));
        assert!(
            !primary.int.contains("done_flag"),
            "done flag must never be routed through raw-int primary storage"
        );
    }

    #[test]
    fn conflicting_facts_do_not_pick_order_dependent_scalar_lane() {
        let mut plan = ScalarRepresentationPlan::default();
        plan.insert_fact(
            "ambiguous".to_string(),
            ScalarRepresentationFact {
                ty: TirType::I64,
                repr: LirRepr::I64,
            },
        );
        plan.insert_fact(
            "ambiguous".to_string(),
            ScalarRepresentationFact {
                ty: TirType::Bool,
                repr: LirRepr::Bool1,
            },
        );
        let func = function("empty", &[], None, vec![]);
        let fact_index = FunctionFactIndex::for_function(&func);
        plan.propagate_integer_family(&func, &fact_index);

        let (int_like, bool_like, _, _, _) = plan.scalar_name_sets();

        assert!(!int_like.contains("ambiguous"));
        assert!(!bool_like.contains("ambiguous"));
        assert!(!plan.integer_family_names().contains("ambiguous"));
    }

    #[test]
    fn plan_uses_entry_param_names_as_scalar_facts() {
        let func = function(
            "typed_params",
            &["x", "flag"],
            Some(vec!["int", "bool"]),
            vec![op("ret", None, Some("x"), &[])],
        );

        let (int_like, bool_like, _, _, _) =
            ScalarRepresentationPlan::for_function_ir(&func).scalar_name_sets();

        assert!(int_like.contains("x"));
        assert!(bool_like.contains("flag"));
    }

    fn graph_fact_test_index() -> (IndexedFunctionFactIndex<'static>, NameId) {
        let mut names = FunctionNameIndex::with_capacity(1);
        let target = names.intern("target");
        let len = names.len();
        (
            IndexedFunctionFactIndex {
                names,
                stores: Vec::new(),
                alias_groups: Vec::new(),
                alias_sources: vec![false; len],
                alias_outputs: vec![false; len],
            },
            target,
        )
    }

    fn int_fact() -> ScalarRepresentationFact {
        ScalarRepresentationFact {
            ty: TirType::I64,
            repr: LirRepr::I64,
        }
    }

    fn bool_fact() -> ScalarRepresentationFact {
        ScalarRepresentationFact {
            ty: TirType::Bool,
            repr: LirRepr::Bool1,
        }
    }

    fn dynbox_top_fact() -> ScalarRepresentationFact {
        ScalarRepresentationFact {
            ty: TirType::DynBox,
            repr: LirRepr::DynBox,
        }
    }

    #[test]
    fn graph_join_does_not_narrow_strong_dynbox_top() {
        let (index, target) = graph_fact_test_index();
        let mut plan = ScalarRepresentationPlan::with_capacity(1);
        plan.insert_fact("target".to_string(), dynbox_top_fact());
        let mut facts = IndexedScalarFacts::from_plan(&plan, &index);
        let int_id = facts.intern_fact(int_fact());

        assert!(!facts.insert_graph_fact_id(target, int_id));
        facts.sync_to_plan(&mut plan, &index);

        assert_eq!(plan.facts_by_name.get("target"), Some(&dynbox_top_fact()));
        assert!(!plan.scalar_name_sets().0.contains("target"));
    }

    #[test]
    fn graph_join_replaces_weak_dynbox_fallback_with_proven_source() {
        let (index, target) = graph_fact_test_index();
        let mut plan = ScalarRepresentationPlan::with_capacity(1);
        plan.insert_fact("target".to_string(), dynbox_top_fact());
        plan.weak_fact_names.insert("target".to_string());
        let mut facts = IndexedScalarFacts::from_plan(&plan, &index);
        let int_id = facts.intern_fact(int_fact());

        assert!(facts.insert_graph_fact_id(target, int_id));
        facts.sync_to_plan(&mut plan, &index);

        assert_eq!(plan.facts_by_name.get("target"), Some(&int_fact()));
        assert!(!plan.weak_fact_names.contains("target"));
        assert!(plan.scalar_name_sets().0.contains("target"));
    }

    #[test]
    fn graph_join_incoming_dynbox_widens_specific_fact() {
        let (index, target) = graph_fact_test_index();
        let mut plan = ScalarRepresentationPlan::with_capacity(1);
        plan.insert_fact("target".to_string(), int_fact());
        let mut facts = IndexedScalarFacts::from_plan(&plan, &index);
        let top_id = facts.intern_fact(dynbox_top_fact());

        assert!(facts.insert_graph_fact_id(target, top_id));
        facts.sync_to_plan(&mut plan, &index);

        assert_eq!(plan.facts_by_name.get("target"), Some(&dynbox_top_fact()));
        assert!(!plan.scalar_name_sets().0.contains("target"));
    }

    #[test]
    fn graph_join_conflicts_different_strong_non_top_facts() {
        let (index, target) = graph_fact_test_index();
        let mut plan = ScalarRepresentationPlan::with_capacity(1);
        plan.insert_fact("target".to_string(), int_fact());
        let mut facts = IndexedScalarFacts::from_plan(&plan, &index);
        let bool_id = facts.intern_fact(bool_fact());

        assert!(facts.insert_graph_fact_id(target, bool_id));
        facts.sync_to_plan(&mut plan, &index);

        assert!(!plan.facts_by_name.contains_key("target"));
        assert!(plan.conflicted_names.contains("target"));
        let (int_like, bool_like, _, _, _) = plan.scalar_name_sets();
        assert!(!int_like.contains("target"));
        assert!(!bool_like.contains("target"));
    }

    #[test]
    fn plan_propagates_store_targets_only_when_all_sources_match() {
        let mixed = function(
            "mixed_store",
            &[],
            None,
            vec![
                const_int("i", 1),
                const_bool("b", true),
                op("store_var", None, Some("slot"), &["i"]),
                op("store_var", None, Some("slot"), &["b"]),
                op("ret", None, Some("slot"), &[]),
            ],
        );
        let (int_like, bool_like, _, _, _) =
            ScalarRepresentationPlan::for_function_ir(&mixed).scalar_name_sets();
        assert!(!int_like.contains("slot"));
        assert!(!bool_like.contains("slot"));

        let uniform = function(
            "uniform_store",
            &[],
            None,
            vec![
                const_int("i", 1),
                op("store_var", None, Some("slot"), &["i"]),
                op("ret", None, Some("slot"), &[]),
            ],
        );
        let (int_like, _, _, _, _) =
            ScalarRepresentationPlan::for_function_ir(&uniform).scalar_name_sets();
        assert!(int_like.contains("slot"));
    }

    #[test]
    fn unknown_store_target_blocks_alias_output_reinsertion() {
        let func = function(
            "store_alias_output_cycle",
            &[],
            None,
            vec![
                op("store_var", None, Some("slot"), &["unknown_source"]),
                op("copy", Some("slot"), None, &["seed"]),
                op("load_var", Some("loaded"), Some("slot"), &[]),
            ],
        );
        let fact_index = FunctionFactIndex::for_function(&func);
        let mut plan = ScalarRepresentationPlan::default();
        let int_fact = ScalarRepresentationFact {
            ty: TirType::I64,
            repr: LirRepr::I64,
        };
        plan.insert_fact("seed".to_string(), int_fact.clone());
        plan.insert_fact("slot".to_string(), int_fact);

        let indexed_fact_index = IndexedFunctionFactIndex::for_function_facts(&fact_index);
        plan.propagate_simple_aliases(&indexed_fact_index);

        let (int_like, _, _, _, _) = plan.scalar_name_sets();
        assert!(int_like.contains("seed"));
        assert!(
            !int_like.contains("slot"),
            "unknown store targets must not be reintroduced through alias outputs"
        );
        assert!(
            !int_like.contains("loaded"),
            "aliases loaded from an unknown store target must remain unproven"
        );
    }

    #[test]
    fn generic_type_hint_does_not_seed_plan_scalar_fact() {
        let mut generic = op("call", Some("maybe_int"), None, &[]);
        generic.type_hint = Some("int".to_string());
        let func = function("generic_hint", &[], None, vec![generic]);

        let (int_like, _, _, _, _) =
            ScalarRepresentationPlan::for_function_ir(&func).scalar_name_sets();

        assert!(!int_like.contains("maybe_int"));
    }

    #[test]
    fn integer_family_preserves_boxed_unbounded_arithmetic_lane() {
        let func = function(
            "integer_family",
            &["seed"],
            Some(vec!["int"]),
            vec![
                const_int("factor", 3_266_489_917),
                op("mul", Some("wide"), None, &["seed", "factor"]),
                const_int("mask", 7),
                op("bit_or", Some("masked"), None, &["wide", "mask"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let (int_like, _, float_like, _, _) = plan.scalar_name_sets();
        let integer_family = plan.integer_family_names();

        assert!(integer_family.contains("wide"));
        assert!(integer_family.contains("masked"));
        assert!(!int_like.contains("wide"));
        assert!(!float_like.contains("wide"));
        assert!(!float_like.contains("masked"));
    }

    #[test]
    fn primary_int_names_admit_bounded_arithmetic_range_proof() {
        let func = function(
            "int_primary",
            &[],
            None,
            vec![
                const_int("lhs", 5),
                const_int("rhs", 3),
                op("bit_xor", Some("masked"), None, &["lhs", "rhs"]),
                op("add", Some("sum"), None, &["lhs", "rhs"]),
                op("lshift", Some("shifted"), None, &["lhs", "rhs"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let primary = plan.primary_name_sets();

        assert!(primary.int.contains("lhs"));
        assert!(primary.int.contains("rhs"));
        assert!(primary.int.contains("masked"));
        assert!(primary.int.contains("sum"));
        assert!(!primary.int.contains("shifted"));
    }

    #[test]
    fn primary_int_names_exclude_unbounded_param_arithmetic_without_range_proof() {
        let func = function(
            "int_primary_params",
            &["lhs", "rhs"],
            Some(vec!["int", "int"]),
            vec![op("add", Some("sum"), None, &["lhs", "rhs"])],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let primary = plan.primary_name_sets();

        assert!(!primary.int.contains("lhs"));
        assert!(!primary.int.contains("rhs"));
        assert!(!primary.int.contains("sum"));
    }

    #[test]
    fn primary_int_names_exclude_arithmetic_that_can_overflow_i64() {
        let func = function(
            "int_primary_overflow",
            &[],
            None,
            vec![
                const_int("lhs", i64::MAX),
                const_int("rhs", 1),
                op("add", Some("sum"), None, &["lhs", "rhs"]),
            ],
        );

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(primary.int.contains("lhs"));
        assert!(primary.int.contains("rhs"));
        assert!(!primary.int.contains("sum"));
    }

    #[test]
    fn counted_store_load_loop_proves_bounded_i64_add() {
        let func = function(
            "counted_store_load_loop",
            &[],
            None,
            vec![
                const_int("init", 0),
                const_int("one", 1),
                const_int("stop", 1_000_000),
                op("store_var", None, Some("i"), &["init"]),
                op("loop_start", None, None, &[]),
                op("load_var", Some("i_cur"), Some("i"), &[]),
                op("lt", Some("keep_going"), None, &["i_cur", "stop"]),
                op("loop_break_if_false", None, None, &["keep_going"]),
                op("add", Some("i_next"), None, &["i_cur", "one"]),
                op("store_var", None, Some("i"), &["i_next"]),
                op("loop_continue", None, None, &[]),
                op("loop_end", None, None, &[]),
                op("load_var", Some("i_after"), Some("i"), &[]),
            ],
        );

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(primary.int.contains("i"));
        assert!(primary.int.contains("i_cur"));
        assert!(primary.int.contains("i_next"));
        assert!(primary.int.contains("i_after"));
    }

    #[test]
    fn mismatched_counted_loop_direction_does_not_prove_update_range() {
        let func = function(
            "mismatched_counted_loop",
            &[],
            None,
            vec![
                const_int("init", 0),
                const_int("one", 1),
                const_int("stop", 1_000_000),
                op("store_var", None, Some("i"), &["init"]),
                op("loop_start", None, None, &[]),
                op("load_var", Some("i_cur"), Some("i"), &[]),
                op("gt", Some("keep_going"), None, &["i_cur", "stop"]),
                op("loop_break_if_false", None, None, &["keep_going"]),
                op("add", Some("i_next"), None, &["i_cur", "one"]),
                op("store_var", None, Some("i"), &["i_next"]),
                op("loop_continue", None, None, &[]),
                op("loop_end", None, None, &[]),
            ],
        );

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(!primary.int.contains("i"));
        assert!(!primary.int.contains("i_cur"));
        assert!(!primary.int.contains("i_next"));
    }

    #[test]
    fn bool_primary_predicate_is_raw_closed() {
        let candidates = BTreeSet::from(["flag".to_string()]);
        let int_primary = BTreeSet::from(["idx".to_string()]);
        let empty_plan = ScalarRepresentationPlan::default();
        let const_bool = const_bool("flag", true);
        assert!(empty_plan.op_produces_raw_bool_for_bool_primary(
            &const_bool,
            &BTreeSet::new(),
            &int_primary,
        ));

        let copy = op("copy_var", Some("flag_copy"), Some("flag"), &[]);
        assert!(empty_plan.op_produces_raw_bool_for_bool_primary(&copy, &candidates, &int_primary));

        let comparison = op("eq", Some("cmp"), None, &["lhs", "rhs"]);
        assert!(empty_plan.op_produces_raw_bool_for_bool_primary(
            &comparison,
            &candidates,
            &int_primary,
        ));

        let boolean_cast = op("bool", Some("casted"), None, &["flag"]);
        assert!(empty_plan.op_produces_raw_bool_for_bool_primary(
            &boolean_cast,
            &candidates,
            &int_primary,
        ));

        let list_bool_index = op("index", Some("item"), None, &["items", "idx"]);
        let list_plan = ScalarRepresentationPlan::for_function_ir(&function(
            "list_bool_index",
            &["items", "idx"],
            Some(vec!["list[bool]", "int"]),
            vec![list_bool_index.clone()],
        ));
        assert!(list_plan.op_produces_raw_bool_for_bool_primary(
            &list_bool_index,
            &candidates,
            &int_primary,
        ));

        let boxed_index = op("index", Some("item"), None, &["items", "boxed_idx"]);
        assert!(!list_plan.op_produces_raw_bool_for_bool_primary(
            &boxed_index,
            &candidates,
            &int_primary,
        ));

        let mut transport_index = op("index", Some("item"), None, &["items", "idx"]);
        transport_index.container_type = Some("list".to_string());
        assert!(!empty_plan.op_produces_raw_bool_for_bool_primary(
            &transport_index,
            &candidates,
            &int_primary,
        ));

        let generic_index = op("index", Some("item"), None, &["items", "idx"]);
        assert!(!empty_plan.op_produces_raw_bool_for_bool_primary(
            &generic_index,
            &candidates,
            &int_primary,
        ));

        let legacy_is_truthy = op("is_truthy", Some("truthy"), None, &["flag"]);
        assert!(!empty_plan.op_produces_raw_bool_for_bool_primary(
            &legacy_is_truthy,
            &candidates,
            &int_primary,
        ));
    }

    #[test]
    fn scalar_lane_does_not_classify_unbounded_int_pow_as_inline_int() {
        let pow = op("pow", Some("powv"), None, &["base", "exp"]);
        let func = function(
            "int_pow",
            &["base", "exp"],
            Some(vec!["int", "int"]),
            vec![pow.clone()],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.op_scalar_lane(&pow), None);
    }

    #[test]
    fn transport_hints_do_not_prove_scalar_representation() {
        let mut add = op("add", Some("sum"), None, &["lhs", "rhs"]);
        add.fast_int = Some(true);
        add.fast_float = Some(true);
        add.type_hint = Some("int".to_string());
        let func = function("hinted_add", &["lhs", "rhs"], None, vec![add.clone()]);

        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.op_scalar_lane(&add), None);
        assert!(!plan.op_prefers_integer_runtime_lane(&add));
        assert!(!plan.op_args_are_integer_family(&add));
    }

    #[test]
    fn typed_operands_prove_integer_runtime_lane_without_transport_hints() {
        let add = op("add", Some("sum"), None, &["lhs", "rhs"]);
        let mul = op("mul", Some("product"), None, &["lhs", "rhs"]);
        let func = function(
            "typed_add",
            &["lhs", "rhs"],
            Some(vec!["int", "int"]),
            vec![add.clone(), mul.clone()],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert!(plan.op_prefers_integer_runtime_lane(&add));
        assert!(plan.op_prefers_integer_runtime_lane(&mul));
        assert!(plan.op_args_are_integer_family(&add));
        assert!(plan.op_args_are_integer_family(&mul));
    }

    #[test]
    fn list_repeat_does_not_take_integer_runtime_lane() {
        let list_new = op("list_new", Some("items"), None, &["item"]);
        let repeat = op("mul", Some("repeated"), None, &["items", "count"]);
        let func = function(
            "list_repeat",
            &["item", "count"],
            Some(vec!["bool", "int"]),
            vec![list_new, repeat.clone()],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.name_scalar_kind("items"), None);
        assert!(!plan.op_prefers_integer_runtime_lane(&repeat));
        assert!(!plan.op_args_are_integer_family(&repeat));
    }

    #[test]
    fn scalar_lane_keeps_float_pow_on_float_lane() {
        let pow = op("pow", Some("powv"), None, &["base", "exp"]);
        let func = function(
            "float_pow",
            &["base", "exp"],
            Some(vec!["float", "float"]),
            vec![pow.clone()],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(plan.op_scalar_lane(&pow), Some(ScalarKind::Float));
    }

    #[test]
    fn scalar_store_targets_are_plan_owned_and_all_sources() {
        let func = function(
            "scalar_store_targets",
            &["callable", "args"],
            None,
            vec![
                const_int("i_seed", 7),
                op("copy_var", Some("i_copy"), None, &["i_seed"]),
                op("store_var", None, Some("i_slot"), &["i_copy"]),
                const_float("f_seed", 1.25),
                op("copy_var", Some("f_copy"), Some("f_seed"), &[]),
                op("store_var", None, Some("f_slot"), &["f_copy"]),
                const_bool("b_seed", true),
                op("identity_alias", Some("b_copy"), None, &["b_seed"]),
                op("store_var", None, Some("b_slot"), &["b_copy"]),
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("s_seed".to_string()),
                    s_value: Some("lane".to_string()),
                    ..OpIR::default()
                },
                op("copy", Some("s_copy"), None, &["s_seed"]),
                op("store_var", None, Some("s_slot"), &["s_copy"]),
                op("store_var", None, Some("mixed_slot"), &["i_seed"]),
                op("store_var", None, Some("mixed_slot"), &["f_seed"]),
                op(
                    "call_indirect",
                    Some("dynamic"),
                    None,
                    &["callable", "args"],
                ),
                op("store_var", None, Some("dynamic_slot"), &["dynamic"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);

        assert_eq!(
            plan.scalar_store_targets(ScalarKind::Int),
            BTreeSet::from(["i_slot".to_string()]),
        );
        assert_eq!(
            plan.scalar_store_targets(ScalarKind::Float),
            BTreeSet::from(["f_slot".to_string()]),
        );
        assert_eq!(
            plan.scalar_store_targets(ScalarKind::Bool),
            BTreeSet::from(["b_slot".to_string()]),
        );
        assert_eq!(
            plan.scalar_store_targets(ScalarKind::Str),
            BTreeSet::from(["s_slot".to_string()]),
        );
    }

    #[test]
    fn raw_loop_iv_copy_used_by_object_ops_stays_primary_until_escape() {
        let func = function(
            "raw_loop_iv_copy_used_by_object_ops",
            &[],
            None,
            vec![
                op("missing", Some("missing_i"), None, &[]),
                op("store_var", None, Some("i"), &["missing_i"]),
                op("copy_var", Some("missing_copy"), None, &["missing_i"]),
                const_int("stop", 3),
                const_int("zero", 0),
                const_int("one", 1),
                op("copy_var", Some("zero_copy"), None, &["zero"]),
                op("store_var", None, Some("_bb1_arg0"), &["zero_copy"]),
                op("store_var", None, Some("_bb1_arg1"), &["missing_copy"]),
                op("loop_start", None, None, &[]),
                op("load_var", Some("iv"), Some("_bb1_arg0"), &[]),
                op("load_var", Some("carried_obj"), Some("_bb1_arg1"), &[]),
                op("lt", Some("cond"), None, &["iv", "stop"]),
                op("loop_break_if_false", None, None, &["cond"]),
                op("store_var", None, Some("i"), &["iv"]),
                op("copy_var", Some("escaped_iv"), None, &["iv"]),
                op("check_exception", None, None, &[]),
                op("type_of", Some("ty"), None, &["escaped_iv"]),
                op("check_exception", None, None, &[]),
                op("str_from_obj", Some("text"), None, &["escaped_iv"]),
                op(
                    "exception_new_builtin_one",
                    Some("exc"),
                    None,
                    &["escaped_iv"],
                ),
                op("add", Some("next"), None, &["iv", "one"]),
                op("store_var", None, Some("iv"), &["next"]),
                op("copy_var", Some("next_copy"), None, &["next"]),
                op("store_var", None, Some("_bb1_arg0"), &["next_copy"]),
                op("store_var", None, Some("_bb1_arg1"), &["escaped_iv"]),
                op("loop_continue", None, None, &[]),
                op("loop_end", None, None, &[]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let int_primary = plan.primary_name_sets().int;

        for name in ["_bb1_arg0", "iv", "escaped_iv", "next", "next_copy"] {
            assert!(
                int_primary.contains(name),
                "{name} must stay int-primary until boxed escape; got {int_primary:?}"
            );
        }
    }

    #[test]
    fn float_primary_scope_excludes_pow_without_disabling_unrelated_float_defs() {
        let func = function(
            "float_primary_pow_scope",
            &["p"],
            Some(vec!["float"]),
            vec![
                const_float("base", 2.0),
                const_float("exp", 3.0),
                op("pow", Some("pow_result"), None, &["base", "exp"]),
                op("add", Some("sum"), None, &["base", "exp"]),
                op("copy_var", Some("sum_copy"), Some("sum"), &[]),
                op("copy_var", Some("param_copy"), Some("p"), &[]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let primary = plan.primary_name_sets();

        assert!(primary.float.contains("base"));
        assert!(primary.float.contains("exp"));
        assert!(primary.float.contains("sum"));
        assert!(primary.float.contains("sum_copy"));
        assert!(!primary.float.contains("pow_result"));
        assert!(!primary.float.contains("p"));
        assert!(primary.float.contains("param_copy"));
        assert!(!plan.is_float_unboxed("pow_result"));
    }

    #[test]
    fn float_primary_store_targets_require_all_sources() {
        let func = function(
            "float_primary_store_sources",
            &[],
            None,
            vec![
                const_float("f_seed", 1.5),
                op("store_var", None, Some("float_slot"), &["f_seed"]),
                const_int("i_seed", 2),
                op("store_var", None, Some("mixed_slot"), &["f_seed"]),
                op("store_var", None, Some("mixed_slot"), &["i_seed"]),
            ],
        );

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(primary.float.contains("f_seed"));
        assert!(primary.float.contains("float_slot"));
        assert!(!primary.float.contains("mixed_slot"));
    }

    #[test]
    fn scalar_primary_excludes_missing_sentinel_store_sources() {
        let func = function(
            "scalar_primary_missing_sentinel_sources",
            &[],
            None,
            vec![
                const_int("i_seed", 7),
                op("store_var", None, Some("int_slot"), &["i_seed"]),
                op("store_var", None, Some("maybe_int_slot"), &["i_seed"]),
                const_bool("b_seed", true),
                op("store_var", None, Some("bool_slot"), &["b_seed"]),
                op("store_var", None, Some("maybe_bool_slot"), &["b_seed"]),
                const_float("f_seed", 1.5),
                op("store_var", None, Some("float_slot"), &["f_seed"]),
                op("store_var", None, Some("maybe_float_slot"), &["f_seed"]),
                op("missing", Some("missing_value"), None, &[]),
                op(
                    "store_var",
                    None,
                    Some("maybe_int_slot"),
                    &["missing_value"],
                ),
                op(
                    "store_var",
                    None,
                    Some("maybe_bool_slot"),
                    &["missing_value"],
                ),
                op(
                    "store_var",
                    None,
                    Some("maybe_float_slot"),
                    &["missing_value"],
                ),
            ],
        );

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(primary.int.contains("int_slot"));
        assert!(primary.bool_.contains("bool_slot"));
        assert!(primary.float.contains("float_slot"));
        assert!(!primary.int.contains("missing_value"));
        assert!(!primary.bool_.contains("missing_value"));
        assert!(!primary.float.contains("missing_value"));
        assert!(!primary.int.contains("maybe_int_slot"));
        assert!(!primary.bool_.contains("maybe_bool_slot"));
        assert!(!primary.float.contains("maybe_float_slot"));
    }

    #[test]
    fn cold_module_chunk_functions_have_empty_primary_sets() {
        let func = function(
            "__molt_module_chunk_0",
            &[],
            None,
            vec![
                const_int("value", 1),
                const_bool("flag", true),
                op("list_new", Some("items"), None, &["value"]),
            ],
        );

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let primary = plan.primary_name_sets();

        assert!(primary.int.is_empty());
        assert!(primary.bool_.is_empty());
        assert!(primary.float.is_empty());
        assert_eq!(plan.name_scalar_kind("value"), None);
        assert_eq!(plan.name_scalar_kind("flag"), None);
        assert_eq!(plan.name_container_kind("items"), None);
    }

    // ======================================================================
    // Value-keyed RawI64Safe promotion via the value-range analysis (S6).
    //
    // These exercise the SOLE proof source for the WASM/LLVM backends:
    // `repr_by_value_for(.., Some(&value_range))`. They directly assert the
    // soundness invariant (no false RawI64Safe → no heap-BigInt truncation)
    // and the perf invariant (range-loop IVs stay RawI64Safe), and that WASM
    // and LLVM derive an identical map from the same `ValueRange` (single
    // source of truth — a divergence would re-create the native-vs-wasm
    // trusted-unbox bug, 2bf51b730).
    // ======================================================================

    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{
        AttrDict, AttrValue as TirAttrValue, Dialect, OpCode as TirOpCode, TirOp,
    };
    use crate::tir::types::TirType;
    use crate::tir::values::TirValue;

    fn tir_op(opcode: TirOpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }
    fn tir_op_nsw(opcode: TirOpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut o = tir_op(opcode, operands, results);
        o.attrs
            .insert("no_signed_wrap".into(), TirAttrValue::Bool(true));
        o
    }
    fn tir_cint(result: ValueId, value: i64) -> TirOp {
        let mut o = tir_op(TirOpCode::ConstInt, vec![], vec![result]);
        o.attrs.insert("value".into(), TirAttrValue::Int(value));
        o
    }

    /// Build the canonical post-range_devirt `for i in range(stop): i + 1`
    /// loop in TIR: a header block-arg IV with a `no_signed_wrap` increment,
    /// the shape SCEV recognises as an `AddRec` and value-range turns into a
    /// proven `[start, last]` range.
    fn range_loop_tir(start_v: i64, stop: i64) -> (TirFunction, ValueId, ValueId) {
        let mut func = TirFunction::new("rl".into(), vec![], TirType::None);
        let startc = func.fresh_value();
        let stopc = func.fresh_value();
        let stepc = func.fresh_value();
        let iv = func.fresh_value();
        let cond = func.fresh_value();
        let body_val = func.fresh_value();
        let one = func.fresh_value();
        let next = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                tir_cint(startc, start_v),
                tir_cint(stopc, stop),
                tir_cint(stepc, 1),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![startc],
            };
        }
        // Type every integer value as I64 (faithful to real lowered TIR, where
        // `type_refine` types every int) so the representation floor maps them to
        // `MaybeBigInt` rather than the unknown-type `DynBox`.
        for v in [startc, stopc, stepc, iv, body_val, one, next] {
            func.value_types.insert(v, TirType::I64);
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: iv,
                    ty: TirType::I64,
                }],
                ops: vec![tir_op(TirOpCode::Lt, vec![iv, stopc], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    tir_cint(one, 1),
                    tir_op(TirOpCode::Add, vec![iv, one], vec![body_val]),
                    tir_op_nsw(TirOpCode::Add, vec![iv, stepc], vec![next]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit, LoopRole::LoopEnd);
        (func, iv, next)
    }

    /// The overflow_peel'd loop's carrier cycle must admit into the native
    /// int-primary set — the slots, their loads, and the checked sums — while
    /// the bool flag lane, the exit-merge slot, and the boxed slow loop must
    /// all be refused. If the fast-lane names are missing the native arm
    /// silently takes the boxed lane (no speedup); if the refused names leak
    /// in, the trusted raw carrier meets boxed values (the 2^47 truncation
    /// miscompile class). Both directions are load-bearing.
    #[test]
    fn checked_loop_seed_admits_peeled_fast_loop_only() {
        let func_ir = super::test_fixtures::peeled_compute_func_ir();
        let plan = ScalarRepresentationPlan::for_function_ir(&func_ir);
        let primary = plan.primary_name_sets();
        let int_primary = &primary.int;

        for name in [
            "_bb1_arg0",
            "_bb1_arg1",
            "_bb1_arg3",
            "_bb1_arg4", // fast slots
            "_v16",
            "_v17",
            "_v41",
            "_v42", // their loads
            "_v22",
            "_v25", // checked sums
        ] {
            assert!(
                int_primary.contains(name),
                "{name} must be int-primary (fast-lane admission); got {int_primary:?}"
            );
            assert!(
                primary.int_full_deopt.contains(name),
                "{name} must be full-deopt, not inline-safe; got {:?}",
                primary.int_full_deopt
            );
            assert!(
                !primary.int_inline_safe.contains(name),
                "{name} must not seed RawI64Safe; got {:?}",
                primary.int_inline_safe
            );
        }
        for name in [
            "_bb1_arg2",
            "_v40",
            "_v48", // overflow-flag lane (bool)
            "_bb5_arg0",
            "_v51", // exit merge (fed by the boxed slow loop)
            "_bb7_arg0",
            "_bb7_arg1",
            "_v29",
            "_v30",
            "v114",
            "v118", // slow loop
        ] {
            assert!(
                !int_primary.contains(name),
                "{name} must NOT be int-primary (boxed lane); got {int_primary:?}"
            );
        }

        // The overflow-flag chain must admit into the RAW BOOL lane — without
        // it the break condition costs ~4 runtime calls per iteration
        // (inc_ref + is_truthy + not + or-select) and the peel's fast loop
        // loses its win.
        let bool_primary = plan.primary_name_sets().bool_;
        for name in [
            "_v46",
            "_v47",      // checked_add overflow flags
            "_v48",      // or fan-in
            "_v40",      // of-slot load
            "_v44",      // not(of)
            "_v45",      // and(cond, not_of) — the break condition
            "v111",      // the guard compare
            "_bb1_arg2", // the carried of slot
        ] {
            assert!(
                bool_primary.contains(name),
                "{name} must be bool-primary (raw flag lane); got {bool_primary:?}"
            );
        }
    }

    fn is_inline_safe(map: &HashMap<ValueId, Repr>, id: ValueId) -> bool {
        map.get(&id) == Some(&Repr::RawI64Safe)
    }

    fn is_full_deopt(map: &HashMap<ValueId, Repr>, id: ValueId) -> bool {
        map.get(&id) == Some(&Repr::RawI64FullDeopt)
    }

    fn is_raw_carrier(map: &HashMap<ValueId, Repr>, id: ValueId) -> bool {
        map.get(&id).is_some_and(|repr| repr.is_raw_i64_carrier())
    }

    /// PERF + SOUNDNESS: a bounded `for i in range(10)` induction variable is
    /// proven `RawI64Safe` (so the loop keeps the bare-i64 lane and beats
    /// CPython), AND that proof flows to its `no_signed_wrap` back-edge update.
    #[test]
    fn range_loop_iv_is_raw_i64_safe_from_value_range() {
        let (func, iv, next) = range_loop_tir(0, 10);
        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert!(
            is_inline_safe(&repr, iv),
            "range(10) IV must be RawI64Safe (range [0,9] ⊂ inline-int47)"
        );
        assert!(
            is_inline_safe(&repr, next),
            "the no_signed_wrap IV update must inherit RawI64Safe (propagated phi)"
        );
    }

    /// SOUNDNESS (the 2bf51b760 truncation bug-class): an induction variable
    /// whose proven range exceeds 2^46 must NOT be RawI64Safe — it could be a
    /// heap BigInt, so it stays `MaybeBigInt` and uses the boxed path. This is
    /// the `apply(1<<60, 7) == 1152921504606846983` invariant expressed at the
    /// representation boundary: a > 2^46 value is never trusted-unboxed.
    #[test]
    fn above_inline_int47_iv_is_not_raw_i64_safe() {
        // start at 2^46 so even iteration 0 is at the inline-int47 ceiling and
        // the very next value (2^46) is outside the window.
        let huge_start = 1i64 << 46;
        let (func, iv, _next) = range_loop_tir(huge_start, huge_start + 10);
        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert!(
            !is_inline_safe(&repr, iv),
            "an IV reaching/exceeding 2^46 must stay MaybeBigInt (no trusted unbox of a possible heap BigInt)"
        );
        assert_eq!(
            repr.get(&iv),
            Some(&Repr::MaybeBigInt),
            "the unproven int floors to the boxed BigInt-safe carrier"
        );
    }

    /// SOUNDNESS: with NO value-range supplied (`None`), nothing is promoted —
    /// every int floors to `MaybeBigInt`. This is the conservative pre-TIR /
    /// unanalysed path that can never miscompile.
    #[test]
    fn no_value_range_leaves_everything_maybe_bigint() {
        let (func, iv, next) = range_loop_tir(0, 10);
        let repr = repr_by_value_for(&func, None);
        assert_eq!(repr.get(&iv), Some(&Repr::MaybeBigInt));
        assert_eq!(repr.get(&next), Some(&Repr::MaybeBigInt));
        assert!(
            repr.values().all(|r| !r.is_raw_i64_safe()),
            "None means no RawI64Safe raise anywhere"
        );
    }

    #[test]
    fn bool_select_range_proof_does_not_promote_to_raw_i64() {
        let mut func = TirFunction::new(
            "bool_select".into(),
            vec![TirType::Bool, TirType::Bool],
            TirType::Bool,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(tir_op(
            TirOpCode::And,
            vec![ValueId(0), ValueId(1)],
            vec![result],
        ));
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        crate::tir::type_refine::refine_types(&mut func);

        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert_eq!(
            repr.get(&result),
            Some(&Repr::Bool),
            "bool values can have [0,1] ranges but must stay in the Bool carrier, not RawI64Safe"
        );
    }

    /// SOUNDNESS: an unbounded accumulator (`total = total + i`, a degree-2
    /// recurrence) is classified `Unknown` by SCEV → no value-range proof →
    /// stays `MaybeBigInt`. This is the loop-IV OOM hazard the strict-subset
    /// property guards against: a wrapping/unbounded accumulator must never be
    /// carried as a raw i64.
    #[test]
    fn unbounded_accumulator_stays_maybe_bigint() {
        // for i in range(10): total = total + i  — `total` is a 2nd phi whose
        // step is the IV itself (not a constant), so it has no proven range.
        let mut func = TirFunction::new("acc".into(), vec![], TirType::None);
        let startc = func.fresh_value();
        let stopc = func.fresh_value();
        let stepc = func.fresh_value();
        let total0 = func.fresh_value();
        let iv = func.fresh_value();
        let total = func.fresh_value();
        let cond = func.fresh_value();
        let total_next = func.fresh_value();
        let next = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                tir_cint(startc, 0),
                tir_cint(stopc, 10),
                tir_cint(stepc, 1),
                tir_cint(total0, 0),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![startc, total0],
            };
        }
        func.value_types.insert(iv, TirType::I64);
        func.value_types.insert(total, TirType::I64);
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![
                    TirValue {
                        id: iv,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: total,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![tir_op(TirOpCode::Lt, vec![iv, stopc], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    tir_op(TirOpCode::Add, vec![total, iv], vec![total_next]),
                    tir_op_nsw(TirOpCode::Add, vec![iv, stepc], vec![next]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next, total_next],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit, LoopRole::LoopEnd);

        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        // The counted IV is fine; the unbounded accumulator must NOT be raw.
        assert!(
            is_inline_safe(&repr, iv),
            "the counted IV is still proven inline-safe"
        );
        assert!(
            !is_raw_carrier(&repr, total),
            "the unbounded accumulator phi must stay MaybeBigInt (degree-2 recurrence → Unknown range)"
        );
        assert!(
            !is_raw_carrier(&repr, total_next),
            "the accumulator update must stay MaybeBigInt"
        );
    }

    /// PERF: GPU thread/block-id intrinsics are pre-seeded RawI64Safe even
    /// though the value-range analysis has no model for them — their results
    /// are hardware lane indices, structurally bounded. Without this seed a GPU
    /// kernel's index arithmetic would regress to the boxed runtime path.
    #[test]
    fn gpu_index_intrinsics_are_pre_seeded_raw_i64_safe() {
        let mut func = TirFunction::new("k".into(), vec![], TirType::None);
        let tid = func.fresh_value();
        func.value_types.insert(tid, TirType::I64);
        let mut call = tir_op(TirOpCode::Call, vec![], vec![tid]);
        call.attrs.insert(
            "s_value".into(),
            TirAttrValue::Str("molt_gpu_thread_id".into()),
        );
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![call];
            entry.terminator = Terminator::Return { values: vec![tid] };
        }
        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert!(
            is_inline_safe(&repr, tid),
            "molt_gpu_thread_id result must be pre-seeded RawI64Safe"
        );

        // A non-GPU runtime call result is NOT pre-seeded — only the bounded
        // GPU index intrinsics are.
        let mut func2 = TirFunction::new("k2".into(), vec![], TirType::None);
        let r = func2.fresh_value();
        func2.value_types.insert(r, TirType::I64);
        let mut other = tir_op(TirOpCode::Call, vec![], vec![r]);
        other.attrs.insert(
            "s_value".into(),
            TirAttrValue::Str("molt_some_runtime".into()),
        );
        {
            let entry = func2.blocks.get_mut(&func2.entry_block).unwrap();
            entry.ops = vec![other];
            entry.terminator = Terminator::Return { values: vec![r] };
        }
        let vr2 = value_range_for(&func2);
        let repr2 = repr_by_value_for(&func2, Some(&vr2));
        assert!(
            !is_raw_carrier(&repr2, r),
            "an arbitrary runtime-call result must NOT be pre-seeded raw (only bounded GPU index intrinsics are)"
        );
    }

    /// Build the live frontend-peeled accumulator shape: a CheckedAdd loop
    /// whose header phi is fed by (a) a proven `ConstInt 0` init, (b) the
    /// CheckedAdd wrapping sum (full-range raw seed), and (c) a vestigial
    /// `LoopEnd` block passing a fabricated `ConstNone` — exactly the edge the
    /// SSA lift keeps as loop metadata. `reachable_vestige` controls whether
    /// that block is wired into the executable CFG or left detached.
    fn checked_loop_with_none_vestige(reachable_vestige: bool) -> (TirFunction, ValueId, ValueId) {
        let mut func = TirFunction::new("cl".into(), vec![], TirType::None);
        let init = func.fresh_value();
        let acc = func.fresh_value();
        let cond = func.fresh_value();
        let step = func.fresh_value();
        let sum = func.fresh_value();
        let of = func.fresh_value();
        let none_v = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let vestige = func.fresh_block();

        for v in [init, acc, step, sum] {
            func.value_types.insert(v, TirType::I64);
        }
        func.value_types.insert(of, TirType::Bool);
        func.value_types.insert(none_v, TirType::None);

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![tir_cint(init, 0)];
            entry.terminator = if reachable_vestige {
                // Wire the vestige into the executable CFG: its None arg can
                // now genuinely flow, so it MUST poison the phi.
                Terminator::CondBranch {
                    cond: init,
                    then_block: header,
                    then_args: vec![init],
                    else_block: vestige,
                    else_args: vec![],
                }
            } else {
                Terminator::Branch {
                    target: header,
                    args: vec![init],
                }
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: acc,
                    ty: TirType::I64,
                }],
                ops: vec![tir_op(TirOpCode::Lt, vec![acc, init], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    tir_cint(step, -20_000_000),
                    tir_op(TirOpCode::CheckedAdd, vec![acc, step], vec![sum, of]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![sum],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        // The vestigial loop-end: materializes a None and re-enters the header
        // with it. In the live lift this block has NO executable predecessor —
        // it survives purely as loop metadata.
        func.blocks.insert(
            vestige,
            TirBlock {
                id: vestige,
                args: vec![],
                ops: vec![tir_op(TirOpCode::ConstNone, vec![], vec![none_v])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![none_v],
                },
            },
        );
        func.loop_roles.insert(vestige, LoopRole::LoopEnd);
        (func, acc, sum)
    }

    /// PERF (the boxed-lane OOM class): the vestigial UNREACHABLE
    /// `loop_end → header` edge passing a fabricated `ConstNone` must NOT
    /// poison the all-incomings phi rule — dead edges deliver no values
    /// (standard SCCP phi semantics). Without dead-edge insensitivity every
    /// frontend-peeled accumulator demotes to the boxed `molt_add` lane on the
    /// value-keyed backends: 30M-iteration loops then leak a boxed int per
    /// iteration (observed: 2.1GB RSS → OOM kill on `sum_negative` @ llvm).
    #[test]
    fn unreachable_none_vestige_does_not_poison_checked_loop_phi() {
        let (func, acc, sum) = checked_loop_with_none_vestige(false);
        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert!(
            is_full_deopt(&repr, sum),
            "the CheckedAdd wrapping sum is the unconditional full-range seed"
        );
        assert!(
            is_full_deopt(&repr, acc),
            "the header phi must be raised: its only REACHABLE incomings are the \
             proven ConstInt init and the CheckedAdd sum; the unreachable \
             ConstNone vestige delivers no value"
        );
    }

    /// SOUNDNESS (the dual of the above): the SAME None-passing edge, made
    /// executable, MUST poison the phi — a `None` can genuinely flow, and a
    /// raw-i64 carrier fed a NaN-boxed None is the trusted-unbox miscompile
    /// class. Reachability is the load-bearing distinction.
    #[test]
    fn reachable_none_edge_still_poisons_checked_loop_phi() {
        let (func, acc, _sum) = checked_loop_with_none_vestige(true);
        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert!(
            !is_raw_carrier(&repr, acc),
            "a REACHABLE None incoming must keep the phi boxed (MaybeBigInt floor)"
        );
    }

    /// SOUNDNESS (native/WASM variable-keyed phi invariant): a loop-header phi
    /// cannot be carried as raw i64 unless every reachable incoming uses the raw
    /// carrier. A single reachable heap/DynBox incoming must force the phi to the
    /// boxed lane, even when the ordinary entry and back-edge values are raw.
    #[test]
    fn reachable_heap_incoming_poisons_raw_loop_phi() {
        let mut func = TirFunction::new("mixed_phi".into(), vec![], TirType::None);
        let init = func.fresh_value();
        let acc = func.fresh_value();
        let cond = func.fresh_value();
        let step = func.fresh_value();
        let sum = func.fresh_value();
        let overflow = func.fresh_value();
        let heap_value = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let heap_pred = func.fresh_block();
        let exit = func.fresh_block();

        for v in [init, acc, step, sum] {
            func.value_types.insert(v, TirType::I64);
        }
        func.value_types.insert(cond, TirType::Bool);
        func.value_types.insert(overflow, TirType::Bool);
        func.value_types.insert(heap_value, TirType::DynBox);

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![tir_cint(init, 0)];
            entry.terminator = Terminator::CondBranch {
                cond: init,
                then_block: header,
                then_args: vec![init],
                else_block: heap_pred,
                else_args: vec![],
            };
        }
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: acc,
                    ty: TirType::I64,
                }],
                ops: vec![tir_op(TirOpCode::Lt, vec![acc, init], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![
                    tir_cint(step, 1),
                    tir_op(TirOpCode::CheckedAdd, vec![acc, step], vec![sum, overflow]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![sum],
                },
            },
        );
        func.blocks.insert(
            heap_pred,
            TirBlock {
                id: heap_pred,
                args: vec![],
                ops: vec![tir_op(TirOpCode::Call, vec![], vec![heap_value])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![heap_value],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let vr = value_range_for(&func);
        let repr = repr_by_value_for(&func, Some(&vr));
        assert!(
            is_full_deopt(&repr, sum),
            "CheckedAdd's wrapping sum remains a valid raw carrier"
        );
        assert!(
            !is_raw_carrier(&repr, heap_value),
            "the heap incoming itself must not be raw"
        );
        assert_eq!(
            repr.get(&acc),
            Some(&Repr::MaybeBigInt),
            "a reachable heap incoming must keep the loop phi boxed; otherwise \
             native/WASM variable-keyed phis can receive raw and heap carriers"
        );
    }

    /// CROSS-BACKEND SINGLE SOURCE OF TRUTH: the WASM path (`repr_by_value_for`)
    /// and the LLVM path (`LlvmReprFacts::build` → same `repr_by_value_for` with
    /// the same `ValueRange`) derive the IDENTICAL `Repr` per `ValueId`. A
    /// divergence here is the native-vs-wasm trusted-unbox bug; this test is the
    /// firewall against it.
    #[test]
    #[cfg(feature = "llvm")]
    fn wasm_and_llvm_derive_identical_repr_from_one_value_range() {
        let (func, _iv, _next) = range_loop_tir(0, 10);
        let vr = value_range_for(&func);
        let wasm_map = repr_by_value_for(&func, Some(&vr));
        let llvm_facts = LlvmReprFacts::build(&func);
        assert_eq!(
            wasm_map, llvm_facts.repr_by_value,
            "WASM and LLVM must derive the same Repr per ValueId from the same ValueRange"
        );
    }
}
