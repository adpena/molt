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

use value_repr::projected_scalar_carrier_name_reprs_for;
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
    data_ops: Vec<&'a OpIR>,
    store_index_ops: Vec<&'a OpIR>,
    sentinel_outputs: PlanHashSet<String>,
    delete_targets: PlanHashSet<String>,
}

impl<'a> FunctionFactIndex<'a> {
    fn for_function(func_ir: &'a FunctionIR) -> Self {
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
        let mut optimized_tir_func = None;
        let mut optimized_names = None;
        if crate::tir::verify::verify_function(&tir_func).is_ok() {
            let mut projected_tir_func = tir_func.clone();
            crate::tir::passes::run_pipeline(
                &mut projected_tir_func,
                &crate::tir::target_info::TargetInfo::native_release_fast(),
            );
            refine_types(&mut projected_tir_func);
            optimized_names = Some(SimpleValueNames::for_function(&projected_tir_func));
            optimized_tir_func = Some(projected_tir_func);
        }
        // Fact extraction uses the semantic type floor. Raw-int carrier homes
        // are projected from `repr_by_value_for` after the LIR facts have been
        // named, so this extraction step never becomes a second carrier proof.
        // Projection also sees the optimized TIR view because the canonical TIR
        // pipeline exposes loop phis and promoted store/load slots that the
        // backend lowers as raw value carriers.
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
        let mut tir_value_views = Vec::with_capacity(2);
        tir_value_views.push((&tir_func, &names));
        if let (Some(optimized_tir_func), Some(optimized_names)) =
            (optimized_tir_func.as_ref(), optimized_names.as_ref())
        {
            tir_value_views.push((optimized_tir_func, optimized_names));
        }
        plan.seed_repr_by_name(func_ir, &fact_index, &tir_value_views);
        plan
    }

