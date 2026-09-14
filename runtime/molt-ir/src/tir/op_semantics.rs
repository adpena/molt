//! Operand-dependent semantic and effect facts for TIR operations.
//!
//! Coarse opcode effects are deliberately conservative for operations that may
//! dispatch Python callbacks. Exact builtin scalar provenance can recover a
//! stronger instance fact here. Semantic result type is independent of physical
//! representation and never licenses raw storage by itself.

use std::collections::HashMap;

use super::op_kinds_generated::{
    OPCODE_EFFECTS_IMPURE, OPCODE_EFFECTS_PURE, OpcodeEffects, PredicateSemantics,
    TypeRefineOperandTypeRule, comparison_scalar_domain, comparison_scalar_pair_effects,
    opcode_accepts_operand_count, opcode_effects_table, opcode_predicate_semantics,
    opcode_primitive_effects_table, opcode_type_refine_operand_type_rule_table,
};
use super::ops::{OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

static DYNAMIC_OPERAND_TYPE: TirType = TirType::DynBox;

trait OperandTypes {
    fn len(&self) -> usize;
    fn get(&self, index: usize) -> Option<&TirType>;
}

impl OperandTypes for [TirType] {
    fn len(&self) -> usize {
        <[TirType]>::len(self)
    }

    fn get(&self, index: usize) -> Option<&TirType> {
        <[TirType]>::get(self, index)
    }
}

struct OpOperandTypes<'a> {
    operands: &'a [ValueId],
    value_types: &'a HashMap<ValueId, TirType>,
}

impl OperandTypes for OpOperandTypes<'_> {
    fn len(&self) -> usize {
        self.operands.len()
    }

    fn get(&self, index: usize) -> Option<&TirType> {
        self.operands
            .get(index)
            .map(|value| self.value_types.get(value).unwrap_or(&DYNAMIC_OPERAND_TYPE))
    }
}

#[derive(Clone, Debug)]
pub struct OpInstanceFacts {
    pub result_type: Option<TirType>,
    pub effects: OpcodeEffects,
}

fn impure(result_type: Option<TirType>) -> OpInstanceFacts {
    OpInstanceFacts {
        result_type,
        effects: OPCODE_EFFECTS_IMPURE,
    }
}

fn facts(result_type: Option<TirType>, effects: OpcodeEffects) -> OpInstanceFacts {
    OpInstanceFacts {
        result_type,
        effects,
    }
}

fn binary_operands<T: OperandTypes + ?Sized>(operand_types: &T) -> Option<(&TirType, &TirType)> {
    if operand_types.len() != 2 {
        return None;
    }
    Some((operand_types.get(0)?, operand_types.get(1)?))
}

fn unary_operand<T: OperandTypes + ?Sized>(operand_types: &T) -> Option<&TirType> {
    if operand_types.len() != 1 {
        return None;
    }
    operand_types.get(0)
}

fn infer_numeric_arithmetic<T: OperandTypes + ?Sized>(operand_types: &T) -> Option<TirType> {
    let (left, right) = binary_operands(operand_types)?;
    match (numeric_result_type(left)?, numeric_result_type(right)?) {
        (TirType::F64, _) | (_, TirType::F64) => Some(TirType::F64),
        _ => Some(TirType::I64),
    }
}

fn numeric_result_type(ty: &TirType) -> Option<TirType> {
    match ty.semantic_type() {
        TirType::Bool | TirType::I64 | TirType::BigInt => Some(TirType::I64),
        TirType::F64 => Some(TirType::F64),
        _ => None,
    }
}

fn sequence_repetition_type(sequence: &TirType, count: &TirType) -> Option<TirType> {
    if numeric_result_type(count) != Some(TirType::I64) {
        return None;
    }
    match sequence.semantic_type() {
        TirType::Str => Some(TirType::Str),
        TirType::Bytes => Some(TirType::Bytes),
        _ => None,
    }
}

