use super::*;
use molt_ir::OpKind;

// Handler-owned routing slice. `code_new` and the CallArgs push/expand
// steps are exact `molt_<kind>` boxed-ABI calls and take the shared admitted
// boxed-call path instead of an arm here.
define_llvm_op_kinds!(
    OpKind::BuiltinFunc,
    OpKind::FuncNew,
    OpKind::FuncNewClosure,
    OpKind::FunctionClosureBits,
    OpKind::AsyncgenNew,
    OpKind::CodeSlotSet,
    OpKind::CodeSlotsInit,
    OpKind::TraceEnterSlot,
    OpKind::TraceExit,
    OpKind::FrameHomeStore,
    OpKind::FrameHomeCell,
    OpKind::FrameHomePrivateCell,
    OpKind::FrameHomeLoad,
    OpKind::FrameHomeTake,
    OpKind::FrameHomeClear,
    OpKind::FrameSetLocation,
    OpKind::FrameLocals,
    OpKind::FrameLocalsSet,
    OpKind::Line,
    OpKind::CallargsNew,
);

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(in crate::llvm_backend::lowering) fn lend_chunk_frame_homes(&mut self) {
        let Some(slot) = self.frame_homes else { return };
        let frame = self.owned_frame_entered;
        if frame.is_some_and(|owned| owned.slot == slot) {
            return;
        }
        let abi = require_function_linkage_abi(self.func, self.backend);
        let (mut cx, mut b) = self.walker();
        crate::codegen_ssa::frame::lend_homes(&mut b, slot, |b| {
            crate::codegen_ssa::returns::emit_return(&mut cx, b, &abi, None, frame.as_ref())
        });
    }

    pub(super) fn lower_preserved_callable_op(&mut self, op: &TirOp, kind: OpKind) -> bool {
        match kind {
            OpKind::BuiltinFunc
            | OpKind::FuncNew
            | OpKind::FuncNewClosure
            | OpKind::FunctionClosureBits
            | OpKind::AsyncgenNew
            | OpKind::CodeSlotSet
            | OpKind::CodeSlotsInit
            | OpKind::CallargsNew
            | OpKind::FrameLocals
            | OpKind::FrameLocalsSet
            | OpKind::Line => {
                let mut lowered = false;
                self.walker_op(|cx, b| lowered = cx.callable_preserved_op(b, op, kind));
                lowered
            }
            OpKind::TraceEnterSlot => {
                let frame = self
                    .owned_frame_entered
                    .expect("trace_enter_slot requires local execution-context ownership");
                let code_id = self.program_slot_id(molt_ir::code_slots::ProgramSlotKind::Code, op);
                let base = self.emit_frame_record_push(code_id, frame.kind);
                self.backend.builder.build_store(frame.slot, base).unwrap();
                true
            }
            OpKind::TraceExit => {
                // The invocation's guarded return owns the pop, including
                // generated suspension and early-error returns.
                true
            }
            OpKind::FrameHomeStore
            | OpKind::FrameHomeCell
            | OpKind::FrameHomePrivateCell
            | OpKind::FrameHomeClear
            | OpKind::FrameHomeTake
            | OpKind::FrameHomeLoad
            | OpKind::FrameSetLocation => {
                let (mut cx, mut b) = self.walker();
                cx.frame_home_op(&mut b, op)
            }
            // A statement marker: it emits no code. Tracebacks read the
            // location that `frame_set_location` publishes before each
            // raising op.
            _ => unreachable!("preserved callable kind predicate accepted unknown kind {kind}"),
        }
    }
}
