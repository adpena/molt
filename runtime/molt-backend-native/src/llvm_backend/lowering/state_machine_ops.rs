use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_alloc_task(&mut self, op: &TirOp) {
        let (mut cx, mut b) = self.walker();
        cx.alloc_task(&mut b, op);
    }

    pub(super) fn lower_preserved_call_async_op(&mut self, op: &TirOp) -> bool {
        let (mut cx, mut b) = self.walker();
        cx.call_async(&mut b, op)
    }

    pub(super) fn emit_state_switch(&mut self) {
        // `state_switch` is now lowered as the first-class `StateDispatch`
        // terminator (see `lower_terminator`), never as a body op: the
        // SimpleIR `state_switch` op is structural (excluded from TIR
        // block ops by `gather_defs_uses`) and the SSA terminator builder
        // emits `Terminator::StateDispatch` for the dispatch block.
        // Reaching it here as a body op means the structural-op invariant
        // broke upstream — fail loud rather than emit a second (synthetic)
        // dispatch that double-switches the saved state.
        panic!(
            "OpCode::StateSwitch reached the LLVM op-lowering body in '{}'; \
             state_switch must lower as the StateDispatch terminator",
            self.func.name
        );
    }

    /// `molt_closure_load` / `molt_closure_store` address the owner's payload
    /// directly: their first word is an object address, not a boxed operand. A
    /// boxed pointer is unboxed, and a poll frame's raw `self` is unchanged.
    pub(super) fn closure_owner_address(&self, owner: ValueId) -> inkwell::values::IntValue<'ctx> {
        let owner_bits = self.ensure_i64(self.resolve(owner));
        self.unbox_ptr_bits(owner_bits)
    }

    pub(super) fn emit_frame_context_set(&mut self, op: &TirOp) {
        let publish = self.ensure_runtime_import(RuntimeFn::FrameContextSet);
        let args: Vec<RuntimeArg<'ctx>> = op.operands[..3]
            .iter()
            .map(|&operand| RuntimeArg::Operand(operand))
            .collect();
        // Its canonical borrowed None needs no retirement; exception transfer
        // remains explicit in TIR.
        self.emit_borrowed_runtime_call(
            op,
            publish,
            &args,
            ResultCustody::canonical(RuntimeFn::FrameContextSet, 3),
            "frame_context_set",
            "frame_context_set",
        );
    }

    /// Save the state named by the transition through the poll frame
    /// parameter. Saving neither suspends nor returns; a ready wait saves its
    /// running state, which no StateDispatch case resumes.
    pub(super) fn emit_state_set(&mut self, op: &TirOp) {
        let frame = self.poll_frame_address();
        let (cx, mut b) = self.walker();
        cx.state_set(&mut b, op, frame);
    }

    pub(super) fn emit_is_pending(&mut self, op: &TirOp) {
        let (mut cx, mut b) = self.walker();
        cx.is_pending(&mut b, op);
    }

    pub(super) fn emit_task_wait(&mut self, op: &TirOp) {
        let frame = self.poll_frame_address();
        let (cx, mut b) = self.walker();
        cx.task_wait(&mut b, op, frame);
    }

    pub(super) fn emit_yield(&mut self, op: &TirOp) {
        self.record_unlowered_structured_op(
                op,
                "lower generators through explicit state-machine poll/resume blocks before LLVM codegen",
        );
    }

    pub(super) fn emit_yield_from(&mut self, op: &TirOp) {
        self.record_unlowered_structured_op(
                op,
                "lower generator delegation through explicit state-machine poll/resume blocks before LLVM codegen",
        );
    }
}