fn operator_result_type<T: OperandTypes + ?Sized>(
    rule: TypeRefineOperandTypeRule,
    operand_types: &T,
) -> Option<TirType> {
    // Bottom is an unresolved SSA input, not evidence of a dynamic callback.
    // Preserve it until the producer is visited, regardless of block numbering.
    // Effects remain conservative while any operand is unresolved.
    if (0..operand_types.len()).any(|index| {
        operand_types
            .get(index)
            .is_some_and(|ty| ty.semantic_type() == &TirType::Never)
    }) {
        return Some(TirType::Never);
    }
    match rule {
        TypeRefineOperandTypeRule::Add => {
            let (left, right) = binary_operands(operand_types)?;
            match (left.semantic_type(), right.semantic_type()) {
                (TirType::Str, TirType::Str) => Some(TirType::Str),
                (TirType::Bytes, TirType::Bytes) => Some(TirType::Bytes),
                _ => infer_numeric_arithmetic(operand_types),
            }
        }
        TypeRefineOperandTypeRule::Mul => {
            let (left, right) = binary_operands(operand_types)?;
            sequence_repetition_type(left, right)
                .or_else(|| sequence_repetition_type(right, left))
                .or_else(|| infer_numeric_arithmetic(operand_types))
        }
        TypeRefineOperandTypeRule::NumericArithmetic => infer_numeric_arithmetic(operand_types),
        TypeRefineOperandTypeRule::TrueDivision => {
            infer_numeric_arithmetic(operand_types).map(|_| TirType::F64)
        }
        // Python powers can change result family with operand values (negative
        // exponent -> float; negative base and fractional exponent -> complex).
        TypeRefineOperandTypeRule::Power => None,
        TypeRefineOperandTypeRule::UnaryNumeric => {
            numeric_result_type(unary_operand(operand_types)?)
        }
        TypeRefineOperandTypeRule::IntegerBitwise | TypeRefineOperandTypeRule::IntegerShift => {
            let (left, right) = binary_operands(operand_types)?;
            if rule == TypeRefineOperandTypeRule::IntegerBitwise
                && left.semantic_type() == &TirType::Bool
                && right.semantic_type() == &TirType::Bool
            {
                Some(TirType::Bool)
            } else {
                infer_numeric_arithmetic(operand_types).filter(|ty| ty == &TirType::I64)
            }
        }
        TypeRefineOperandTypeRule::IntegerInvert => {
            numeric_result_type(unary_operand(operand_types)?).filter(|ty| ty == &TirType::I64)
        }
        _ => None,
    }
}

fn primitive_operator_effects<T: OperandTypes + ?Sized>(
    opcode: OpCode,
    operand_types: &T,
) -> Option<OpcodeEffects> {
    match operand_types.len() {
        1 => {
            let operands = [operand_types.get(0)?];
            opcode_primitive_effects_table(opcode, &operands)
        }
        2 => {
            let operands = [operand_types.get(0)?, operand_types.get(1)?];
            opcode_primitive_effects_table(opcode, &operands)
        }
        // Generated operator admission is fail-closed for malformed arity.
        _ => opcode_primitive_effects_table(opcode, &[]),
    }
}

fn predicate_instance_facts<T: OperandTypes + ?Sized>(
    opcode: OpCode,
    operand_types: &T,
) -> Option<OpInstanceFacts> {
    if opcode_type_refine_operand_type_rule_table(opcode) == TypeRefineOperandTypeRule::BoolSelect {
        let Some((left, right)) = binary_operands(operand_types) else {
            return Some(impure(Some(TirType::DynBox)));
        };
        return Some(facts(
            Some(left.meet(right)),
            if comparison_scalar_domain(left).is_some() {
                OPCODE_EFFECTS_PURE
            } else {
                OPCODE_EFFECTS_IMPURE
            },
        ));
    }
    let category = opcode_predicate_semantics(opcode)?;
    if category == PredicateSemantics::Truth {
        return Some(match unary_operand(operand_types) {
            Some(operand) => facts(
                Some(TirType::Bool),
                if comparison_scalar_domain(operand).is_some() {
                    OPCODE_EFFECTS_PURE
                } else {
                    OPCODE_EFFECTS_IMPURE
                },
            ),
            _ => impure(Some(TirType::DynBox)),
        });
    }
    let Some((left, right)) = binary_operands(operand_types) else {
        return Some(impure(Some(TirType::DynBox)));
    };
    if category == PredicateSemantics::Containment {
        return Some(impure(Some(TirType::Bool)));
    }
    if matches!(left.semantic_type(), TirType::Never)
        || matches!(right.semantic_type(), TirType::Never)
    {
        return Some(impure(Some(TirType::Never)));
    }
    if comparison_scalar_domain(left).is_none() || comparison_scalar_domain(right).is_none() {
        return Some(impure(Some(TirType::DynBox)));
    }
    Some(facts(
        Some(TirType::Bool),
        comparison_scalar_pair_effects(category, left, right),
    ))
}

