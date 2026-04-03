//! Lower typed TIR into representation-aware LIR.

use std::collections::HashMap;

use super::blocks::{Terminator, TirBlock};
use super::function::TirFunction;
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::ops::{AttrDict, AttrValue, OpCode, TirOp};
use super::type_refine::{extract_type_map, refine_types};
use super::types::TirType;
use super::values::{TirValue, ValueId};

pub fn lower_function_to_lir(func: &TirFunction) -> LirFunction {
    let mut refined = func.clone();
    refine_types(&mut refined);
    let type_map = extract_type_map(&refined);
    let mut allocator = ValueIdAllocator::new(refined.next_value);

    let mut block_ids: Vec<_> = refined.blocks.keys().copied().collect();
    block_ids.sort_by_key(|bid| bid.0);
    let blocks = block_ids
        .into_iter()
        .map(|bid| {
            let block = refined
                .blocks
                .get(&bid)
                .expect("sorted block id must exist");
            (bid, lower_block(block, &type_map, &mut allocator))
        })
        .collect();

    LirFunction {
        name: refined.name,
        param_names: refined.param_names,
        param_types: refined.param_types,
        return_types: match refined.return_type {
            TirType::None => Vec::new(),
            other => vec![other],
        },
        blocks,
        entry_block: refined.entry_block,
    }
}

pub fn lower_block_args(args: &[TirValue]) -> Vec<LirValue> {
    args.iter()
        .map(|arg| LirValue {
            id: arg.id,
            ty: arg.ty.clone(),
            repr: LirRepr::for_type(&arg.ty),
        })
        .collect()
}

fn lower_block(
    block: &TirBlock,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
) -> LirBlock {
    let mut ops = lower_block_ops(block.ops.as_slice(), type_map, allocator);
    let terminator = lower_terminator(&block.terminator, type_map, allocator, &mut ops);
    LirBlock {
        id: block.id,
        args: lower_block_args(&block.args),
        ops,
        terminator,
    }
}

fn lower_block_ops(
    ops: &[TirOp],
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
) -> Vec<LirOp> {
    ops.iter()
        .map(|op| lower_op(op, type_map, allocator))
        .collect()
}

fn lower_op(
    op: &TirOp,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
) -> LirOp {
    if lowers_to_checked_i64_arithmetic(op, type_map) {
        return lower_checked_i64_arithmetic(op, type_map, allocator);
    }
    if op.opcode == OpCode::BoxVal {
        return lower_box_op(op, type_map);
    }
    if op.opcode == OpCode::UnboxVal {
        return lower_unbox_op(op, type_map);
    }

    LirOp {
        tir_op: op.clone(),
        result_values: op
            .results
            .iter()
            .map(|result_id| lir_value_from_type_map(*result_id, type_map))
            .collect(),
    }
}

fn lowers_to_checked_i64_arithmetic(op: &TirOp, type_map: &HashMap<ValueId, TirType>) -> bool {
    matches!(op.opcode, OpCode::Add | OpCode::Sub | OpCode::Mul)
        && op.results.len() == 1
        && op.operands.len() == 2
        && op
            .operands
            .iter()
            .all(|operand| matches!(type_map.get(operand), Some(TirType::I64)))
        && matches!(type_map.get(&op.results[0]), Some(TirType::I64))
}

fn lower_checked_i64_arithmetic(
    op: &TirOp,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
) -> LirOp {
    let mut tir_op = op.clone();
    let overflow_box = allocator.fresh();
    let overflow_flag = allocator.fresh();
    tir_op.results = vec![op.results[0], overflow_box, overflow_flag];
    tir_op
        .attrs
        .insert("lir.checked_overflow".to_string(), AttrValue::Bool(true));

    let mut result_values = vec![lir_value_from_type_map(op.results[0], type_map)];
    result_values.push(LirValue {
        id: overflow_box,
        ty: TirType::DynBox,
        repr: LirRepr::DynBox,
    });
    result_values.push(LirValue {
        id: overflow_flag,
        ty: TirType::Bool,
        repr: LirRepr::Bool1,
    });

    LirOp {
        tir_op,
        result_values,
    }
}

