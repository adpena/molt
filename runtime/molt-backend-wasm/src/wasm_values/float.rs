use molt_codegen_abi::CANONICAL_NAN_BITS;
use wasm_encoder::{BlockType, Function, Instruction, ValType};

const F64_EXPONENT_MASK: i64 = 0x7ff0_0000_0000_0000u64 as i64;
const F64_FRACTION_MASK: i64 = 0x000f_ffff_ffff_ffffu64 as i64;

/// Push WASM instructions that convert an f64 on the stack to Molt's canonical
/// NaN-boxed float word.
///
/// This mirrors `molt_codegen_abi::box_float_bits`: any NaN payload, quiet or
/// signaling, becomes `CANONICAL_NAN_BITS`; every non-NaN keeps its raw IEEE
/// payload. Keeping this as the instruction authority lets generic WASM and
/// LIR-fast boxing share one semantics path.
pub(crate) fn push_f64_to_i64_canonical(
    mut push: impl FnMut(Instruction<'static>),
    scratch_local: u32,
) {
    push(Instruction::I64ReinterpretF64);
    push(Instruction::LocalTee(scratch_local));
    push(Instruction::LocalGet(scratch_local));
    push(Instruction::I64Const(F64_EXPONENT_MASK));
    push(Instruction::I64And);
    push(Instruction::I64Const(F64_EXPONENT_MASK));
    push(Instruction::I64Eq);
    push(Instruction::LocalGet(scratch_local));
    push(Instruction::I64Const(F64_FRACTION_MASK));
    push(Instruction::I64And);
    push(Instruction::I64Const(0));
    push(Instruction::I64Ne);
    push(Instruction::I32And);
    push(Instruction::If(BlockType::Result(ValType::I64)));
    push(Instruction::I64Const(CANONICAL_NAN_BITS as i64));
    push(Instruction::Else);
    push(Instruction::LocalGet(scratch_local));
    push(Instruction::End);
}

/// Emit WASM instructions to convert an f64 on the stack to a NaN-canonicalized
/// i64 using `scratch_local` as temporary storage.
pub(crate) fn emit_f64_to_i64_canonical(func: &mut Function, scratch_local: u32) {
    push_f64_to_i64_canonical(
        |instruction| {
            func.instruction(&instruction);
        },
        scratch_local,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_canonicalizer_detects_all_nan_payloads_not_only_qnan_tags() {
        let mut instructions = Vec::new();
        push_f64_to_i64_canonical(|instruction| instructions.push(instruction), 7);

        assert!(
            instructions.iter().any(
                |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == F64_EXPONENT_MASK)
            ),
            "float canonicalizer must test the IEEE exponent mask"
        );
        assert!(
            instructions.iter().any(
                |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == F64_FRACTION_MASK)
            ),
            "float canonicalizer must test the IEEE fraction mask"
        );
        assert!(
            instructions.iter().any(
                |instruction| matches!(instruction, Instruction::I64Const(bits) if *bits == CANONICAL_NAN_BITS as i64)
            ),
            "float canonicalizer must emit the shared canonical NaN bits"
        );
    }
}
