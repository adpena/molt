use crate::tir::cfg::CFG;
use crate::tir::dominators::SimpleProgramPoint;
use crate::tir::simple_def_use::{
    SimpleDefinitionFacts, SimpleDefinitionSite, simple_ir_out_result,
};
use crate::{FunctionIR, OpIR};
use molt_ir::literal_payload::SimpleLiteral;
use molt_ir::tir::op_kinds_generated::OwnedLiteralPayloadKind;
use std::collections::HashSet;

/// Exact runtime type tags, derived from literal payloads rather than hints or
/// machine carriers. These are the builtin tags consumed by molt_guard_type.
fn literal_guard_tag(op: &OpIR) -> Option<(i64, bool)> {
    match SimpleLiteral::from_simple(op).ok().flatten()? {
        SimpleLiteral::Int(value) => {
            Some((1, !crate::tir::IntRange::point(value).fits_inline_int47()))
        }
        SimpleLiteral::Owned(OwnedLiteralPayloadKind::BigintDecimal, _) => Some((1, true)),
        SimpleLiteral::Float(_) => Some((2, false)),
        SimpleLiteral::Bool(_) => Some((3, false)),
        SimpleLiteral::None => Some((4, false)),
        SimpleLiteral::Owned(OwnedLiteralPayloadKind::String, _) => Some((5, true)),
        SimpleLiteral::Owned(OwnedLiteralPayloadKind::Bytes, _) => Some((6, true)),
    }
}

/// Positive tag admission and telemetry-elision are distinct facts. A literal
/// i64 tag is admitted by runtime `to_i64` regardless of the source's type.
/// Dynamic tags retain the runtime's broader admission and error behavior.
#[derive(Default)]
pub struct RuntimeGuardFacts {
    valid_tags: HashSet<String>,
    satisfied: HashSet<usize>,
    nonallocating_reads: HashSet<Vec<String>>,
}

impl RuntimeGuardFacts {
    pub fn for_function(func: &FunctionIR) -> Self {
        let mut facts = Self::default();
        if !func
            .ops
            .iter()
            .any(|op| matches!(op.kind.as_str(), "guard_tag" | "guard_type"))
        {
            return facts;
        }
        let definitions = SimpleDefinitionFacts::compute(&func.params, &func.ops);
        let cfg = CFG::build(&func.ops);
        let dominance = cfg.execution_points(&func.ops);
        let mut invalid_tags = HashSet::new();
        let mut invalid_reads = HashSet::new();
        for (index, op) in func.ops.iter().enumerate() {
            if !matches!(op.kind.as_str(), "guard_tag" | "guard_type") || op.var.is_some() {
                continue;
            }
            let Some(args) = op.args.as_deref().filter(|args| args.len() == 2) else {
                continue;
            };
            let producer = |name: &str| {
                let site = definitions.unique_definition(name)?;
                let SimpleDefinitionSite::Operation(producer) = site else {
                    return None;
                };
                dominance
                    .definition_available(site, SimpleProgramPoint::Before(index))
                    .then_some(producer)
            };
            let expected =
                producer(&args[1]).and_then(|source| {
                    match SimpleLiteral::from_simple(&func.ops[source])
                        .ok()
                        .flatten()?
                    {
                        SimpleLiteral::Int(value) => Some(value),
                        _ => None,
                    }
                });
            let Some(expected) = expected else {
                invalid_tags.insert(args[1].clone());
                continue;
            };
            facts.valid_tags.insert(args[1].clone());
            let Some(source) = producer(&args[0]) else {
                invalid_reads.insert(args.to_vec());
                continue;
            };
            // Literal source transport has no second allocation; an already
            // pending literal-allocation error is tracked independently by the
            // exception observer. Full-i64 integer boxing remains fallible.
            if crate::tir::IntRange::point(expected).fits_inline_int47()
                && literal_guard_tag(&func.ops[source])
                    .is_some_and(|(tag, can_fail)| tag != 1 || !can_fail)
            {
                facts.nonallocating_reads.insert(args.to_vec());
            }
            let checked_success = |source: usize| {
                func.ops
                    .get(source + 1)
                    .is_some_and(|next| next.kind == "check_exception" && next.value.is_some())
                    && dominance.definition_available(
                        SimpleDefinitionSite::Operation(source + 1),
                        SimpleProgramPoint::Before(index),
                    )
            };
            let actual = literal_guard_tag(&func.ops[source])
                .and_then(|(tag, can_fail)| (!can_fail || checked_success(source)).then_some(tag))
                .or_else(|| {
                    (func.ops[source].kind == "string_split_field" && checked_success(source))
                        .then_some(5)
                });
            if actual == Some(expected) {
                facts.satisfied.insert(index);
            }
        }
        // Name-keyed clients may visit an equal-looking guard before its tag
        // definition. Certify a name only when every guard use is dominated.
        facts.valid_tags.retain(|tag| !invalid_tags.contains(tag));
        facts
            .nonallocating_reads
            .retain(|args| facts.valid_tags.contains(&args[1]) && !invalid_reads.contains(args));
        facts
    }