fn lower_box_op(op: &TirOp, type_map: &HashMap<ValueId, TirType>) -> LirOp {
    let operand_ty = op
        .operands
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or(TirType::DynBox);
    let result_ty = op
        .results
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or_else(|| TirType::Box(Box::new(operand_ty)));
    let result_id = op.results[0];
    LirOp {
        tir_op: op.clone(),
        result_values: vec![LirValue {
            id: result_id,
            ty: result_ty,
            repr: LirRepr::DynBox,
        }],
    }
}

fn lower_unbox_op(op: &TirOp, type_map: &HashMap<ValueId, TirType>) -> LirOp {
    let operand_ty = op
        .operands
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or(TirType::DynBox);
    let result_ty = op
        .results
        .first()
        .and_then(|id| type_map.get(id))
        .cloned()
        .unwrap_or_else(|| match operand_ty {
            TirType::Box(inner) => inner.as_ref().clone(),
            _ => TirType::DynBox,
        });
    let result_id = op.results[0];
    LirOp {
        tir_op: op.clone(),
        result_values: vec![LirValue {
            id: result_id,
            repr: LirRepr::for_type(&result_ty),
            ty: result_ty,
        }],
    }
}

fn lir_value_from_type_map(id: ValueId, type_map: &HashMap<ValueId, TirType>) -> LirValue {
    let ty = type_map.get(&id).cloned().unwrap_or(TirType::DynBox);
    LirValue {
        id,
        repr: LirRepr::for_type(&ty),
        ty,
    }
}

fn lower_terminator(
    terminator: &Terminator,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
) -> LirTerminator {
    match terminator {
        Terminator::Branch { target, args } => LirTerminator::Branch {
            target: *target,
            args: args.clone(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => LirTerminator::CondBranch {
            cond: materialize_branch_condition(*cond, type_map, allocator, ops),
            then_block: *then_block,
            then_args: then_args.clone(),
            else_block: *else_block,
            else_args: else_args.clone(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => LirTerminator::Switch {
            value: *value,
            cases: cases.clone(),
            default: *default,
            default_args: default_args.clone(),
        },
        Terminator::Return { values } => LirTerminator::Return {
            values: values.clone(),
        },
        Terminator::Unreachable => LirTerminator::Unreachable,
    }
}

fn materialize_branch_condition(
    cond: ValueId,
    type_map: &HashMap<ValueId, TirType>,
    allocator: &mut ValueIdAllocator,
    ops: &mut Vec<LirOp>,
) -> ValueId {
    if matches!(type_map.get(&cond), Some(TirType::Bool)) {
        return cond;
    }

    let result_id = allocator.fresh();
    let mut attrs = AttrDict::new();
    attrs.insert(
        "callee".to_string(),
        AttrValue::Str("molt_is_truthy".to_string()),
    );
    attrs.insert("lir.truthy_cond".to_string(), AttrValue::Bool(true));
    ops.push(LirOp {
        tir_op: TirOp {
            dialect: super::ops::Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: vec![cond],
            results: vec![result_id],
            attrs,
            source_span: None,
        },
        result_values: vec![LirValue {
            id: result_id,
            ty: TirType::Bool,
            repr: LirRepr::Bool1,
        }],
    });
    result_id
}

struct ValueIdAllocator {
    next: u32,
}

impl ValueIdAllocator {
    fn new(next: u32) -> Self {
        Self { next }
    }

    fn fresh(&mut self) -> ValueId {
        let id = ValueId(self.next);
        self.next += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, TirBlock};
    use crate::tir::ops::{AttrDict, Dialect};

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    #[test]
    fn lowers_checked_i64_add_with_overflow_side_channels() {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: vec![],
                ops: vec![
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(0)]),
                    make_op(OpCode::ConstInt, vec![], vec![ValueId(1)]),
                    make_op(OpCode::Add, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]),
                ],
                terminator: Terminator::Return {
                    values: vec![ValueId(2)],
                },
            },
        );
        let func = TirFunction {
            name: "checked_add".into(),
            param_names: vec![],
            param_types: vec![],
            return_type: TirType::I64,
            blocks,
            entry_block: entry,
            next_value: 3,
            next_block: 1,
            attrs: AttrDict::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
        };

        let lir = lower_function_to_lir(&func);
        let add = &lir.blocks[&entry].ops[2];
        assert_eq!(add.result_values.len(), 3);
        assert_eq!(
            add.tir_op.attrs.get("lir.checked_overflow"),
            Some(&AttrValue::Bool(true))
        );
    }
}
