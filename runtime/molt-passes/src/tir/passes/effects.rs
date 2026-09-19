//! Instance-sensitive effects analysis for TIR operations.
//!
//! Callable names and frontend receiver hints are not effect proofs. SCCP owns
//! exact-`ConstVal` call folding, while escape analysis requires an explicit
//! non-capture fact before weakening an opaque call boundary.

use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use std::collections::HashMap;

/// Operand maps passed to these instance oracles must come from
/// `type_refine::extract_exact_scalar_map`, never annotation-derived types.
pub(super) fn op_effects_with_types(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
) -> crate::tir::op_kinds_generated::OpcodeEffects {
    // A marked CheckException is not merely a local exception-status read: it
    // services pending calls/eval-breaker work and can therefore invoke Python.
    // This instance fact must dominate every opcode/type/proof refinement.
    if !op.has_valid_shape() || op.is_async_work_poll() {
        return crate::tir::op_kinds_generated::OPCODE_EFFECTS_IMPURE;
    }
    if op.opcode == OpCode::Copy {
        // Copy is also the fallback carrier for unrelated SimpleIR operations.
        // Reuse the single value-identity authority: only a shape/spelling that
        // it proves forwards one SSA value is semantically pure. Everything
        // else retains the conservative generated Copy floor.
        if super::value_identity::copy_value_source(op).is_some() {
            return crate::tir::op_kinds_generated::OPCODE_EFFECTS_PURE;
        }
        if op.attrs.contains_key("_original_kind") {
            return crate::tir::op_kinds_generated::OPCODE_EFFECTS_IMPURE;
        }
    }
    crate::tir::op_semantics::op_instance_effects_for_op(op, value_types)
}

pub(super) fn op_may_throw_with_types(op: &TirOp, value_types: &HashMap<ValueId, TirType>) -> bool {
    !op_effects_with_types(op, value_types).nothrow
}

pub(super) fn op_has_observable_effect_when_dead_with_types(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
) -> bool {
    let effects = op_effects_with_types(op, value_types);
    !effects.effect_free || !effects.nothrow
}

pub(super) fn op_is_pure_movable_with_types(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
) -> bool {
    let effects = op_effects_with_types(op, value_types);
    effects.consistent && effects.effect_free && effects.nothrow
}

