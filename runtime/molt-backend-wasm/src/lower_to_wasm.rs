//! TIR → WASM type-specialized lowering.
//!
//! Converts a [`TirFunction`] into WASM instructions using the `wasm-encoder` crate.
//! The key insight: TIR carries refined type information from optimization passes,
//! so we can emit **native WASM arithmetic** for unboxed scalars instead of falling
//! back to runtime dispatch calls for every operation.
//!
//! ## Type mapping
//!
//! | TirType     | WASM ValType | Notes                          |
//! |-------------|-------------|--------------------------------|
//! | I64         | i64         | Native 64-bit integer          |
//! | F64         | f64         | Native 64-bit float            |
//! | Bool        | i32         | 0 or 1                         |
//! | None        | i64         | Sentinel constant              |
//! | DynBox      | i64         | NaN-boxed runtime value        |
//! | Ref64       | i64         | Runtime reference word         |
//! | Str/List/…  | i64         | Heap pointer as i64            |
//!
//! ## SSA → stack machine
//!
//! TIR is register-based SSA; WASM is a stack machine. We allocate one WASM local
//! per SSA value and emit explicit local.get/local.set around each operation.
//! A peephole pass (`peephole_set_get_to_tee`) runs after emission to collapse
//! `local.set X; local.get X` pairs into `local.tee X`, eliminating redundant
//! stack traffic.

#[cfg(feature = "wasm-backend")]
use wasm_encoder::{BlockType, Ieee64, Instruction, ValType};

#[cfg(all(test, feature = "wasm-backend"))]
use crate::wasm_lir_fast_output::assert_named_runtime_call_pairing;
#[cfg(feature = "wasm-backend")]
use crate::wasm_lir_fast_output::{NAMED_RUNTIME_CALL_PLACEHOLDER, WasmFunctionOutput};
#[cfg(feature = "wasm-backend")]
use std::collections::HashMap;

#[cfg(feature = "wasm-backend")]
use molt_codegen_abi::{
    INLINE_INT_BIAS, INLINE_INT_LIMIT, INT_MASK, INT_MAX_INLINE as INLINE_INT_MAX,
    INT_MIN_INLINE as INLINE_INT_MIN, INT_SHIFT as INT_SHIFT_BITS, QNAN_TAG_BOOL_I64,
    QNAN_TAG_INT_I64, box_none_bits,
};

#[cfg(feature = "wasm-backend")]
use molt_tir::tir::blocks::BlockId;
#[cfg(feature = "wasm-backend")]
use molt_tir::tir::function::TirFunction;
#[cfg(feature = "wasm-backend")]
use molt_tir::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator};
#[cfg(feature = "wasm-backend")]
use molt_tir::tir::lower_to_lir::{lower_function_to_lir, lower_function_to_lir_with_inline_proof};
#[cfg(feature = "wasm-backend")]
use molt_tir::tir::ops::{AttrValue, OpCode};
#[cfg(feature = "wasm-backend")]
use molt_tir::tir::values::ValueId;

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Lower a TIR function to WASM instructions.
///
/// Type-specialized: `I64` → `wasm i64`, `F64` → `wasm f64`, `DynBox` → runtime call.
#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm(func: &TirFunction) -> WasmFunctionOutput {
    // The generic path derives carriers from the same pure-TIR `repr_by_value`
    // authority as the boxed-i64 ABI path and LLVM. Semantic `I64` alone is not
    // a raw machine carrier; unproven ints lower as DynBox/boxed runtime values,
    // while Bool/F64 and range-proven ints keep their scalar lanes.
    let lir = lower_function_to_lir(func);
    lower_lir_to_wasm(&lir)
}

mod lir_context;
mod lir_control;
mod lir_ops;
mod lir_scalar;
mod peephole;

use lir_context::{LirLowerCtx, lir_repr_to_val, lir_terminator_successors};
use lir_control::{
    emit_lir_terminator, emit_lir_terminator_boxed_i64_abi, emit_lir_terminator_multiblock,
    emit_lir_terminator_multiblock_boxed_i64_abi,
};
use lir_ops::emit_lir_block_ops;
use peephole::peephole_set_get_to_tee;

