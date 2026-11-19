use crate::tir::cfg::CFG;
use crate::tir::dominators::SimpleProgramPoint;
use crate::tir::simple_def_use::{SimpleDefinitionFacts, SimpleDefinitionSite};
use crate::{FunctionIR, OpIR};
use molt_ir::deterministic_hash::HashSet;
use molt_ir::literal_payload::SimpleLiteral;
use molt_ir::tir::op_kinds_generated::OwnedLiteralPayloadKind;

/// Exact runtime type tags, derived from literal payloads rather than hints or
/// machine carriers. These are the builtin tags consumed by molt_guard_type.
fn literal_guard_tag(op: &OpIR) -> Option<(i64, bool)> {
    match SimpleLiteral::from_simple(op).ok().flatten()? {
        SimpleLiteral::Int(value) => {
            Some((1, !crate::tir::IntRange::point(value).fits_inline_int47()))
        }
        SimpleLiteral::Owned(OwnedLiteralPayloadKind::BigintBytes, _) => Some((1, true)),
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
    nonallocating_reads: HashSet<Vec<String>>,
}

impl RuntimeGuardFacts {
    pub fn for_function(func: &FunctionIR) -> Self {
        let mut facts = Self::default();
        if !func.ops.iter().any(|op| {
            matches!(
                op.kind,
                molt_ir::OpKind::GuardTag | molt_ir::OpKind::GuardType
            )
        }) {
            return facts;
        }
        let definitions = SimpleDefinitionFacts::compute(&func.params, &func.ops);
        let cfg = CFG::build(&func.ops);
        let dominance = cfg.execution_points(&func.ops);
        let mut invalid_tags = HashSet::default();
        let mut invalid_reads = HashSet::default();
        for (index, op) in func.ops.iter().enumerate() {
            if !matches!(
                op.kind,
                molt_ir::OpKind::GuardTag | molt_ir::OpKind::GuardType
            ) || op.var.is_some()
            {
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
        matches!(
            op.kind,
            molt_ir::OpKind::GuardTag | molt_ir::OpKind::GuardType
        ) && op.var.is_none()
            && op
                .args
                .as_deref()
                .is_some_and(|args| args.len() == 2 && self.valid_tags.contains(&args[1]))
    }
}

pub use molt_passes::tir::passes::guard_profile::SsaRuntimeGuardFacts;

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            module: None,
            name: "runtime_guard_facts".into(),
            params: vec!["dynamic_tag".into()],
            ops,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: vec![],
            execution_context: Default::default(),
        }
    }
    fn integer(name: &str, value: i64) -> OpIR {
        OpIR {
            kind: molt_ir::OpKind::Const,
            out: Some(name.into()),
            value: Some(value),
            ..OpIR::new(molt_ir::OpKind::Const)
        }
    }
    fn guard(kind: molt_ir::OpKind, expected: &str) -> OpIR {
        OpIR {
            kind,
            args: Some(vec!["source".into(), expected.into()]),
            out: Some("checked".into()),
            s_value: Some("int".into()),
            source_line: Some(37),
            ..OpIR::new(kind)
        }
    }

    #[test]
    fn runtime_guard_tag_proof_respects_exception_positions_in_tir_and_lir() {
        use crate::tir::blocks::{Terminator, TirBlock};
        use crate::tir::function::TirFunction;
        use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
        use crate::tir::types::TirType;
        for kind in [molt_ir::OpKind::GuardTag, molt_ir::OpKind::GuardType] {
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
                    origin: None,
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::CheckException,
                    operands: vec![],
                    results: vec![],
                    attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(99))]),
                    source_span: None,
                },
                TirOp {
                    origin: None,
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![tag],
                    attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(5))]),
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
                        origin: Some(kind),
                        runtime: None,
                        callee: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![source, tag],
                        results: vec![],
                        attrs: AttrDict::from_iter([]),
                        source_span: None,
                    }],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            assert!(
                !SsaRuntimeGuardFacts::for_function(&func).has_profile_only(),
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
        }
    }

    #[test]
    fn runtime_guard_admission_is_distinct_from_mismatch_and_boxing_proofs() {
        for kind in [molt_ir::OpKind::GuardTag, molt_ir::OpKind::GuardType] {
            let func = fixture(vec![
                integer("source", 7),
                integer("tag", 5),
                guard(kind, "tag"),
            ]);
            let facts = RuntimeGuardFacts::for_function(&func);
            assert!(facts.is_profile_only(&func.ops[2]));
            assert!(facts.is_nonthrowing(&func.ops[2], None));

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
                        kind: molt_ir::OpKind::ConstStr,
                        out: Some("tag".into()),
                        s_value: Some("5".into()),
                        ..OpIR::new(molt_ir::OpKind::ConstStr)
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
}
