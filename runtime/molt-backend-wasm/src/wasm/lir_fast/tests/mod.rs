use super::peephole::peephole_set_get_to_tee;
use super::runtime_calls::LirRuntimeCall;
use super::{lower_lir_to_wasm, lower_tir_to_wasm, lower_tir_to_wasm_boxed_i64_abi};
use crate::repr::Repr;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::lower_to_lir::lower_function_to_lir_with_inline_proof;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use crate::wasm::body::{WasmBodyOps, WasmLirFallbackReason};
use molt_codegen_abi::{
    CANONICAL_NAN_BITS, INT_MASK, QNAN_TAG_INT_I64, QNAN_TAG_MASK_I64, box_int_bits, box_none_bits,
    stable_ic_site_id,
};
use std::collections::HashMap;
use wasm_encoder::{Instruction, ValType};


const F64_EXPONENT_MASK: i64 = 0x7ff0_0000_0000_0000u64 as i64;

const F64_FRACTION_MASK: i64 = 0x000f_ffff_ffff_ffffu64 as i64;

fn peephole_instrs(input: Vec<Instruction<'static>>) -> Vec<Instruction<'static>> {
    peephole_set_get_to_tee(WasmBodyOps::from_instructions(input)).into_instructions_for_tests()
}

/// Build a trivial function: returns a constant i64.
fn make_const_return_func(val: i64) -> TirFunction {
    let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(val));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_scalar_const_return_func(
    name: &str,
    opcode: OpCode,
    return_type: TirType,
    attrs: AttrDict,
) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], return_type);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![result_id],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_fixed_runtime_service_func(
    name: &str,
    opcode: OpCode,
    operand_count: usize,
    has_result: bool,
) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox; operand_count],
        if has_result {
            TirType::DynBox
        } else {
            TirType::None
        },
    );
    let result_id = has_result.then(|| {
        let id = func.fresh_value();
        func.value_types.insert(id, TirType::DynBox);
        id
    });
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
        results: result_id.into_iter().collect(),
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: result_id.into_iter().collect(),
    };
    func
}

fn make_copy_original_kind_runtime_func(
    name: &str,
    original_kind: &str,
    operand_count: usize,
    has_result: bool,
) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox; operand_count],
        if has_result {
            TirType::DynBox
        } else {
            TirType::None
        },
    );
    let result_id = has_result.then(|| {
        let id = func.fresh_value();
        func.value_types.insert(id, TirType::DynBox);
        id
    });
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
        results: result_id.into_iter().collect(),
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str(original_kind.into()),
            );
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: result_id.into_iter().collect(),
    };
    func
}

/// Build `f(a: int, b: int) -> int = a + b` with two i64-typed params and a
/// single Add. The caller supplies the `Repr` override.
fn make_add_two_params_func() -> TirFunction {
    let mut func = TirFunction::new(
        "add_two_params".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );
    let result_id = func.fresh_value(); // ValueId(2)
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_binary_two_consts_func(name: &str, opcode: OpCode, lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], TirType::I64);
    let lhs_id = func.fresh_value();
    let rhs_id = func.fresh_value();
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs_id, lhs), (rhs_id, rhs)] {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![id],
            attrs,
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![lhs_id, rhs_id],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

fn make_add_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    make_binary_two_consts_func("add_two_consts", OpCode::Add, lhs, rhs)
}

fn make_checked_mul_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new("checked_mul_two_consts".into(), vec![], TirType::I64);
    let lhs_id = func.fresh_value();
    let rhs_id = func.fresh_value();
    let product_id = func.fresh_value();
    let flag_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs_id, lhs), (rhs_id, rhs)] {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![id],
            attrs,
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckedMul,
        operands: vec![lhs_id, rhs_id],
        results: vec![product_id, flag_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![product_id],
    };
    func
}

fn make_lt_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
    let mut func = TirFunction::new("lt_two_consts".into(), vec![], TirType::Bool);
    let lhs_id = func.fresh_value();
    let rhs_id = func.fresh_value();
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    for (id, value) in [(lhs_id, lhs), (rhs_id, rhs)] {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![id],
            attrs,
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Lt,
        operands: vec![lhs_id, rhs_id],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

/// Count occurrences of the inline-int NaN-box packing
/// (`emit_box_inline_i64`): `i64.const INT_MASK; i64.and; i64.const
/// (QNAN|TAG_INT); i64.or`. This is how a proven raw-i64 operand is boxed
/// before a runtime helper call in the mixed-repr boxed arm.
fn count_inline_int_boxes(instrs: &[Instruction<'static>]) -> usize {
    instrs
        .windows(4)
        .filter(|w| {
            matches!(w[0], Instruction::I64Const(m) if m == INT_MASK as i64)
                && matches!(w[1], Instruction::I64And)
                && matches!(w[2], Instruction::I64Const(t) if t == QNAN_TAG_INT_I64)
                && matches!(w[3], Instruction::I64Or)
        })
        .count()
}

fn has_native_binary_instruction(instructions: &[Instruction<'static>], opcode: OpCode) -> bool {
    instructions.iter().any(|instruction| match opcode {
        OpCode::Add => matches!(instruction, Instruction::I64Add),
        OpCode::Sub => matches!(instruction, Instruction::I64Sub),
        OpCode::Mul => matches!(instruction, Instruction::I64Mul),
        other => panic!("unsupported native binary assertion for {other:?}"),
    })
}

mod const_materialization;
mod refcount;
mod arithmetic;
mod index_subscript;
mod membership_iter;
mod runtime_service;
mod alloc_object;
mod name_attrs;
mod control_flow;
mod peephole;
mod int_carrier_abi;
mod manifest;