/// Whether value-specific guards disprove every remaining Python exception for
/// an exact builtin operator instance. Callers supply their own range/constant
/// evidence, but opcode/type admission is shared here so no pass mistakes a
/// nonzero divisor for a complete proof of true-division or float safety.
pub(super) fn guarded_throw_condition_disproven(
    op: &TirOp,
    value_types: &HashMap<ValueId, TirType>,
    shift_count_valid: bool,
    divisor_nonzero: bool,
) -> bool {
    if !op.has_valid_shape() || op.operands.len() != 2 {
        return false;
    }
    let exact_integer_operands = op.operands.iter().all(|value| {
        matches!(
            value_types.get(value),
            Some(TirType::Bool | TirType::I64 | TirType::BigInt)
        )
    });
    if !exact_integer_operands {
        return false;
    }
    if crate::tir::op_kinds_generated::opcode_requires_i64_shift_count_guard_table(op.opcode) {
        return shift_count_valid;
    }
    matches!(op.opcode, OpCode::FloorDiv | OpCode::Mod) && divisor_nonzero
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::op_kinds_generated::{
        GvnNumberingRole, TypeRefineOperandTypeRule, opcode_accepts_operand_count,
        opcode_effects_table, opcode_gvn_numbering_role_table,
        opcode_type_refine_operand_type_rule_table,
    };

    // Unified generated operation-effect oracle.
    #[test]
    fn generated_exception_projection_agrees_for_every_opcode() {
        for &opcode in crate::tir::op_kinds_generated::ALL_OPCODES {
            assert_eq!(
                opcode_effects_table(opcode).nothrow,
                !crate::tir::op_kinds_generated::opcode_may_throw_table(opcode),
                "{opcode:?}: effect and exception consumers must share one throw fact",
            );
        }
    }

    // Generated pure-op oracle invariants.
    //
    // Opcode membership comes from op_kinds_generated::ALL_OPCODES. The table's
    // exhaustiveness is pinned by tests/test_gen_op_kinds.py and by rustc through
    // the generated wildcard-free matches; this module only verifies that the
    // consumer predicates preserve the generated effect lattice.

    fn all_opcodes() -> impl Iterator<Item = OpCode> {
        crate::tir::op_kinds_generated::ALL_OPCODES.iter().copied()
    }

    #[test]
    fn generated_gvn_numbering_roles_are_backed_by_effect_core() {
        for op in all_opcodes() {
            let role = opcode_gvn_numbering_role_table(op);
            let category = crate::tir::op_kinds_generated::opcode_predicate_semantics(op);
            if category == Some(crate::tir::op_kinds_generated::PredicateSemantics::Containment) {
                assert_eq!(role, GvnNumberingRole::Never);
                let coarse = opcode_effects_table(op);
                assert!(
                    !coarse.consistent || !coarse.effect_free,
                    "{op:?}: generic predicate must not be CSE-safe"
                );
                continue;
            }
            if role == GvnNumberingRole::Never {
                continue;
            }
            let coarse = opcode_effects_table(op);
            if coarse.consistent && coarse.effect_free {
                continue;
            }
            assert_eq!(
                role,
                GvnNumberingRole::TypeGated,
                "{op:?}: numbering without opcode-level purity requires instance proof"
            );
            let count = (1..=2)
                .find(|&count| opcode_accepts_operand_count(op, count))
                .unwrap_or_else(|| panic!("{op:?}: add a valid primitive fixture"));
            let mut operands = vec![TirType::I64; count];
            if count == 2
                && !matches!(
                    opcode_type_refine_operand_type_rule_table(op),
                    TypeRefineOperandTypeRule::IntegerBitwise
                        | TypeRefineOperandTypeRule::IntegerShift
                )
            {
                operands[1] = TirType::F64;
            }
            let facts = crate::tir::op_semantics::op_instance_facts(op, &operands)
                .unwrap_or_else(|| panic!("{op:?}: missing instance effect authority"));
            assert!(
                facts.effects.consistent && facts.effects.effect_free,
                "{op:?}: admitted primitive instance is not CSE-safe"
            );
        }
    }

    #[test]
    fn value_selection_requires_exact_left_operand_for_effect_elision() {
        use crate::tir::ops::{AttrDict, Dialect};
        for opcode in [OpCode::And, OpCode::Or] {
            assert!(
                crate::tir::op_kinds_generated::opcode_may_throw_table(opcode),
                "{opcode:?}: __bool__ can raise"
            );
            assert!(
                crate::tir::op_kinds_generated::opcode_is_side_effecting_table(opcode),
                "{opcode:?}: __bool__ can mutate"
            );
            let coarse = opcode_effects_table(opcode);
            assert!(!coarse.consistent || !coarse.effect_free);
            let op = TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: vec![ValueId(0), ValueId(1)],
                results: vec![ValueId(2)],
                attrs: AttrDict::new(),
                source_span: None,
            };
            assert!(op_may_throw_with_types(&op, &HashMap::new()));
            assert!(op_has_observable_effect_when_dead_with_types(
                &op,
                &HashMap::new()
            ));
            assert!(!op_is_pure_movable_with_types(&op, &HashMap::new()));
            let exact_left = HashMap::from([(ValueId(0), TirType::Bool)]);
            assert!(!op_may_throw_with_types(&op, &exact_left));
            assert!(!op_has_observable_effect_when_dead_with_types(
                &op,
                &exact_left
            ));
            assert!(op_is_pure_movable_with_types(&op, &exact_left));
        }
    }
}
