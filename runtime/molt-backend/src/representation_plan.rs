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
                for result in &op.result_values {
                    plan.insert_lir_value(names.value_name(result.id), result);
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

    #[cfg(any(feature = "native-backend", test))]
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
        self.integer_family_names
            .extend(self.facts_by_name.iter().filter_map(|(name, fact)| {
                (!self.non_scalar_names.contains(name)
                    && (matches!(fact.ty, TirType::I64) && fact.repr == LirRepr::I64
                        || matches!(fact.ty, TirType::BigInt)))
                .then(|| name.clone())
            }));

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
                (!is_safe_int_op && int_like.contains(out)).then(|| out.clone())
            })
            .collect();
        let vars_with_non_int_defs = self.vars_with_non_int_defs(func_ir, int_like, bool_like);
        let passes_filter = |name: &str| {
            int_like.contains(name)
                && !param_name_set.contains(name)
                && !int_unsafe_outputs.contains(name)
                && !vars_with_non_int_defs.contains(name)
                && !float_like.contains(name)
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
                if op_produces_raw_i64_for_int_primary(op, &candidates)
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
    ) -> BTreeSet<String> {
        let mut non_int = BTreeSet::new();
        for op in &func_ir.ops {
            if op.kind == "store_var" {
                let target = op.var.as_ref().or(op.out.as_ref());
                let source = op.args.as_ref().and_then(|a| a.first());
                if let (Some(t), Some(s)) = (target, source)
                    && !int_like.contains(s)
                    && !bool_like.contains(s)
                {
                    non_int.insert(t.clone());
                }
            }
            if let Some(out) = op.out.as_ref() {
                let lane = self.infer_scalar_lane(op);
                let proven_int = matches!(lane, Some(ScalarKind::Int) | Some(ScalarKind::Bool));
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
                if args.len() >= 2 && is_float(&args[1]) {
                    Some(ScalarKind::Float)
                } else if args_all(&is_float)
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
            "index" | "store_index" | "dict_set" => {
                args.get(1).filter(|k| is_int(k)).map(|_| ScalarKind::Int)
            }
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

fn op_produces_raw_i64_for_int_primary(op: &OpIR, candidates: &BTreeSet<String>) -> bool {
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
        _ => false,
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
            | "dict_new"
            | "exception_new"
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
    fn primary_int_names_exclude_unbounded_arithmetic_without_range_proof() {
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
        assert!(!primary.int.contains("sum"));
        assert!(!primary.int.contains("shifted"));
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
