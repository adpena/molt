pub(super) use std::collections::HashMap;

pub(super) use molt_codegen_abi::{
    INLINE_INT_BIAS, INLINE_INT_LIMIT, INT_MASK, INT_MAX_INLINE as INLINE_INT_MAX,
    INT_MIN_INLINE as INLINE_INT_MIN, INT_SHIFT as INT_SHIFT_BITS, QNAN_TAG_BOOL_I64,
    QNAN_TAG_INT_I64, box_none_bits,
};
pub(super) use molt_tir::tir::blocks::BlockId;
pub(super) use molt_tir::tir::function::TirFunction;
pub(super) use molt_tir::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator};
pub(super) use molt_tir::tir::lower_to_lir::{
    lower_function_to_lir, lower_function_to_lir_with_inline_proof,
};
pub(super) use molt_tir::tir::ops::{AttrValue, OpCode};
pub(super) use molt_tir::tir::values::ValueId;
pub(super) use wasm_encoder::{BlockType, Ieee64, Instruction, ValType};

pub(super) use crate::wasm_lir_fast_output::{NAMED_RUNTIME_CALL_PLACEHOLDER, WasmFunctionOutput};
