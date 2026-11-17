use super::*;
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use std::f64::consts::PI;

/// Helper: build a simple function with one block containing the given ops.
fn single_block_func(ops: Vec<TirOp>, next_value: u32) -> TirFunction {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![],
        ops,
        terminator: Terminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::default();
    blocks.insert(entry_id, block);
    TirFunction {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "test".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        exact_param_types: Vec::new(),
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value,
        next_block: 1,
        attrs: AttrDict::default(),
        value_types: HashMap::default(),
        has_exception_handling: false,
        label_id_map: HashMap::default(),
        loop_roles: HashMap::default(),
        loop_pairs: HashMap::default(),
        loop_break_kinds: HashMap::default(),
        loop_cond_blocks: HashMap::default(),
    }
}

fn make_op(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: AttrDict,
) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

fn int_attr(val: i64) -> AttrDict {
    let mut m = AttrDict::default();
    m.insert("value".into(), AttrValue::Int(val));
    m
}

fn float_attr(val: f64) -> AttrDict {
    let mut m = AttrDict::default();
    m.insert("f_value".into(), AttrValue::Float(val));
    m
}

fn str_attr(val: &str) -> AttrDict {
    let mut m = AttrDict::default();
    m.insert("s_value".into(), AttrValue::Str(val.into()));
    m
}

mod proven;
mod result_attrs;
mod result_builtins_iterators;
mod result_containers;
mod result_integer_bits;
mod result_scalars;
mod solver_cfg;
mod solver_loops;
