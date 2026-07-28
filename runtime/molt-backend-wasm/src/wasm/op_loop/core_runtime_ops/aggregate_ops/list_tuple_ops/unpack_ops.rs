use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_binary::emit_call;
use molt_codegen_abi::box_none_bits;
use wasm_encoder::{BlockType, Function, Instruction, MemArg};

const I64_ALIGN_EXPONENT: u32 = 3;

fn result_memarg(index: usize) -> MemArg {
    MemArg {
        offset: (index as u64) * 8,
        align: I64_ALIGN_EXPONENT,
        memory_index: 0,
    }
}

pub(super) fn emit_unpack_sequence(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) {
    let mut source = None;
    let mut read_count = 0;
    molt_tir::tir::simple_def_use::visit_simple_ir_reads(op, |read| {
        read_count += 1;
        source.get_or_insert(read.name);
    });
    let mut output_count = 0;
    molt_tir::tir::simple_def_use::visit_simple_ir_result_names(op, |_| {
        output_count += 1;
    });
    let expected_count = usize::try_from(op.value.unwrap_or_default())
        .expect("verified unpack_sequence count must fit usize");
    assert_eq!(
        (read_count, output_count),
        (1, expected_count),
        "verified unpack_sequence must name one source and every result"
    );

    let seq = ctx.locals[source.expect("verified unpack source")];
    let unpack_import =
        ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::UnpackSequence];

    // Even a zero-target assignment must consume the iterable and validate
    // exact arity. A null output pointer is the runtime ABI for that case.
    if expected_count == 0 {
        func.instruction(&Instruction::LocalGet(seq));
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::I64Const(0));
        emit_call(func, ctx.reloc_enabled, unpack_import);
        func.instruction(&Instruction::Drop);
        return;
    }

    // Result locals are transactionally initialized before allocation. The
    // checked scratch allocator raises on failure; success transfers exactly
    // one owned result from the runtime buffer into each local.
    molt_tir::tir::simple_def_use::visit_simple_ir_result_names(op, |out_name| {
        func.instruction(&Instruction::I64Const(box_none_bits() as i64));
        func.instruction(&Instruction::LocalSet(ctx.locals[out_name]));
    });

    let scratch = ctx.locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
    let scratch_bytes = (expected_count as u64)
        .checked_mul(8)
        .expect("verified unpack_sequence result buffer size overflow");
    func.instruction(&Instruction::I64Const(scratch_bytes as i64));
    emit_call(
        func,
        ctx.reloc_enabled,
        ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ScratchAlloc],
    );
    func.instruction(&Instruction::LocalTee(scratch));
    func.instruction(&Instruction::I64Eqz);
    func.instruction(&Instruction::If(BlockType::Empty));
    // ScratchAlloc owns MemoryError publication; preserve that pending error.
    func.instruction(&Instruction::Else);

    func.instruction(&Instruction::LocalGet(seq));
    func.instruction(&Instruction::I64Const(expected_count as i64));
    func.instruction(&Instruction::LocalGet(scratch));
    emit_call(func, ctx.reloc_enabled, unpack_import);
    func.instruction(&Instruction::Drop);

    let mut index = 0;
    molt_tir::tir::simple_def_use::visit_simple_ir_result_names(op, |out_name| {
        func.instruction(&Instruction::LocalGet(scratch));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::I64Load(result_memarg(index)));
        func.instruction(&Instruction::LocalSet(ctx.locals[out_name]));
        index += 1;
    });

    func.instruction(&Instruction::LocalGet(scratch));
    func.instruction(&Instruction::I64Const(scratch_bytes as i64));
    emit_call(
        func,
        ctx.reloc_enabled,
        ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ScratchFree],
    );
    func.instruction(&Instruction::End);
}
