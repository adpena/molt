use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::box_none;
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

const I64_ALIGN_EXPONENT: u32 = 3;

/// A runtime constructor whose values arrive as one borrowed word range,
/// between its other already-boxed operand locals.
#[derive(Clone, Copy)]
pub(super) struct WordRangeConstructor<'a> {
    pub(super) import: WasmRuntimeImport,
    pub(super) leading: &'a [u32],
    pub(super) trailing: &'a [u32],
}

/// Construct from already-evaluated operands passed as `(address, length)`.
/// The range is a scratch allocation owned by this one construction: written
/// here, copied and retained by the runtime in a single call, then freed before
/// the next operation. No static buffer is visible to a reentrant callback,
/// another activation or another thread. There is no per-element failure path;
/// a failed allocation leaves None and its pending exception for the enclosing
/// SimpleIR exception route.
pub(super) fn emit_word_range_constructor(
    func: &mut Function,
    value_names: &[String],
    out: u32,
    constructor: WordRangeConstructor<'_>,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    if value_names.is_empty() {
        // An empty range needs no address, allocation or failure branch.
        emit_word_range_call(
            func,
            constructor,
            Instruction::I64Const(0),
            0,
            import_ids,
            reloc_enabled,
        );
        func.instruction(&Instruction::LocalSet(out));
        return;
    }
    let scratch = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
    let bytes = i64::try_from(value_names.len())
        .ok()
        .and_then(|count| count.checked_mul(8))
        .expect("fixed-arity operand range exceeds the i64 scratch ABI");
    func.instruction(&Instruction::I64Const(bytes));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::ScratchAlloc],
    );
    func.instruction(&Instruction::LocalTee(scratch));
    func.instruction(&Instruction::I64Eqz);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    // ScratchAlloc owns MemoryError publication.
    func.instruction(&Instruction::I64Const(box_none()));
    func.instruction(&Instruction::Else);
    for (index, value_name) in value_names.iter().enumerate() {
        func.instruction(&Instruction::LocalGet(scratch));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(locals[value_name]));
        func.instruction(&Instruction::I64Store(word_memarg(index)));
    }
    emit_word_range_call(
        func,
        constructor,
        Instruction::LocalGet(scratch),
        value_names.len(),
        import_ids,
        reloc_enabled,
    );
    // The runtime copied the range; free it while the result waits on the stack.
    func.instruction(&Instruction::LocalGet(scratch));
    func.instruction(&Instruction::I64Const(bytes));
    emit_call(
        func,
        reloc_enabled,
        import_ids[WasmRuntimeImport::ScratchFree],
    );
    func.instruction(&Instruction::End);
    func.instruction(&Instruction::LocalSet(out));
}

fn emit_word_range_call(
    func: &mut Function,
    constructor: WordRangeConstructor<'_>,
    address: Instruction<'_>,
    count: usize,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    for &local in constructor.leading {
        func.instruction(&Instruction::LocalGet(local));
    }
    func.instruction(&address);
    func.instruction(&Instruction::I64Const(count as i64));
    for &local in constructor.trailing {
        func.instruction(&Instruction::LocalGet(local));
    }
    emit_call(func, reloc_enabled, import_ids[constructor.import]);
}

fn word_memarg(index: usize) -> MemArg {
    MemArg {
        offset: (index as u64) * 8,
        align: I64_ALIGN_EXPONENT,
        memory_index: 0,
    }
}