#[cfg(feature = "wasm-backend")]
pub fn lower_lir_to_wasm(func: &LirFunction) -> WasmFunctionOutput {
    let mut ctx = LirLowerCtx::new(func);

    let param_types: Vec<ValType> = func
        .blocks
        .get(&func.entry_block)
        .map(|entry| {
            entry
                .args
                .iter()
                .map(|arg| lir_repr_to_val(arg.repr))
                .collect()
        })
        .unwrap_or_default();
    let result_types: Vec<ValType> = func
        .return_types
        .iter()
        .map(|ty| lir_repr_to_val(LirRepr::for_type(ty)))
        .collect();

    if let Some(entry) = func.blocks.get(&func.entry_block) {
        for arg in &entry.args {
            ctx.local_for(arg);
        }
    }
    for &bid in &ctx.rpo.clone() {
        if let Some(block) = func.blocks.get(&bid) {
            for arg in &block.args {
                ctx.local_for(arg);
            }
            for op in &block.ops {
                for value in &op.result_values {
                    ctx.local_for(value);
                }
            }
        }
    }

    let num_params = param_types.len() as u32;
    let total_locals = ctx.next_local;
    let mut locals = Vec::with_capacity((total_locals - num_params) as usize);
    for idx in num_params..total_locals {
        let ty = ctx.local_types.get(&idx).copied().unwrap_or(ValType::I64);
        locals.push(ty);
    }

    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();
    if num_blocks <= 1 {
        if let Some(block) = func.blocks.get(&func.entry_block) {
            emit_lir_block_ops(&mut ctx, block);
            emit_lir_terminator(&mut ctx, &block.terminator);
        }
    } else {
        let back_edge_targets: HashMap<BlockId, bool> = {
            let mut targets = HashMap::new();
            for (src_idx, &bid) in rpo.iter().enumerate() {
                if let Some(block) = func.blocks.get(&bid) {
                    for succ in lir_terminator_successors(&block.terminator) {
                        if let Some(&tgt_idx) = ctx.block_index.get(&succ)
                            && tgt_idx <= src_idx
                        {
                            targets.insert(succ, true);
                        }
                    }
                }
            }
            targets
        };

        for (i, &bid) in rpo.iter().enumerate() {
            if i < num_blocks - 1 {
                if back_edge_targets.contains_key(&bid) {
                    ctx.instructions.push(Instruction::Loop(BlockType::Empty));
                } else {
                    ctx.instructions.push(Instruction::Block(BlockType::Empty));
                }
            }
        }

        for (i, &bid) in rpo.iter().enumerate() {
            if let Some(block) = func.blocks.get(&bid) {
                emit_lir_block_ops(&mut ctx, block);
                emit_lir_terminator_multiblock(&mut ctx, &block.terminator, num_blocks);
            }
            if i < num_blocks - 1 {
                ctx.instructions.push(Instruction::End);
            }
        }
    }

    ctx.instructions.push(Instruction::End);
    let instructions = peephole_set_get_to_tee(ctx.instructions);
    WasmFunctionOutput {
        param_types,
        result_types,
        locals,
        instructions,
        runtime_calls: ctx.runtime_calls,
    }
}

#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm_boxed_i64_abi(func: &TirFunction) -> Option<WasmFunctionOutput> {
    let vr = crate::representation_plan::value_range_for(func);
    let repr = crate::representation_plan::repr_by_value_for(func, Some(&vr));
    lower_tir_to_wasm_boxed_i64_abi_with_proof(func, &repr, &vr)
}

/// Boxed-i64 WASM ABI lowering with the value-range proof explicitly paired to
/// the value-keyed Repr map. The production WASM fast lane uses this entry so
/// full-range raw carriers can never take the 47-bit-window checked-i64 triple.
#[cfg(feature = "wasm-backend")]
pub fn lower_tir_to_wasm_boxed_i64_abi_with_proof(
    func: &TirFunction,
    repr: &HashMap<ValueId, crate::repr::Repr>,
    inline_proof: &crate::tir::passes::value_range::ValueRangeResult,
) -> Option<WasmFunctionOutput> {
    let lir = lower_function_to_lir_with_inline_proof(func, repr, inline_proof);
    lower_lir_to_wasm_boxed_i64_abi(&lir)
}

