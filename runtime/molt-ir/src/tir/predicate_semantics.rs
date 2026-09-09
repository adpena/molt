//! One operand-dependent result/effect contract for Python predicates.
//!
//! Opcode membership is generated. Unknown operands can dispatch callbacks and
//! return any owned Python object; exact scalar pairs retain their primitive
//! result and optimization facts without making those facts opcode-wide.

use std::collections::HashMap;

use super::op_kinds_generated::{
    OPCODE_EFFECTS_IMPURE, OPCODE_EFFECTS_PURE, OPCODE_EFFECTS_PURE_MAY_THROW, OpcodeEffects,
    PredicateSemantics, TypeRefineOperandTypeRule, comparison_scalar_domain,
    comparison_scalar_pair_nothrow, opcode_predicate_semantics,
    opcode_type_refine_operand_type_rule_table,
};
use super::ops::{OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

#[derive(Clone, Debug)]
pub struct PredicateFacts {
    pub result_type: TirType,
    pub effects: OpcodeEffects,
}

fn scalar_type(ty: &TirType) -> &TirType {
    match ty {
        TirType::Box(inner) => scalar_type(inner),
        _ => ty,
    }
}

pub fn predicate_facts(opcode: OpCode, operand_types: &[TirType]) -> Option<PredicateFacts> {
    facts_for_opcode(opcode, operand_types.iter())
}

fn facts_for_opcode<'a>(
    opcode: OpCode,
    mut operands: impl Iterator<Item = &'a TirType>,
) -> Option<PredicateFacts> {
    if opcode_type_refine_operand_type_rule_table(opcode) == TypeRefineOperandTypeRule::BoolSelect {
        // And/Or test only the left operand and return one operand unchanged.
        // Generated select membership joins the predicate authority here, so
        // effect passes and every type/carrier consumer use the same facts.
        let (Some(left), Some(right), None) = (operands.next(), operands.next(), operands.next())
        else {
            return Some(PredicateFacts {
                result_type: TirType::DynBox,
                effects: OPCODE_EFFECTS_IMPURE,
            });
        };
        return Some(PredicateFacts {
            result_type: left.meet(right),
            effects: if comparison_scalar_domain(scalar_type(left)).is_some() {
                OPCODE_EFFECTS_PURE
            } else {
                OPCODE_EFFECTS_IMPURE
            },
        });
    }
    Some(facts_for_operands(
        opcode_predicate_semantics(opcode)?,
        operands,
    ))
}

fn facts_for_operands<'a>(
    category: PredicateSemantics,
    mut operands: impl Iterator<Item = &'a TirType>,
) -> PredicateFacts {
    let left = operands.next();
    let right = operands.next();
    let extra = operands.next();
    if category == PredicateSemantics::Truth {
        return match (left, right) {
            (Some(operand), None) => PredicateFacts {
                result_type: TirType::Bool,
                effects: if comparison_scalar_domain(operand).is_some() {
                    OPCODE_EFFECTS_PURE
                } else {
                    OPCODE_EFFECTS_IMPURE
                },
            },
            _ => PredicateFacts {
                result_type: TirType::DynBox,
                effects: OPCODE_EFFECTS_IMPURE,
            },
        };
    }
    let (Some(left), Some(right), None) = (left, right, extra) else {
        return PredicateFacts {
            result_type: TirType::DynBox,
            effects: OPCODE_EFFECTS_IMPURE,
        };
    };
    if category == PredicateSemantics::Containment {
        return PredicateFacts {
            result_type: TirType::Bool,
            effects: OPCODE_EFFECTS_IMPURE,
        };
    }
    facts_for_pair(category, left, right)
}

fn facts_for_pair(category: PredicateSemantics, left: &TirType, right: &TirType) -> PredicateFacts {
    let left = scalar_type(left);
    let right = scalar_type(right);
    if matches!(left, TirType::Never) || matches!(right, TirType::Never) {
        return PredicateFacts {
            result_type: TirType::Never,
            effects: OPCODE_EFFECTS_IMPURE,
        };
    }
    let (Some(left), Some(right)) = (
        comparison_scalar_domain(left),
        comparison_scalar_domain(right),
    ) else {
        return PredicateFacts {
            result_type: TirType::DynBox,
            effects: OPCODE_EFFECTS_IMPURE,
        };
    };
    let nothrow = comparison_scalar_pair_nothrow(category, left, right);
    PredicateFacts {
        result_type: TirType::Bool,
        effects: if nothrow {
            OPCODE_EFFECTS_PURE
        } else {
            OPCODE_EFFECTS_PURE_MAY_THROW
        },
    }
}

