use super::facts::{contains_bottom_type, is_bottom_type};
use super::hints::{parse_guard_type, parse_return_type_str, structural_builtin_return_type};
use crate::tir::op_kinds_generated::{
    TypeRefineAttrResultTypeRule, TypeRefineOperandTypeRule, opcode_accepts_shape,
    opcode_operand_independent_result_tir_type, opcode_type_refine_attr_result_type_rule_table,
    opcode_type_refine_operand_type_rule_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, builtin_call_view};
use crate::tir::types::TirType;

pub(super) fn infer_result_facts_with_attrs(
    opcode: OpCode,
    operand_facts: &[TirType],
    attrs: Option<&AttrDict>,
    result_count: usize,
) -> Vec<TirType> {
    if !opcode_accepts_shape(opcode, operand_facts.len(), result_count) {
        return vec![TirType::DynBox; result_count];
    }
    if result_count == 0 {
        return vec![];
    }

    if matches!(opcode, OpCode::TypeGuard)
        && let Some(attrs) = attrs
        && let Some(proven_ty) = parse_guard_type(attrs)
    {
        return vec![proven_ty; result_count];
    }

    let operands_ready = operand_facts.iter().all(|ty| !is_bottom_type(ty));
    let operand_types: Vec<TirType> = operand_facts.to_vec();
    infer_result_types_with_attrs(opcode, &operand_types, attrs, result_count)
        .into_iter()
        .map(|inferred| match inferred {
            Some(ty) if contains_bottom_type(&ty) => TirType::Never,
            Some(ty) => ty,
            None if operands_ready => TirType::DynBox,
            None => TirType::Never,
        })
        .collect()
}

fn tuple_index_result_type(items: &[TirType]) -> TirType {
    items
        .iter()
        .fold(TirType::Never, |acc, item| acc.meet(item))
}

fn dict_index_key_matches(dict_key_ty: &TirType, index_ty: &TirType) -> bool {
    matches!(dict_key_ty, TirType::DynBox) || dict_key_ty == index_ty
}

pub(super) fn attr_result_type_override<T>(
    opcode: OpCode,
    attrs: &AttrDict,
    operands: &[T],
) -> Option<TirType> {
    match opcode_type_refine_attr_result_type_rule_table(opcode) {
        TypeRefineAttrResultTypeRule::None => None,
        TypeRefineAttrResultTypeRule::ObjectTypeHint => match attrs.get("_type_hint") {
            Some(AttrValue::Str(name)) => match TirType::from_type_hint(name) {
                class_ty @ TirType::UserClass(_) => Some(class_ty),
                _ => None,
            },
            _ => None,
        },
        TypeRefineAttrResultTypeRule::CallReturnType => {
            attrs.get("return_type").and_then(|v| match v {
                AttrValue::Str(s) => parse_return_type_str(s.as_str()),
                _ => None,
            })
        }
        TypeRefineAttrResultTypeRule::CallBuiltinReturnType => {
            let call = builtin_call_view(opcode, attrs, operands)?;
            if call.wire_kind != "call_builtin" {
                return None;
            }
            structural_builtin_return_type(call.named_target()?)
        }
        TypeRefineAttrResultTypeRule::TypeGuard => parse_guard_type(attrs),
        TypeRefineAttrResultTypeRule::CopyOriginalKind => {
            let original_kind = match attrs.get("_original_kind") {
                Some(AttrValue::Str(k)) => Some(k.as_str()),
                _ => None,
            };
            if original_kind == Some("len") && operands.len() == 1 {
                return Some(TirType::I64);
            }
            crate::tir::passes::alias_analysis::copy_kind_raw_carrier_type(original_kind).or_else(
                || {
                    original_kind
                        .filter(|k| {
                            crate::tir::passes::alias_analysis::copy_kind_mints_fresh_owned_ref(k)
                        })
                        .map(fresh_value_kind_result_type)
                },
            )
        }
    }
}

pub(super) fn infer_result_types_with_attrs(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&AttrDict>,
    result_count: usize,
) -> Vec<Option<TirType>> {
    if result_count == 0 {
        return vec![];
    }
    if !opcode_accepts_shape(opcode, operand_types.len(), result_count) {
        return vec![None; result_count];
    }
    (0..result_count)
        .map(|index| {
            opcode_operand_independent_result_tir_type(opcode, index).or_else(|| {
                if opcode == OpCode::IterNextUnboxed && index == 0 {
                    match operand_types {
                        [TirType::Iterator(elem_ty)] => Some(elem_ty.as_ref().clone()),
                        _ => None,
                    }
                } else if result_count == 1 {
                    infer_single_result_type_with_attrs(opcode, operand_types, attrs)
                } else {
                    None
                }
            })
        })
        .collect()
}

