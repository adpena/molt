use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ir::{FunctionIR, OpIR};
use crate::tir::function::TirFunction;
use crate::tir::lir::{LirRepr, LirValue};
use crate::tir::lower_from_simple::lower_to_tir;
use crate::tir::lower_to_lir::lower_function_to_lir;
use crate::tir::lower_to_simple::SimpleValueNames;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::type_refine::refine_types;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
#[cfg(feature = "llvm")]
use crate::tir::blocks::{BlockId, Terminator};
#[cfg(feature = "llvm")]
use crate::tir::ops::OpCode;

/// Scalar lane derived from the backend-facing TIR/LIR contract.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ScalarKind {
    Int,
    Bool,
    Float,
    Str,
    NoneValue,
}

/// Container dispatch lane derived from the backend-facing TIR/LIR contract.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ContainerKind {
    List,
    Dict,
    Set,
    Tuple,
    Str,
}

/// Physical container storage proof derived from structural producers.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ContainerStorageKind {
    FlatListInt,
}

/// A proven physical storage layout for a container value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerStorageFact {
    pub(crate) kind: ContainerStorageKind,
    pub(crate) elem_ty: TirType,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScalarRepresentationFact {
    pub(crate) ty: TirType,
    pub(crate) repr: LirRepr,
}

impl ScalarRepresentationFact {
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
pub(crate) struct ScalarRepresentationPlan {
    facts_by_name: BTreeMap<String, ScalarRepresentationFact>,
    conflicted_names: BTreeSet<String>,
    non_scalar_names: BTreeSet<String>,
    integer_family_names: BTreeSet<String>,
    container_storage_by_name: BTreeMap<String, ContainerStorageFact>,
    container_storage_conflicted_names: BTreeSet<String>,
    container_storage_ops: BTreeMap<usize, ContainerStorageFact>,
    primary_names: ScalarPrimaryNameSets,
    scalar_slot_exclusion_unsafe: BTreeSet<String>,
    scalar_store_targets_by_kind: BTreeMap<ScalarKind, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScalarPrimaryNameSets {
    pub(crate) int: BTreeSet<String>,
    pub(crate) bool_: BTreeSet<String>,
    pub(crate) float: BTreeSet<String>,
}

/// Per-function representation facts consumed by the LLVM backend.
///
/// The LLVM backend lowers `TirFunction` (SSA `ValueId`s) directly, while the
/// `ScalarRepresentationPlan` is keyed by the legacy SimpleIR variable
/// namespace. This struct bridges the two: it carries the plan's
/// representation decisions (overflow-safe int carrier subset, container
/// dispatch kinds) and the `ValueId -> SimpleIR name` mapping derived from the
/// *same* `TirFunction` the LLVM backend is lowering.
///
/// This makes the LLVM backend consume the identical typed facts the
/// native/WASM/Luau backends consume, rather than treating `TirType::I64` as an
/// exact-i64 carrier (which it is not — `type_refine` assigns `add(I64, I64) ->
/// I64` with no overflow proof, so unbounded integer arithmetic must stay
/// boxed/runtime-backed until a range proof exists).
#[derive(Clone, Debug, Default)]
#[cfg(feature = "llvm")]
pub(crate) struct LlvmReprFacts {
    /// Container dispatch kind keyed by SimpleIR name (the plan's authority for
    /// `len`/container-kind specialization).
    pub(crate) container_kind_by_name: BTreeMap<String, ContainerKind>,
    /// `ValueId -> SimpleIR name` for this exact `TirFunction`. Built from the
    /// LLVM backend's own post-pipeline TIR so names line up with the op
    /// `_simple_out` attributes the plan keys on.
    pub(crate) name_by_value: HashMap<ValueId, String>,
    /// TIR `ValueId`s that are overflow-safe exact-i64 carriers.
    ///
    /// Seeded from the plan's `primary_name_sets().int` (an interval-proven,
    /// no-i64-wrap carrier subset, keyed by SimpleIR name) and then propagated
    /// across the TIR SSA graph: through `Copy` chains and through block
    /// arguments (phis) — a phi is overflow-safe only when *every* incoming
    /// edge value is overflow-safe. Loop-carried block arguments have no stable
    /// `_simple_out` name (they are canonical slot names), so a pure name
    /// lookup cannot classify them; the dataflow propagation is what lets the
    /// backend keep a non-overflow-safe accumulator phi boxed (DynBox) instead
    /// of unboxing a runtime BigInt result back into a truncating raw i64.
    pub(crate) overflow_safe_values: std::collections::HashSet<ValueId>,
}

#[cfg(feature = "llvm")]
impl LlvmReprFacts {
    /// Build the LLVM representation facts for `func_ir` (the SimpleIR function
    /// about to be lowered) and `tir_func` (the LLVM backend's post-pipeline
    /// TIR for that function).
    pub(crate) fn build(func_ir: &FunctionIR, tir_func: &TirFunction) -> Self {
        let plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        let overflow_safe_int_names = &plan.primary_names.int;
        let container_kind_by_name = plan
            .facts_by_name
            .iter()
            .filter_map(|(name, fact)| fact.container_kind().map(|kind| (name.clone(), kind)))
            .collect();
        let names = SimpleValueNames::for_function(tir_func);
        let mut name_by_value = HashMap::new();
        for block in tir_func.blocks.values() {
            for op in &block.ops {
                for &result in &op.results {
                    name_by_value.insert(result, names.value_name(result));
                }
            }
            for arg in &block.args {
                name_by_value.insert(arg.id, names.value_name(arg.id));
            }
        }
        let overflow_safe_values =
            compute_overflow_safe_values(tir_func, overflow_safe_int_names, &name_by_value);
        Self {
            container_kind_by_name,
            name_by_value,
            overflow_safe_values,
        }
    }

