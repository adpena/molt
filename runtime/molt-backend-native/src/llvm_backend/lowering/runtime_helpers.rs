use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Lower a preserved operation through its admitted runtime boxed-call ABI.
    /// Both bound and discarded calls execute: owned results are bound or
    /// released, while void calls produce no value. Generated semantic facts,
    /// not symbol spelling or i64 carriers, authorize this route. Dedicated
    /// raw/mixed-ABI operations are handled before this dispatch; an unclassified
    /// runtime symbol fails closed instead of borrowing operand-zero semantics.
    pub(super) fn try_lower_preserved_runtime_call(&mut self, op: &TirOp, kind: &str) -> bool {
        let symbol = format!("molt_{kind}");
        if !self.backend.runtime_callable_symbols.contains(&symbol) {
            return false;
        }
        let Some(abi) = runtime_boxed_abi(&symbol, op.operands.len()) else {
            self.record_fatal(format!(
                "preserved SimpleIR op `{kind}` maps to runtime symbol `{symbol}`, \
                 but that symbol has no positional boxed-value ABI classification"
            ));
            return true;
        };
        self.emit_boxed_runtime_call(op, abi);
        true
    }

    /// Emit a classified positional boxed ABI call for either a direct runtime
    /// CALL or a preserved operation. Both routes share boxing and return rules.
    pub(super) fn emit_boxed_runtime_call(&mut self, op: &TirOp, abi: &RuntimeBoxedAbi) {
        let symbol = abi.symbol;
        if op.operands.len() != abi.arity {
            self.record_fatal(format!(
                "boxed runtime symbol `{symbol}` has mismatched arity"
            ));
            return;
        }
        let return_abi = match abi.result {
            RuntimeBoxedReturn::OwnedValue => RuntimeReturnAbi::I64,
            RuntimeBoxedReturn::Void => RuntimeReturnAbi::Void,
        };
        // Keep declaration custody distinct from value semantics. Dedicated
        // raw/mixed lowering may use the same machine-signature authority.
        if runtime_import_return_abi(symbol, abi.arity) != Some(return_abi) {
            self.record_fatal(format!(
                "boxed runtime symbol `{symbol}` has no matching LLVM machine ABI"
            ));
            return;
        }
        if op.results.len() > 1 {
            self.record_fatal(format!(
                "boxed runtime symbol `{symbol}` has multiple result values"
            ));
            return;
        }
        if return_abi == RuntimeReturnAbi::Void && !op.results.is_empty() {
            self.record_fatal(format!(
                "call to void runtime symbol `{symbol}` has result values"
            ));
            return;
        }
        // Runtime arguments borrow their object values. Materializing a raw
        // full-width integer may mint a temporary BigInt owner; the original
        // raw SSA value's RC cannot retire that owner. Already-boxed values
        // remain borrowed and must not enter this cleanup list.
        let mut temporary_owners = Vec::new();
        let arg_bits: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
            .operands
            .iter()
            .map(|&id| {
                let bits = self.materialize_dynbox_operand(id);
                if self.value_types.get(&id) == Some(&TirType::I64)
                    && !self.repr_facts.is_inline_safe_int(id)
                {
                    temporary_owners.push(bits);
                }
                bits.into()
            })
            .collect();
        let result = match return_abi {
            RuntimeReturnAbi::Void => {
                let func = self.ensure_runtime_void_fn(symbol, op.operands.len());
                self.backend
                    .builder
                    .build_call(func, &arg_bits, symbol)
                    .unwrap();
                None
            }
            RuntimeReturnAbi::I64 => {
                let func = self.ensure_runtime_i64_fn(symbol, op.operands.len());
                let result = self
                    .backend
                    .builder
                    .build_call(func, &arg_bits, symbol)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                Some(result)
            }
        };
        if !temporary_owners.is_empty() {
            let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
            for bits in temporary_owners {
                self.backend
                    .builder
                    .build_call(release, &[bits.into()], "")
                    .unwrap();
            }
        }
        if let Some(result) = result {
            // The boxed-call contract transfers an owned result. A call
            // without an SSA result still has to retire that ownership.
            self.bind_owned_runtime_result(op, result);
        }
    }

    /// Bind a transferred object owner, or retire it when the op discards it.
    /// Dedicated mixed-ABI calls use this only after establishing ownership;
    /// a borrowed runtime result must be explicitly retained first.
    pub(super) fn bind_owned_runtime_result(&mut self, op: &TirOp, result: BasicValueEnum<'ctx>) {
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        } else {
            let release = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
            self.backend
                .builder
                .build_call(release, &[result.into()], "")
                .unwrap();
        }
    }

    pub(super) fn emit_call_bind_runtime(
        &self,
        callable: BasicValueEnum<'ctx>,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let callable_i64 = self.ensure_i64(callable);
        let new_fn = self.ensure_runtime_i64_fn("molt_callargs_new", 2);
        let builder_val = self
            .backend
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(arg_ids.len() as u64, false).into(),
                    i64_ty.const_int(0, false).into(),
                ],
                "callargs",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_pos", 2);
        for &arg_id in arg_ids {
            // The dynamic-call ABI (`molt_callargs_push_pos` -> `molt_call_bind`
            // -> trampoline) carries every argument as a NaN-boxed `DynBox`; the
            // callee trampoline then decodes each box into its parameter's raw
            // representation (`unbox_dynbox_to_param_ty_with_builder`). Passing a
            // raw scalar here (the old `ensure_i64`, a bitcast-level cast that
            // does NOT NaN-box) made the trampoline decode a raw `I64`/`F64`
            // payload as a boxed tag — e.g. a closure returning its arg, or a
            // bare `sum`/`format` result, surfaced as a denormal float / `15.0`.
            // `materialize_dynbox_operand` boxes per the value's representation
            // plan, mirroring the direct-call arg path (`coerce_to_tir_type`).
            let arg_i64 = self.materialize_dynbox_operand(arg_id);
            self.backend
                .builder
                .build_call(push_fn, &[builder_val.into(), arg_i64.into()], "push")
                .unwrap();
        }
        let bind_fn = self.ensure_runtime_i64_fn("molt_call_bind", 2);
        self.backend
            .builder
            .build_call(
                bind_fn,
                &[callable_i64.into(), builder_val.into()],
                "call_result",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    pub(super) fn emit_call_func_runtime(
        &self,
        callable: BasicValueEnum<'ctx>,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let callable_i64 = self.ensure_i64(callable);
        if arg_ids.len() <= 3 {
            let rt_name = format!("molt_call_func_fast{}", arg_ids.len());
            let fast_fn = self.ensure_runtime_i64_fn(&rt_name, arg_ids.len() + 1);
            let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                Vec::with_capacity(arg_ids.len() + 1);
            args.push(callable_i64.into());
            for &arg_id in arg_ids {
                // `molt_call_func_fast{N}` is the boxed-domain dynamic-dispatch
                // entry: it forwards each argument into the callee's trampoline,
                // which decodes a NaN-boxed `DynBox` into the parameter's raw
                // representation. A raw scalar passed here (the old `ensure_i64`)
                // is decoded as a boxed payload by the trampoline — the closure
                // call/return ABI carrier miscompile (#58/#37). Box per the
                // value's representation plan, exactly like the bind path above
                // and the direct-call path (`coerce_to_tir_type`).
                args.push(self.materialize_dynbox_operand(arg_id).into());
            }
            return self
                .backend
                .builder
                .build_call(fast_fn, &args, "call_func")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
        }
        self.emit_call_bind_runtime(callable, arg_ids)
    }

    pub(super) fn emit_call_func_or_bind_runtime(
        &mut self,
        callable: BasicValueEnum<'ctx>,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let callable_i64 = self.ensure_i64(callable);
        let is_func_fn = self.ensure_runtime_i64_fn("molt_is_function_obj", 1);
        let is_func_bits = self
            .backend
            .builder
            .build_call(is_func_fn, &[callable_i64.into()], "is_function_obj")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let truthy_fn = self.ensure_runtime_i64_fn("molt_is_truthy", 1);
        let is_func_truthy = self
            .backend
            .builder
            .build_call(truthy_fn, &[is_func_bits.into()], "is_function_truthy")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let cond_i1 = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                is_func_truthy,
                self.backend.context.i64_type().const_zero(),
                "call_func_fast_guard",
            )
            .unwrap();
        let current_fn = self.llvm_fn;
        let fast_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "call_func_fast");
        let bind_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "call_func_bind");
        let merge_bb = self
            .backend
            .context
            .append_basic_block(current_fn, "call_func_merge");
        self.all_llvm_blocks.push(fast_bb);
        self.all_llvm_blocks.push(bind_bb);
        self.all_llvm_blocks.push(merge_bb);
        self.backend
            .builder
            .build_conditional_branch(cond_i1, fast_bb, bind_bb)
            .unwrap();

        self.backend.builder.position_at_end(fast_bb);
        let fast_result = self.emit_call_func_runtime(callable, arg_ids);
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let fast_exit_bb = self.backend.builder.get_insert_block().unwrap();

        self.backend.builder.position_at_end(bind_bb);
        let bind_result = self.emit_call_bind_runtime(callable, arg_ids);
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        let bind_exit_bb = self.backend.builder.get_insert_block().unwrap();

        self.backend.builder.position_at_end(merge_bb);
        let phi = self
            .backend
            .builder
            .build_phi(self.backend.context.i64_type(), "call_func_or_bind_phi")
            .unwrap();
        phi.add_incoming(&[
            (&fast_result.into_int_value(), fast_exit_bb),
            (&bind_result.into_int_value(), bind_exit_bb),
        ]);
        phi.as_basic_value()
    }

    pub(super) fn next_call_site_bits(&mut self, lane: &str) -> inkwell::values::IntValue<'ctx> {
        let site_id = molt_codegen_abi::stable_ic_site_id(
            self.func.name.as_str(),
            self.call_site_counter,
            lane,
        );
        self.call_site_counter += 1;
        let raw: BasicValueEnum<'ctx> = self
            .backend
            .context
            .i64_type()
            .const_int(site_id as u64, true)
            .into();
        self.materialize_dynbox_bits(raw, &TirType::I64)
    }

    pub(super) fn source_call_site_bits(
        &self,
        op: &TirOp,
        lane: &str,
    ) -> inkwell::values::IntValue<'ctx> {
        let source_op_idx = op
            .source_op_index()
            .unwrap_or_else(|| panic!("{lane} requires source op index"));
        let site_id =
            molt_codegen_abi::stable_ic_site_id(self.func.name.as_str(), source_op_idx, lane);
        let raw: BasicValueEnum<'ctx> = self
            .backend
            .context
            .i64_type()
            .const_int(site_id as u64, true)
            .into();
        self.materialize_dynbox_bits(raw, &TirType::I64)
    }

    pub(super) fn generator_self_bits(&self) -> inkwell::values::IntValue<'ctx> {
        let idx = self
            .func
            .param_names
            .iter()
            .position(|name| name == "self")
            .unwrap_or(0);
        let value = self
            .llvm_fn
            .get_nth_param(idx as u32)
            .unwrap_or_else(|| self.backend.context.i64_type().const_zero().into());
        self.ensure_i64(value)
    }

    /// Map each `_poll` resume state id to the REAL TIR resume-continuation
    /// block (an entry in `block_map`), NOT a synthetic block.
    ///
    /// The single source of truth for the state → resume-block mapping is the
    /// entry block's `StateDispatch` terminator, which the SSA pass built from
    /// `cfg.state_resume_edges`: each `(state_id, resume_bid, args)` case names
    /// the real TIR block that the dispatch resumes into.  Lowering the dispatch
    /// to those real blocks (whose phis the SSA pass placed) is what makes the
    /// `_poll` state machine dominance-correct on LLVM — the old design created
    /// fresh synthetic `state_resume_*` blocks and `position_at_end`-ed the
    /// continuation into them, so the real TIR continuation block's phis were
    /// missing the dispatch incoming (the "Instruction does not dominate all
    /// uses!" class).
    ///
    /// The re-poll suspend ops (`state_transition` / `chan_*_yield`) carry a
    /// *pending* state id whose resume target is the suspend op's OWN block (it
    /// re-polls from its own position); those are also dispatch cases, so they
    /// are covered by the same `StateDispatch` case list.
    pub(super) fn initialize_state_resume_blocks(&mut self) {
        let Some(entry) = self.func.blocks.get(&self.func.entry_block) else {
            return;
        };
        if let Terminator::StateDispatch { cases, .. } = &entry.terminator {
            // Clone the (state_id, block_id) pairs first to avoid borrowing
            // `self.func` while mutating `self.state_resume_blocks`.
            let pairs: Vec<(i64, BlockId)> =
                cases.iter().map(|(state, bid, _)| (*state, *bid)).collect();
            for (state_id, resume_bid) in pairs {
                if let Some(&bb) = self.block_map.get(&resume_bid) {
                    self.state_resume_blocks.insert(state_id, bb);
                }
            }
        }
    }

    pub(super) fn raw_i64_operand(
        &self,
        operand_id: ValueId,
        current_bb: BasicBlock<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let value = self.resolve(operand_id);
        let source_ty = self
            .value_types
            .get(&operand_id)
            .cloned()
            .unwrap_or(TirType::DynBox);
        self.coerce_to_tir_type(value, &source_ty, &TirType::I64, current_bb)
            .into_int_value()
    }

    pub(super) fn resume_block_for_state(&self, state_id: i64) -> BasicBlock<'ctx> {
        *self
            .state_resume_blocks
            .get(&state_id)
            .unwrap_or_else(|| panic!("missing resume block for state {}", state_id))
    }

    /// Call a 2-argument runtime function that returns i64.
    ///
    /// The callee is declared on demand through the central runtime-import helper
    /// when it is not already in the fixed table. On-demand declarations carry
    /// only the globally valid runtime attributes; stronger facts such as
    /// `willreturn` must be promoted into `runtime_imports/declarations.rs`.
    pub(super) fn call_runtime_2(
        &self,
        name: &str,
        lhs: BasicValueEnum<'ctx>,
        rhs: BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let func = self.ensure_runtime_i64_fn(name, 2);
        let lhs_i64 = self.ensure_i64(lhs);
        let rhs_i64 = self.ensure_i64(rhs);
        self.backend
            .builder
            .build_call(func, &[lhs_i64.into(), rhs_i64.into()], name)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }
}
