use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;

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

    /// The closure slot read (`molt_codegen_abi::closure_slot`), always
    /// inlined: `(context, owner, offset, admission) -> owned word`.
    fn ensure_closure_load_helper(&self) -> FunctionValue<'ctx> {
        self.backend
            .site_helper("__molt_closure_load", 3, |b, words| {
                let &[owner, offset] = words else {
                    unreachable!("the closure load helper's parameters")
                };
                molt_codegen_abi::closure_slot::generate_closure_load(b, owner, offset)
            })
    }

    /// The closure slot store, whose miss calls `molt_closure_store(owner,
    /// offset, value)`: `(context, owner, offset, value, admission) -> None`.
    fn ensure_closure_store_helper(&self) -> FunctionValue<'ctx> {
        let cold_fn = self.ensure_runtime_import(RuntimeFn::ClosureStore);
        self.backend
            .site_helper("__molt_closure_store", 4, |b, words| {
                let &[owner, offset, value] = words else {
                    unreachable!("the closure store helper's parameters")
                };
                molt_codegen_abi::closure_slot::generate_closure_store(
                    b,
                    owner,
                    offset,
                    value,
                    molt_codegen_abi::list_item::ListLocking::BUILD,
                    |b| {
                        b.words
                            .builder
                            .build_execution_call(
                                cold_fn,
                                &[owner.into(), offset.into(), value.into()],
                                "closure_store_miss",
                            )
                            .unwrap();
                    },
                );
                use molt_codegen_abi::word::WordBuilder;
                b.constant(molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE)
            })
    }

    fn closure_slot_offset(op: &TirOp) -> u64 {
        op.attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v as u64),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub(super) fn emit_closure_load(&mut self, op: &TirOp) {
        let owner_bits = self.closure_owner_address(op.operands[0]);
        let offset = self
            .backend
            .context
            .i64_type()
            .const_int(Self::closure_slot_offset(op), true);
        let load_fn = self.ensure_closure_load_helper();
        let admission = self.list_item_admission();
        // The loaded slot value is returned retained.
        let result = self
            .backend
            .builder
            .build_execution_call(
                load_fn,
                &[owner_bits.into(), offset.into(), admission.into()],
                "closure_load",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        self.bind_owned_runtime_result(op, result);
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

    pub(super) fn emit_closure_store(&mut self, op: &TirOp) {
        let owner_bits = self.closure_owner_address(op.operands[0]);
        let offset_bits = self
            .backend
            .context
            .i64_type()
            .const_int(Self::closure_slot_offset(op), true);
        let store_fn = self.ensure_closure_store_helper();
        // The payload retains the borrowed value; the body returns None.
        self.emit_borrowed_runtime_call(
            op,
            store_fn,
            &[
                RuntimeArg::Word(owner_bits.into()),
                RuntimeArg::Word(offset_bits.into()),
                RuntimeArg::Operand(op.operands[1]),
                RuntimeArg::Word(self.list_item_admission().into()),
            ],
            ResultCustody::Unowned,
            "closure_store",
            "closure_store",
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
