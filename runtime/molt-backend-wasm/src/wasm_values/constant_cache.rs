use crate::wasm_values::box_none;
use molt_codegen_abi::{
    INT_MAX_INLINE, INT_MIN_INLINE, INT_SHIFT, QNAN_TAG_MASK_I64, QNAN_TAG_PTR_I64,
};
use wasm_encoder::{Function, Instruction};

/// Cache of WASM local indices holding frequently-used i64 constants.
/// When a function body contains 3+ fast_int operations, these locals are
/// pre-allocated and initialized once at function entry, replacing repeated
/// `i64.const` immediates with cheaper `local.get` instructions.
#[derive(Clone, Copy, Default)]
pub(crate) struct ConstantCache {
    pub(crate) int_shift: Option<u32>,
    pub(crate) int_min: Option<u32>,
    pub(crate) int_max: Option<u32>,
    pub(crate) none_bits: Option<u32>,
    pub(crate) qnan_tag_mask: Option<u32>,
    pub(crate) qnan_tag_ptr: Option<u32>,
}

impl ConstantCache {
    /// Emit the initialization sequence for all cached constants.
    /// Must be called once, right after the WASM `Function` is created and
    /// before any op emission.
    pub(crate) fn emit_init(&self, func: &mut Function) {
        if let Some(local) = self.int_shift {
            func.instruction(&Instruction::I64Const(INT_SHIFT));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.int_min {
            func.instruction(&Instruction::I64Const(INT_MIN_INLINE));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.int_max {
            func.instruction(&Instruction::I64Const(INT_MAX_INLINE));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.none_bits {
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.qnan_tag_mask {
            func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
            func.instruction(&Instruction::LocalSet(local));
        }
        if let Some(local) = self.qnan_tag_ptr {
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
            func.instruction(&Instruction::LocalSet(local));
        }
    }

    /// Emit `box_none()` â€” uses cached local if available, otherwise literal.
    #[inline]
    pub(crate) fn emit_none(&self, func: &mut Function) {
        if let Some(local) = self.none_bits {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(box_none()));
        }
    }

    /// Emit `QNAN_TAG_MASK_I64` â€” uses cached local if available, otherwise literal.
    #[inline]
    pub(crate) fn emit_qnan_tag_mask(&self, func: &mut Function) {
        if let Some(local) = self.qnan_tag_mask {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
        }
    }

    /// Emit `QNAN_TAG_PTR_I64` â€” uses cached local if available, otherwise literal.
    #[inline]
    pub(crate) fn emit_qnan_tag_ptr(&self, func: &mut Function) {
        if let Some(local) = self.qnan_tag_ptr {
            func.instruction(&Instruction::LocalGet(local));
        } else {
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
        }
    }
}
