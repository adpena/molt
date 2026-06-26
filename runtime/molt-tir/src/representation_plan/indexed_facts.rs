use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use crate::ir::{FunctionIR, OpIR};
use crate::repr::{ContainerStorageFact, ContainerStorageKind, ScalarKind};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::{ScalarRepresentationFact, ScalarRepresentationPlan};

const PLAN_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PLAN_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone)]
pub(super) struct PlanHasher(u64);

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

pub(super) type PlanBuildHasher = BuildHasherDefault<PlanHasher>;
pub(super) type PlanHashMap<K, V> = HashMap<K, V, PlanBuildHasher>;
pub(super) type PlanHashSet<T> = HashSet<T, PlanBuildHasher>;

pub(super) fn plan_hash_map<K, V>(capacity: usize) -> PlanHashMap<K, V> {
    HashMap::with_capacity_and_hasher(capacity, PlanBuildHasher::default())
}

pub(super) fn plan_hash_set<T>(capacity: usize) -> PlanHashSet<T> {
    HashSet::with_capacity_and_hasher(capacity, PlanBuildHasher::default())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NameId(usize);

impl NameId {
    fn slot(self) -> usize {
        self.0
    }
}

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

    pub(super) fn get(&self, name: &str) -> Option<NameId> {
        self.ids_by_name.get(name).copied()
    }

    pub(super) fn len(&self) -> usize {
        self.names.len()
    }
}

struct NameMarkSet {
    marks: Vec<u32>,
    epoch: u32,
}

impl NameMarkSet {
    pub(super) fn new(name_count: usize) -> Self {
        Self {
            marks: vec![0; name_count],
            epoch: 1,
        }
    }