    /// Include physical transport in a no-failure claim. With a concrete
    /// representation plan, only full-width raw integers can allocate boxes;
    /// before planning, certify only nonallocating literal reads.
    pub fn is_nonthrowing(
        &self,
        op: &OpIR,
        plan: Option<&crate::representation_plan::ScalarRepresentationPlan>,
    ) -> bool {
        self.is_profile_only(op)
            && !op.is_async_work_poll()
            && op.args.as_deref().is_some_and(|args| {
                plan.map_or_else(
                    || self.nonallocating_reads.contains(args),
                    |plan| args.iter().all(|name| !plan.is_full_deopt_int_name(name)),
                )
            })
    }

    /// Only profiling can make this runtime call observable. Operand boxing is
    /// separately owned and may still fail on the enabled profiling path.
    pub fn has_profile_only(&self) -> bool {
        !self.valid_tags.is_empty()
    }

    pub fn is_profile_only(&self, op: &OpIR) -> bool {
        matches!(op.kind.as_str(), "guard_tag" | "guard_type")
            && op.var.is_none()
            && op
                .args
                .as_deref()
                .is_some_and(|args| args.len() == 2 && self.valid_tags.contains(&args[1]))
    }
}

/// Whole-function SSA projection for LLVM and LIR consumers. The exact same
/// unique-definition and executable-dominance admission applies to both.
#[derive(Default)]
pub struct SsaRuntimeGuardFacts {
    valid_tags: HashSet<crate::tir::values::ValueId>,
}

impl SsaRuntimeGuardFacts {
    pub fn for_function(func: &crate::tir::function::TirFunction) -> Self {
        Self::for_graph(func)
    }

    pub fn for_lir(func: &crate::tir::lir::LirFunction) -> Self {
        Self::for_graph(func)
    }

    fn for_graph(func: &impl crate::tir::dominators::ProgramPointGraph) -> Self {
        use crate::tir::dominators::ProgramPointDominance;
        use crate::tir::ops::{AttrValue, OpCode};
        if !func
            .block_ids()
            .any(|bid| func.operations(bid).any(Self::is_guard))
        {
            return Self::default();
        }
        let mut definitions = std::collections::HashMap::new();
        let mut duplicate = HashSet::new();
        for bid in func.block_ids() {
            for arg in func.block_argument_ids(bid) {
                if definitions.insert(arg, None).is_some() {
                    duplicate.insert(arg);
                }
            }
            for (index, op) in func.operations(bid).enumerate() {
                for &result in &op.results {
                    if definitions.insert(result, Some((bid, index, op))).is_some() {
                        duplicate.insert(result);
                    }
                }
            }
        }
        let dominance = ProgramPointDominance::compute_executable_graph(func);
        let mut valid_tags = HashSet::new();
        let mut invalid_tags = duplicate;
        for bid in func.block_ids() {
            for (index, op) in func.operations(bid).enumerate() {
                if !Self::is_guard(op) {
                    continue;
                }
                let tag = op.operands[1];
                let admitted = definitions.get(&tag).copied().flatten().is_some_and(
                    |(defined, at, producer)| {
                        producer.opcode == OpCode::ConstInt
                            && producer.has_valid_shape()
                            && matches!(producer.attrs.get("value"), Some(AttrValue::Int(_)))
                            && dominance.definition_available(defined, Some(at), bid, index)
                    },
                );
                if admitted {
                    valid_tags.insert(tag);
                } else {
                    invalid_tags.insert(tag);
                }
            }
        }
        valid_tags.retain(|tag| !invalid_tags.contains(tag));
        Self { valid_tags }
    }

