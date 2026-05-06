use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{FunctionIR, OpIR};
use crate::tir::lir::{LirRepr, LirValue};
use crate::tir::lower_from_simple::lower_to_tir;
use crate::tir::lower_to_lir::lower_function_to_lir;
use crate::tir::lower_to_simple::SimpleValueNames;
use crate::tir::ops::AttrValue;
use crate::tir::type_refine::refine_types;
use crate::tir::types::TirType;

/// Native scalar lane derived from the backend-facing TIR/LIR contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeScalarKind {
    Int,
    Bool,
    Float,
    Str,
    NoneValue,
}

/// A typed representation fact for a name in the legacy SimpleIR namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeRepresentationFact {
    pub(crate) ty: TirType,
    pub(crate) repr: LirRepr,
}

impl NativeRepresentationFact {
    fn scalar_kind(&self) -> Option<NativeScalarKind> {
        match (&self.ty, self.repr) {
            (TirType::I64, LirRepr::I64) => Some(NativeScalarKind::Int),
            (TirType::Bool, LirRepr::Bool1) => Some(NativeScalarKind::Bool),
            (TirType::F64, LirRepr::F64) => Some(NativeScalarKind::Float),
            (TirType::Str, _) => Some(NativeScalarKind::Str),
            (TirType::None, _) => Some(NativeScalarKind::NoneValue),
            _ => None,
        }
    }
}

/// The native backend's read-only view of final typed representation facts.
///
/// This is built from the exact `FunctionIR` that Cranelift is about to lower,
/// after module-level TIR roundtrip and post-TIR SimpleIR rewrites have already
/// run. It deliberately does not trust transport hints (`fast_int`,
/// `fast_float`, or `type_hint`) as representation authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NativeRepresentationPlan {
    facts_by_name: BTreeMap<String, NativeRepresentationFact>,
    conflicted_names: BTreeSet<String>,
    integer_family_names: BTreeSet<String>,
}

impl NativeRepresentationPlan {
    pub(crate) fn for_function_ir(func_ir: &FunctionIR) -> Self {
        let mut tir_func = lower_to_tir(func_ir);
        refine_types(&mut tir_func);
        let names = SimpleValueNames::for_function(&tir_func);
        let lir_func = lower_function_to_lir(&tir_func);

        let mut plan = Self::default();
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
                Some(NativeScalarKind::Int) => {
                    int_like.insert(name.clone());
                }
                Some(NativeScalarKind::Bool) => {
                    bool_like.insert(name.clone());
                }
                Some(NativeScalarKind::Float) => {
                    float_like.insert(name.clone());
                }
                Some(NativeScalarKind::Str) => {
                    str_like.insert(name.clone());
                }
                Some(NativeScalarKind::NoneValue) => {
                    none_like.insert(name.clone());
                }
                None => {}
            }
        }
        (int_like, bool_like, float_like, str_like, none_like)
    }

    pub(crate) fn integer_family_names(&self) -> BTreeSet<String> {
        self.integer_family_names.clone()
    }

    fn insert_lir_value(&mut self, name: String, value: &LirValue) {
        self.insert_fact(
            name,
            NativeRepresentationFact {
                ty: value.ty.clone(),
                repr: value.repr,
            },
        );
    }

    fn insert_fact(&mut self, name: String, fact: NativeRepresentationFact) -> bool {
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
    ) -> BTreeMap<String, Option<NativeRepresentationFact>> {
        let mut facts_by_target: BTreeMap<String, Option<NativeRepresentationFact>> =
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
        facts_by_target: BTreeMap<String, Option<NativeRepresentationFact>>,
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

    fn propagate_integer_family(&mut self, func_ir: &FunctionIR) {
        self.integer_family_names
            .extend(self.facts_by_name.iter().filter_map(|(name, fact)| {
                (matches!(fact.ty, TirType::I64) && fact.repr == LirRepr::I64
                    || matches!(fact.ty, TirType::BigInt))
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
                        !args.is_empty()
                            && args.iter().all(|arg| {
                                self.integer_family_names.contains(arg) || self.name_is_bool(arg)
                            })
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

    fn name_is_bool(&self, name: &str) -> bool {
        self.facts_by_name
            .get(name)
            .is_some_and(|fact| fact.scalar_kind() == Some(NativeScalarKind::Bool))
    }
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

    #[test]
    fn dynbox_i64_fact_is_not_a_scalar_integer() {
        let mut plan = NativeRepresentationPlan::default();
        plan.insert_fact(
            "boxed_word".to_string(),
            NativeRepresentationFact {
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
    fn conflicting_facts_do_not_pick_order_dependent_scalar_lane() {
        let mut plan = NativeRepresentationPlan::default();
        plan.insert_fact(
            "ambiguous".to_string(),
            NativeRepresentationFact {
                ty: TirType::I64,
                repr: LirRepr::I64,
            },
        );
        plan.insert_fact(
            "ambiguous".to_string(),
            NativeRepresentationFact {
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
            NativeRepresentationPlan::for_function_ir(&func).scalar_name_sets();

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
            NativeRepresentationPlan::for_function_ir(&mixed).scalar_name_sets();
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
            NativeRepresentationPlan::for_function_ir(&uniform).scalar_name_sets();
        assert!(int_like.contains("slot"));
    }

    #[test]
    fn generic_type_hint_does_not_seed_plan_scalar_fact() {
        let mut generic = op("call", Some("maybe_int"), None, &[]);
        generic.type_hint = Some("int".to_string());
        let func = function("generic_hint", &[], None, vec![generic]);

        let (int_like, _, _, _, _) =
            NativeRepresentationPlan::for_function_ir(&func).scalar_name_sets();

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

        let plan = NativeRepresentationPlan::for_function_ir(&func);
        let (int_like, _, float_like, _, _) = plan.scalar_name_sets();
        let integer_family = plan.integer_family_names();

        assert!(integer_family.contains("wide"));
        assert!(integer_family.contains("masked"));
        assert!(!int_like.contains("wide"));
        assert!(!float_like.contains("wide"));
        assert!(!float_like.contains("masked"));
    }
}
