//! Lower typed TIR into representation-aware LIR.

use std::collections::HashMap;

use super::blocks::Terminator;
use super::function::TirFunction;
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::ops::{AttrValue, OpCode, TirOp};
use super::type_refine::{extract_type_map, refine_types};
use super::types::TirType;
use super::values::{TirValue, ValueId};

pub fn lower_function_to_lir(func: &TirFunction) -> LirFunction {
    let mut refined = func.clone();
    refine_types(&mut refined);
    let type_map = extract_type_map(&refined);
    let mut allocator = ValueIdAllocator::new(refined.next_value);

    let blocks = refined
        .blocks
        .iter()
        .map(|(bid, block)| {
            (
                *bid,
                LirBlock {
                    id: block.id,
                    args: lower_block_args(&block.args),
                    ops: lower_block_ops(block.ops.as_slice(), &type_map, &mut allocator),
                    terminator: lower_terminator(&block.terminator),
                },
            )
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

fn lir_value_from_type_map(id: ValueId, type_map: &HashMap<ValueId, TirType>) -> LirValue {
    let ty = type_map.get(&id).cloned().unwrap_or(TirType::DynBox);
    LirValue {
        id,
        repr: LirRepr::for_type(&ty),
        ty,
    }
}

fn lower_terminator(terminator: &Terminator) -> LirTerminator {
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
            cond: *cond,
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
    use crate::tir::blocks::TirBlock;
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
