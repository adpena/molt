use super::RuntimeServiceOpContext;
use crate::OpIR;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_linear_memory_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let locals = context.locals;

    match op.kind.as_str() {
        // ---------------------------------------------------------------
        // memory_copy: bulk linear-memory copy (WASM 2.0 bulk-memory op)
        //
        // IR signature:  memory_copy(dst, src, len)
        //   dst, src  - i64 boxed integers holding i32 linear-memory byte
        //               offsets (e.g. from handle_resolve)
        //   len       - i64 boxed integer holding the byte count
        //
        // Emits:  memory.copy  (dst_mem=0, src_mem=0)
        //         stack: [dst:i32, src:i32, len:i32]
        //
        // This intrinsic enables the IR to emit efficient buffer-to-buffer
        // copies without round-tripping through host imports.  See
        // WASM_OPTIMIZATION_PLAN.md Section 3.3.
        // ---------------------------------------------------------------
        "memory_copy" => {
            let args = op.args.as_ref().unwrap();
            debug_assert!(
                args.len() == 3,
                "memory_copy requires exactly 3 args (dst, src, len)"
            );
            let dst = locals[&args[0]];
            let src = locals[&args[1]];
            let len = locals[&args[2]];
            // Unbox each i64 value to i32 for the memory.copy instruction.
            func.instruction(&Instruction::LocalGet(dst));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
        }
        // ---------------------------------------------------------------
        // memory_fill: bulk linear-memory fill (WASM 2.0 bulk-memory op)
        //
        // IR signature:  memory_fill(dst, val, len)
        //   dst  - i64 boxed integer holding i32 linear-memory byte offset
        //   val  - i64 boxed integer holding the fill byte (0-255)
        //   len  - i64 boxed integer holding the byte count
        //
        // Emits:  memory.fill  (mem=0)
        //         stack: [dst:i32, val:i32, len:i32]
        //
        // Enables efficient zero-init and constant-fill of linear memory
        // regions without round-tripping through host imports or byte loops.
        // ---------------------------------------------------------------
        "memory_fill" => {
            let args = op.args.as_ref().unwrap();
            debug_assert!(
                args.len() == 3,
                "memory_fill requires exactly 3 args (dst, val, len)"
            );
            let dst = locals[&args[0]];
            let val = locals[&args[1]];
            let len = locals[&args[2]];
            // Unbox each i64 value to i32 for the memory.fill instruction.
            func.instruction(&Instruction::LocalGet(dst));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::MemoryFill(0));
        }
        _ => return false,
    }
    true
}