fn op_instance_facts_from_types<T: OperandTypes + ?Sized>(
    opcode: OpCode,
    operand_types: &T,
) -> Option<OpInstanceFacts> {
    if !opcode_accepts_operand_count(opcode, operand_types.len()) {
        return Some(impure(None));
    }
    if let Some(facts) = predicate_instance_facts(opcode, operand_types) {
        return Some(facts);
    }
    let rule = opcode_type_refine_operand_type_rule_table(opcode);
    if !matches!(
        rule,
        TypeRefineOperandTypeRule::Add
            | TypeRefineOperandTypeRule::Mul
            | TypeRefineOperandTypeRule::NumericArithmetic
            | TypeRefineOperandTypeRule::TrueDivision
            | TypeRefineOperandTypeRule::Power
            | TypeRefineOperandTypeRule::UnaryNumeric
            | TypeRefineOperandTypeRule::IntegerBitwise
            | TypeRefineOperandTypeRule::IntegerShift
            | TypeRefineOperandTypeRule::IntegerInvert
    ) {
        return None;
    }
    Some(facts(
        operator_result_type(rule, operand_types),
        primitive_operator_effects(opcode, operand_types).unwrap_or(OPCODE_EFFECTS_IMPURE),
    ))
}

/// Compute builtin operation semantics under exact operand provenance.
/// An annotation or an isinstance-style guard can admit overriding subclasses
/// and must never supply facts used to elide callbacks or exceptions here.
pub fn op_instance_facts(opcode: OpCode, operand_types: &[TirType]) -> Option<OpInstanceFacts> {
    op_instance_facts_from_types(opcode, operand_types)
}

/// Borrow exact SSA operand facts without allocating an operand-type vector.
/// The map must exclude annotation-only or subclass-admitting type hints.
pub fn op_instance_facts_for_op(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
) -> Option<OpInstanceFacts> {
    // Validate before optional classifier dispatch: even an otherwise pure
    // opcode outside the primitive family must not hide malformed operations.
    if !op.has_valid_shape() {
        return Some(impure(None));
    }
    let operand_types = OpOperandTypes {
        operands: &op.operands,
        value_types,
    };
    op_instance_facts_from_types(op.opcode, &operand_types)
}