    /// SimpleIR name for a TIR `ValueId`, if known.
    pub(crate) fn name_for(&self, id: ValueId) -> Option<&str> {
        self.name_by_value.get(&id).map(String::as_str)
    }

    /// Whether the value `id` is an overflow-safe exact-i64 carrier (may use raw
    /// machine arithmetic and a raw i64 representation).
    pub(crate) fn is_overflow_safe_int(&self, id: ValueId) -> bool {
        self.overflow_safe_values.contains(&id)
    }

    /// Container dispatch kind for the value named by `id`, per the plan.
    pub(crate) fn container_kind(&self, id: ValueId) -> Option<ContainerKind> {
        self.name_for(id)
            .and_then(|name| self.container_kind_by_name.get(name).copied())
    }
}

/// Propagate overflow-safety across the TIR SSA graph to a fixpoint.
///
/// A value is overflow-safe when the LLVM backend may carry it as a raw i64 and
/// emit raw machine arithmetic for it. The seed is the plan's interval-proven
/// int carrier subset (`overflow_safe_int_names`, keyed by SimpleIR
/// `_simple_out` name). Safety then flows along value-preserving edges:
///
/// - A `Copy` result is overflow-safe iff its source operand is.
/// - A block argument (phi) is overflow-safe iff *every* value passed to it on
///   every incoming branch edge is overflow-safe.
///
/// Built upward from the seed (monotone — only ever adds safety), so the
/// worklist terminates; back-edges resolve because a phi becomes safe only once
/// all of its incomings are known safe.
#[cfg(feature = "llvm")]
fn compute_overflow_safe_values(
    tir_func: &TirFunction,
    overflow_safe_int_names: &BTreeSet<String>,
    name_by_value: &HashMap<ValueId, String>,
) -> std::collections::HashSet<ValueId> {
    use std::collections::HashSet;

    // Collect block-argument incoming edges: (target block, arg index) -> list
    // of source values passed on each emitted branch edge.
    let mut block_arg_incomings: HashMap<(BlockId, usize), Vec<ValueId>> = HashMap::new();
    let mut add_edge = |target: BlockId, args: &[ValueId]| {
        for (index, &arg) in args.iter().enumerate() {
            block_arg_incomings
                .entry((target, index))
                .or_default()
                .push(arg);
        }
    };
    for block in tir_func.blocks.values() {
        match &block.terminator {
            Terminator::Branch { target, args } => add_edge(*target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                add_edge(*then_block, then_args);
                add_edge(*else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, target, args) in cases {
                    add_edge(*target, args);
                }
                add_edge(*default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

    // Index Copy producers and block-arg membership for the worklist.
    let mut copy_source: HashMap<ValueId, ValueId> = HashMap::new();
    let mut block_arg_ids: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    let mut all_value_ids: Vec<ValueId> = Vec::new();
    for block in tir_func.blocks.values() {
        for (index, arg) in block.args.iter().enumerate() {
            block_arg_ids.insert(arg.id, (block.id, index));
            all_value_ids.push(arg.id);
        }
        for op in &block.ops {
            if op.opcode == OpCode::Copy
                && let (Some(&result), Some(&source)) = (op.results.first(), op.operands.first())
            {
                copy_source.insert(result, source);
            }
            for &result in &op.results {
                all_value_ids.push(result);
            }
        }
    }

    let name_seeded = |id: ValueId| -> bool {
        name_by_value
            .get(&id)
            .is_some_and(|name| overflow_safe_int_names.contains(name))
    };

    let mut safe: HashSet<ValueId> = HashSet::new();
    for &id in &all_value_ids {
        if name_seeded(id) {
            safe.insert(id);
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for &id in &all_value_ids {
            if safe.contains(&id) {
                continue;
            }
            let becomes_safe = if let Some(&src) = copy_source.get(&id) {
                safe.contains(&src)
            } else if let Some(&(block, index)) = block_arg_ids.get(&id) {
                block_arg_incomings
                    .get(&(block, index))
                    .is_some_and(|incomings| {
                        !incomings.is_empty() && incomings.iter().all(|src| safe.contains(src))
                    })
            } else {
                false
            };
            if becomes_safe {
                safe.insert(id);
                changed = true;
            }
        }
    }
    safe
}

impl ScalarRepresentationPlan {
    pub(crate) fn for_function_ir(func_ir: &FunctionIR) -> Self {
        let mut tir_func = lower_to_tir(func_ir);
        refine_types(&mut tir_func);
        let names = SimpleValueNames::for_function(&tir_func);
        let lir_func = lower_function_to_lir(&tir_func);

        let mut plan = Self::default();
        plan.seed_container_storage_from_tir(&tir_func, &names);
        let mut block_ids: Vec<_> = lir_func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|block_id| block_id.0);
        for block_id in block_ids {
            let block = &lir_func.blocks[&block_id];
            for (index, arg) in block.args.iter().enumerate() {
                plan.insert_lir_value(names.value_name(arg.id), arg);
                plan.insert_lir_value(names.block_arg_slot(block.id, index), arg);
            }
            for op in &block.ops {
                let checked_i64_arithmetic = matches!(
                    op.tir_op.attrs.get("lir.checked_overflow"),
                    Some(AttrValue::Bool(true))
                );
                for (index, result) in op.result_values.iter().enumerate() {
                    let name = if checked_i64_arithmetic && index == 0 {
                        SimpleValueNames::canonical_value_name(result.id)
                    } else {
                        names.value_name(result.id)
                    };
                    plan.insert_lir_value(name, result);
                }
                if op.result_values.len() == 1
                    && let Some(AttrValue::Str(simple_out)) = op.tir_op.attrs.get("_simple_out")
                    && let Some(result) = op.result_values.first()
                {
                    plan.insert_lir_value(simple_out.clone(), result);
                }
            }
        }
        plan.propagate_simple_aliases(func_ir);
        plan.propagate_integer_family(func_ir);
        plan.propagate_container_storage(func_ir);
        plan.mark_container_storage_ops(func_ir);
        plan.scalar_slot_exclusion_unsafe = plan.compute_scalar_slot_exclusion_unsafe(func_ir);
        plan.scalar_store_targets_by_kind = plan.compute_scalar_store_targets(func_ir);
        plan.primary_names = plan.compute_primary_name_sets(func_ir);
        plan
    }

    pub(crate) fn scalar_name_sets(
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

    #[cfg(test)]
    pub(crate) fn integer_family_names(&self) -> BTreeSet<String> {
        self.integer_family_names.clone()
    }

    #[cfg(any(feature = "native-backend", feature = "llvm", test))]
    pub(crate) fn primary_name_sets(&self) -> ScalarPrimaryNameSets {
        self.primary_names.clone()
    }

    #[cfg(any(feature = "native-backend", test))]
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) fn scalar_slot_exclusion_unsafe(&self) -> BTreeSet<String> {
        self.scalar_slot_exclusion_unsafe.clone()
    }

    #[cfg(any(feature = "native-backend", test))]
    pub(crate) fn scalar_store_targets(&self, kind: ScalarKind) -> BTreeSet<String> {
        self.scalar_store_targets_by_kind
            .get(&kind)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn op_scalar_lane(&self, op: &OpIR) -> Option<ScalarKind> {
        self.infer_scalar_lane(op)
    }

    pub(crate) fn name_container_storage_kind(&self, name: &str) -> Option<ContainerStorageKind> {
        self.container_storage_by_name
            .get(name)
            .map(|fact| fact.kind)
    }

    pub(crate) fn op_has_container_storage(
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

    pub(crate) fn op_prefers_integer_runtime_lane(&self, op: &OpIR) -> bool {
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
                | "rshift"
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

    pub(crate) fn op_index_key_is_integer_family(&self, op: &OpIR) -> bool {
        matches!(op.kind.as_str(), "index" | "store_index" | "dict_set")
            && op.args.as_ref().is_some_and(|args| {
                args.get(1)
                    .is_some_and(|key| self.name_is_integer_family(key))
            })
    }

    fn insert_lir_value(&mut self, name: String, value: &LirValue) {
        self.insert_fact(
            name,
            ScalarRepresentationFact {
                ty: value.ty.clone(),
                repr: value.repr,
            },
        );
    }

    fn insert_fact(&mut self, name: String, fact: ScalarRepresentationFact) -> bool {
        if self.conflicted_names.contains(&name) {
            return false;
        }
        if let Some(existing) = self.facts_by_name.get(&name) {
            if existing != &fact {
                self.facts_by_name.remove(&name);
                self.conflicted_names.insert(name);
                return true;
            }
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

    fn remove_container_storage_fact(&mut self, name: &str) -> bool {
        self.container_storage_by_name.remove(name).is_some()
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

    fn propagate_simple_aliases(&mut self, func_ir: &FunctionIR) {
        let mut changed = true;
        while changed {
            changed = false;
            let store_target_facts = self.store_target_facts(func_ir);
            for (target, fact) in &store_target_facts {
                if fact.is_none() && self.facts_by_name.remove(target).is_some() {
                    changed = true;
                }
            }
            changed |= self.propagate_store_targets(store_target_facts.clone());
            for op in &func_ir.ops {
                let Some(out) = op.out.as_ref() else {
                    continue;
                };
                let Some(source) = alias_source_name(op) else {
                    continue;
                };
                if store_target_facts
                    .get(source)
                    .is_some_and(|fact| fact.is_none())
                {
                    if self.facts_by_name.remove(out).is_some() {
                        changed = true;
                    }
                    continue;
                }
                if self.facts_by_name.contains_key(out) {
                    continue;
                }
                let Some(fact) = self.facts_by_name.get(source).cloned() else {
                    continue;
                };
                changed |= self.insert_fact(out.clone(), fact);
            }
        }
    }

    fn store_target_facts(
        &self,
        func_ir: &FunctionIR,
    ) -> BTreeMap<String, Option<ScalarRepresentationFact>> {
        let mut facts_by_target: BTreeMap<String, Option<ScalarRepresentationFact>> =
            BTreeMap::new();
        for op in &func_ir.ops {
            let Some(target) = store_var_target_name(op) else {
                continue;
            };
            let source_fact = store_var_source_name(op)
                .and_then(|source| self.facts_by_name.get(source))
                .cloned();
            facts_by_target
                .entry(target.to_string())
                .and_modify(|existing| {
                    if existing.as_ref() != source_fact.as_ref() {
                        *existing = None;
                    }
                })
                .or_insert(source_fact);
        }
        facts_by_target
    }

    fn propagate_store_targets(
        &mut self,
        facts_by_target: BTreeMap<String, Option<ScalarRepresentationFact>>,
    ) -> bool {
        let mut changed = false;
        for (target, fact) in facts_by_target {
            let Some(fact) = fact else {
                continue;
            };
            if self.facts_by_name.get(&target) != Some(&fact) {
                changed |= self.insert_fact(target, fact);
            }
        }
        changed
    }

    fn propagate_container_storage(&mut self, func_ir: &FunctionIR) {
        let mut changed = true;
        while changed {
            changed = false;
            let store_target_facts = self.container_storage_store_target_facts(func_ir);
            for (target, fact) in &store_target_facts {
                if fact.is_none() {
                    changed |= self.remove_container_storage_fact(target);
                }
            }
            changed |= self.propagate_container_storage_store_targets(store_target_facts.clone());
            for op in &func_ir.ops {
                if let Some(out) = op.out.as_ref()
                    && let Some(source) = alias_source_name(op)
                {
                    if store_target_facts
                        .get(source)
                        .is_some_and(|fact| fact.is_none())
                    {
                        changed |= self.remove_container_storage_fact(out);
                        continue;
                    }
                    if !self.container_storage_by_name.contains_key(out)
                        && let Some(fact) = self.container_storage_by_name.get(source).cloned()
                    {
                        changed |= self.insert_container_storage_fact(out.clone(), fact);
                    }
                }
                if op.kind == "store_index" {
                    let Some(args) = op.args.as_ref() else {
                        continue;
                    };
                    let Some(container) = args.first() else {
                        continue;
                    };
                    if self.name_container_storage_kind(container)
                        != Some(ContainerStorageKind::FlatListInt)
                    {
                        continue;
                    }
                    let value_preserves_flat_int = args.get(2).is_some_and(|value| {
                        self.name_scalar_kind(value) == Some(ScalarKind::Int)
                            || self.name_is_integer_family(value)
                    });
                    if value_preserves_flat_int {
                        if let Some(out) = op.out.as_ref()
                            && let Some(fact) =
                                self.container_storage_by_name.get(container).cloned()
                        {
                            changed |= self.insert_container_storage_fact(out.clone(), fact);
                        }
                    } else {
                        changed |= self.remove_container_storage_fact(container);
                        if let Some(out) = op.out.as_ref() {
                            changed |= self.remove_container_storage_fact(out);
                        }
                    }
                }
            }
        }
    }

    fn container_storage_store_target_facts(
        &self,
        func_ir: &FunctionIR,
    ) -> BTreeMap<String, Option<ContainerStorageFact>> {
        let mut facts_by_target: BTreeMap<String, Option<ContainerStorageFact>> = BTreeMap::new();
        for op in &func_ir.ops {
            let Some(target) = store_var_target_name(op) else {
                continue;
            };
            let source_fact = store_var_source_name(op)
                .and_then(|source| self.container_storage_by_name.get(source))
                .cloned();
            facts_by_target
                .entry(target.to_string())
                .and_modify(|existing| {
                    if existing.as_ref() != source_fact.as_ref() {
                        *existing = None;
                    }
                })
                .or_insert(source_fact);
        }
        facts_by_target
    }

    fn propagate_container_storage_store_targets(
        &mut self,
        facts_by_target: BTreeMap<String, Option<ContainerStorageFact>>,
    ) -> bool {
        let mut changed = false;
        for (target, fact) in facts_by_target {
            let Some(fact) = fact else {
                continue;
            };
            if self.container_storage_by_name.get(&target) != Some(&fact) {
                changed |= self.insert_container_storage_fact(target, fact);
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

    fn propagate_integer_family(&mut self, func_ir: &FunctionIR) {
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
            changed |= self.propagate_integer_store_targets(func_ir);
            for op in &func_ir.ops {
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

    fn non_scalar_simple_outputs(&self, func_ir: &FunctionIR) -> BTreeSet<String> {
        func_ir
            .ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_deref()?;
                simple_op_produces_non_scalar_value(op.kind.as_str()).then(|| out.to_string())
            })
            .collect()
    }

    fn propagate_integer_store_targets(&mut self, func_ir: &FunctionIR) -> bool {
        let mut targets: BTreeMap<String, bool> = BTreeMap::new();
        for op in &func_ir.ops {
            let Some(target) = store_var_target_name(op) else {
                continue;
            };
            let source_is_integer = store_var_source_name(op)
                .is_some_and(|source| self.integer_family_names.contains(source));
            targets
                .entry(target.to_string())
                .and_modify(|all_sources_integer| {
                    *all_sources_integer &= source_is_integer;
                })
                .or_insert(source_is_integer);
        }

        let mut changed = false;
        for (target, all_sources_integer) in targets {
            if all_sources_integer && self.integer_family_names.insert(target) {
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn name_scalar_kind(&self, name: &str) -> Option<ScalarKind> {
        if self.non_scalar_names.contains(name) {
            return None;
        }
        self.facts_by_name
            .get(name)
            .and_then(ScalarRepresentationFact::scalar_kind)
    }

    pub(crate) fn name_container_kind(&self, name: &str) -> Option<ContainerKind> {
        self.facts_by_name
            .get(name)
            .and_then(ScalarRepresentationFact::container_kind)
    }

    pub(crate) fn op_container_kind(&self, op: &OpIR) -> Option<ContainerKind> {
        op.args
            .as_ref()
            .and_then(|args| args.first())
            .and_then(|name| self.name_container_kind(name))
    }

    pub(crate) fn op_has_container_kind(&self, op: &OpIR, kind: ContainerKind) -> bool {
        self.op_container_kind(op) == Some(kind)
    }

    pub(crate) fn name_is_integer_family(&self, name: &str) -> bool {
        if self.non_scalar_names.contains(name) {
            return false;
        }
        self.integer_family_names.contains(name)
            || self.name_scalar_kind(name) == Some(ScalarKind::Bool)
    }

    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) fn op_args_are_integer_family(&self, op: &OpIR) -> bool {
        op.args.as_ref().is_some_and(|args| {
            !args.is_empty() && args.iter().all(|arg| self.name_is_integer_family(arg))
        })
    }

    fn name_has_scalar_kind(&self, name: &str, kind: ScalarKind) -> bool {
        self.name_scalar_kind(name) == Some(kind)
    }

    fn compute_scalar_store_targets(
        &self,
        func_ir: &FunctionIR,
    ) -> BTreeMap<ScalarKind, BTreeSet<String>> {
        let mut targets = BTreeMap::new();
        for kind in [
            ScalarKind::Int,
            ScalarKind::Bool,
            ScalarKind::Float,
            ScalarKind::Str,
        ] {
            targets.insert(kind, self.scalar_lane_store_target_names(func_ir, kind));
        }
        targets
    }

    fn scalar_lane_store_target_names(
        &self,
        func_ir: &FunctionIR,
        lane: ScalarKind,
    ) -> BTreeSet<String> {
        let mut lane_outputs = BTreeSet::new();
        let mut changed = true;
        while changed {
            changed = propagate_store_var_targets_in(func_ir, &mut lane_outputs);
            for op in &func_ir.ops {
                if op.kind == "store_var" {
                    continue;
                }
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
                    "copy_var" | "copy" | "load_var" | "identity_alias"
                ) && first_arg_is_lane
                    || matches!(op.kind.as_str(), "copy_var" | "load_var") && var_source_is_lane;

                if (inferred_lane == Some(lane) || is_lane_alias)
                    && lane_outputs.insert(out.clone())
                {
                    changed = true;
                }
            }
        }
        store_var_targets_all_sources_in(func_ir, &lane_outputs)
    }

    fn compute_primary_name_sets(&self, func_ir: &FunctionIR) -> ScalarPrimaryNameSets {
        if is_cold_module_chunk_function(&func_ir.name) {
            return ScalarPrimaryNameSets::default();
        }

        let (int_like, bool_like, float_like, str_like, _) = self.scalar_name_sets();
        let param_name_set: BTreeSet<&str> = func_ir.params.iter().map(String::as_str).collect();
        let int_primary = self.compute_int_primary_names(
            func_ir,
            &param_name_set,
            &int_like,
            &bool_like,
            &float_like,
        );
        let bool_primary = self.compute_bool_primary_names(
            func_ir,
            &param_name_set,
            &int_primary,
            &bool_like,
            &int_like,
            &float_like,
            &str_like,
        );
        let float_primary =
            self.compute_float_primary_names(func_ir, &param_name_set, &int_like, &float_like);

        ScalarPrimaryNameSets {
            int: int_primary,
            bool_: bool_primary,
            float: float_primary,
        }
    }

    fn compute_int_primary_names(
        &self,
        func_ir: &FunctionIR,
        param_name_set: &BTreeSet<&str>,
        int_like: &BTreeSet<String>,
        bool_like: &BTreeSet<String>,
        float_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let bounded_i64_names = compute_i64_interval_facts(func_ir);
        let int_unsafe_outputs: BTreeSet<String> = func_ir
            .ops
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
            self.vars_with_non_int_defs(func_ir, int_like, bool_like, &bounded_i64_name_set);
        let passes_filter = |name: &str| {
            (int_like.contains(name) || bounded_i64_names.contains_key(name))
                && !param_name_set.contains(name)
                && !int_unsafe_outputs.contains(name)
                && !vars_with_non_int_defs.contains(name)
                && !float_like.contains(name)
        };
        let mut candidates: BTreeSet<String> =
            bounded_store_load_loop_seed_names(func_ir, &bounded_i64_names)
                .into_iter()
                .filter(|name| passes_filter(name))
                .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for target in store_var_targets_all_sources_in(func_ir, &candidates) {
                if passes_filter(&target) && candidates.insert(target) {
                    changed = true;
                }
            }
            for op in &func_ir.ops {
                if op.kind == "store_var" {
                    continue;
                }
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
        candidates
    }

    fn vars_with_non_int_defs(
        &self,
        func_ir: &FunctionIR,
        int_like: &BTreeSet<String>,
        bool_like: &BTreeSet<String>,
        extra_int_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut non_int = BTreeSet::new();
        for op in &func_ir.ops {
            if op.kind == "store_var" {
                let target = op.var.as_ref().or(op.out.as_ref());
                let source = op.args.as_ref().and_then(|a| a.first());
                if let (Some(t), Some(s)) = (target, source)
                    && !int_like.contains(s)
                    && !bool_like.contains(s)
                    && !extra_int_like.contains(s)
                {
                    non_int.insert(t.clone());
                }
            }
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

    fn compute_bool_primary_names(
        &self,
        func_ir: &FunctionIR,
        param_name_set: &BTreeSet<&str>,
        int_primary: &BTreeSet<String>,
        bool_like: &BTreeSet<String>,
        int_like: &BTreeSet<String>,
        float_like: &BTreeSet<String>,
        str_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let bool_unsafe_outputs: BTreeSet<String> = func_ir
            .ops
            .iter()
            .filter_map(|op| {
                let out = op.out.as_ref()?;
                let is_safe_bool_op = op.kind == "store_var"
                    || self.op_produces_raw_bool_for_bool_primary(op, bool_like, int_primary);
                (!is_safe_bool_op && bool_like.contains(out)).then(|| out.clone())
            })
            .collect();
        let vars_with_non_bool_defs = self.vars_with_non_bool_defs(func_ir, bool_like, int_primary);
        let passes_filter = |name: &str| {
            bool_like.contains(name)
                && !param_name_set.contains(name)
                && !bool_unsafe_outputs.contains(name)
                && !vars_with_non_bool_defs.contains(name)
                && !int_like.contains(name)
                && !float_like.contains(name)
                && !str_like.contains(name)
                && !self.scalar_slot_exclusion_unsafe.contains(name)
        };
        let mut candidates = BTreeSet::new();
        let mut changed = true;
        while changed {
            changed = false;
            for target in store_var_targets_all_sources_in(func_ir, &candidates) {
                if passes_filter(&target) && candidates.insert(target) {
                    changed = true;
                }
            }
            for op in &func_ir.ops {
                if op.kind == "store_var" {
                    continue;
                }
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
        func_ir: &FunctionIR,
        bool_like: &BTreeSet<String>,
        int_primary: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut non_bool = BTreeSet::new();
        for op in &func_ir.ops {
            if op.kind == "store_var" {
                let target = op.var.as_ref().or(op.out.as_ref());
                let source = op.args.as_ref().and_then(|a| a.first());
                if let (Some(t), Some(s)) = (target, source)
                    && !bool_like.contains(s)
                {
                    non_bool.insert(t.clone());
                }
            }
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
            "copy" | "copy_var" | "load_var" | "identity_alias" => {
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
        func_ir: &FunctionIR,
        param_name_set: &BTreeSet<&str>,
        int_like: &BTreeSet<String>,
        float_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let float_unsafe_outputs: BTreeSet<String> = func_ir
            .ops
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
                        | "neg"
                        | "unary_neg"
                        | "copy_var"
                        | "load_var"
                        | "identity_alias"
                        | "store_var"
                        | "float_from_obj"
                );
                (!is_safe_float_op && float_like.contains(out)).then(|| out.clone())
            })
            .collect();
        let vars_with_non_float_defs = self.vars_with_non_float_defs(func_ir, float_like);
        float_like
            .iter()
            .filter(|name| {
                !param_name_set.contains(name.as_str())
                    && !float_unsafe_outputs.contains(*name)
                    && !int_like.contains(*name)
                    && !vars_with_non_float_defs.contains(*name)
            })
            .cloned()
            .collect()
    }

    fn vars_with_non_float_defs(
        &self,
        func_ir: &FunctionIR,
        float_like: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut non_float = BTreeSet::new();
        for op in &func_ir.ops {
            if op.kind == "store_var" {
                let target = op.var.as_ref().or(op.out.as_ref());
                let source = op.args.as_ref().and_then(|a| a.first());
                if let (Some(t), Some(s)) = (target, source)
                    && !float_like.contains(s)
                {
                    non_float.insert(t.clone());
                }
            }
            if let Some(out) = op.out.as_ref() {
                let lane = self.infer_scalar_lane(op);
                if lane != Some(ScalarKind::Float) && float_like.contains(out) {
                    non_float.insert(out.clone());
                }
            }
        }
        non_float
    }

    fn compute_scalar_slot_exclusion_unsafe(&self, func_ir: &FunctionIR) -> BTreeSet<String> {
        let mut unsafe_set = BTreeSet::new();
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
                _ => {}
            }
        }
        unsafe_set
    }

    fn collect_scalar_args(&self, op: &OpIR, into: &mut BTreeSet<String>) {
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

    fn infer_scalar_lane_with_overrides(
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
            "copy" | "copy_var" | "load_var" | "identity_alias" => first_source().and_then(|src| {
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
            "pow" => {
                if (args.len() >= 2 && is_float(&args[1]))
                    || args_all(&is_float)
                    || (args_any(&is_float) && args.iter().all(|arg| is_float(arg) || is_int(arg)))
                {
                    Some(ScalarKind::Float)
                } else {
                    None
                }
            }
            "div" => {
                if args_all(&is_float)
                    || (args_any(&is_float) && args.iter().all(|arg| is_float(arg) || is_int(arg)))
                {
                    Some(ScalarKind::Float)
                } else {
                    None
                }
            }
            "lshift" | "rshift" | "shl" | "shr" => {
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
    func_ir: &FunctionIR,
    proven_outputs: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut targets: BTreeMap<String, bool> = BTreeMap::new();
    for op in &func_ir.ops {
        let Some(target) = store_var_target_name(op) else {
            continue;
        };
        let source_proven =
            store_var_source_name(op).is_some_and(|src| proven_outputs.contains(src));
        targets
            .entry(target.to_string())
            .and_modify(|all_sources_proven| *all_sources_proven &= source_proven)
            .or_insert(source_proven);
    }
    targets
        .into_iter()
        .filter_map(|(target, all_sources_proven)| all_sources_proven.then_some(target))
        .collect()
}

fn propagate_store_var_targets_in(
    func_ir: &FunctionIR,
    proven_outputs: &mut BTreeSet<String>,
) -> bool {
    let mut changed = false;
    for target in store_var_targets_all_sources_in(func_ir, proven_outputs) {
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
    candidates: &BTreeSet<String>,
    bounded_i64_names: &BTreeMap<String, I64Interval>,
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
        "copy" | "copy_var" | "load_var" | "identity_alias" | "pos" | "invert" => {
            first_source().is_some_and(|s| candidates.contains(s))
        }
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

fn compute_i64_interval_facts(func_ir: &FunctionIR) -> BTreeMap<String, I64Interval> {
    let mut intervals = BTreeMap::new();
    let loop_backedge_updates = loop_backedge_update_names(func_ir);
    let mut changed = true;
    while changed {
        changed = false;
        for op in &func_ir.ops {
            if let Some(out) = op.out.as_deref()
                && let Some(interval) =
                    interval_for_simple_op(op, &intervals, &loop_backedge_updates)
            {
                changed |= insert_interval(&mut intervals, out, interval);
            }
        }
        changed |= propagate_store_target_intervals(func_ir, &mut intervals);
        changed |= propagate_counted_loop_intervals(func_ir, &mut intervals);
    }
    intervals
}

fn insert_interval(
    intervals: &mut BTreeMap<String, I64Interval>,
    name: &str,
    interval: I64Interval,
) -> bool {
    match intervals.get(name).copied() {
        Some(existing) => {
            let joined = existing.union(interval);
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
    intervals: &BTreeMap<String, I64Interval>,
    loop_backedge_updates: &BTreeSet<String>,
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
        "copy" | "copy_var" | "load_var" | "identity_alias" | "pos" | "loop_index_start"
        | "loop_index_next" => interval_for_first_source(op, intervals),
        "add" | "inplace_add" => interval_for_binary_args(op, intervals, I64Interval::checked_add),
        "sub" | "inplace_sub" => interval_for_binary_args(op, intervals, I64Interval::checked_sub),
        _ => None,
    }
}

fn interval_for_first_source(
    op: &OpIR,
    intervals: &BTreeMap<String, I64Interval>,
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
    intervals: &BTreeMap<String, I64Interval>,
    combine: fn(I64Interval, I64Interval) -> Option<I64Interval>,
) -> Option<I64Interval> {
    let args = op.args.as_ref()?;
    let lhs = intervals.get(args.first()?)?;
    let rhs = intervals.get(args.get(1)?)?;
    combine(*lhs, *rhs)
}

fn propagate_store_target_intervals(
    func_ir: &FunctionIR,
    intervals: &mut BTreeMap<String, I64Interval>,
) -> bool {
    let mut targets: BTreeMap<String, Option<I64Interval>> = BTreeMap::new();
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
            changed |= insert_interval(intervals, &target, interval);
        }
    }
    changed
}

fn propagate_counted_loop_intervals(
    func_ir: &FunctionIR,
    intervals: &mut BTreeMap<String, I64Interval>,
) -> bool {
    let mut changed = false;
    for (start, end) in loop_regions(&func_ir.ops) {
        if let Some(proof) = loop_index_interval_proof(func_ir, start, end, intervals) {
            for (name, interval) in proof.names {
                changed |= insert_interval(intervals, &name, interval);
            }
        }
        if let Some(proof) = store_load_loop_interval_proof(func_ir, start, end, intervals) {
            for (name, interval) in proof.names {
                changed |= insert_interval(intervals, &name, interval);
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
    intervals: &BTreeMap<String, I64Interval>,
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
    intervals: &BTreeMap<String, I64Interval>,
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
    intervals: &BTreeMap<String, I64Interval>,
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
    intervals: &BTreeMap<String, I64Interval>,
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
    intervals: &BTreeMap<String, I64Interval>,
) -> Option<(usize, String, i64)> {
    let ops = &func_ir.ops;
    for idx in (start + 1..end).rev() {
        let op = &ops[idx];
        if store_var_target_name(op) != Some(slot_name) {
            continue;
        }
        let next_name = store_var_source_name(op)?.to_string();
        let update_idx = (start + 1..idx)
            .rev()
            .find(|candidate| ops[*candidate].out.as_deref() == Some(next_name.as_str()))?;
        let step = induction_update_step(func_ir, update_idx, iv_name, intervals)?;
        return Some((idx, next_name, step));
    }
    None
}

fn induction_update_step(
    func_ir: &FunctionIR,
    update_idx: usize,
    iv_name: &str,
    intervals: &BTreeMap<String, I64Interval>,
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
    intervals: &BTreeMap<String, I64Interval>,
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
        if let Some(interval) = interval_for_simple_op(op, intervals, &BTreeSet::new()) {
            return Some(interval);
        }
        match op.kind.as_str() {
            "copy" | "copy_var" | "load_var" | "identity_alias" | "pos" => {
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
    intervals: &BTreeMap<String, I64Interval>,
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

fn loop_backedge_update_names(func_ir: &FunctionIR) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
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
            let Some(update_idx) = (start + 1..idx)
                .rev()
                .find(|candidate| ops[*candidate].out.as_deref() == Some(source))
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
                names.insert(source.to_string());
            }
        }
    }
    names
}

fn bounded_store_load_loop_seed_names(
    func_ir: &FunctionIR,
    bounded_i64_names: &BTreeMap<String, I64Interval>,
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
    bounded_i64_names: &BTreeMap<String, I64Interval>,
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
    bounded_i64_names: &BTreeMap<String, I64Interval>,
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
            "copy" | "copy_var" | "load_var" | "identity_alias" | "pos" => {
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
            | "rshift"
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
        "async_sleep_new"
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
        "copy" | "copy_var" | "load_var" | "identity_alias" => op.var.as_deref().or_else(|| {
            op.args
                .as_ref()
                .and_then(|args| args.first().map(String::as_str))
        }),
        _ => None,
    }
}

fn store_var_target_name(op: &OpIR) -> Option<&str> {
    if op.kind == "store_var" {
        op.var.as_deref().or(op.out.as_deref())
    } else {
        None
    }
}

fn store_var_source_name(op: &OpIR) -> Option<&str> {
    op.args
        .as_ref()
        .and_then(|args| args.first().map(String::as_str))
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
        plan.propagate_integer_family(&function("empty", &[], None, vec![]));

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
        plan.propagate_integer_family(&function("empty", &[], None, vec![]));

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

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

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

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

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

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(primary.float.contains("base"));
        assert!(primary.float.contains("exp"));
        assert!(primary.float.contains("sum"));
        assert!(primary.float.contains("sum_copy"));
        assert!(!primary.float.contains("pow_result"));
        assert!(!primary.float.contains("p"));
        assert!(primary.float.contains("param_copy"));
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
    fn cold_module_chunk_functions_have_empty_primary_sets() {
        let func = function(
            "__molt_module_chunk_0",
            &[],
            None,
            vec![const_int("value", 1), const_bool("flag", true)],
        );

        let primary = ScalarRepresentationPlan::for_function_ir(&func).primary_name_sets();

        assert!(primary.int.is_empty());
        assert!(primary.bool_.is_empty());
        assert!(primary.float.is_empty());
    }
}
