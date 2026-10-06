use super::*;
use molt_tir::trampolines::TaskConstructorLayout;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_alloc_task(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let i64_ty = self.backend.context.i64_type();
        let closure_size = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let task_kind = op.attrs.get("task_kind").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        });
        let layout = TaskConstructorLayout::for_alloc_kind(task_kind);
        let Some(poll_func_name) = op.attrs.get("s_value").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        }) else {
            panic!(
                "alloc_task missing poll function name in {}",
                self.func.name
            );
        };
        let poll_fn = self.ensure_function_symbol(poll_func_name, 1, false);
        let poll_addr = self
            .backend
            .builder
            .build_ptr_to_int(
                poll_fn.as_global_value().as_pointer_value(),
                i64_ty,
                "task_poll_ptr",
            )
            .unwrap();
        let task_bits = self.emit_task_new_with_payload(
            poll_addr,
            closure_size,
            layout,
            &op.operands,
            "task_new",
        );
        self.values.insert(result_id, task_bits);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    pub(super) fn lower_preserved_call_async_op(&mut self, op: &TirOp) -> bool {
        let i64_ty = self.backend.context.i64_type();
        let Some(poll_func_name) = op.attrs.get("s_value").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        }) else {
            return false;
        };
        let Some(&result_id) = op.results.first() else {
            return false;
        };
        if poll_func_name == "molt_async_sleep" {
            if op.operands.len() > 2 {
                return false;
            }
            // The sleep future retains its delay and result. Absent operands
            // default to the float 0.0 and None, whose words own nothing.
            let defaults = [
                i64_ty.const_int(0.0_f64.to_bits(), false),
                i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false),
            ];
            let args: Vec<RuntimeArg<'ctx>> = defaults
                .iter()
                .enumerate()
                .map(|(idx, &default)| match op.operands.get(idx) {
                    Some(&operand) => RuntimeArg::Operand(operand),
                    None => RuntimeArg::Word(default.into()),
                })
                .collect();
            let sleep_fn = self.ensure_runtime_i64_fn("molt_async_sleep", 2);
            self.emit_borrowed_runtime_call(
                op,
                sleep_fn,
                &args,
                Self::canonical_boxed_return("molt_async_sleep", 2),
                "async_sleep",
                "call_async_sleep",
            );
            return true;
        }

        let poll_fn = self.ensure_function_symbol(poll_func_name, 1, false);
        let poll_addr = self
            .backend
            .builder
            .build_ptr_to_int(
                poll_fn.as_global_value().as_pointer_value(),
                i64_ty,
                "call_async_poll_ptr",
            )
            .unwrap();
        let layout = TaskConstructorLayout::for_call_async();
        let task_bits = self.emit_task_new_with_payload(
            poll_addr,
            layout.required_closure_size(op.operands.len(), false, crate::GENERATOR_CONTROL_BYTES),
            layout,
            &op.operands,
            "call_async_task_new",
        );
        self.values.insert(result_id, task_bits);
        self.value_types.insert(result_id, TirType::DynBox);
        true
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
    fn closure_owner_address(&self, owner: ValueId) -> inkwell::values::IntValue<'ctx> {
        let owner_bits = self.ensure_i64(self.resolve(owner));
        self.unbox_ptr_bits(owner_bits)
    }

    pub(super) fn emit_closure_load(&mut self, op: &TirOp) {
        let owner_bits = self.closure_owner_address(op.operands[0]);
        let offset = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let load_fn = self.ensure_runtime_i64_fn("molt_closure_load", 2);
        // The loaded slot value is returned retained.
        let result = self
            .backend
            .builder
            .build_call(
                load_fn,
                &[
                    owner_bits.into(),
                    self.backend
                        .context
                        .i64_type()
                        .const_int(offset as u64, true)
                        .into(),
                ],
                "closure_load",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        self.bind_owned_runtime_result(op, result);
    }

    pub(super) fn emit_frame_context_set(&mut self, op: &TirOp) {
        let publish = self.ensure_runtime_i64_fn("molt_frame_context_set", 3);
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
            Self::canonical_boxed_return("molt_frame_context_set", 3),
            "frame_context_set",
            "frame_context_set",
        );
    }

    pub(super) fn emit_closure_store(&mut self, op: &TirOp) {
        let owner_bits = self.closure_owner_address(op.operands[0]);
        let offset = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let offset_bits = self
            .backend
            .context
            .i64_type()
            .const_int(offset as u64, true);
        let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
        // The payload retains the borrowed value; the runtime returns None.
        self.emit_borrowed_runtime_call(
            op,
            store_fn,
            &[
                RuntimeArg::Word(owner_bits.into()),
                RuntimeArg::Word(offset_bits.into()),
                RuntimeArg::Operand(op.operands[1]),
            ],
            RuntimeResultCustody::Unowned,
            "closure_store",
            "closure_store",
        );
    }

    /// Save the state named by the transition through the poll frame
    /// parameter. Saving neither suspends nor returns; a ready wait saves its
    /// running state, which no StateDispatch case resumes.
    pub(super) fn emit_state_set(&mut self, op: &TirOp) {
        let Some(AttrValue::Int(state)) = op.attrs.get("value") else {
            panic!("state_set in '{}' requires its state", self.func.name);
        };
        let state = *state;
        let self_bits = self.generator_self_bits();
        let state_bits = self
            .backend
            .context
            .i64_type()
            .const_int(state as u64, true);
        let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
        self.backend
            .builder
            .build_call(set_state_fn, &[self_bits.into(), state_bits.into()], "")
            .unwrap();
    }

    /// The scheduler sentinel is one exact word: compare it, with no
    /// truthiness, callback or owner.
    pub(super) fn emit_is_pending(&mut self, op: &TirOp) {
        let &[poll] = op.operands.as_slice() else {
            panic!("is_pending in '{}' expects one poll result", self.func.name);
        };
        let word = self.activation_object_word(poll, "is_pending");
        let pending = self
            .backend
            .context
            .i64_type()
            .const_int(molt_codegen_abi::pending_bits() as u64, true);
        let is_pending = self
            .backend
            .builder
            .build_int_compare(inkwell::IntPredicate::EQ, word, pending, "is_pending")
            .unwrap();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, is_pending.into());
            self.value_types.insert(result_id, TirType::Bool);
        }
    }

    /// Register this activation to wake when the future completes. Nothing is
    /// retained or returned: explicit TIR still owns the future.
    pub(super) fn emit_task_wait(&mut self, op: &TirOp) {
        let &[future] = op.operands.as_slice() else {
            panic!("task_wait in '{}' expects one future", self.func.name);
        };
        let future_bits = self.activation_object_word(future, "task_wait");
        self.emit_sleep_register(future_bits);
    }

    /// Poll results and awaited futures are objects. A raw scalar carrier could
    /// alias the sentinel's bits and would need a box that nothing owns.
    fn activation_object_word(
        &self,
        operand: ValueId,
        kind: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let ty = self
            .value_types
            .get(&operand)
            .cloned()
            .unwrap_or(TirType::DynBox);
        assert!(
            Self::tir_type_is_dynbox_like(&ty),
            "{kind} in '{}' requires an object carrier, found {ty:?}",
            self.func.name
        );
        self.ensure_i64(self.resolve(operand))
    }

    /// `molt_sleep_register` takes object addresses, not boxed words: the poll
    /// frame parameter and the future's unboxed pointer.
    fn emit_sleep_register(&self, future_bits: inkwell::values::IntValue<'ctx>) {
        let i64_ty = self.backend.context.i64_type();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let fn_ty = i64_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false);
        let sleep_fn = declare_fixed_runtime_function(
            self.backend.context,
            &self.backend.module,
            "molt_sleep_register",
        )
        .unwrap_or_else(|| panic!("molt_sleep_register must be a fixed LLVM runtime import"));
        let sleep_fn = require_llvm_function_type("molt_sleep_register", sleep_fn, fn_ty);
        let frame_ptr = self
            .backend
            .builder
            .build_int_to_ptr(self.generator_self_bits(), ptr_ty, "task_wait_frame")
            .unwrap();
        let future_address = self.unbox_ptr_bits(future_bits);
        let future_ptr = self
            .backend
            .builder
            .build_int_to_ptr(future_address, ptr_ty, "task_wait_future")
            .unwrap();
        self.backend
            .builder
            .build_call(
                sleep_fn,
                &[frame_ptr.into(), future_ptr.into()],
                "task_wait",
            )
            .unwrap();
    }

    pub(super) fn emit_yield(&mut self, op: &TirOp) {
        self.record_removed_runtime_delegate(
            op,
            "molt_yield",
            "lower generators through explicit state-machine poll/resume blocks before LLVM codegen",
        );
    }

    pub(super) fn emit_yield_from(&mut self, op: &TirOp) {
        self.record_removed_runtime_delegate(
            op,
            "molt_yield_from",
            "lower generator delegation through explicit state-machine poll/resume blocks before LLVM codegen",
        );
    }
}
