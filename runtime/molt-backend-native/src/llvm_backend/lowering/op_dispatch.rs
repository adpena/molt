use super::*;
#[cfg(feature = "llvm")]
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        // The walker's dispatch first (`FunctionCx::walker_owned_op`); the
        // arms below are the families not yet moved into it.
        if self.walker_owned(source_block, op) {
            return;
        }
        match op.opcode {
            // -- SSA Copy --
            // Also serves as the fallback for unknown frontend ops that were
            // mapped to Copy by the SSA converter.  Handle all combinations of
            // operand/result counts gracefully:
            //   - 0 operands, 0 results: no-op (side-effect only)
            //   - 0 operands, 1+ results: produce NaN-boxed None per result
            //   - 1+ operands, 0 results: no-op (side-effect only)
            //   - 1+ operands, 1+ results: pass-through first operand
            OpCode::FrameSetLocation => {
                assert!(self.lower_preserved_simpleir_op(op, OpKind::FrameSetLocation));
            }
            OpCode::Copy => {
                let original_kind = op.origin;
                if let Some(kind) = original_kind
                    && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
                // A preserved op no arm claimed is refused; a genuine SSA copy
                // passes operand 0 through (`FunctionCx::copy_op`).
                self.walker_op(|cx, b| cx.copy_op(b, op));
            }

            // -- Async / generator / channel state machine --
            OpCode::AllocTask => self.emit_alloc_task(op),
            OpCode::StateSwitch => self.emit_state_switch(),
            OpCode::FrameContextSet => self.emit_frame_context_set(op),
            OpCode::StateYield | OpCode::StateTransition => panic!(
                "{}: the shared terminal drop pass must expose it as explicit activation exits before LLVM emission",
                self.func.name
            ),
            OpCode::StateSet => self.emit_state_set(op),
            OpCode::IsPending => self.emit_is_pending(op),
            OpCode::TaskWait => self.emit_task_wait(op),
            OpCode::Yield => self.emit_yield(op),
            OpCode::YieldFrom => self.emit_yield_from(op),

            // -- SCF dialect ops --
            // These must reach LLVM as real CFG, loop, and phi structure. The
            // old runtime-delegate names were never exported by molt-runtime, so
            // keeping them as declarations converted missing lowering into link
            // failures. Fail closed here until the structured lowering pass owns
            // the full CFG form.
            OpCode::ScfIf => self.record_unlowered_structured_op(
                op,
                "lower structured if/else regions into LLVM CFG before codegen",
            ),
            OpCode::ScfFor => self.record_unlowered_structured_op(
                op,
                "lower counted or iterator loops into LLVM CFG before codegen",
            ),
            OpCode::ScfWhile => self.record_unlowered_structured_op(
                op,
                "lower while regions into LLVM CFG before codegen",
            ),
            OpCode::ScfYield => self.record_unlowered_structured_op(
                op,
                "thread SCF region results through block arguments or phi nodes before codegen",
            ),

            _ => unreachable!("the walker lowers {:?}", op.opcode),
        }
    }
}