    fn is_guard(op: &crate::tir::ops::TirOp) -> bool {
        use crate::tir::ops::{AttrValue, OpCode};
        op.opcode == OpCode::Copy
            && op.operands.len() == 2
            && op.results.len() <= 1
            && matches!(op.attrs.get("_original_kind"), Some(AttrValue::Str(kind))
                if matches!(kind.as_str(), "guard_tag" | "guard_type"))
    }

    pub fn has_profile_only(&self) -> bool {
        !self.valid_tags.is_empty()
    }

    pub fn is_profile_only(&self, op: &crate::tir::ops::TirOp) -> bool {
        Self::is_guard(op) && self.valid_tags.contains(&op.operands[1])
    }
}

/// Counter-free elision requires the source to match, not merely a valid tag.
pub(super) fn statically_satisfied_guards(func: &FunctionIR) -> HashSet<usize> {
    RuntimeGuardFacts::for_function(func).satisfied
}

pub fn eliminate_redundant_guard_tags(func_ir: &mut FunctionIR) {
    if std::env::var("MOLT_DISABLE_GUARD_ELIM").is_ok() {
        return;
    }
    let satisfied = statically_satisfied_guards(func_ir);
    for (index, op) in func_ir.ops.iter_mut().enumerate() {
        if !satisfied.contains(&index) {
            continue;
        }
        // A discharged check still defines its optional alias result.
        let source_site = op.source_site();
        let out = simple_ir_out_result(op).map(str::to_string);
        let source = op.args.as_ref().expect("proven guard args")[0].clone();
        *op = OpIR {
            kind: if out.is_some() {
                "identity_alias"
            } else {
                "nop"
            }
            .into(),
            args: out.as_ref().map(|_| vec![source]),
            out,
            ..OpIR::default()
        };
        source_site.apply_to_op(op);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: "runtime_guard_facts".into(),
            params: vec!["dynamic_tag".into()],
            ops,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: vec![],
            execution_context: Default::default(),
        }
    }
    fn integer(name: &str, value: i64) -> OpIR {
        OpIR {
            kind: "const".into(),
            out: Some(name.into()),
            value: Some(value),
            ..OpIR::default()
        }
    }
    fn guard(kind: &str, expected: &str) -> OpIR {
        OpIR {
            kind: kind.into(),
            args: Some(vec!["source".into(), expected.into()]),
            out: Some("checked".into()),
            s_value: Some("int".into()),
            source_line: Some(37),
            ..OpIR::default()
        }
    }

    #[test]
    fn runtime_guard_tag_proof_respects_exception_positions_in_tir_and_lir() {
        use crate::tir::blocks::{Terminator, TirBlock};
        use crate::tir::function::TirFunction;
        use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
        use crate::tir::types::TirType;
        for kind in ["guard_tag", "guard_type"] {
            let mut func = TirFunction::new(
                "exception_tag_proof".into(),
                vec![TirType::DynBox],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let handler = func.fresh_block();
            let tag = func.fresh_value();
            func.label_id_map.insert(handler.0, 99);
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let source = entry.args[0].id;
            entry.ops = vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::CheckException,
                    operands: vec![],
                    results: vec![],
                    attrs: AttrDict::from([("value".into(), AttrValue::Int(99))]),
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![tag],
                    attrs: AttrDict::from([("value".into(), AttrValue::Int(5))]),
                    source_span: None,
                },
            ];
            entry.terminator = Terminator::Branch {
                target: handler,
                args: vec![],
            };
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![source, tag],
                        results: vec![],
                        attrs: AttrDict::from([(
                            "_original_kind".into(),
                            AttrValue::Str(kind.into()),
                        )]),
                        source_span: None,
                    }],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            assert!(
                !SsaRuntimeGuardFacts::for_function(&func).has_profile_only(),
                "{kind}"
            );
            let lir = crate::tir::lower_to_lir::lower_function_to_lir(&func);
            assert!(
                !SsaRuntimeGuardFacts::for_lir(&lir).has_profile_only(),
                "{kind}"
            );
            // Moving the literal above the observation establishes it on both entries.
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .swap(0, 1);
            assert!(
                SsaRuntimeGuardFacts::for_function(&func).has_profile_only(),
                "{kind}"
            );
            let lir = crate::tir::lower_to_lir::lower_function_to_lir(&func);
            assert!(
                SsaRuntimeGuardFacts::for_lir(&lir).has_profile_only(),
                "{kind}"
            );
        }
    }

    #[test]
    fn runtime_guard_admission_is_distinct_from_mismatch_and_boxing_proofs() {
        for kind in ["guard_tag", "guard_type"] {
            let func = fixture(vec![
                integer("source", 7),
                integer("tag", 5),
                guard(kind, "tag"),
            ]);
            let facts = RuntimeGuardFacts::for_function(&func);
            assert!(facts.is_profile_only(&func.ops[2]));
            assert!(facts.is_nonthrowing(&func.ops[2], None));
            assert!(
                facts.satisfied.is_empty(),
                "mismatch still counts with profiling enabled"
            );

            let wide = fixture(vec![
                integer("source", i64::MAX),
                integer("tag", 5),
                guard(kind, "tag"),
            ]);
            let facts = RuntimeGuardFacts::for_function(&wide);
            assert!(facts.is_profile_only(&wide.ops[2]));
            assert!(
                !facts.is_nonthrowing(&wide.ops[2], None),
                "physical boxing can fail"
            );

            for ops in [
                vec![
                    integer("source", 7),
                    guard(kind, "tag"),
                    integer("tag", 5),
                    guard(kind, "tag"),
                ],
                vec![integer("source", 7), guard(kind, "dynamic_tag")],
                vec![
                    integer("source", 7),
                    OpIR {
                        kind: "const_str".into(),
                        out: Some("tag".into()),
                        s_value: Some("5".into()),
                        ..OpIR::default()
                    },
                    guard(kind, "tag"),
                ],
            ] {
                let invalid = fixture(ops);
                let facts = RuntimeGuardFacts::for_function(&invalid);
                assert!(!facts.has_profile_only());
            }
            let predefinition = fixture(vec![
                integer("tag", 5),
                guard(kind, "tag"),
                integer("source", 7),
                guard(kind, "tag"),
            ]);
            let facts = RuntimeGuardFacts::for_function(&predefinition);
            assert!(!facts.is_nonthrowing(&predefinition.ops[1], None));
            assert!(!facts.is_nonthrowing(&predefinition.ops[3], None));
        }
    }

    #[test]
    fn runtime_guard_satisfaction_compares_both_reads_and_preserves_alias_result() {
        for kind in ["guard_tag", "guard_type"] {
            for expected in [1, 2, 3, 5] {
                let mut func = fixture(vec![
                    integer("source", 11),
                    integer("tag", expected),
                    guard(kind, "tag"),
                ]);
                eliminate_redundant_guard_tags(&mut func);
                assert_eq!(
                    func.ops[2].kind,
                    if expected == 1 {
                        "identity_alias"
                    } else {
                        kind
                    }
                );
                assert_eq!(func.ops[2].out.as_deref(), Some("checked"));
                assert_eq!(func.ops[2].source_line, Some(37));
                if expected == 1 {
                    assert_eq!(
                        func.ops[2].args.as_deref(),
                        Some(["source".into()].as_slice())
                    );
                }
            }
            for ops in [
                vec![integer("source", 11), guard(kind, "dynamic_tag")],
                vec![integer("source", 11), guard(kind, "tag"), integer("tag", 1)],
                vec![
                    integer("source", 11),
                    integer("tag", 1),
                    integer("tag", 5),
                    guard(kind, "tag"),
                ],
                vec![
                    integer("source", 11),
                    OpIR {
                        kind: "jump".into(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    integer("tag", 1),
                    OpIR {
                        kind: "label".into(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    guard(kind, "tag"),
                ],
                vec![
                    OpIR {
                        kind: "add".into(),
                        out: Some("source".into()),
                        args: Some(vec!["dynamic_tag".into(), "dynamic_tag".into()]),
                        type_hint: Some("int".into()),
                        ..OpIR::default()
                    },
                    integer("tag", 1),
                    guard(kind, "tag"),
                ],
            ] {
                let mut func = fixture(ops);
                assert!(statically_satisfied_guards(&func).is_empty());
                eliminate_redundant_guard_tags(&mut func);
                assert!(func.ops.iter().any(|op| op.kind == kind));
            }
            let op = guard(kind, "dynamic_tag");
            assert!(!crate::passes::simple_ir_op_is_provably_nonthrowing_with_facts(None, &op));
        }
    }
}
