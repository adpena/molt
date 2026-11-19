//! Profile-only guards on final TIR (`GUARD_PROFILE_ONLY_ATTR`). A `GuardType`
//! or `GuardTag` whose tag operand is a uniquely defined, dominating integer
//! literal only profiles on a mismatch; this pass marks each such guard once,
//! after drop insertion, so every native backend reads the fact on the op and
//! none re-derives it.

use molt_ir::deterministic_hash::HashSet;

use super::PassStats;

/// Mark the profile-only guards (`mark_profile_only_guards`).
pub fn run(func: &mut crate::tir::function::TirFunction) -> PassStats {
    let changed = mark_profile_only_guards(func);
    PassStats {
        name: "guard_profile_marking",
        attrs_changed: changed,
        ..Default::default()
    }
}

/// Whole-function SSA projection of the guard-tag proof. The SimpleIR twin
/// (`molt_tir::passes::RuntimeGuardFacts`) applies the same unique-definition
/// and executable-dominance admission until Cranelift reads final TIR.
#[derive(Default)]
struct SsaRuntimeGuardFacts {
    valid_tags: HashSet<crate::tir::values::ValueId>,
}

impl SsaRuntimeGuardFacts {
    fn for_function(func: &crate::tir::function::TirFunction) -> Self {
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
        let mut definitions = molt_ir::deterministic_hash::HashMap::default();
        let mut duplicate = HashSet::default();
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
        let mut valid_tags = HashSet::default();
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
        use crate::tir::ops::OpCode;
        op.opcode == OpCode::Copy
            && op.operands.len() == 2
            && op.results.len() <= 1
            && matches!(op.origin, Some(kind)
                if matches!(kind, molt_ir::OpKind::GuardTag | molt_ir::OpKind::GuardType))
    }

    fn is_profile_only(&self, op: &crate::tir::ops::TirOp) -> bool {
        Self::is_guard(op) && self.valid_tags.contains(&op.operands[1])
    }
}

/// Mark each profile-only guard of final TIR with `GUARD_PROFILE_ONLY_ATTR`
/// and clear a stale mark from every other op, so the one whole-function
/// fact reaches every backend on the op itself. Answers how many marks
/// changed.
pub fn mark_profile_only_guards(func: &mut crate::tir::function::TirFunction) -> usize {
    use crate::tir::ops::{AttrValue, GUARD_PROFILE_ONLY_ATTR};
    let facts = SsaRuntimeGuardFacts::for_function(func);
    let mut changed = 0;
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            let marked = matches!(
                op.attrs.get(GUARD_PROFILE_ONLY_ATTR),
                Some(AttrValue::Bool(true))
            );
            let profile_only = facts.is_profile_only(op);
            if profile_only && !marked {
                op.attrs
                    .insert(GUARD_PROFILE_ONLY_ATTR.to_owned(), AttrValue::Bool(true));
                changed += 1;
            } else if !profile_only && op.attrs.remove(GUARD_PROFILE_ONLY_ATTR).is_some() {
                changed += 1;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_profile_marks_require_dominance_on_exception_edges() {
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
            assert_eq!(mark_profile_only_guards(&mut func), 0, "{kind}");
            // Moving the literal above the observation establishes it on both entries.
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .swap(0, 1);
            assert_eq!(mark_profile_only_guards(&mut func), 1, "{kind}");
            assert!(
                matches!(
                    func.blocks[&handler].ops[0]
                        .attrs
                        .get(crate::tir::ops::GUARD_PROFILE_ONLY_ATTR),
                    Some(AttrValue::Bool(true))
                ),
                "{kind}"
            );
        }
    }
}
