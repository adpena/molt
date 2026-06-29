use super::*;

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
        let task_kind = op
            .attrs
            .get("task_kind")
            .and_then(|v| match v {
                AttrValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("future");
        let (kind_bits, payload_base) = match task_kind {
            "generator" => (crate::TASK_KIND_GENERATOR, crate::GENERATOR_CONTROL_BYTES),
            "future" => (crate::TASK_KIND_FUTURE, 0),
            "coroutine" => (crate::TASK_KIND_COROUTINE, 0),
            _ => panic!("unknown task kind: {task_kind}"),
        };
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
            kind_bits as i64,
            payload_base,
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
            let delay_bits = op
                .operands
                .first()
                .map(|&id| self.materialize_dynbox_operand(id))
                .unwrap_or_else(|| {
                    let zero: BasicValueEnum<'ctx> =
                        self.backend.context.f64_type().const_float(0.0).into();
                    self.materialize_dynbox_bits(zero, &TirType::F64)
                });
            let result_bits = op
                .operands
                .get(1)
                .map(|&id| self.materialize_dynbox_operand(id))
                .unwrap_or_else(|| i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false));
            let sleep_fn = self.ensure_runtime_i64_fn("molt_async_sleep", 2);
            let result = self
                .backend
                .builder
                .build_call(
                    sleep_fn,
                    &[delay_bits.into(), result_bits.into()],
                    "call_async_sleep",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
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
        let task_bits = self.emit_task_new_with_payload(
            poll_addr,
            (op.operands.len() * 8) as i64,
            crate::TASK_KIND_FUTURE,
            0,
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

    pub(super) fn emit_closure_load(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let self_bits = self.materialize_dynbox_operand(op.operands[0]);
        let offset = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let load_fn = self.ensure_runtime_i64_fn("molt_closure_load", 2);
        let result = self
            .backend
            .builder
            .build_call(
                load_fn,
                &[
                    self_bits.into(),
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
        self.values.insert(result_id, result);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    pub(super) fn emit_closure_store(&mut self, op: &TirOp) {
        let self_bits = self.materialize_dynbox_operand(op.operands[0]);
        let val_bits = self.materialize_dynbox_operand(op.operands[1]);
        let offset = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
        let result = self
            .backend
            .builder
            .build_call(
                store_fn,
                &[
                    self_bits.into(),
                    self.backend
                        .context
                        .i64_type()
                        .const_int(offset as u64, true)
                        .into(),
                    val_bits.into(),
                ],
                "closure_store",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
    }

    pub(super) fn emit_state_yield(&mut self, op: &TirOp) {
        let next_state_id = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let self_bits = self.generator_self_bits();
        let pair_bits = self.materialize_dynbox_operand(op.operands[0]);
        let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[
                    self_bits.into(),
                    self.backend
                        .context
                        .i64_type()
                        .const_int(next_state_id as u64, true)
                        .into(),
                ],
                "state_yield_set_state",
            )
            .unwrap();
        let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
        let _ = self
            .backend
            .builder
            .build_call(inc_fn, &[pair_bits.into()], "state_yield_inc_ref")
            .unwrap();
        // The suspend `ret`s the yielded pair.  This `build_return`
        // terminates the suspend block; the main lowering loop detects
        // the terminator and moves on to the NEXT TIR block (the real
        // post-yield resume continuation, which the `StateDispatch`
        // terminator dispatches to).  We do NOT `position_at_end` into a
        // synthetic resume block — the continuation is a first-class TIR
        // block reached via the dispatch, and its phis were placed by the
        // SSA pass on the real `state_resume_edges`.
        self.backend.builder.build_return(Some(&pair_bits)).unwrap();
        let _ = next_state_id;
    }

    pub(super) fn emit_state_transition(&mut self, op: &TirOp) {
        let (slot_id, pending_state_operand) = match op.operands.as_slice() {
            [_, pending_state] => (None, *pending_state),
            [_, slot, pending_state] => (Some(*slot), *pending_state),
            other => panic!(
                "state_transition expected 2 or 3 operands in {}: {:?}",
                self.func.name, other
            ),
        };
        let pending_state_id = self.const_i64_operand(pending_state_operand);
        let next_state_id = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let pending_bb = self.resume_block_for_state(pending_state_id);
        let current_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("state_transition must be inside a block");
        if current_bb != pending_bb {
            self.record_llvm_edge(current_bb, pending_bb);
            self.backend
                .builder
                .build_unconditional_branch(pending_bb)
                .unwrap();
            self.backend.builder.position_at_end(pending_bb);
        }
        let i64_ty = self.backend.context.i64_type();
        let self_bits = self.generator_self_bits();
        let future_bits = self.materialize_dynbox_operand(op.operands[0]);
        let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
        let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[self_bits.into(), pending_state_bits.into()],
                "state_transition_set_pending",
            )
            .unwrap();
        let poll_fn = self.ensure_runtime_i64_fn("molt_future_poll", 1);
        let res = self
            .backend
            .builder
            .build_call(poll_fn, &[future_bits.into()], "state_transition_poll")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let pending_const = i64_ty.const_int(molt_codegen_abi::pending_bits() as u64, true);
        let is_pending = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                res,
                pending_const,
                "state_transition_is_pending",
            )
            .unwrap();
        let pending_path = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("state_transition_pending{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        let ready_path = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("state_transition_ready{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        self.all_llvm_blocks.push(pending_path);
        self.all_llvm_blocks.push(ready_path);
        let branch_from_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("state_transition branch must be in block");
        self.record_llvm_edge(branch_from_bb, pending_path);
        self.record_llvm_edge(branch_from_bb, ready_path);
        self.backend
            .builder
            .build_conditional_branch(is_pending, pending_path, ready_path)
            .unwrap();
        self.backend.builder.position_at_end(pending_path);
        let sleep_fn = self.ensure_runtime_i64_fn("molt_sleep_register", 2);
        let _ = self
            .backend
            .builder
            .build_call(
                sleep_fn,
                &[self_bits.into(), future_bits.into()],
                "state_transition_sleep",
            )
            .unwrap();
        self.backend
            .builder
            .build_return(Some(&pending_const))
            .unwrap();
        self.backend.builder.position_at_end(ready_path);
        if let Some(slot_id) = slot_id {
            let slot_bits = self.raw_i64_operand(slot_id, ready_path);
            let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
            let _ = self
                .backend
                .builder
                .build_call(
                    store_fn,
                    &[self_bits.into(), slot_bits.into(), res.into()],
                    "state_transition_store",
                )
                .unwrap();
        } else if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, res.into());
            self.value_types.insert(result_id, TirType::DynBox);
        }
        let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[self_bits.into(), next_state_bits.into()],
                "state_transition_set_next",
            )
            .unwrap();
        let next_bb = self.resume_block_for_state(next_state_id);
        let ready_from_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("state_transition ready must be in block");
        self.record_llvm_edge(ready_from_bb, next_bb);
        self.backend
            .builder
            .build_unconditional_branch(next_bb)
            .unwrap();
        self.backend.builder.position_at_end(next_bb);
    }

    pub(super) fn emit_chan_send_yield(&mut self, op: &TirOp) {
        let pending_state_id = self.const_i64_operand(op.operands[2]);
        let next_state_id = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let pending_bb = self.resume_block_for_state(pending_state_id);
        let current_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("chan_send_yield must be inside a block");
        if current_bb != pending_bb {
            self.record_llvm_edge(current_bb, pending_bb);
            self.backend
                .builder
                .build_unconditional_branch(pending_bb)
                .unwrap();
            self.backend.builder.position_at_end(pending_bb);
        }
        let i64_ty = self.backend.context.i64_type();
        let self_bits = self.generator_self_bits();
        let chan_bits = self.materialize_dynbox_operand(op.operands[0]);
        let val_bits = self.materialize_dynbox_operand(op.operands[1]);
        let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
        let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[self_bits.into(), pending_state_bits.into()],
                "chan_send_set_pending",
            )
            .unwrap();
        let send_fn = self.ensure_runtime_i64_fn("molt_chan_send", 2);
        let res = self
            .backend
            .builder
            .build_call(send_fn, &[chan_bits.into(), val_bits.into()], "chan_send")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let pending_const = i64_ty.const_int(molt_codegen_abi::pending_bits() as u64, true);
        let is_pending = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                res,
                pending_const,
                "chan_send_is_pending",
            )
            .unwrap();
        let pending_path = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("chan_send_pending{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        let ready_path = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("chan_send_ready{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        self.all_llvm_blocks.push(pending_path);
        self.all_llvm_blocks.push(ready_path);
        let branch_from_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("chan_send_yield branch must be in block");
        self.record_llvm_edge(branch_from_bb, pending_path);
        self.record_llvm_edge(branch_from_bb, ready_path);
        self.backend
            .builder
            .build_conditional_branch(is_pending, pending_path, ready_path)
            .unwrap();
        self.backend.builder.position_at_end(pending_path);
        self.backend
            .builder
            .build_return(Some(&pending_const))
            .unwrap();
        self.backend.builder.position_at_end(ready_path);
        let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[self_bits.into(), next_state_bits.into()],
                "chan_send_set_next",
            )
            .unwrap();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, res.into());
            self.value_types.insert(result_id, TirType::DynBox);
        }
        let next_bb = self.resume_block_for_state(next_state_id);
        self.record_llvm_edge(ready_path, next_bb);
        self.backend
            .builder
            .build_unconditional_branch(next_bb)
            .unwrap();
        self.backend.builder.position_at_end(next_bb);
    }

    pub(super) fn emit_chan_recv_yield(&mut self, op: &TirOp) {
        let pending_state_id = self.const_i64_operand(op.operands[1]);
        let next_state_id = op
            .attrs
            .get("value")
            .and_then(|v| match v {
                AttrValue::Int(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        let pending_bb = self.resume_block_for_state(pending_state_id);
        let current_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("chan_recv_yield must be inside a block");
        if current_bb != pending_bb {
            self.record_llvm_edge(current_bb, pending_bb);
            self.backend
                .builder
                .build_unconditional_branch(pending_bb)
                .unwrap();
            self.backend.builder.position_at_end(pending_bb);
        }
        let i64_ty = self.backend.context.i64_type();
        let self_bits = self.generator_self_bits();
        let chan_bits = self.materialize_dynbox_operand(op.operands[0]);
        let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
        let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[self_bits.into(), pending_state_bits.into()],
                "chan_recv_set_pending",
            )
            .unwrap();
        let recv_fn = self.ensure_runtime_i64_fn("molt_chan_recv", 1);
        let res = self
            .backend
            .builder
            .build_call(recv_fn, &[chan_bits.into()], "chan_recv")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let pending_const = i64_ty.const_int(molt_codegen_abi::pending_bits() as u64, true);
        let is_pending = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                res,
                pending_const,
                "chan_recv_is_pending",
            )
            .unwrap();
        let pending_path = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("chan_recv_pending{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        let ready_path = self.backend.context.append_basic_block(
            self.llvm_fn,
            &format!("chan_recv_ready{}", self.synthetic_block_counter),
        );
        self.synthetic_block_counter += 1;
        self.all_llvm_blocks.push(pending_path);
        self.all_llvm_blocks.push(ready_path);
        let branch_from_bb = self
            .backend
            .builder
            .get_insert_block()
            .expect("chan_recv_yield branch must be in block");
        self.record_llvm_edge(branch_from_bb, pending_path);
        self.record_llvm_edge(branch_from_bb, ready_path);
        self.backend
            .builder
            .build_conditional_branch(is_pending, pending_path, ready_path)
            .unwrap();
        self.backend.builder.position_at_end(pending_path);
        self.backend
            .builder
            .build_return(Some(&pending_const))
            .unwrap();
        self.backend.builder.position_at_end(ready_path);
        let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
        let _ = self
            .backend
            .builder
            .build_call(
                set_state_fn,
                &[self_bits.into(), next_state_bits.into()],
                "chan_recv_set_next",
            )
            .unwrap();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, res.into());
            self.value_types.insert(result_id, TirType::DynBox);
        }
        let next_bb = self.resume_block_for_state(next_state_id);
        self.record_llvm_edge(ready_path, next_bb);
        self.backend
            .builder
            .build_unconditional_branch(next_bb)
            .unwrap();
        self.backend.builder.position_at_end(next_bb);
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