    /// Compute the integer/bool/float raw-carrier sets and translate them into
    /// the native `repr_by_name` view. Integer carriers are projected from the
    /// value-keyed `repr_by_value_for` authority for the semantic and optimized
    /// TIR views, then transported through SimpleIR names only as a lowering view.
    /// Bool/F64 names still floor to boxed `DynBox` here and are raised only by
    /// their raw-bool/raw-f64 eligibility filters, so semantic type facts alone
    /// cannot authorize unboxed native storage.
    fn seed_repr_by_name(
        &mut self,
        func_ir: &FunctionIR,
        fact_index: &FunctionFactIndex<'_>,
        tir_value_views: &[(&TirFunction, &SimpleValueNames)],
    ) {
        let primary = self.compute_primary_name_sets(func_ir, fact_index, tir_value_views);
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
    /// lattice. `int` is the native raw-i64 carrier union projected from the
    /// value-keyed TIR representation proof; `int_inline_safe` and
    /// `int_full_deopt` expose the two tiers separately so box sites cannot
    /// confuse the inline-int47 proof with the overflow-peel proof. Bool and
    /// float are the raw 0/1 and unboxed-f64 views over the same `repr_by_name`
    /// lowering view.
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

    /// Inline-int47 raw-i64 carrier names — the `{RawI64Safe}` view over the
    /// value-backed native `repr_by_name` projection. Use
    /// [`Self::int_raw_carrier_names`] or [`Self::is_raw_int_carrier_name`] when
    /// a native consumer means "any raw i64 storage"; box sites must distinguish
    /// this tier from [`Self::int_full_deopt_names`].
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

    pub fn name_has_scalar_kind(&self, name: &str, kind: ScalarKind) -> bool {
        self.name_scalar_kind(name) == Some(kind)
    }

    pub fn name_is_integer_scalar(&self, name: &str) -> bool {
        matches!(
            self.name_scalar_kind(name),
            Some(ScalarKind::Int | ScalarKind::Bool)
        ) || self.is_raw_int_carrier_name(name)
            || self.is_bool_unboxed(name)
    }

    pub fn name_is_float_scalar(&self, name: &str) -> bool {
        self.name_has_scalar_kind(name, ScalarKind::Float) || self.is_float_unboxed(name)
    }

    pub fn name_is_numeric_scalar(&self, name: &str) -> bool {
        self.name_is_integer_scalar(name) || self.name_is_float_scalar(name)
    }

    pub fn name_is_bool_scalar(&self, name: &str) -> bool {
        self.name_has_scalar_kind(name, ScalarKind::Bool) || self.is_bool_unboxed(name)
    }

    pub fn name_is_str_scalar(&self, name: &str) -> bool {
        self.name_has_scalar_kind(name, ScalarKind::Str)
    }

    pub fn name_is_none_scalar(&self, name: &str) -> bool {
        self.name_has_scalar_kind(name, ScalarKind::NoneValue)
    }

    pub fn name_is_non_heap_scalar(&self, name: &str) -> bool {
        matches!(
            self.name_scalar_kind(name),
            Some(ScalarKind::Int | ScalarKind::Bool | ScalarKind::Float | ScalarKind::NoneValue)
        ) || self.is_raw_int_carrier_name(name)
            || self.is_bool_unboxed(name)
            || self.is_float_unboxed(name)
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
        tir_value_views: &[(&TirFunction, &SimpleValueNames)],
    ) -> ScalarPrimaryNameSets {
        if is_cold_module_chunk_function(&func_ir.name) {
            return ScalarPrimaryNameSets::default();
        }

        let projected = self.project_scalar_name_reprs_from_tir_views(fact_index, tir_value_views);
        let int_inline_safe = Self::projected_names_with_repr(&projected, Repr::RawI64Safe);
        let int_full_deopt = Self::projected_names_with_repr(&projected, Repr::RawI64FullDeopt);
        let mut int_primary = int_inline_safe.clone();
        int_primary.extend(int_full_deopt.iter().cloned());
        let bool_primary = Self::projected_names_with_repr(&projected, Repr::Bool);
        let float_primary = Self::projected_names_with_repr(&projected, Repr::FloatUnboxed);

        ScalarPrimaryNameSets {
            int: int_primary,
            int_inline_safe,
            int_full_deopt,
            bool_: bool_primary,
            float: float_primary,
        }
    }

    fn projected_names_with_repr(
        projected: &BTreeMap<String, Repr>,
        expected: Repr,
    ) -> BTreeSet<String> {
        projected
            .iter()
            .filter(|(_, repr)| **repr == expected)
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn project_scalar_name_reprs_from_tir_views(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        tir_value_views: &[(&TirFunction, &SimpleValueNames)],
    ) -> BTreeMap<String, Repr> {
        let mut projected = BTreeMap::new();
        let mut blocked = BTreeSet::new();
        for (tir_func, names) in tir_value_views {
            for (name, repr) in projected_scalar_carrier_name_reprs_for(tir_func, names) {
                self.insert_projected_scalar_name_repr(
                    fact_index,
                    name,
                    repr,
                    &mut projected,
                    &mut blocked,
                );
            }
        }
        self.propagate_projected_scalar_name_reprs(fact_index, &mut projected, &mut blocked);
        projected
    }

    fn insert_projected_scalar_name_repr(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        name: impl Into<String>,
        repr: Repr,
        projected: &mut BTreeMap<String, Repr>,
        blocked: &mut BTreeSet<String>,
    ) -> bool {
        let name = name.into();
        if blocked.contains(&name)
            || !self.name_allows_projected_scalar_repr(fact_index, &name, repr)
        {
            return false;
        }
        match projected.get(&name).copied() {
            Some(existing) if existing == repr => false,
            Some(existing) => {
                if let Some(merged) = Self::merge_projected_scalar_repr(existing, repr) {
                    if merged != existing {
                        projected.insert(name, merged);
                        true
                    } else {
                        false
                    }
                } else {
                    projected.remove(&name);
                    blocked.insert(name);
                    true
                }
            }
            None => {
                projected.insert(name, repr);
                true
            }
        }
    }

    fn name_allows_projected_scalar_repr(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        name: &str,
        repr: Repr,
    ) -> bool {
        if fact_index.sentinel_outputs.contains(name) || fact_index.delete_targets.contains(name) {
            return false;
        }
        match repr {
            Repr::RawI64Safe | Repr::RawI64FullDeopt => {
                matches!(self.name_scalar_kind(name), None | Some(ScalarKind::Int))
            }
            Repr::Bool => {
                self.name_scalar_kind(name) == Some(ScalarKind::Bool)
                    && !self.scalar_slot_exclusion_unsafe.contains(name)
            }
            Repr::FloatUnboxed => self.name_scalar_kind(name) == Some(ScalarKind::Float),
            _ => false,
        }
    }

    fn merge_projected_scalar_repr(existing: Repr, incoming: Repr) -> Option<Repr> {
        match (existing, incoming) {
            (lhs, rhs) if lhs == rhs => Some(lhs),
            (Repr::RawI64Safe, Repr::RawI64FullDeopt)
            | (Repr::RawI64FullDeopt, Repr::RawI64Safe)
            | (Repr::RawI64FullDeopt, Repr::RawI64FullDeopt) => Some(Repr::RawI64FullDeopt),
            _ => None,
        }
    }

    fn block_projected_scalar_name(
        projected: &mut BTreeMap<String, Repr>,
        blocked: &mut BTreeSet<String>,
        name: &str,
    ) -> bool {
        let removed = projected.remove(name).is_some();
        let inserted = blocked.insert(name.to_string());
        removed || inserted
    }

    fn projected_scalar_repr_for_name(
        name: &str,
        projected: &BTreeMap<String, Repr>,
    ) -> Option<Repr> {
        projected.get(name).copied()
    }

    fn store_var_target_projected_scalar_reprs(
        fact_index: &FunctionFactIndex<'_>,
        projected: &BTreeMap<String, Repr>,
    ) -> Vec<(String, Option<Repr>)> {
        let mut target_reprs: PlanHashMap<&str, Option<Repr>> =
            plan_hash_map(fact_index.stores.len().saturating_add(1));
        for edge in &fact_index.stores {
            let source_repr = edge
                .source
                .and_then(|source| Self::projected_scalar_repr_for_name(source, projected));
            target_reprs
                .entry(edge.target)
                .and_modify(|existing| {
                    *existing = match (*existing, source_repr) {
                        (Some(lhs), Some(rhs)) => Self::merge_projected_scalar_repr(lhs, rhs),
                        _ => None,
                    };
                })
                .or_insert(source_repr);
        }
        target_reprs
            .into_iter()
            .map(|(target, repr)| (target.to_string(), repr))
            .collect()
    }

    fn propagate_projected_scalar_name_reprs(
        &self,
        fact_index: &FunctionFactIndex<'_>,
        projected: &mut BTreeMap<String, Repr>,
        blocked: &mut BTreeSet<String>,
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            for (target, repr) in
                Self::store_var_target_projected_scalar_reprs(fact_index, projected)
            {
                if let Some(repr) = repr {
                    changed |= self.insert_projected_scalar_name_repr(
                        fact_index, target, repr, projected, blocked,
                    );
                } else {
                    changed |= Self::block_projected_scalar_name(projected, blocked, &target);
                }
            }
            for edge in &fact_index.aliases {
                if let Some(repr) = Self::projected_scalar_repr_for_name(edge.source, projected) {
                    changed |= self.insert_projected_scalar_name_repr(
                        fact_index,
                        edge.out.to_string(),
                        repr,
                        projected,
                        blocked,
                    );
                }
            }
        }
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
pub(crate) mod test_fixtures;

#[cfg(test)]
mod tests;