pub fn op_instance_effects_for_op(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
) -> OpcodeEffects {
    op_instance_facts_for_op(op, value_types)
        .map_or_else(|| opcode_effects_table(op.opcode), |facts| facts.effects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::op_kinds_generated::{ALL_OPCODES, OPCODE_EFFECTS_PURE_MAY_THROW};
    use crate::tir::ops::{AttrDict, Dialect};

    fn boxed(ty: &TirType, depth: usize) -> TirType {
        (0..depth).fold(ty.clone(), |inner, _| TirType::Box(Box::new(inner)))
    }

    #[test]
    fn numeric_result_and_effect_facts_share_all_scalar_storage_forms() {
        let numeric = [TirType::Bool, TirType::I64, TirType::BigInt, TirType::F64];
        for left in &numeric {
            for right in &numeric {
                let result = if left == &TirType::F64 || right == &TirType::F64 {
                    TirType::F64
                } else {
                    TirType::I64
                };
                let converts_unbounded_int = (left == &TirType::F64
                    && matches!(right, TirType::I64 | TirType::BigInt))
                    || (right == &TirType::F64 && matches!(left, TirType::I64 | TirType::BigInt));
                for depth_left in 0..=2 {
                    for depth_right in 0..=2 {
                        let operands = [boxed(left, depth_left), boxed(right, depth_right)];
                        for opcode in [
                            OpCode::Add,
                            OpCode::Sub,
                            OpCode::Mul,
                            OpCode::InplaceAdd,
                            OpCode::InplaceSub,
                            OpCode::InplaceMul,
                            OpCode::Div,
                            OpCode::FloorDiv,
                            OpCode::Mod,
                            OpCode::Pow,
                        ] {
                            let facts = op_instance_facts(opcode, &operands).unwrap();
                            let value_dependent = matches!(
                                opcode,
                                OpCode::Div | OpCode::FloorDiv | OpCode::Mod | OpCode::Pow
                            );
                            let expected_result = match opcode {
                                OpCode::Div => Some(TirType::F64),
                                OpCode::Pow => None,
                                _ => Some(result.clone()),
                            };
                            assert_eq!(
                                facts.result_type, expected_result,
                                "{opcode:?} {operands:?}"
                            );
                            assert_eq!(
                                facts.effects,
                                if converts_unbounded_int || value_dependent {
                                    OPCODE_EFFECTS_PURE_MAY_THROW
                                } else {
                                    OPCODE_EFFECTS_PURE
                                },
                                "{opcode:?} {operands:?}"
                            );
                        }
                    }
                }
            }
            for depth in 0..=2 {
                for opcode in [OpCode::Neg, OpCode::Pos] {
                    let facts = op_instance_facts(opcode, &[boxed(left, depth)]).unwrap();
                    assert_eq!(
                        facts.result_type,
                        Some(if left == &TirType::F64 {
                            TirType::F64
                        } else {
                            TirType::I64
                        })
                    );
                    assert_eq!(facts.effects, OPCODE_EFFECTS_PURE);
                }
            }
        }
    }

    #[test]
    fn unresolved_operator_inputs_keep_bottom_without_admitting_pure_effects() {
        for &opcode in ALL_OPCODES {
            let Some(count) = (1..=2).find(|&count| opcode_accepts_operand_count(opcode, count))
            else {
                continue;
            };
            if opcode_predicate_semantics(opcode).is_some()
                || opcode_type_refine_operand_type_rule_table(opcode)
                    == TypeRefineOperandTypeRule::BoolSelect
            {
                continue;
            }
            // Only operations owned by this instance oracle participate.
            let concrete = vec![TirType::I64; count];
            if op_instance_facts(opcode, &concrete).is_none() {
                continue;
            }
            for index in 0..count {
                let mut unresolved = concrete.clone();
                unresolved[index] = boxed(&TirType::Never, 2);
                let facts = op_instance_facts(opcode, &unresolved).unwrap();
                assert_eq!(facts.result_type, Some(TirType::Never), "{opcode:?}");
                assert_eq!(facts.effects, OPCODE_EFFECTS_IMPURE, "{opcode:?}");
            }
        }
    }

    #[test]
    fn sequence_results_preserve_length_overflow_and_reject_callbacks() {
        for sequence in [TirType::Str, TirType::Bytes] {
            for depth in 0..=2 {
                let value = boxed(&sequence, depth);
                for opcode in [OpCode::Add, OpCode::InplaceAdd] {
                    let facts = op_instance_facts(opcode, &[value.clone(), value.clone()]).unwrap();
                    assert_eq!(facts.result_type, Some(sequence.clone()));
                    assert_eq!(facts.effects, OPCODE_EFFECTS_PURE_MAY_THROW);
                }
                for count in [
                    TirType::Bool,
                    TirType::I64,
                    TirType::BigInt,
                    TirType::F64,
                    TirType::DynBox,
                    TirType::UserClass("IndexOverride".into()),
                ] {
                    for operands in [
                        [value.clone(), boxed(&count, depth)],
                        [boxed(&count, depth), value.clone()],
                    ] {
                        for opcode in [OpCode::Mul, OpCode::InplaceMul] {
                            let facts = op_instance_facts(opcode, &operands).unwrap();
                            let exact_count =
                                matches!(count, TirType::Bool | TirType::I64 | TirType::BigInt);
                            assert_eq!(facts.result_type, exact_count.then(|| sequence.clone()));
                            assert_eq!(
                                facts.effects,
                                if exact_count {
                                    OPCODE_EFFECTS_PURE_MAY_THROW
                                } else {
                                    OPCODE_EFFECTS_IMPURE
                                },
                                "{opcode:?} {operands:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn integer_operations_preserve_bool_results_only_for_boolean_bitwise_pairs() {
        for left in [TirType::Bool, TirType::I64, TirType::BigInt] {
            for right in [TirType::Bool, TirType::I64, TirType::BigInt] {
                for depth in 0..=2 {
                    for opcode in [
                        OpCode::BitAnd,
                        OpCode::BitOr,
                        OpCode::BitXor,
                        OpCode::Shl,
                        OpCode::Shr,
                    ] {
                        let shift = matches!(opcode, OpCode::Shl | OpCode::Shr);
                        let facts =
                            op_instance_facts(opcode, &[boxed(&left, depth), boxed(&right, depth)])
                                .unwrap();
                        assert_eq!(
                            facts.result_type,
                            Some(
                                if !shift && left == TirType::Bool && right == TirType::Bool {
                                    TirType::Bool
                                } else {
                                    TirType::I64
                                }
                            )
                        );
                        assert_eq!(
                            facts.effects,
                            if shift {
                                OPCODE_EFFECTS_PURE_MAY_THROW
                            } else {
                                OPCODE_EFFECTS_PURE
                            }
                        );
                    }
                }
            }
            for depth in 0..=2 {
                let facts = op_instance_facts(OpCode::BitNot, &[boxed(&left, depth)]).unwrap();
                assert_eq!(facts.result_type, Some(TirType::I64));
                assert_eq!(
                    facts.effects,
                    if left == TirType::Bool {
                        OPCODE_EFFECTS_IMPURE // CPython version-dependent warning/error.
                    } else {
                        OPCODE_EFFECTS_PURE
                    }
                );
            }
        }
    }

    #[test]
    fn dynamic_and_malformed_overloaded_operators_are_observable() {
        let binary = [
            OpCode::Add,
            OpCode::Sub,
            OpCode::Mul,
            OpCode::InplaceAdd,
            OpCode::InplaceSub,
            OpCode::InplaceMul,
            OpCode::Div,
            OpCode::FloorDiv,
            OpCode::Mod,
            OpCode::Pow,
            OpCode::BitAnd,
            OpCode::BitOr,
            OpCode::BitXor,
            OpCode::Shl,
            OpCode::Shr,
        ];
        for opcode in binary {
            for operands in [
                vec![TirType::DynBox, TirType::DynBox],
                vec![
                    TirType::UserClass("Left".into()),
                    TirType::UserClass("Right".into()),
                ],
                vec![TirType::I64],
                vec![TirType::I64, TirType::I64, TirType::I64],
                vec![],
            ] {
                let facts = op_instance_facts(opcode, &operands).unwrap();
                assert_eq!(
                    facts.effects, OPCODE_EFFECTS_IMPURE,
                    "{opcode:?} {operands:?}"
                );
            }
        }
        for opcode in [OpCode::Neg, OpCode::Pos, OpCode::BitNot] {
            for operands in [
                vec![TirType::DynBox],
                vec![TirType::UserClass("Operand".into())],
                vec![TirType::I64, TirType::I64],
                vec![],
            ] {
                let facts = op_instance_facts(opcode, &operands).unwrap();
                assert_eq!(
                    facts.effects, OPCODE_EFFECTS_IMPURE,
                    "{opcode:?} {operands:?}"
                );
            }
        }

        let malformed = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        };
        let exact = HashMap::from([(ValueId(0), TirType::I64), (ValueId(1), TirType::I64)]);
        let malformed_facts = op_instance_facts_for_op(&malformed, &exact).unwrap();
        assert_eq!(malformed_facts.result_type, None);
        assert_eq!(malformed_facts.effects, OPCODE_EFFECTS_IMPURE);
    }

    #[test]
    fn every_malformed_declared_operation_shape_is_observable() {
        for &opcode in ALL_OPCODES {
            for operands in 0..=4 {
                for results in 0..=3 {
                    if crate::tir::op_kinds_generated::opcode_accepts_shape(
                        opcode, operands, results,
                    ) {
                        continue;
                    }
                    let op = TirOp {
                        dialect: Dialect::Molt,
                        opcode,
                        operands: (0..operands).map(|index| ValueId(index as u32)).collect(),
                        results: (0..results)
                            .map(|index| ValueId(10 + index as u32))
                            .collect(),
                        attrs: AttrDict::new(),
                        source_span: None,
                    };
                    let exact = op
                        .operands
                        .iter()
                        .map(|&value| (value, TirType::I64))
                        .collect();
                    let facts = op_instance_facts_for_op(&op, &exact)
                        .expect("malformed shape is an instance fact");
                    assert_eq!(
                        facts.result_type, None,
                        "{opcode:?}: {operands} -> {results}"
                    );
                    assert_eq!(facts.effects, OPCODE_EFFECTS_IMPURE);
                    assert_eq!(
                        op_instance_effects_for_op(&op, &exact),
                        OPCODE_EFFECTS_IMPURE
                    );
                }
            }
        }
    }

    #[test]
    fn exact_builtin_effects_keep_value_dependent_exceptions() {
        for opcode in [OpCode::Add, OpCode::Sub, OpCode::Mul] {
            assert_eq!(
                op_instance_facts(opcode, &[TirType::I64, TirType::I64])
                    .unwrap()
                    .effects,
                OPCODE_EFFECTS_PURE
            );
            assert_eq!(
                op_instance_facts(opcode, &[TirType::I64, TirType::F64])
                    .unwrap()
                    .effects,
                OPCODE_EFFECTS_PURE_MAY_THROW
            );
        }
        for opcode in [
            OpCode::Div,
            OpCode::FloorDiv,
            OpCode::Mod,
            OpCode::Pow,
            OpCode::Shl,
            OpCode::Shr,
        ] {
            assert_eq!(
                op_instance_facts(opcode, &[TirType::I64, TirType::I64])
                    .unwrap()
                    .effects,
                OPCODE_EFFECTS_PURE_MAY_THROW,
                "{opcode:?}"
            );
        }
        assert_eq!(
            op_instance_facts(OpCode::BitNot, &[TirType::Bool])
                .unwrap()
                .effects,
            OPCODE_EFFECTS_IMPURE
        );
        assert_eq!(
            op_instance_facts(OpCode::BitNot, &[TirType::I64])
                .unwrap()
                .effects,
            OPCODE_EFFECTS_PURE
        );
    }

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
                let facts = op_instance_facts(opcode, &[left, TirType::F64]).unwrap();
                assert_eq!(facts.result_type, Some(TirType::Bool));
                assert_eq!(facts.effects, OPCODE_EFFECTS_PURE);
            }
            for unknown in [TirType::DynBox, TirType::UserClass("Override".into())] {
                let facts = op_instance_facts(opcode, &[unknown, TirType::None]).unwrap();
                assert_eq!(facts.result_type, Some(TirType::DynBox));
                assert_eq!(facts.effects, OPCODE_EFFECTS_IMPURE);
            }
            let facts = op_instance_facts(opcode, &[TirType::None, TirType::I64]).unwrap();
            assert_eq!(facts.result_type, Some(TirType::Bool));
            assert_eq!(
                facts.effects.nothrow,
                category == PredicateSemantics::Equality
            );
            assert!(facts.effects.consistent && facts.effects.effect_free);
            assert_eq!(
                op_instance_facts(opcode, &[TirType::Never, TirType::I64])
                    .unwrap()
                    .result_type,
                Some(TirType::Never)
            );
            assert_eq!(
                op_instance_facts(
                    opcode,
                    &[TirType::Box(Box::new(TirType::Str)), TirType::Str]
                )
                .unwrap()
                .effects,
                OPCODE_EFFECTS_PURE
            );
            assert_eq!(
                op_instance_facts(opcode, &[]).unwrap().effects,
                OPCODE_EFFECTS_IMPURE
            );
        }
    }

    #[test]
    fn bytes_equality_warning_pairs_remain_observable() {
        for opcode in [OpCode::Eq, OpCode::Ne] {
            for other in [TirType::Str, TirType::I64, TirType::BigInt, TirType::Bool] {
                for operands in [[TirType::Bytes, other.clone()], [other, TirType::Bytes]] {
                    let effects = op_instance_facts(opcode, &operands).unwrap().effects;
                    assert_eq!(
                        effects, OPCODE_EFFECTS_IMPURE,
                        "{opcode:?} {operands:?}: BytesWarning is observable"
                    );
                }
            }
            for other in [TirType::F64, TirType::None, TirType::Bytes] {
                let effects = op_instance_facts(opcode, &[TirType::Bytes, other])
                    .unwrap()
                    .effects;
                assert_eq!(effects, OPCODE_EFFECTS_PURE, "{opcode:?}");
            }
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
                let facts = op_instance_facts(opcode, &[ty.clone(), ty.clone()]).unwrap();
                assert_eq!(facts.result_type, Some(ty));
                assert_eq!(facts.effects, OPCODE_EFFECTS_PURE);
            }
            let facts = op_instance_facts(opcode, &[TirType::DynBox, TirType::Bool]).unwrap();
            assert_eq!(facts.result_type, Some(TirType::DynBox));
            assert_eq!(facts.effects, OPCODE_EFFECTS_IMPURE);
            let facts = op_instance_facts(opcode, &[TirType::Bool, TirType::DynBox]).unwrap();
            assert_eq!(facts.result_type, Some(TirType::DynBox));
            assert_eq!(
                facts.effects, OPCODE_EFFECTS_PURE,
                "right value is selected, not tested"
            );
            let facts = op_instance_facts(opcode, &[TirType::Bool, TirType::I64]).unwrap();
            assert_ne!(
                facts.result_type,
                Some(TirType::Bool),
                "selection must not coerce values"
            );
            assert_eq!(
                op_instance_facts(opcode, &[]).unwrap().effects,
                OPCODE_EFFECTS_IMPURE
            );
        }
        assert_eq!(count, 2);
    }
}