    pub(super) fn clear(&mut self) {
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
    pub(super) fn contains(&self, id: NameId) -> bool {
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

    pub(super) fn insert(&mut self, id: NameId) -> bool {
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

    pub(super) fn iter(&self) -> impl Iterator<Item = NameId> + '_ {
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

pub(super) struct FunctionFactIndex<'a> {
    stores: Vec<StoreVarEdge<'a>>,
    aliases: Vec<AliasEdge<'a>>,
    pub(super) data_ops: Vec<&'a OpIR>,
    pub(super) store_index_ops: Vec<&'a OpIR>,
    pub(super) sentinel_outputs: PlanHashSet<String>,
    pub(super) delete_targets: PlanHashSet<String>,
}

impl<'a> FunctionFactIndex<'a> {
    pub(super) fn for_function(func_ir: &'a FunctionIR) -> Self {
        let mut stores = Vec::with_capacity(func_ir.ops.len() / 4 + 1);
        let mut aliases = Vec::with_capacity(func_ir.ops.len() / 4 + 1);
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
            data_ops,
            store_index_ops,
            sentinel_outputs,
            delete_targets,
        }
    }

    pub(super) fn has_scalar_alias_or_store_edges(&self) -> bool {
        !self.stores.is_empty() || !self.aliases.is_empty()
    }

    pub(super) fn has_container_storage_edges(&self) -> bool {
        !self.stores.is_empty() || !self.aliases.is_empty() || !self.store_index_ops.is_empty()
    }

    pub(super) fn needs_indexed_name_graph(&self) -> bool {
        self.has_scalar_alias_or_store_edges() || self.has_container_storage_edges()
    }

    pub(super) fn store_count(&self) -> usize {
        self.stores.len()
    }

    pub(super) fn store_edges(&self) -> impl Iterator<Item = (&'a str, Option<&'a str>)> + '_ {
        self.stores.iter().map(|edge| (edge.target, edge.source))
    }

    pub(super) fn alias_edges(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        self.aliases.iter().map(|edge| (edge.out, edge.source))
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

pub(super) struct IndexedFunctionFactIndex<'a> {
    names: FunctionNameIndex<'a>,
    stores: Vec<IndexedStoreVarEdge>,
    alias_groups: Vec<IndexedAliasGroup>,
    alias_sources: Vec<bool>,
    alias_outputs: Vec<bool>,
}

impl<'a> IndexedFunctionFactIndex<'a> {
    pub(super) fn for_function_facts(fact_index: &FunctionFactIndex<'a>) -> Self {
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

    pub(super) fn target_is_none(&self, target: NameId) -> bool {
        self.none_targets.contains(target)
    }

    pub(super) fn target_is_pending(&self, target: NameId) -> bool {
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

    pub(super) fn merge(
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

    pub(super) fn finish_into(&mut self, facts: &mut IndexedStoreTargetFacts) {
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
    pub(super) fn from_plan(
        plan: &ScalarRepresentationPlan,
        index: &IndexedFunctionFactIndex<'_>,
    ) -> Self {
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

    pub(super) fn fact_id(&self, id: NameId) -> Option<ScalarFactId> {
        self.fact_ids_by_name.get(id.0).and_then(|fact| *fact)
    }

    pub(super) fn fact(&self, id: ScalarFactId) -> &ScalarRepresentationFact {
        &self.facts[id.0]
    }

    pub(super) fn conflict(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.weak[id.0] = false;
        self.fact_ids_by_name[id.0] = None;
        self.conflicted[id.0] = true;
        true
    }

    pub(super) fn clear_fact(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.weak[id.0] = false;
        self.fact_ids_by_name[id.0].take().is_some()
    }

    pub(super) fn insert_graph_fact_id(&mut self, id: NameId, fact_id: ScalarFactId) -> bool {
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

    pub(super) fn sync_to_plan(
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
    pub(super) fn from_plan(
        plan: &ScalarRepresentationPlan,
        index: &IndexedFunctionFactIndex<'_>,
    ) -> Self {
        let mut facts = Vec::with_capacity(index.names.len());
        let mut conflicted = Vec::with_capacity(index.names.len());
        for name in &index.names.names {
            facts.push(plan.container_storage_by_name.get(*name).cloned());
            conflicted.push(plan.container_storage_conflicted_names.contains(*name));
        }
        Self { facts, conflicted }
    }

    pub(super) fn get(&self, id: NameId) -> Option<&ContainerStorageFact> {
        self.facts.get(id.0).and_then(Option::as_ref)
    }

    pub(super) fn contains(&self, id: NameId) -> bool {
        self.get(id).is_some()
    }

    pub(super) fn kind(&self, id: NameId) -> Option<ContainerStorageKind> {
        self.get(id).map(|fact| fact.kind)
    }

    pub(super) fn conflict(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.facts[id.0] = None;
        self.conflicted[id.0] = true;
        true
    }

    pub(super) fn clear_fact(&mut self, id: NameId) -> bool {
        if self.conflicted[id.0] {
            return false;
        }
        self.facts[id.0].take().is_some()
    }

    pub(super) fn insert(&mut self, id: NameId, fact: ContainerStorageFact) -> bool {
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

    pub(super) fn sync_to_plan(
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

trait IndexedFactDomain {
    type Fact: Clone + PartialEq;

    fn has_fact(&self, id: NameId) -> bool;
    fn is_conflicted(&self, id: NameId) -> bool;
    fn fact_for_name(&self, id: NameId) -> Option<Self::Fact>;
    fn conflict(&mut self, id: NameId) -> bool;
    fn clear_fact(&mut self, id: NameId) -> bool;
    fn insert_fact(&mut self, id: NameId, fact: Self::Fact) -> bool;

    fn names_have_same_fact(&self, lhs: NameId, rhs: NameId) -> bool {
        self.fact_for_name(lhs) == self.fact_for_name(rhs)
    }
}

impl IndexedFactDomain for IndexedScalarFacts {
    type Fact = ScalarFactId;

    fn has_fact(&self, id: NameId) -> bool {
        self.fact_id(id).is_some()
    }

    fn is_conflicted(&self, id: NameId) -> bool {
        self.conflicted[id.slot()]
    }

    fn fact_for_name(&self, id: NameId) -> Option<Self::Fact> {
        self.fact_id(id)
    }

    fn conflict(&mut self, id: NameId) -> bool {
        IndexedScalarFacts::conflict(self, id)
    }

    fn clear_fact(&mut self, id: NameId) -> bool {
        IndexedScalarFacts::clear_fact(self, id)
    }

    fn insert_fact(&mut self, id: NameId, fact: Self::Fact) -> bool {
        self.insert_graph_fact_id(id, fact)
    }
}

impl IndexedFactDomain for IndexedContainerFacts {
    type Fact = ContainerStorageFact;

    fn has_fact(&self, id: NameId) -> bool {
        self.contains(id)
    }

    fn is_conflicted(&self, id: NameId) -> bool {
        self.conflicted[id.slot()]
    }

    fn fact_for_name(&self, id: NameId) -> Option<Self::Fact> {
        self.get(id).cloned()
    }

    fn conflict(&mut self, id: NameId) -> bool {
        IndexedContainerFacts::conflict(self, id)
    }

    fn clear_fact(&mut self, id: NameId) -> bool {
        IndexedContainerFacts::clear_fact(self, id)
    }

    fn insert_fact(&mut self, id: NameId, fact: Self::Fact) -> bool {
        self.insert(id, fact)
    }
}

impl ScalarRepresentationPlan {
    pub(super) fn propagate_simple_aliases(&mut self, index: &IndexedFunctionFactIndex<'_>) {
        let mut facts = IndexedScalarFacts::from_plan(self, index);
        propagate_indexed_fact_graph(index, &mut facts, |_| false);
        facts.sync_to_plan(self, index);
    }

    pub(super) fn propagate_container_storage(
        &mut self,
        fact_index: &FunctionFactIndex<'_>,
        index: &IndexedFunctionFactIndex<'_>,
    ) {
        let mut facts = IndexedContainerFacts::from_plan(self, index);
        propagate_indexed_fact_graph(index, &mut facts, |facts| {
            self.propagate_indexed_container_storage_store_index_ops(fact_index, index, facts)
        });
        facts.sync_to_plan(self, index);
    }

    fn propagate_indexed_container_storage_store_index_ops(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        index: &IndexedFunctionFactIndex<'_>,
        facts: &mut IndexedContainerFacts,
    ) -> bool {
        let mut changed = false;
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
        changed
    }
}

fn propagate_indexed_fact_graph<D>(
    index: &IndexedFunctionFactIndex<'_>,
    facts: &mut D,
    mut extra_transfer: impl FnMut(&mut D) -> bool,
) where
    D: IndexedFactDomain,
{
    let mut store_target_scratch =
        IndexedStoreTargetScratch::new(index.names.len(), index.stores.len());
    let mut store_target_facts =
        IndexedStoreTargetFacts::new(index.names.len(), index.stores.len());
    let mut blocked = NameWorkSet::new(index.names.len());
    let mut changed = true;
    while changed {
        changed = false;
        blocked.clear();
        indexed_store_target_facts(
            index,
            facts,
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
        collect_pending_alias_outputs(&store_target_facts, index, &mut blocked);
        for target in blocked.iter() {
            changed |= facts.clear_fact(target);
        }
        changed |= propagate_indexed_store_targets(facts, &store_target_facts, &blocked);
        changed |= propagate_indexed_alias_groups(facts, &store_target_facts, index, &blocked);
        changed |= extra_transfer(facts);
    }
}

fn collect_pending_alias_outputs(
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

fn propagate_indexed_alias_groups<D>(
    facts: &mut D,
    store_target_facts: &IndexedStoreTargetFacts,
    index: &IndexedFunctionFactIndex<'_>,
    blocked: &NameWorkSet,
) -> bool
where
    D: IndexedFactDomain,
{
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
        let mut joined: Option<D::Fact> = None;
        let mut unknown_or_conflict = false;
        let mut pending = false;
        for source in &group.sources {
            if store_target_facts.target_is_none(*source) || facts.is_conflicted(*source) {
                unknown_or_conflict = true;
                break;
            }
            if store_target_facts.target_is_pending(*source) {
                pending = true;
                break;
            }
            let Some(fact) = facts.fact_for_name(*source) else {
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
            changed |= facts.insert_fact(group.out, fact);
        }
    }
    changed
}

fn indexed_store_target_facts<D>(
    index: &IndexedFunctionFactIndex<'_>,
    facts: &D,
    scratch: &mut IndexedStoreTargetScratch,
    out: &mut IndexedStoreTargetFacts,
) where
    D: IndexedFactDomain,
{
    for edge in &index.stores {
        let source = match edge.source {
            Some(source) if facts.has_fact(source) => StoreTargetMergeSource::Proven(source),
            Some(_) => StoreTargetMergeSource::Pending,
            None => StoreTargetMergeSource::Missing,
        };
        let relevant = || {
            facts.has_fact(edge.target)
                || index.alias_sources[edge.target.slot()]
                || index.alias_outputs[edge.target.slot()]
        };
        scratch.merge(edge.target, source, relevant, |lhs, rhs| {
            facts.names_have_same_fact(lhs, rhs)
        });
    }
    scratch.finish_into(out);
}

fn propagate_indexed_store_targets<D>(
    facts: &mut D,
    facts_by_target: &IndexedStoreTargetFacts,
    blocked: &NameWorkSet,
) -> bool
where
    D: IndexedFactDomain,
{
    let mut changed = false;
    for (target, source) in &facts_by_target.entries {
        if blocked.contains(*target) {
            continue;
        }
        let Some(source) = *source else {
            continue;
        };
        let Some(fact) = facts.fact_for_name(source) else {
            continue;
        };
        changed |= facts.insert_fact(*target, fact);
    }
    changed
}

pub(super) fn store_var_targets_all_sources_in(
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

pub(super) fn propagate_store_var_targets_in(
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

pub(super) fn tir_container_storage_facts(
    tir_func: &TirFunction,
) -> HashMap<ValueId, ContainerStorageFact> {
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

pub(super) fn is_cold_module_chunk_function(name: &str) -> bool {
    name.contains("__molt_module_chunk_")
}

pub(super) fn integer_arithmetic_result_op(kind: &str) -> bool {
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

pub(super) fn integer_only_result_op(kind: &str) -> bool {
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

pub(super) fn simple_op_produces_non_scalar_value(kind: &str) -> bool {
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

pub(super) fn alias_source_name(op: &OpIR) -> Option<&str> {
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
pub(super) fn container_constructor_result_ty(kind: &str) -> Option<TirType> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::lir::LirRepr;

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
}
