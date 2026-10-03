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
        let Some(abi) = runtime_boxed_abi(&symbol, op.operands.len()) else {
            if !self.backend.runtime_callable_symbols.contains(&symbol) {
                return false;
            }
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
    /// CALL or a preserved operation. Both routes share admission, the
    /// borrowed-operand custody and the canonical result custody.
    pub(super) fn emit_boxed_runtime_call(&mut self, op: &TirOp, abi: &RuntimeBoxedAbi) {
        let symbol = abi.symbol;
        // Semantic classification is not availability in the selected runtime.
        // Direct calls and preserved operations must share this admission before
        // declaring symbols or materializing any temporary argument owners.
        if !self.backend.runtime_callable_symbols.contains(symbol) {
            self.record_fatal(format!(
                "boxed runtime symbol `{symbol}` is unavailable in the selected runtime"
            ));
            return;
        }
        if op.operands.len() != abi.arity {
            self.record_fatal(format!(
                "boxed runtime symbol `{symbol}` has mismatched arity"
            ));
            return;
        }
        let return_abi = match abi.result {
            RuntimeBoxedReturn::OwnedValue
            | RuntimeBoxedReturn::BorrowedValue
            | RuntimeBoxedReturn::PollValue => RuntimeReturnAbi::I64,
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
        let callee = match return_abi {
            RuntimeReturnAbi::Void => self.ensure_runtime_void_fn(symbol, abi.arity),
            RuntimeReturnAbi::I64 => self.ensure_runtime_i64_fn(symbol, abi.arity),
        };
        self.emit_positional_runtime_call(
            op,
            callee,
            RuntimeResultCustody::Boxed(abi.result),
            "boxed_call",
            symbol,
        );
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

    /// Build a positional CallArgs for `args` and let `consume` bind it. The
    /// builder reserves exactly `args.len()` positional slots, so no push can
    /// fail; each push retains its argument and returns None. `consume`
    /// (`molt_call_bind*`, `molt_call_builtin`) owns and frees the builder.
    /// A failed allocation returns the raw word 0 with MemoryError pending.
    /// Every builder entry and consumer observes a pending exception first, so
    /// the pushes then retain nothing and the consumer returns None without
    /// running a callee: the runtime protocol needs no branch here.
    pub(super) fn with_callargs(
        &mut self,
        args: &[inkwell::values::IntValue<'ctx>],
        consume: impl FnOnce(
            &mut Self,
            inkwell::values::IntValue<'ctx>,
        ) -> inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let new_fn = self.ensure_runtime_i64_fn("molt_callargs_new", 2);
        let builder_bits = self
            .backend
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(args.len() as u64, false).into(),
                    i64_ty.const_zero().into(),
                ],
                "callargs",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_pos", 2);
        for &arg in args {
            self.backend
                .builder
                .build_call(push_fn, &[builder_bits.into(), arg.into()], "callargs_push")
                .unwrap();
        }
        consume(&mut *self, builder_bits)
    }

    /// Borrow a dynamic call's callable and positional arguments through one
    /// custody. The dynamic-call ABI (`molt_call_func_fast{N}`,
    /// `molt_callargs_push_pos` -> `molt_call_bind`) carries every argument as
    /// a NaN-boxed `DynBox` word, which the callee trampoline decodes into its
    /// parameter's raw representation (`unbox_dynbox_to_param_ty_with_builder`).
    /// A raw scalar passed unboxed was decoded as a boxed payload (a closure
    /// returning its argument, or a bare `sum`/`format` result, surfaced as a
    /// denormal float or `15.0`), so each value is boxed once here, and a box
    /// minted for it is released after the call returns.
    fn borrow_dynamic_call_operands(
        &mut self,
        callable: ValueId,
        args: &[ValueId],
    ) -> (
        BorrowedOperands<'ctx>,
        inkwell::values::IntValue<'ctx>,
        Vec<inkwell::values::IntValue<'ctx>>,
    ) {
        let mut operands = Vec::with_capacity(args.len() + 1);
        operands.push(callable);
        operands.extend_from_slice(args);
        let mut custody = self.begin_borrowed_operands(&operands, "dynamic_call");
        let callable_bits = self.borrowed_operand(&mut custody, callable);
        let mut arg_bits = Vec::with_capacity(args.len());
        for &arg in args {
            arg_bits.push(self.borrowed_operand(&mut custody, arg));
        }
        (custody, callable_bits, arg_bits)
    }

    /// `molt_call_bind(callable, CallArgs(args))`.
    fn bind_call_words(
        &mut self,
        callable: inkwell::values::IntValue<'ctx>,
        args: &[inkwell::values::IntValue<'ctx>],
    ) -> inkwell::values::IntValue<'ctx> {
        let bind_fn = self.ensure_runtime_i64_fn("molt_call_bind", 2);
        self.with_callargs(args, |this, builder| {
            this.backend
                .builder
                .build_call(bind_fn, &[callable.into(), builder.into()], "call_result")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        })
    }

    /// `molt_call_func_fast{N}` for at most three positional arguments (the
    /// callee trampoline borrows them), else the CallArgs route.
    fn call_func_words(
        &mut self,
        callable: inkwell::values::IntValue<'ctx>,
        args: &[inkwell::values::IntValue<'ctx>],
    ) -> inkwell::values::IntValue<'ctx> {
        if args.len() > 3 {
            return self.bind_call_words(callable, args);
        }
        let rt_name = format!("molt_call_func_fast{}", args.len());
        let fast_fn = self.ensure_runtime_i64_fn(&rt_name, args.len() + 1);
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(args.len() + 1);
        call_args.push(callable.into());
        for &arg in args {
            call_args.push(arg.into());
        }
        self.backend
            .builder
            .build_call(fast_fn, &call_args, "call_func")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value()
    }

    pub(super) fn emit_call_bind_runtime(
        &mut self,
        callable: ValueId,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let (custody, callable_bits, arg_bits) =
            self.borrow_dynamic_call_operands(callable, arg_ids);
        let result = self.bind_call_words(callable_bits, &arg_bits);
        self.finish_borrowed_operands(custody, result, "dynamic_call_result", |_| {})
            .into()
    }

    pub(super) fn emit_call_func_runtime(
        &mut self,
        callable: ValueId,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let (custody, callable_bits, arg_bits) =
            self.borrow_dynamic_call_operands(callable, arg_ids);
        let result = self.call_func_words(callable_bits, &arg_bits);
        self.finish_borrowed_operands(custody, result, "dynamic_call_result", |_| {})
            .into()
    }

    /// An ordinary source call (`call_func`, `call_guarded`, `call_method`)
    /// that adopted its callable and its arguments:
    /// `molt_call_func_owned(callable, args, nargs, 0)`. Every word belongs to
    /// the runtime's owned lane, which ends a temporary bound method before its
    /// function runs, moves the arguments into an adopting frame or releases
    /// them after a borrowing callee, and releases the callable last. Nothing
    /// is released here on success. Failed materialization skips entry and
    /// releases the instruction's inputs through operation-local custody.
    pub(super) fn emit_call_func_owned_runtime(&mut self, op: &TirOp) -> BasicValueEnum<'ctx> {
        let mut custody = self.begin_call_operands(
            &op.operands,
            &op.operands[1..],
            Some(op.operands[0]),
            "owned_call",
        );
        let words: Vec<inkwell::values::IntValue<'ctx>> = op
            .operands
            .iter()
            .map(|&operand| self.borrowed_operand(&mut custody, operand))
            .collect();
        let (args_ptr, nargs) = self.spill_call_words(&words[1..], "owned_call_args");
        let owned_fn = self.ensure_runtime_i64_fn("molt_call_func_owned", 4);
        let no_site = self.backend.context.i64_type().const_zero();
        self.surrender_borrowed_owners(&custody, &op.operands, &[]);
        let result = self
            .backend
            .builder
            .build_call(
                owned_fn,
                &[
                    words[0].into(),
                    args_ptr.into(),
                    nargs.into(),
                    no_site.into(),
                ],
                "call_func_owned",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        self.finish_borrowed_operands(custody, result, "owned_call_result", |_| {})
            .into()
    }

    pub(super) fn emit_call_func_or_bind_runtime(
        &mut self,
        callable: ValueId,
        arg_ids: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let (custody, callable_bits, arg_bits) =
            self.borrow_dynamic_call_operands(callable, arg_ids);
        let is_func_fn = self.ensure_runtime_i64_fn("molt_is_function_obj", 1);
        let is_func_bits = self
            .backend
            .builder
            .build_call(is_func_fn, &[callable_bits.into()], "is_function_obj")
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
        let source_bb = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_conditional_branch(cond_i1, fast_bb, bind_bb)
            .unwrap();
        self.record_llvm_edge(source_bb, fast_bb);
        self.record_llvm_edge(source_bb, bind_bb);

        self.backend.builder.position_at_end(fast_bb);
        let fast_result = self.call_func_words(callable_bits, &arg_bits);
        let fast_exit_bb = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        self.record_llvm_edge(fast_exit_bb, merge_bb);

        self.backend.builder.position_at_end(bind_bb);
        let bind_result = self.bind_call_words(callable_bits, &arg_bits);
        let bind_exit_bb = self.backend.builder.get_insert_block().unwrap();
        self.backend
            .builder
            .build_unconditional_branch(merge_bb)
            .unwrap();
        self.record_llvm_edge(bind_exit_bb, merge_bb);

        self.backend.builder.position_at_end(merge_bb);
        let phi = self
            .backend
            .builder
            .build_phi(self.backend.context.i64_type(), "call_func_or_bind_phi")
            .unwrap();
        phi.add_incoming(&[(&fast_result, fast_exit_bb), (&bind_result, bind_exit_bb)]);
        let result = phi.as_basic_value().into_int_value();
        self.finish_borrowed_operands(custody, result, "dynamic_call_result", |_| {})
            .into()
    }

    pub(super) fn next_call_site_bits(&mut self, lane: &str) -> inkwell::values::IntValue<'ctx> {
        let site_id = molt_codegen_abi::stable_ic_site_id(
            self.func.name.as_str(),
            self.call_site_counter,
            lane,
        );
        self.call_site_counter += 1;
        // Site ids fit the inline window by construction, so the boxed word is
        // a compile-time constant that owns nothing.
        self.inline_int_constant(site_id)
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
        self.inline_int_constant(site_id)
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
}