fn infer_single_result_type_with_attrs(
    opcode: OpCode,
    operand_types: &[TirType],
    attrs: Option<&AttrDict>,
) -> Option<TirType> {
    if let Some(result_type) = crate::tir::op_semantics::op_instance_facts(opcode, operand_types)
        .and_then(|facts| facts.result_type)
    {
        return Some(result_type);
    }
    if let Some(attrs) = attrs
        && let Some(ty) = attr_result_type_override(opcode, attrs, operand_types)
    {
        return Some(ty);
    }
    match opcode_type_refine_operand_type_rule_table(opcode) {
        // Operand-dependent operator result semantics are owned by
        // op_semantics above. This match remains exhaustive so adding a new
        // generated rule cannot silently create a second classifier here.
        TypeRefineOperandTypeRule::Add
        | TypeRefineOperandTypeRule::Mul
        | TypeRefineOperandTypeRule::NumericArithmetic
        | TypeRefineOperandTypeRule::TrueDivision
        | TypeRefineOperandTypeRule::Power
        | TypeRefineOperandTypeRule::UnaryNumeric
        | TypeRefineOperandTypeRule::IntegerBitwise
        | TypeRefineOperandTypeRule::IntegerShift
        | TypeRefineOperandTypeRule::IntegerInvert => None,

        // Containers with operand-dependent element shape stay here.
        TypeRefineOperandTypeRule::BuildTuple => Some(TirType::Tuple(operand_types.to_vec())),
        TypeRefineOperandTypeRule::GetIter => match operand_types {
            [TirType::List(elem_ty) | TirType::Set(elem_ty)] => {
                Some(TirType::Iterator(Box::new(elem_ty.as_ref().clone())))
            }
            [TirType::Tuple(items)] if !items.is_empty() => {
                Some(TirType::Iterator(Box::new(tuple_index_result_type(items))))
            }
            [TirType::Dict(key_ty, _)] => {
                Some(TirType::Iterator(Box::new(key_ty.as_ref().clone())))
            }
            [TirType::Str] => Some(TirType::Iterator(Box::new(TirType::Str))),
            [TirType::Bytes] => Some(TirType::Iterator(Box::new(TirType::I64))),
            _ => None,
        },
        TypeRefineOperandTypeRule::IterNext => match operand_types {
            [TirType::Iterator(elem_ty)] => Some(elem_ty.as_ref().clone()),
            _ => None,
        },
        TypeRefineOperandTypeRule::Index => match operand_types {
            [TirType::Str, TirType::I64 | TirType::Bool] => Some(TirType::Str),
            [TirType::Bytes, TirType::I64 | TirType::Bool] => Some(TirType::I64),
            [TirType::List(elem_ty), TirType::I64 | TirType::Bool] => {
                Some(elem_ty.as_ref().clone())
            }
            [TirType::Tuple(items), TirType::I64 | TirType::Bool] if !items.is_empty() => {
                Some(tuple_index_result_type(items))
            }
            [TirType::Dict(key_ty, value_ty), index_ty]
                if dict_index_key_matches(key_ty.as_ref(), index_ty) =>
            {
                Some(value_ty.as_ref().clone())
            }
            _ => None,
        },
        // Fresh-value and raw-carrier Copy spellings are handled by the attr
        // rule before this point. The operand rule means transparent aliasing.
        TypeRefineOperandTypeRule::Copy => operand_types.first().cloned(),

        // Box/Unbox
        TypeRefineOperandTypeRule::BoxVal => operand_types
            .first()
            .map(|t| TirType::Box(Box::new(t.clone()))),
        TypeRefineOperandTypeRule::UnboxVal => match operand_types.first() {
            Some(TirType::Box(inner)) => Some(inner.as_ref().clone()),
            _ => None,
        },

        // Value-select results are owned by op_semantics above.
        TypeRefineOperandTypeRule::BoolSelect | TypeRefineOperandTypeRule::None => None,
    }
}

/// Result type of a fresh-value-minting op (one that falls back to
/// `OpCode::Copy` carrying its kind in `_original_kind` but, per
/// [`crate::tir::passes::alias_analysis::copy_kind_mints_fresh_owned_ref`],
/// constructs a NEW owned object rather than aliasing operand[0]).
///
/// The result type is intrinsic to the op, NOT operand[0]'s type. The vast
/// majority mint heap objects the TIR does not model further (`complex`, dicts,
/// lists, sets, tuples, ranges, slices, iterators, generic instances) → DynBox.
/// A handful mint a statically-known scalar/str result and are typed precisely
/// so the scalar lanes still fire on them. `int()`/`int_from_*` are intentionally
/// DynBox (may return a heap BigInt; an I64 type would license a trusted-unbox on
/// a BigInt pointer — the same carrier-soundness rule `ConstBigInt` follows).
fn fresh_value_kind_result_type(kind: &str) -> TirType {
    match kind {
        "float_from_obj" => TirType::F64,
        "str_from_obj" | "repr_from_obj" | "ascii_from_obj" | "string_format" | "string_join" => {
            TirType::Str
        }
        _ => TirType::DynBox,
    }
}
