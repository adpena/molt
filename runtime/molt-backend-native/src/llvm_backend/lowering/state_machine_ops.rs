use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
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