#[cfg(feature = "wasm-backend")]
pub fn lower_lir_to_wasm_boxed_i64_abi(func: &LirFunction) -> Option<WasmFunctionOutput> {
    if func
        .param_types
        .iter()
        .any(|ty| *ty != crate::tir::types::TirType::I64)
    {
        return None;
    }
    if func.return_types.len() != 1 || func.return_types[0] != crate::tir::types::TirType::I64 {
        return None;
    }
    let entry = func.blocks.get(&func.entry_block)?;
    if entry.args.iter().any(|arg| arg.repr != LirRepr::I64) {
        return None;
    }

    let param_count = entry.args.len() as u32;
    let mut ctx = LirLowerCtx::new_with_local_base(func, param_count);

    for arg in &entry.args {
        ctx.local_for(arg);
    }
    for &bid in &ctx.rpo.clone() {
        if let Some(block) = func.blocks.get(&bid) {
            for arg in &block.args {
                ctx.local_for(arg);
            }
            for op in &block.ops {
                for value in &op.result_values {
                    ctx.local_for(value);
                }
            }
        }
    }

    let param_types = vec![ValType::I64; param_count as usize];
    let result_types = vec![ValType::I64];
    let total_locals = ctx.next_local;
    let mut locals = Vec::new();
    for idx in param_count..total_locals {
        let ty = ctx
            .value_locals
            .iter()
            .find(|&(_, &local_idx)| local_idx == idx)
            .and_then(|(vid, _)| ctx.value_reprs.get(vid))
            .copied()
            .map(lir_repr_to_val)
            .unwrap_or(ValType::I64);
        locals.push(ty);
    }

    for (idx, arg) in entry.args.iter().enumerate() {
        ctx.instructions.push(Instruction::LocalGet(idx as u32));
        ctx.instructions.push(Instruction::I64Const(INT_SHIFT_BITS));
        ctx.instructions.push(Instruction::I64Shl);
        ctx.instructions.push(Instruction::I64Const(INT_SHIFT_BITS));
        ctx.instructions.push(Instruction::I64ShrS);
        ctx.emit_set(arg.id);
    }

    let rpo = ctx.rpo.clone();
    let num_blocks = rpo.len();
    if num_blocks <= 1 {
        if let Some(block) = func.blocks.get(&func.entry_block) {
            emit_lir_block_ops(&mut ctx, block);
            emit_lir_terminator_boxed_i64_abi(&mut ctx, &block.terminator);
        }
    } else {
        let back_edge_targets: HashMap<BlockId, bool> = {
            let mut targets = HashMap::new();
            for (src_idx, &bid) in rpo.iter().enumerate() {
                if let Some(block) = func.blocks.get(&bid) {
                    for succ in lir_terminator_successors(&block.terminator) {
                        if let Some(&tgt_idx) = ctx.block_index.get(&succ)
                            && tgt_idx <= src_idx
                        {
                            targets.insert(succ, true);
                        }
                    }
                }
            }
            targets
        };

        for (i, &bid) in rpo.iter().enumerate() {
            if i < num_blocks - 1 {
                if back_edge_targets.contains_key(&bid) {
                    ctx.instructions.push(Instruction::Loop(BlockType::Empty));
                } else {
                    ctx.instructions.push(Instruction::Block(BlockType::Empty));
                }
            }
        }

        for (i, &bid) in rpo.iter().enumerate() {
            if let Some(block) = func.blocks.get(&bid) {
                emit_lir_block_ops(&mut ctx, block);
                emit_lir_terminator_multiblock_boxed_i64_abi(
                    &mut ctx,
                    &block.terminator,
                    num_blocks,
                );
            }
            if i < num_blocks - 1 {
                ctx.instructions.push(Instruction::End);
            }
        }
    }

    ctx.instructions.push(Instruction::End);
    let instructions = peephole_set_get_to_tee(ctx.instructions);
    Some(WasmFunctionOutput {
        param_types,
        result_types,
        locals,
        instructions,
        runtime_calls: ctx.runtime_calls,
    })
}

