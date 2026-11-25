use super::facts::{contains_bottom_type, is_bottom_type};
use super::hints::parse_return_type_str;
use crate::tir::op_kinds_generated::{
    TypeRefineAttrResultTypeRule, TypeRefineOperandTypeRule, opcode_accepts_shape,
    opcode_operand_independent_result_tir_type, opcode_type_refine_attr_result_type_rule_table,
    opcode_type_refine_operand_type_rule_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::types::TirType;

pub(super) fn infer_result_facts_with_attrs(
    opcode: OpCode,
    origin: Option<molt_ir::OpKind>,
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

    let operands_ready = operand_facts.iter().all(|ty| !is_bottom_type(ty));
    let operand_types: Vec<TirType> = operand_facts.to_vec();
    infer_result_types_with_attrs(opcode, origin, &operand_types, attrs, result_count)
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

/// The exact class of an allocation's result: its class statement's
/// instance (I6, C1), by the same rule [`attr_result_type_override`] applies.
pub(super) fn exact_allocation_class(op: &crate::tir::ops::TirOp) -> Option<TirType> {
    (opcode_type_refine_attr_result_type_rule_table(op.opcode)
        == TypeRefineAttrResultTypeRule::ObjectClassDefinition)
        .then(|| attr_result_type_override(op.opcode, op.origin, &op.attrs, &[] as &[TirType]))
        .flatten()
}

pub(super) fn attr_result_type_override<T>(
    opcode: OpCode,
    origin: Option<molt_ir::OpKind>,
    attrs: &AttrDict,
    operands: &[T],
) -> Option<TirType> {
    match opcode_type_refine_attr_result_type_rule_table(opcode) {
        TypeRefineAttrResultTypeRule::None => None,
        // The allocation's class statement (I6, C1).
        TypeRefineAttrResultTypeRule::ObjectClassDefinition => {
            molt_ir::class_identity::stable_allocation_class(attrs).map(TirType::Class)
        }
        TypeRefineAttrResultTypeRule::CallReturnType => {
            attrs.get("return_type").and_then(|v| match v {
                AttrValue::Str(s) => parse_return_type_str(s.as_str()),
                _ => None,
            })
        }
        TypeRefineAttrResultTypeRule::CopyOriginalKind => {
            let original_kind = origin;
            if let Some(ty) =
                original_kind.and_then(crate::tir::op_semantics::container_constructor_result_type)
            {
                return Some(ty);
            }
            if original_kind == Some(molt_ir::OpKind::Len) && operands.len() == 1 {
                return Some(TirType::I64);
            }
            crate::tir::passes::alias_analysis::copy_kind_raw_carrier_type(original_kind).or_else(
                || {
                    original_kind
                        .filter(|k| {
                            crate::tir::passes::alias_analysis::copy_kind_mints_owned_value(*k)
                        })
                        .map(owned_value_kind_result_type)
                },
            )
        }
    }
}

pub(super) fn infer_result_types_with_attrs(
    opcode: OpCode,
    origin: Option<molt_ir::OpKind>,
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
                // Checked arithmetic keeps the corresponding Python payload
                // type. Only its overflow flag is independent of the operands.
                if index == 0 && matches!(opcode, OpCode::CheckedAdd | OpCode::CheckedMul) {
                    let arithmetic = if opcode == OpCode::CheckedAdd {
                        OpCode::Add
                    } else {
                        OpCode::Mul
                    };
                    return crate::tir::op_semantics::op_instance_facts(arithmetic, operand_types)
                        .and_then(|facts| facts.result_type);
                }
                if opcode == OpCode::IterNextUnboxed && index == 0 {
                    match operand_types {
                        [TirType::Iterator(elem_ty)] => Some(elem_ty.as_ref().clone()),
                        _ => None,
                    }
                } else if opcode == OpCode::IterNextUnpack2 && index < 2 {
                    // The two targets of a proven pair item.
                    match operand_types {
                        [TirType::Iterator(elem_ty)] => match elem_ty.as_ref() {
                            TirType::Tuple(elems) if elems.len() == 2 => Some(elems[index].clone()),
                            _ => None,
                        },
                        _ => None,
                    }
                } else if result_count == 1 {
                    infer_single_result_type_with_attrs(opcode, origin, operand_types, attrs)
                } else {
                    None
                }
            })
        })
        .collect()
}

fn infer_single_result_type_with_attrs(
    opcode: OpCode,
    origin: Option<molt_ir::OpKind>,
    operand_types: &[TirType],
    attrs: Option<&AttrDict>,
) -> Option<TirType> {
    if let Some(result_type) = crate::tir::op_semantics::op_instance_facts(opcode, operand_types)
        .and_then(|facts| facts.result_type)
    {
        return Some(result_type);
    }
    if let Some(attrs) = attrs
        && let Some(ty) = attr_result_type_override(opcode, origin, attrs, operand_types)
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
        // Owned-value and raw-carrier Copy spellings are handled by the attr
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

/// An owned result has an independent value type, never operand zero's type.
/// Ownership does not prove allocation freshness or native-callable identity:
/// `builtin_func` can acquire any public replacement and therefore is DynBox.
/// Scalar conversion primitives retain their explicitly declared result types;
/// Python text protocols may return an overriding str subclass.
fn owned_value_kind_result_type(kind: molt_ir::OpKind) -> TirType {
    match kind {
        molt_ir::OpKind::StrFromObj
        | molt_ir::OpKind::ReprFromObj
        | molt_ir::OpKind::AsciiFromObj
        | molt_ir::OpKind::StringFormat => TirType::DynBox,
        molt_ir::OpKind::StringJoin => TirType::Str,
        _ => TirType::DynBox,
    }
}