pub fn predicate_facts_for_op(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
) -> Option<PredicateFacts> {
    let facts = facts_for_opcode(
        op.opcode,
        op.operands
            .iter()
            .map(|value| value_types.get(value).unwrap_or(&TirType::DynBox)),
    )?;
    if !op.has_valid_result_arity() {
        return Some(PredicateFacts {
            result_type: TirType::DynBox,
            effects: OPCODE_EFFECTS_IMPURE,
        });
    }
    Some(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::op_kinds_generated::ALL_OPCODES;

    #[test]
    fn generated_comparisons_share_scalar_and_dynamic_contracts() {
        for &opcode in ALL_OPCODES {
            let Some(category) = opcode_predicate_semantics(opcode) else {
                continue;
            };
            if matches!(
                category,
                PredicateSemantics::Truth | PredicateSemantics::Containment
            ) {
                continue;
            }
            for left in [TirType::I64, TirType::F64, TirType::Bool, TirType::BigInt] {
                let facts = predicate_facts(opcode, &[left, TirType::F64]).unwrap();
                assert_eq!(facts.result_type, TirType::Bool);
                assert_eq!(facts.effects, OPCODE_EFFECTS_PURE);
            }
            for unknown in [TirType::DynBox, TirType::UserClass("Override".into())] {
                let facts = predicate_facts(opcode, &[unknown, TirType::None]).unwrap();
                assert_eq!(facts.result_type, TirType::DynBox);
                assert_eq!(facts.effects, OPCODE_EFFECTS_IMPURE);
            }
            let facts = predicate_facts(opcode, &[TirType::None, TirType::I64]).unwrap();
            assert_eq!(facts.result_type, TirType::Bool);
            assert_eq!(
                facts.effects.nothrow,
                category == PredicateSemantics::Equality
            );
            assert!(facts.effects.consistent && facts.effects.effect_free);
            assert_eq!(
                predicate_facts(opcode, &[TirType::Never, TirType::I64])
                    .unwrap()
                    .result_type,
                TirType::Never
            );
            assert_eq!(
                predicate_facts(
                    opcode,
                    &[TirType::Box(Box::new(TirType::Str)), TirType::Str]
                )
                .unwrap()
                .effects,
                OPCODE_EFFECTS_PURE
            );
            assert_eq!(
                predicate_facts(opcode, &[]).unwrap().effects,
                OPCODE_EFFECTS_IMPURE
            );
        }
    }
    #[test]
    fn generated_value_selects_preserve_values_and_only_test_the_left_operand() {
        let mut count = 0;
        for &opcode in ALL_OPCODES {
            if opcode_type_refine_operand_type_rule_table(opcode)
                != TypeRefineOperandTypeRule::BoolSelect
            {
                continue;
            }
            count += 1;
            for ty in [TirType::Bool, TirType::I64, TirType::F64, TirType::None] {
                let facts = predicate_facts(opcode, &[ty.clone(), ty.clone()]).unwrap();
                assert_eq!(facts.result_type, ty);
                assert_eq!(facts.effects, OPCODE_EFFECTS_PURE);
            }
            let facts = predicate_facts(opcode, &[TirType::DynBox, TirType::Bool]).unwrap();
            assert_eq!(facts.result_type, TirType::DynBox);
            assert_eq!(facts.effects, OPCODE_EFFECTS_IMPURE);
            let facts = predicate_facts(opcode, &[TirType::Bool, TirType::DynBox]).unwrap();
            assert_eq!(facts.result_type, TirType::DynBox);
            assert_eq!(
                facts.effects, OPCODE_EFFECTS_PURE,
                "right value is selected, not tested"
            );
            let facts = predicate_facts(opcode, &[TirType::Bool, TirType::I64]).unwrap();
            assert_ne!(
                facts.result_type,
                TirType::Bool,
                "selection must not coerce values"
            );
            assert_eq!(
                predicate_facts(opcode, &[]).unwrap().effects,
                OPCODE_EFFECTS_IMPURE
            );
        }
        assert_eq!(count, 2);
    }
}