// ---------------------------------------------------------------------------
// Op emission
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(feature = "wasm-backend")]
mod tests {
    use super::*;
    use crate::repr::Repr;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

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

    #[test]
    fn binding_alias_copy_retains_before_forwarding_bits() {
        let mut func = TirFunction::new(
            "binding_alias_copy".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let alias = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![ValueId(0)],
            results: vec![alias],
            attrs: {
                let mut m = AttrDict::new();
                m.insert(
                    "_original_kind".into(),
                    AttrValue::Str("binding_alias".into()),
                );
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![alias],
        };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"inc_ref_obj"),
            "binding_alias Copy must retain its forwarded source: {:?}",
            output.runtime_calls
        );
    }

    #[test]
    fn trivial_const_return() {
        let func = make_const_return_func(42);
        let output = lower_tir_to_wasm(&func);

        assert_eq!(output.param_types, vec![]);
        assert_eq!(output.result_types, vec![ValType::I64]);

        // Should contain i64.const 42 somewhere.
        let has_const = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Const(42)));
        assert!(has_const, "expected i64.const 42 in output");

        // Should end with `end`.
        assert!(matches!(output.instructions.last(), Some(Instruction::End)));
    }

    #[test]
    fn lir_fast_lane_dec_ref_emits_named_runtime_call() {
        let mut func = TirFunction::new("drop_ref".into(), vec![], TirType::None);
        let owned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![owned],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DecRef,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"dec_ref_obj"),
            "WASM LIR fast lane must consume shared DecRef through dec_ref_obj; got {:?}",
            output.runtime_calls
        );
        assert_named_runtime_call_pairing("drop_ref", &output);
    }

    #[test]
    fn lir_fast_lane_del_boundary_emits_named_dec_ref_runtime_call() {
        let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::None);
        let owned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![owned],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::DelBoundary,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"dec_ref_obj"),
            "WASM LIR fast lane must consume DelBoundary through dec_ref_obj; got {:?}",
            output.runtime_calls
        );
        assert_named_runtime_call_pairing("del_boundary_release", &output);
    }

    #[test]
    fn add_two_i64s() {
        let func = make_add_two_consts_func(20, 22);

        let output = lower_tir_to_wasm(&func);

        assert_eq!(output.param_types, Vec::<ValType>::new());

        // Should contain i64.add.
        let has_add = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Add));
        assert!(has_add, "expected i64.add instruction");
    }

    #[test]
    fn bool1_and_stays_raw_without_selected_ref_retain() {
        let mut func = TirFunction::new(
            "and_bool1".into(),
            vec![TirType::Bool, TirType::Bool],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::And,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::I32And)),
            "raw Bool1 and must stay a native i32.and: {:?}",
            output.instructions
        );
        assert!(
            !output.runtime_calls.contains(&"inc_ref_obj"),
            "raw Bool1 and must not retain a selected boxed operand: {:?}",
            output.runtime_calls
        );
    }

    #[test]
    fn dynbox_or_retains_selected_operand_result() {
        let mut func = TirFunction::new(
            "or_dynbox".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Or,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);
        assert!(
            output.runtime_calls.contains(&"is_truthy"),
            "boxed or must test Python truthiness: {:?}",
            output.runtime_calls
        );
        assert!(
            output.runtime_calls.contains(&"inc_ref_obj"),
            "boxed or must retain the selected borrowed operand result: {:?}",
            output.runtime_calls
        );
        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::LocalTee(_))),
            "boxed or must tee the selected result before retaining it: {:?}",
            output.instructions
        );
    }

    #[test]
    fn add_two_f64s() {
        let mut func = TirFunction::new(
            "add_f64".into(),
            vec![TirType::F64, TirType::F64],
            TirType::F64,
        );
        let result_id = func.fresh_value();
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

        let output = lower_tir_to_wasm(&func);

        assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
        let has_f64_add = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::F64Add));
        assert!(has_f64_add, "expected f64.add instruction");
    }

    #[test]
    fn conditional_branch() {
        let mut func = TirFunction::new("cond_branch".into(), vec![TirType::Bool], TirType::I64);

        let then_id = func.fresh_block();
        let else_id = func.fresh_block();

        let ret_then = func.fresh_value();
        let ret_else = func.fresh_value();

        // Patch entry block to branch on param.
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };

        let then_block = TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_then],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_then],
            },
        };

        let else_block = TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_else],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_else],
            },
        };

        func.blocks.insert(then_id, then_block);
        func.blocks.insert(else_id, else_block);

        let output = lower_tir_to_wasm(&func);

        // Should contain br_if for the conditional branch.
        let has_br_if = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::BrIf(_)));
        assert!(
            has_br_if,
            "expected br_if instruction for conditional branch"
        );
    }

    #[test]
    fn comparison_i64_emits_native() {
        let func = make_lt_two_consts_func(20, 22);

        let output = lower_tir_to_wasm(&func);

        let has_lt = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64LtS));
        assert!(has_lt, "expected i64.lt_s instruction");
    }

    #[test]
    fn dynbox_add_falls_back_to_call() {
        let mut func = TirFunction::new(
            "add_dyn".into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
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

        let output = lower_tir_to_wasm(&func);

        // DynBox add should emit a Call (runtime dispatch), not i64.add.
        let has_call = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_)));
        assert!(has_call, "expected runtime call for DynBox add");

        let has_i64_add = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Add));
        assert!(!has_i64_add, "should NOT emit i64.add for DynBox operands");
    }

    #[test]
    fn alloc_task_falls_back_to_runtime_call() {
        let mut func =
            TirFunction::new("alloc_task".into(), vec![TirType::DynBox], TirType::DynBox);
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::AllocTask,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("s_value".into(), AttrValue::Str("task_poll".into()));
                m.insert("task_kind".into(), AttrValue::Str("future".into()));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        let has_call = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_)));
        assert!(has_call, "expected runtime call for alloc_task");
    }

    #[test]
    fn state_switch_falls_back_to_runtime_call() {
        let mut func = TirFunction::new(
            "state_switch".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::StateSwitch,
            operands: vec![ValueId(0)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func);

        let has_call = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_)));
        assert!(has_call, "expected runtime call for state_switch");
    }

    // -----------------------------------------------------------------------
    // Peephole pass tests
    // -----------------------------------------------------------------------

    #[test]
    fn peephole_collapses_set_get_to_tee() {
        let input = vec![
            Instruction::I64Const(42),
            Instruction::LocalSet(3),
            Instruction::LocalGet(3),
            Instruction::End,
        ];
        let output = peephole_set_get_to_tee(input);
        assert_eq!(output.len(), 3);
        assert!(
            matches!(output[0], Instruction::I64Const(42)),
            "const preserved"
        );
        assert!(
            matches!(output[1], Instruction::LocalTee(3)),
            "set+get collapsed to tee"
        );
        assert!(matches!(output[2], Instruction::End), "end preserved");
    }

    #[test]
    fn peephole_preserves_mismatched_set_get() {
        let input = vec![
            Instruction::LocalSet(1),
            Instruction::LocalGet(2), // different local
            Instruction::End,
        ];
        let output = peephole_set_get_to_tee(input);
        assert_eq!(output.len(), 3);
        assert!(
            matches!(output[0], Instruction::LocalSet(1)),
            "set preserved"
        );
        assert!(
            matches!(output[1], Instruction::LocalGet(2)),
            "get preserved"
        );
    }

    #[test]
    fn peephole_handles_consecutive_tee_chains() {
        // Pattern: set(1) get(1) set(2) get(2) → tee(1) tee(2)
        let input = vec![
            Instruction::I64Const(10),
            Instruction::LocalSet(1),
            Instruction::LocalGet(1),
            Instruction::LocalSet(2),
            Instruction::LocalGet(2),
            Instruction::End,
        ];
        let output = peephole_set_get_to_tee(input);
        assert_eq!(output.len(), 4);
        assert!(matches!(output[1], Instruction::LocalTee(1)));
        assert!(matches!(output[2], Instruction::LocalTee(2)));
    }

    #[test]
    fn peephole_empty_and_single() {
        assert!(peephole_set_get_to_tee(vec![]).is_empty());
        let single = vec![Instruction::End];
        assert_eq!(peephole_set_get_to_tee(single).len(), 1);
    }

    #[test]
    fn peephole_applied_in_const_return() {
        // A const-return function should have tee instead of set+get.
        let func = make_const_return_func(99);
        let output = lower_tir_to_wasm(&func);

        // After peephole, the pattern: i64.const 99; local.set X; local.get X; return
        // becomes: i64.const 99; local.tee X; return
        let has_tee = output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::LocalTee(_)));
        assert!(has_tee, "expected local.tee from peephole optimization");

        // Should have no set+get pairs for the same local.
        for window in output.instructions.windows(2) {
            if let (Instruction::LocalSet(s), Instruction::LocalGet(g)) = (&window[0], &window[1]) {
                assert_ne!(
                    s, g,
                    "found redundant set+get pair for local {s} that peephole should have eliminated"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Phase 1: mixed-repr integer arithmetic (the delicate correctness core)
    // -----------------------------------------------------------------------

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

    fn make_add_two_consts_func(lhs: i64, rhs: i64) -> TirFunction {
        let mut func = TirFunction::new("add_two_consts".into(), vec![], TirType::I64);
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
            opcode: OpCode::Add,
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

    #[test]
    fn generic_tir_to_wasm_uses_value_repr_not_type_floor_for_int_params() {
        let func = make_add_two_params_func();
        let output = lower_tir_to_wasm(&func);

        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call(_))),
            "unproven int params must lower through boxed runtime dispatch, not a type-floor raw i64 op"
        );
        for (idx, inst) in output.instructions.iter().enumerate() {
            if matches!(inst, Instruction::I64Add) {
                assert!(
                    matches!(
                        output.instructions.get(idx + 1),
                        Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                    ),
                    "generic lower_tir_to_wasm emitted a bare operand i64.add at {idx}"
                );
            }
        }
    }

    /// Full-range raw carriers must box through the OVERFLOW-SAFE path: a
    /// full-range raw value without an inline-window range proof (the CheckedAdd
    /// sum / overflow_peel accumulator case) boxed at a runtime-call or
    /// return site must emit the fits-check + named `int_from_i64` cold
    /// call, never the bare 47-bit mask (which truncates mod 2^47).
    #[test]
    fn full_range_raw_carrier_boxes_overflow_safe_with_named_call() {
        let func = make_add_two_params_func();
        // The values are raw full-deopt carriers, and the value-range proof for
        // opaque params does not prove the 47-bit inline window. The checked
        // triple is therefore refused and the add takes the boxed runtime path,
        // boxing both raw operands through the overflow-safe cold call.
        let repr: HashMap<ValueId, Repr> = HashMap::from([
            (ValueId(0), Repr::RawI64FullDeopt),
            (ValueId(1), Repr::RawI64FullDeopt),
            (ValueId(2), Repr::RawI64FullDeopt),
        ]);
        let vr = crate::representation_plan::value_range_for(&func);
        let lir =
            crate::tir::lower_to_lir::lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
        // Triple refused without an inline-window proof: no op carries
        // lir.checked_overflow.
        let has_triple = lir.blocks.values().flat_map(|b| b.ops.iter()).any(|op| {
            matches!(
                op.tir_op.attrs.get("lir.checked_overflow"),
                Some(crate::tir::ops::AttrValue::Bool(true))
            )
        });
        assert!(
            !has_triple,
            "checked-i64 triple must be refused without a value-range proof"
        );

        let output = lower_lir_to_wasm(&lir);
        // The raw operands are boxed overflow-safely: the cold arm is a
        // NAMED int_from_i64 runtime call recorded in runtime_calls.
        assert!(
            output
                .runtime_calls
                .iter()
                .filter(|name| **name == "int_from_i64")
                .count()
                >= 2,
            "both full-range raw operands must box through the int_from_i64 cold path; got {:?}",
            output.runtime_calls
        );
        assert_named_runtime_call_pairing("add_two_params", &output);
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

    /// THE regression guard for finding #3: an integer `add` with one proven
    /// `RawI64Safe` operand and one `MaybeBigInt` operand must NOT emit a bare
    /// `i64.add` (the unsound op on a NaN-boxed word). Both operands must be
    /// NaN-boxed before the runtime `Call` (`molt_add`): the proven operand via
    /// the inline-int box, the unproven operand passed through already-boxed.
    #[test]
    fn mixed_repr_int_add_boxes_both_operands_no_bare_i64_add() {
        let func = make_add_two_params_func();
        // a (ValueId 0) is proven RawI64Safe; b (ValueId 1) is an unproven
        // MaybeBigInt; the result (ValueId 2) is therefore MaybeBigInt too (it
        // cannot be proven from an unproven operand). This forces the generic
        // boxed path (NOT the checked-overflow triple, which requires all three
        // to be RawI64Safe).
        let repr: HashMap<ValueId, Repr> = HashMap::from([
            (ValueId(0), Repr::RawI64Safe),
            (ValueId(1), Repr::MaybeBigInt),
            (ValueId(2), Repr::MaybeBigInt),
        ]);
        let lir = lower_function_to_lir_with_inline_proof(
            &func,
            &repr,
            &crate::representation_plan::value_range_for(&func),
        );
        let output = lower_lir_to_wasm(&lir);

        // No bare OPERAND i64.add: a raw machine add on a possibly-heap-BigInt
        // operand is exactly the truncation bug-class this phase makes
        // un-emittable. The overflow-safe box legitimately contains an
        // `i64.add` (the `src + 2^46` fits-inline bias), so the precise
        // invariant is: every I64Add in the stream is a fits-check add —
        // immediately followed by the `2^47` window-limit const — never an
        // operand-pair add.
        for (idx, inst) in output.instructions.iter().enumerate() {
            if matches!(inst, Instruction::I64Add) {
                assert!(
                    matches!(
                        output.instructions.get(idx + 1),
                        Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                    ),
                    "mixed-repr add emitted a bare operand i64.add at {idx} (operand may be a heap BigInt)"
                );
            }
        }
        // Runtime dispatch through the boxed helper (placeholder Call(0)).
        assert!(
            output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::Call(_))),
            "mixed-repr add must dispatch through the boxed runtime helper"
        );
        // The proven RawI64Safe operand `a` is NaN-boxed (inline-int box) before
        // the call. (`b` is already a DynBox word and passes through, so exactly
        // one inline-int box is emitted for the operands of this add.)
        assert!(
            count_inline_int_boxes(&output.instructions) >= 1,
            "the proven raw-i64 operand must be NaN-boxed before the runtime call"
        );
    }

    /// The perf-preservation direction: when BOTH operands are proven
    /// `RawI64Safe`, the fast `i64.add` is still emitted (the checked-overflow
    /// triple), and no boxed runtime `Call` is needed for the add itself.
    #[test]
    fn proven_raw_i64_add_still_emits_native_i64_add() {
        let func = make_add_two_consts_func(20, 22);
        let output = lower_tir_to_wasm(&func);

        let has_operand_add = output.instructions.iter().enumerate().any(|(idx, inst)| {
            matches!(inst, Instruction::I64Add)
                && !matches!(
                    output.instructions.get(idx + 1),
                    Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
                )
        });
        assert!(
            has_operand_add,
            "range-proven const add must emit an operand-pair native i64.add, got {:?}",
            output.instructions
        );
    }

    /// On the production boxed-i64 ABI path, a function whose integer params are
    /// proven `RawI64Safe` keeps the fast path (entry args lower to `I64`); a
    /// `MaybeBigInt` param forces the entry arg to `DynBox`, so the boxed-i64 ABI
    /// (which requires all-`I64` entry args) bails to `None` — falling back to
    /// the IntFastLane-guarded slow path. This is the structural gate that keeps
    /// the unsound bare op un-emittable for unproven ints.
    #[test]
    fn boxed_i64_abi_bails_when_param_is_maybe_bigint() {
        let proven = make_add_two_consts_func(20, 22);
        assert!(
            lower_tir_to_wasm_boxed_i64_abi(&proven).is_some(),
            "range-proven raw-i64 values keep the boxed-i64 ABI fast path"
        );

        let unproven = make_add_two_params_func();
        assert!(
            lower_tir_to_wasm_boxed_i64_abi(&unproven).is_none(),
            "a MaybeBigInt param must bail the boxed-i64 ABI (entry arg is DynBox)"
        );
    }
}
