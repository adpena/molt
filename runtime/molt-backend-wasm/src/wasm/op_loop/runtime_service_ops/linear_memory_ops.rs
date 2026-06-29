use super::RuntimeServiceOpContext;
use crate::OpIR;
use crate::wasm_abi_generated::{
    WasmBulkMemoryInstruction, WasmBulkMemoryOpSpec, wasm_bulk_memory_op,
};
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_linear_memory_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let Some(spec) = wasm_bulk_memory_op(op.kind.as_str()) else {
        return false;
    };
    emit_bulk_memory_op(context, func, op, spec);
    true
}

fn emit_bulk_memory_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    spec: WasmBulkMemoryOpSpec,
) {
    let args = op.args.as_ref().unwrap_or_else(|| {
        panic!(
            "wasm bulk memory op '{}' expected {} args, got none",
            op.kind, spec.arg_count
        )
    });
    assert_eq!(
        args.len(),
        spec.arg_count,
        "wasm bulk memory op '{}' expected {} args, got {}",
        op.kind,
        spec.arg_count,
        args.len()
    );
    for arg in args {
        func.instruction(&Instruction::LocalGet(context.locals[arg]));
        func.instruction(&Instruction::I32WrapI64);
    }
    match spec.instruction {
        WasmBulkMemoryInstruction::Copy => {
            func.instruction(&Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
        }
        WasmBulkMemoryInstruction::Fill => {
            func.instruction(&Instruction::MemoryFill(0));
        }
    }
}
