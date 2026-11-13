use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeCallForm;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn ensure_function_symbol(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let requested_arity = arity + usize::from(has_closure);
        let linkage_abi = self
            .backend
            .function_linkage_abis
            .get(name)
            .unwrap_or_else(|| {
                panic!(
                    "LLVM function symbol `{name}` has no exact native linkage ABI; refusing to invent one"
                )
            });
        assert_eq!(
            requested_arity,
            linkage_abi.param_types.len(),
            "LLVM caller/linkage arity mismatch for `{name}`"
        );
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = linkage_abi
            .param_types
            .iter()
            .map(|ty| lower_type(self.backend.context, ty).into())
            .collect();
        let logical_fn_ty = match linkage_abi.return_type.as_ref() {
            Some(return_type) => {
                lower_type(self.backend.context, return_type).fn_type(&params, false)
            }
            None => self.backend.context.void_type().fn_type(&params, false),
        };
        let fn_ty = prepend_execution_context(logical_fn_ty);
        if let Some(func) = self.backend.module.get_compiled_function(name) {
            return require_llvm_function_type(name, func, fn_ty);
        }
        self.backend.module.add_compiled_function(
            name,
            logical_fn_ty,
            Some(inkwell::module::Linkage::External),
        )
    }

    /// A compiled function's linkage row owns its machine parameter list. An
    /// operation's `value` is a Python-level arity claim that may be absent (a
    /// task poll entry registers with none) and is checked only where present;
    /// a missing row still fails closed inside `ensure_function_symbol`.
    pub(super) fn ensure_linked_function_symbol(
        &self,
        name: &str,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let linkage_arity = self
            .backend
            .function_linkage_abis
            .get(name)
            .map(|abi| {
                abi.param_types
                    .len()
                    .saturating_sub(usize::from(has_closure))
            })
            .unwrap_or(0);
        self.ensure_function_symbol(name, linkage_arity, has_closure)
    }

    pub(super) fn ensure_plain_trampoline(
        &self,
        name: &str,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let target_fn = self.ensure_linked_function_symbol(name, has_closure);
        let abi = &self.backend.function_linkage_abis[name];
        self.ensure_typed_trampoline(
            name,
            target_fn,
            has_closure,
            &abi.param_types,
            abi.return_type.as_ref(),
            &abi.parameter_custody,
        )
    }

    /// Runtime function objects use the manifest's callable ABI, never a
    /// compiled Python function's representation or parameter-custody plan.
    /// The target is declared through the one runtime declaration path, which
    /// classifies the symbol from the same generated callable ABI.
    pub(super) fn ensure_builtin_callable(
        &mut self,
        name: &str,
        runtime: Option<RuntimeFn>,
        arity: usize,
    ) -> Option<(FunctionValue<'ctx>, FunctionValue<'ctx>)> {
        use molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi;
        // The typed runtime entry decoded at TIR ingest (HF-821 B1).
        let Some((function, trampoline_abi)) =
            runtime.and_then(|function| function.callable_trampoline().map(|abi| (function, abi)))
        else {
            self.record_fatal(format!(
                "builtin_func target `{name}` has no runtime callable ABI; use func_new for compiled functions"
            ));
            return None;
        };
        if function.params() != arity {
            self.record_fatal(format!(
                "builtin_func arity mismatch for `{name}`: manifest requires {}, got {arity}",
                function.params()
            ));
            return None;
        }
        let target_fn = self.ensure_runtime_import(function);
        let trampoline = match trampoline_abi {
            // This runtime entry already consumes (closure, argv, argc).
            RuntimeCallableTrampolineAbi::CallFrame => target_fn,
            RuntimeCallableTrampolineAbi::UnpackArgs => self.ensure_typed_trampoline(
                name,
                target_fn,
                false,
                &vec![TirType::DynBox; arity],
                Some(&TirType::DynBox),
                &vec![crate::ir::ParameterCustody::Borrowed; arity],
            ),
        };
        Some((target_fn, trampoline))
    }

    fn ensure_typed_trampoline(
        &self,
        name: &str,
        target_fn: FunctionValue<'ctx>,
        has_closure: bool,
        param_tir_types: &[TirType],
        return_tir_type: Option<&TirType>,
        parameter_custody: &[crate::ir::ParameterCustody],
    ) -> FunctionValue<'ctx> {
        let callable_arity = param_tir_types.len() - usize::from(has_closure);
        let closure_suffix = if has_closure { "_closure" } else { "" };
        let trampoline_name =
            format!("{name}__molt_llvm_trampoline_{callable_arity}{closure_suffix}");
        if let Some(func) = self.backend.module.get_compiled_function(&trampoline_name) {
            return func;
        }

        let i64_ty = self.backend.context.i64_type();
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let trampoline_fn = self.backend.module.add_compiled_function(
            &trampoline_name,
            fn_ty,
            Some(inkwell::module::Linkage::Internal),
        );

        let builder = self.backend.context.create_builder();
        let entry = self
            .backend
            .context
            .append_basic_block(trampoline_fn, "entry");
        builder.position_at_end(entry);

        let closure_bits = trampoline_fn
            .get_nth_param(1)
            .expect("trampoline closure param missing")
            .into_int_value();
        let args_bits = trampoline_fn
            .get_nth_param(2)
            .expect("trampoline args param missing")
            .into_int_value();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let args_ptr = builder
            .build_int_to_ptr(args_bits, ptr_ty, "trampoline_args_ptr")
            .unwrap();

        // The target function's parameter SEMANTIC types (the representation
        // plan's `TirType` per param), used to decode each NaN-boxed argument
        // into the raw machine representation the direct ABI expects. Indexed
        // 1:1 with the LLVM params: when `has_closure`, index 0 is the closure
        // object (a boxed reference — no payload decode). A raw-`I64` param must
        // be sign-extended out of its 47-bit inline NaN-box payload; passing the
        // boxed bits straight through (as this trampoline did before) made the
        // callee body decode a NaN-box tag/pointer as a raw integer — the
        // trusted-unbox truncation bug-class for a heap-BigInt argument. This is
        // the dynamic-dispatch dual of the direct-call arg coercion
        // (`coerce_to_tir_type`).
        let coerce_trampoline_arg = |bits: inkwell::values::IntValue<'ctx>,
                                     target_ty: inkwell::types::BasicTypeEnum<'ctx>,
                                     name: &str|
         -> inkwell::values::BasicMetadataValueEnum<'ctx> {
            match target_ty {
                inkwell::types::BasicTypeEnum::IntType(target_int) => {
                    if target_int.get_bit_width() == 64 {
                        bits.into()
                    } else if target_int.get_bit_width() < 64 {
                        builder
                            .build_int_truncate(bits, target_int, name)
                            .unwrap()
                            .into()
                    } else {
                        builder
                            .build_int_z_extend(bits, target_int, name)
                            .unwrap()
                            .into()
                    }
                }
                inkwell::types::BasicTypeEnum::FloatType(target_float) => builder
                    .build_bit_cast(bits, target_float, name)
                    .unwrap()
                    .into(),
                inkwell::types::BasicTypeEnum::PointerType(target_ptr) => builder
                    .build_int_to_ptr(bits, target_ptr, name)
                    .unwrap()
                    .into(),
                other => panic!(
                    "unsupported trampoline argument coercion for {} to {:?}",
                    name, other
                ),
            }
        };

        // An adopting entry owns each argument's reference. A raw parameter
        // keeps only the value extracted from its box, so the trampoline ends
        // that reference once the entry returns (raw entry extraction); the
        // representation plan chooses a raw entry only where the object's
        // identity and frame visibility are unobservable.
        let mut extracted_owners: Vec<inkwell::values::IntValue<'ctx>> = Vec::new();
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(callable_arity + usize::from(has_closure));
        if has_closure {
            let target_ty = target_fn
                .get_nth_param(1)
                .map(|param| param.get_type())
                .unwrap_or_else(|| i64_ty.into());
            call_args.push(coerce_trampoline_arg(
                closure_bits,
                target_ty,
                "trampoline_closure_arg",
            ));
        }
        for idx in 0..callable_arity {
            let elem_ptr = unsafe {
                builder
                    .build_gep(
                        i64_ty,
                        args_ptr,
                        &[i64_ty.const_int(idx as u64, false)],
                        &format!("trampoline_arg_ptr_{idx}"),
                    )
                    .unwrap()
            };
            let arg = builder
                .build_load(i64_ty, elem_ptr, &format!("trampoline_arg_{idx}"))
                .unwrap()
                .into_int_value();
            // Decode the NaN-boxed argument into the raw representation the
            // target parameter expects, BEFORE the LLVM-type cast. The args
            // array always carries `DynBox` (NaN-boxed) values; a raw-`I64`
            // param needs full-width integer extraction, a `Bool` its
            // low payload bit. `F64`/reference params are already the raw bits.
            let param_index = idx + usize::from(has_closure);
            if parameter_custody.get(param_index) == Some(&crate::ir::ParameterCustody::Transferred)
                && !Self::tir_type_is_dynbox_like(&param_tir_types[param_index])
            {
                extracted_owners.push(arg);
            }
            let arg = unbox_dynbox_to_param_ty_with_builder(
                &builder,
                self.backend.context,
                &self.backend.module,
                arg,
                &param_tir_types[param_index],
            );
            let target_ty = target_fn
                .get_nth_param(param_index as u32 + 1)
                .map(|param| param.get_type())
                .unwrap_or_else(|| i64_ty.into());
            call_args.push(coerce_trampoline_arg(
                arg,
                target_ty,
                &format!("trampoline_arg_cast_{idx}"),
            ));
        }

        let result = builder
            .build_execution_call(target_fn, &call_args, "trampoline_call")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap_or_else(|| {
                i64_ty
                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    .into()
            });
        self.release_call_inputs(&builder, None, &extracted_owners);
        let ret_bits = materialize_dynbox_bits_with_builder(
            &builder,
            self.backend.context,
            &self.backend.module,
            trampoline_fn,
            result,
            return_tir_type.unwrap_or(&TirType::DynBox),
        );
        builder.build_return(Some(&ret_bits)).unwrap();
        trampoline_fn
    }

    /// CPython's LOAD_ATTR / LOAD_SUPER_ATTR in method form: a method site's
    /// load body (`method_site.rs`), whose miss is
    /// `molt_method_site_load(record, receiver, self_out)`, or
    /// `molt_load_super_method(site, class, first_arg, name, len, self_out)`.
    /// The load returns the callable and writes `self_or_null` (the receiver
    /// or the null sentinel) to an entry-block slot. Operands are borrowed;
    /// both results are owned.
    pub(super) fn emit_load_method(&mut self, op: &TirOp) {
        let (lane, operand_count) = if op.opcode == OpCode::LoadSuperMethod {
            ("load_super_method", 2)
        } else {
            ("load_method", 1)
        };
        let Some(AttrValue::Str(name)) = op.attrs.get("name") else {
            self.record_fatal(format!(
                "malformed {op:?}: a method load names its attribute"
            ));
            return;
        };
        let &[callable_id, self_id] = op.results.as_slice() else {
            self.record_fatal(format!("malformed {op:?}: a method load has two results"));
            return;
        };
        if op.operands.len() != operand_count {
            self.record_fatal(format!(
                "malformed {op:?}: {operand_count} operand(s) expected"
            ));
            return;
        }
        let i64_ty = self.backend.context.i64_type();
        let null = i64_ty.const_int(molt_codegen_abi::loaded_method_null_bits() as u64, false);
        let self_slot = self.build_entry_i64_alloca(&format!("{lane}_self"));
        // The slot holds the null sentinel until the runtime writes it, so a
        // call that raises before its write leaves a releasable word.
        self.backend.builder.build_store(self_slot, null).unwrap();
        let mut custody = self.begin_borrowed_operands(&op.operands, lane);
        let operands: Vec<inkwell::values::IntValue<'ctx>> = op
            .operands
            .iter()
            .map(|&operand| self.borrowed_operand(&mut custody, operand))
            .collect();
        let self_out = self
            .backend
            .builder
            .build_ptr_to_int(self_slot, i64_ty, &format!("{lane}_self_ptr"))
            .unwrap();
        let callable = if op.opcode == OpCode::LoadMethod {
            let Some(site) = op.method_site() else {
                self.record_fatal(format!("{op:?}: a method load is a method site"));
                return;
            };
            let Some(callable) = self.emit_method_site_load_call(site, operands[0], self_out)
            else {
                self.record_fatal(format!("{op:?}: method site {site} has no record"));
                return;
            };
            callable
        } else {
            let call_fn = self.ensure_runtime_import(RuntimeFn::LoadSuperMethod);
            let site_bits = self.next_call_site_bits(lane);
            let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(name);
            let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                vec![site_bits.into()];
            call_args.extend(operands.iter().map(
                |&operand| -> inkwell::values::BasicMetadataValueEnum<'ctx> { operand.into() },
            ));
            call_args.extend::<[inkwell::values::BasicMetadataValueEnum<'ctx>; 3]>([
                name_ptr.into(),
                name_len_bits.into(),
                self_out.into(),
            ]);
            self.backend
                .builder
                .build_execution_call(call_fn, &call_args, lane)
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        };
        // A receiver that could not be boxed was never loaded: its pair is
        // None and the null sentinel, never a previous load's words.
        let callable =
            self.finish_borrowed_operands(custody, callable, &format!("{lane}_callable"), |this| {
                this.backend.builder.build_store(self_slot, null).unwrap();
            });
        let self_or_null = self
            .backend
            .builder
            .build_load(i64_ty, self_slot, &format!("{lane}_self_or_null"))
            .unwrap();
        self.values.set_value(callable_id, callable.into());
        self.values
            .set_repr(callable_id, crate::codegen_ssa::Repr::of(&TirType::DynBox));
        self.values.set_value(self_id, self_or_null);
        self.values
            .set_repr(self_id, crate::codegen_ssa::Repr::of(&TirType::DynBox));
    }

    /// Consume a loaded receiver before evaluating user arguments. The builder
    /// borrows its existing carrier; push_pos retains a real receiver, whose
    /// original credit ends even when the push fails. The null sentinel never
    /// crosses a Python-object or refcount boundary.
    pub(super) fn emit_callargs_push_method_receiver(&mut self, op: &TirOp) {
        let &[builder_id, receiver_id] = op.operands.as_slice() else {
            self.record_fatal(format!(
                "malformed {op:?}: method receiver prefix takes two operands"
            ));
            return;
        };
        let builder_value = self.resolve(builder_id);
        let builder_word = self.ensure_i64(builder_value);
        let receiver_value = self.resolve(receiver_id);
        let receiver_word = self.ensure_i64(receiver_value);
        let i64_ty = self.backend.context.i64_type();
        let null = i64_ty.const_int(molt_codegen_abi::loaded_method_null_bits() as u64, false);
        let present = self
            .backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                receiver_word,
                null,
                "method_receiver_present",
            )
            .unwrap();
        let push = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, "method_receiver_push");
        let join = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, "method_receiver_done");
        self.backend
            .builder
            .build_conditional_branch(present, push, join)
            .unwrap();
        self.backend.builder.position_at_end(push);
        let push_fn = self.ensure_runtime_import(RuntimeFn::CallargsPushPos);
        let result = self
            .backend
            .builder
            .build_execution_call(
                push_fn,
                &[builder_word.into(), receiver_word.into()],
                "method_receiver_push_result",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        self.bind_owned_runtime_result(op, result);
        let release = self
            .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Release);
        release.emit(&self.backend.builder, receiver_word.into());
        self.backend
            .builder
            .build_unconditional_branch(join)
            .unwrap();
        self.backend.builder.position_at_end(join);
    }

    /// CPython's CALL after a method load: `[callable, self_or_null, args...]`,
    /// a source call that adopts every operand. `molt_call_loaded_method{N}`
    /// (or the spilled entry above four arguments) takes every word over: it
    /// enters the selected function with `self` prepended, or calls the
    /// attribute when `self_or_null` is the null sentinel. Failed boxing skips
    /// the entry and releases the instruction's inputs.
    pub(super) fn emit_call_loaded_method(&mut self, op: &TirOp) {
        const CALLS: [RuntimeFn; 5] = [
            RuntimeFn::CallLoadedMethod0,
            RuntimeFn::CallLoadedMethod1,
            RuntimeFn::CallLoadedMethod2,
            RuntimeFn::CallLoadedMethod3,
            RuntimeFn::CallLoadedMethod4,
        ];
        let adopts_all = op.argument_custody().is_some_and(|custody| {
            custody
                .iter()
                .all(|&position| position == crate::ir::ParameterCustody::Transferred)
        });
        if op.operands.len() < 2 || !adopts_all {
            self.record_fatal(format!(
                "malformed {op:?}: a loaded method call adopts its pair and every argument"
            ));
            return;
        }
        let lane = "call_loaded_method";
        let mut custody =
            self.begin_call_operands(&op.operands, &op.operands[1..], Some(op.operands[0]), lane);
        let words: Vec<inkwell::values::IntValue<'ctx>> = op
            .operands
            .iter()
            .map(|&operand| self.borrowed_operand(&mut custody, operand))
            .collect();
        // A method site's call: the shared call body adopts every word too.
        if let Some(site) = op.method_site()
            && Self::method_site_call_admits(words.len() - 2)
        {
            self.surrender_borrowed_owners(&custody, &op.operands, &[]);
            let Some(result) = self.emit_method_site_call(site, &words) else {
                self.record_fatal(format!("{op:?}: method site {site} has no record"));
                return;
            };
            let result =
                self.finish_borrowed_operands(custody, result, "call_loaded_method_result", |_| {});
            self.bind_owned_runtime_result(op, result.into());
            return;
        }
        let (symbol, call_args): (
            RuntimeFn,
            Vec<inkwell::values::BasicMetadataValueEnum<'ctx>>,
        ) = match CALLS.get(words.len() - 2) {
            Some(&symbol) => (symbol, words.iter().map(|&word| word.into()).collect()),
            None => {
                let (args_ptr, nargs) =
                    self.spill_call_words(&words[2..], "call_loaded_method_args");
                (
                    RuntimeFn::CallLoadedMethodSpilled,
                    vec![
                        words[0].into(),
                        words[1].into(),
                        args_ptr.into(),
                        nargs.into(),
                    ],
                )
            }
        };
        let call_fn = self.ensure_runtime_import(symbol);
        self.surrender_borrowed_owners(&custody, &op.operands, &[]);
        let result = self
            .backend
            .builder
            .build_execution_call(call_fn, &call_args, lane)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let result =
            self.finish_borrowed_operands(custody, result, "call_loaded_method_result", |_| {});
        self.bind_owned_runtime_result(op, result.into());
    }

    pub(super) fn emit_call(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let original_kind = op.origin;

        if matches!(
            original_kind,
            Some(OpKind::CallFunc) | Some(OpKind::CallFunction)
        ) && !op.operands.is_empty()
        {
            // An ordinary source call adopted its callable and arguments.
            let result = if op.argument_custody().is_some() {
                self.emit_call_func_owned_runtime(op)
            } else {
                self.emit_call_func_or_bind_runtime(op.operands[0], &op.operands[1..])
            };
            self.bind_owned_runtime_result(op, result);
            return;
        }

        // Direct call by name: call_guarded stores the target function
        // name in s_value / _var, with all operands being arguments
        // (not a callable reference).  If the target already exists in
        // the LLVM module (same compilation unit), call it directly.
        let direct_target: Option<String> = op
            .attrs
            .get("s_value")
            .or_else(|| op.attrs.get("_var"))
            .and_then(|v| match v {
                AttrValue::Str(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            });
        let direct_operands: &[ValueId] = if matches!(original_kind, Some(OpKind::CallGuarded)) {
            op.operands.get(1..).unwrap_or(&[])
        } else {
            &op.operands
        };
        let guarded_callable = if matches!(original_kind, Some(OpKind::CallGuarded)) {
            op.operands.first().copied()
        } else {
            None
        };

        if matches!(
            original_kind,
            Some(OpKind::CallBind) | Some(OpKind::CallIndirect)
        ) && op.operands.len() >= 2
        {
            let (lane, runtime_name) = if matches!(original_kind, Some(OpKind::CallIndirect)) {
                ("call_indirect", RuntimeFn::CallIndirectIc)
            } else {
                ("call_bind", RuntimeFn::CallBindIc)
            };
            // An ordinary call (a stack-form builder) adopted its callable as
            // well as the builder; one owned entry serves both spellings.
            let owned_callable = op.operand_custody(0) == crate::ir::ParameterCustody::Transferred;
            let runtime_name = if owned_callable {
                RuntimeFn::CallBindIcOwned
            } else {
                runtime_name
            };
            let result = self.emit_builder_consuming_call(
                op.operands[0],
                op.operands[1],
                lane,
                runtime_name,
                owned_callable,
            );
            self.bind_owned_runtime_result(op, result.into());
            return;
        }

        if matches!(original_kind, Some(OpKind::CallGuarded))
            && let Some(callable_id) = guarded_callable
        {
            // A guarded call adopts as an instruction, whatever it resolves
            // to: its callable and arguments take the owned lane.
            let result = if op.argument_custody().is_some() {
                self.emit_call_func_owned_runtime(op)
            } else {
                self.emit_call_func_runtime(callable_id, direct_operands)
            };
            self.bind_owned_runtime_result(op, result);
            return;
        }

        if let Some(ref target_name) = direct_target {
            let target_fn = self
                .backend
                .function_linkage_abis
                .get(target_name.as_str())
                .map(|abi| abi.param_types.len())
                .map(|arity| self.ensure_function_symbol(target_name, arity, false));
            if let Some(target_fn) = target_fn {
                let target_return_tir_ty = self
                    .backend
                    .function_linkage_abis
                    .get(target_name.as_str())
                    .and_then(|abi| abi.return_type.clone())
                    .unwrap_or(TirType::DynBox);
                let expected_params = target_fn.count_params() as usize - 1;
                if expected_params != direct_operands.len()
                    && let Some(callable_id) = guarded_callable
                {
                    let result = self.emit_call_bind_runtime(callable_id, direct_operands);
                    self.bind_owned_runtime_result(op, result);
                    return;
                }
                self.emit_direct_compiled_call(
                    op,
                    target_name,
                    target_fn,
                    direct_operands,
                    op.operands.len() - direct_operands.len(),
                    &target_return_tir_ty,
                );
            } else {
                if let Some(callable_id) = guarded_callable {
                    let result = self.emit_call_bind_runtime(callable_id, direct_operands);
                    self.bind_owned_runtime_result(op, result);
                    return;
                }
                // Source functions use exact native linkage above. Only the
                // external/runtime CALL role, or a first-class TIR `Call` whose
                // own identity names the target (`direct_call_symbol_for_op`),
                // may use a classified runtime ABI; neither a symbol prefix nor
                // operand count invents a signature.
                let runtime_role = match original_kind {
                    Some(kind) => kind == OpKind::Call,
                    None => {
                        crate::tir::call_targets::direct_call_symbol_for_op(op)
                            == Some(target_name.as_str())
                    }
                };
                // The runtime entry decoded at TIR ingest (HF-821 B1), admitted
                // only at its exact arity.
                let runtime_target = op
                    .runtime
                    .filter(|function| runtime_role && function.params() == direct_operands.len());
                if let Some(abi) = runtime_target.and_then(RuntimeFn::boxed_abi) {
                    self.emit_boxed_runtime_call(op, abi);
                    return;
                }
                if let Some(function) =
                    runtime_target.filter(|function| function.form() == RuntimeCallForm::RawCall)
                {
                    self.emit_raw_runtime_call(op, function);
                    return;
                }
                self.record_fatal(format!(
                    "direct call target `{target_name}` has no exact native linkage ABI; refusing to invent an i64 signature"
                ));
                if let Some(&result_id) = op.results.first() {
                    self.values.set_value(
                        result_id,
                        i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into(),
                    );
                    self.values
                        .set_repr(result_id, crate::codegen_ssa::Repr::of(&TirType::DynBox));
                }
            }
        } else if !op.operands.is_empty() {
            // Indirect call: operands[0] = callable, rest = positional args.
            let result = self.emit_call_bind_runtime(op.operands[0], &op.operands[1..]);

            self.bind_owned_runtime_result(op, result);
        } else {
            // No operands, no direct target — emit None.
            if let Some(&result_id) = op.results.first() {
                let none_val: BasicValueEnum<'ctx> = i64_ty
                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    .into();
                self.values.set_value(result_id, none_val);
                self.values
                    .set_repr(result_id, crate::codegen_ssa::Repr::of(&TirType::DynBox));
            }
        }
    }

    /// The compiled target a guarded call or its late-TIR ops name, when its
    /// linkage row exists.
    fn guarded_target_fn(&self, op: &TirOp) -> Option<(String, FunctionValue<'ctx>, TirType)> {
        let name = ["s_value", "_var"]
            .iter()
            .find_map(|key| match op.attrs.get(*key) {
                Some(AttrValue::Str(symbol)) if !symbol.is_empty() => Some(symbol.clone()),
                _ => None,
            })?;
        let abi = self.backend.function_linkage_abis.get(name.as_str())?;
        let arity = abi.param_types.len();
        let return_ty = abi.return_type.clone().unwrap_or(TirType::DynBox);
        let target = self.ensure_function_symbol(&name, arity, false);
        Some((name, target, return_ty))
    }

    /// `CodeIs` (HF-821 D4): the function word's code pointer, the first word
    /// of the function object, equals the compiled target's address.
    pub(super) fn emit_code_is(&mut self, op: &TirOp) {
        let Some((_, target, _)) = self.guarded_target_fn(op) else {
            self.record_fatal("CodeIs names no compiled target".to_string());
            return;
        };
        let context = self.backend.context;
        let i64_ty = context.i64_type();
        let word = self.ensure_i64(self.resolve(op.operands[0]));
        let address = self.unbox_ptr_bits(word);
        let builder = &self.backend.builder;
        let object = builder
            .build_int_to_ptr(
                address,
                context.ptr_type(inkwell::AddressSpace::default()),
                "callee_object",
            )
            .unwrap();
        let code = builder
            .build_load(i64_ty, object, "callee_code")
            .unwrap()
            .into_int_value();
        let expected = builder
            .build_ptr_to_int(
                target.as_global_value().as_pointer_value(),
                i64_ty,
                "target_code",
            )
            .unwrap();
        let same = builder
            .build_int_compare(inkwell::IntPredicate::EQ, code, expected, "code_is")
            .unwrap();
        self.values.set_value(op.results[0], same.into());
        self.values
            .set_repr(op.results[0], crate::codegen_ssa::Repr::of(&TirType::Bool));
    }

    /// `DirectCall` (HF-821 D4): hand the callable to the callee's frame entry
    /// through the thread record's invocation word, call the compiled target
    /// with the operands after the callable, and put the outer handoff back.
    pub(super) fn emit_guarded_direct_call(&mut self, op: &TirOp) {
        let Some((name, target, return_ty)) = self.guarded_target_fn(op) else {
            self.record_fatal("DirectCall names no compiled target".to_string());
            return;
        };
        let context = self.backend.context;
        let i64_ty = context.i64_type();
        let callee = self.ensure_i64(self.resolve(op.operands[0]));
        let record = self.thread_record();
        let slot = unsafe {
            self.backend
                .builder
                .build_gep(
                    context.i8_type(),
                    record,
                    &[i64_ty.const_int(
                        molt_codegen_abi::THREAD_RECORD_PENDING_INVOCATION_OFFSET as u64,
                        false,
                    )],
                    "pending_invocation",
                )
                .unwrap()
        };
        let outer = self
            .backend
            .builder
            .build_load(i64_ty, slot, "outer_invocation")
            .unwrap();
        self.backend.builder.build_store(slot, callee).unwrap();
        self.emit_direct_compiled_call(op, &name, target, &op.operands[1..], 1, &return_ty);
        self.backend.builder.build_store(slot, outer).unwrap();
    }

    /// A direct call into compiled code. Every argument is coerced from its
    /// SOURCE TirType to the CALLEE's declared param TirType (DynBox = the boxed
    /// molt ABI default). A plain `call`/`call_internal` once passed an
    /// I64-typed value (or constant) RAW into a NaN-boxed parameter, where the
    /// raw bits decode as a garbage float (`compute(1000000)` received
    /// ~4.9e-318); the LLVM-type coercion below is a bitcast-level cast and
    /// cannot substitute for representation boxing. Compiled callees borrow
    /// their arguments and return an owned result, so a scalar boxed for a
    /// boxed parameter is a temporary of this call: the operation's custody
    /// materializes it (skipping the call when a box cannot be allocated) and
    /// releases it after the call returns.
    fn emit_direct_compiled_call(
        &mut self,
        op: &TirOp,
        target_name: &str,
        target_fn: FunctionValue<'ctx>,
        operands: &[ValueId],
        first_operand: usize,
        target_return_tir_ty: &TirType,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let param_tir_types: Vec<TirType> = (0..operands.len())
            .map(|idx| {
                self.backend
                    .function_linkage_abis
                    .get(target_name)
                    .and_then(|abi| abi.param_types.get(idx))
                    .cloned()
                    .unwrap_or(TirType::DynBox)
            })
            .collect();
        let source_tir_types: Vec<TirType> = operands.iter().map(|id| self.repr_ty(*id)).collect();
        let boxes_for_parameter = |idx: usize| {
            Self::tir_type_is_dynbox_like(&param_tir_types[idx])
                && !Self::tir_type_is_dynbox_like(&source_tir_types[idx])
        };
        let borrowed: Vec<ValueId> = operands
            .iter()
            .enumerate()
            .filter(|&(idx, _)| boxes_for_parameter(idx))
            .map(|(_, &operand)| operand)
            .collect();
        // Positions the instruction adopted (`argument_custody`, the target's
        // own parameter custody): the adopting entry owns one reference to each
        // such argument once it runs.
        // `operands` starts at the op's `first_operand`: a guarded call's
        // operand 0 is its callable, which no entry adopts.
        let adopted = |idx: usize| {
            op.operand_custody(idx + first_operand) == crate::ir::ParameterCustody::Transferred
        };
        let adopted_inputs: Vec<ValueId> = operands
            .iter()
            .enumerate()
            .filter(|&(idx, _)| adopted(idx))
            .map(|(_, &operand)| operand)
            .collect();
        let mut custody = self.begin_call_operands(&borrowed, &adopted_inputs, None, "direct_call");
        let mut args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(operands.len());
        // Adopted object references that a raw parameter does not keep: the
        // raw entry extracted its value, so the reference ends after the call.
        let mut extracted_owners = Vec::new();
        for (idx, &id) in operands.iter().enumerate() {
            let value: BasicValueEnum<'ctx> = if boxes_for_parameter(idx) {
                self.borrowed_operand(&mut custody, id).into()
            } else {
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("direct call must be emitted inside a basic block");
                let v = self.resolve(id);
                if adopted(idx)
                    && Self::tir_type_is_dynbox_like(&source_tir_types[idx])
                    && !Self::tir_type_is_dynbox_like(&param_tir_types[idx])
                {
                    extracted_owners.push(self.ensure_i64(v));
                }
                self.coerce_to_tir_type(
                    v,
                    &source_tir_types[idx],
                    &param_tir_types[idx],
                    current_bb,
                    Some(id),
                )
            };
            let target_ty = target_fn
                .get_nth_param(idx as u32 + 1)
                .map(|param| param.get_type())
                .unwrap_or_else(|| i64_ty.into());
            let current_bb = self
                .backend
                .builder
                .get_insert_block()
                .expect("direct call must be emitted inside a basic block");
            args.push(self.coerce_to_type(value, target_ty, current_bb).into());
        }
        // Boxes minted for adopted positions belong to the entry once it runs.
        let adopted_boxes: Vec<ValueId> = operands
            .iter()
            .enumerate()
            .filter(|&(idx, _)| adopted(idx) && boxes_for_parameter(idx))
            .map(|(_, &operand)| operand)
            .collect();
        let borrowed_boxes: Vec<ValueId> = operands
            .iter()
            .enumerate()
            .filter(|&(idx, _)| !adopted(idx) && boxes_for_parameter(idx))
            .map(|(_, &operand)| operand)
            .collect();
        self.surrender_borrowed_owners(&custody, &adopted_boxes, &borrowed_boxes);
        let call_result = self
            .backend
            .builder
            .build_execution_call(target_fn, &args, "direct_call")
            .unwrap();
        self.release_call_inputs(&self.backend.builder, None, &extracted_owners);
        let returned = call_result.try_as_basic_value().basic();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        if let Some(&result_id) = op.results.first() {
            let raw_result = returned.unwrap_or_else(|| none.into());
            let result = if *target_return_tir_ty == TirType::DynBox {
                raw_result.into_int_value()
            } else {
                materialize_dynbox_bits_with_builder(
                    &self.backend.builder,
                    self.backend.context,
                    &self.backend.module,
                    self.llvm_fn,
                    raw_result,
                    target_return_tir_ty,
                )
            };
            let result =
                self.finish_borrowed_operands(custody, result, "direct_call_result", |_| {});
            self.values.set_value(result_id, result.into());
            self.values
                .set_repr(result_id, crate::codegen_ssa::Repr::of(&TirType::DynBox));
        } else if !target_return_tir_ty.is_unboxed()
            && let Some(result) = returned
        {
            // Semantic object returns already use boxed handles. Raw scalar
            // results have no owner and need no boxing just to discard them;
            // void calls produce no value at all.
            let result = self.finish_borrowed_operands(
                custody,
                result.into_int_value(),
                "direct_call_result",
                |_| {},
            );
            self.bind_owned_runtime_result(op, result.into());
        } else {
            self.finish_borrowed_operands(custody, none, "direct_call_result", |_| {});
        }
    }

    /// A call consuming a frontend-built CallArgs builder: `call_bind` and
    /// `call_indirect` free their last operand (the generated
    /// `[[consuming_kind]]` facts), and on entry they observe an exception left
    /// pending by an earlier builder step, releasing the builder and returning
    /// None without running the callee. The callable is borrowed. The builder
    /// is an object carrier and is requested first, so its word dominates the
    /// failure block, which releases it when a failed callable box skips the
    /// call.
    ///
    /// With `owned_callable` the entry consumes the callable as well (an
    /// ordinary call that adopted it), so a box custody minted for it belongs
    /// to the entry once the call runs.
    fn emit_builder_consuming_call(
        &mut self,
        callable: ValueId,
        builder: ValueId,
        lane: &str,
        runtime_name: RuntimeFn,
        owned_callable: bool,
    ) -> inkwell::values::IntValue<'ctx> {
        let builder_ty = self.repr_ty(builder);
        assert!(
            Self::tir_type_is_dynbox_like(&builder_ty),
            "{lane} builder %{} must be an object carrier, not {builder_ty:?}",
            builder.0
        );
        let mut custody = self.begin_call_operands(
            &[builder, callable],
            &[builder],
            owned_callable.then_some(callable),
            lane,
        );
        let builder_bits = self.borrowed_operand(&mut custody, builder);
        let callable_bits = self.borrowed_operand(&mut custody, callable);
        if owned_callable {
            self.surrender_borrowed_owners(&custody, &[callable], &[]);
        }
        let site_bits = self.next_call_site_bits(lane);
        let runtime_fn = self.ensure_runtime_import(runtime_name);
        let bound = self
            .backend
            .builder
            .build_execution_call(
                runtime_fn,
                &[site_bits.into(), callable_bits.into(), builder_bits.into()],
                runtime_name.symbol(),
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        self.finish_borrowed_operands(custody, bound, &format!("{lane}_result"), |_| {})
    }

    pub(super) fn emit_call_method(&mut self, op: &TirOp) {
        // CallMethod: receiver.method(args...).
        // Protocol: molt_call_bind_ic(site, bound_method_bits, args_builder) -> u64.
        if op.operands.is_empty() {
            return;
        }
        if op.argument_custody().is_some() {
            // An ordinary call that adopted its bound method and arguments:
            // the owned lane ends a temporary bound method of a Python
            // function before that function runs.
            let result = self.emit_call_func_owned_runtime(op);
            self.bind_owned_runtime_result(op, result);
            return;
        }
        // The bound method and its positional arguments are borrowed operands:
        // method-call arguments flow through `molt_call_bind_ic` into the bound
        // method's trampoline, which decodes each NaN-boxed `DynBox` into its
        // parameter's raw representation, so a raw `I64`/`F64` argument is boxed
        // once (never passed as raw bits) and a minted box is released after the
        // call.
        let mut custody = self.begin_borrowed_operands(&op.operands, "call_method");
        let method_bits = self.borrowed_operand(&mut custody, op.operands[0]);
        let mut arg_bits = Vec::with_capacity(op.operands.len() - 1);
        for &arg_id in &op.operands[1..] {
            arg_bits.push(self.borrowed_operand(&mut custody, arg_id));
        }
        let site_bits = self.next_call_site_bits("call_method");
        let call_bind_fn = self.ensure_runtime_import(RuntimeFn::CallBindIc);
        let bound = self.with_callargs(&arg_bits, |this, builder| {
            this.backend
                .builder
                .build_execution_call(
                    call_bind_fn,
                    &[site_bits.into(), method_bits.into(), builder.into()],
                    "call_method_bind",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        });
        let result = self.finish_borrowed_operands(custody, bound, "call_method_result", |_| {});
        self.bind_owned_runtime_result(op, result.into());
    }

    pub(super) fn emit_call_builtin(&mut self, op: &TirOp) {
        // IR owns executable identity and argument roles. Named calls carry
        // arguments only; dynamic lookup alone consumes operand zero as name.
        let i64_ty = self.backend.context.i64_type();
        let Some(call) = op.builtin_call() else {
            self.record_fatal("malformed CallBuiltin identity or argument contract");
            return;
        };

        if matches!(call.wire_kind, OpKind::Print | OpKind::BuiltinPrint) {
            // PRINT is a dedicated frontend op. By the time it reaches
            // backend IR, multi-argument CPython semantics have already
            // been normalized into a single joined display string plus
            // explicit newline behavior. Lower it directly to the
            // runtime print surface just like the native backend. Each
            // argument is borrowed by its print call; a failed box skips
            // every later print.
            let print_fn = self.ensure_runtime_import(RuntimeFn::PrintObj);
            let mut custody = self.begin_borrowed_operands(call.arguments, "print");
            for &arg_id in call.arguments {
                let arg_bits = self.borrowed_operand(&mut custody, arg_id);
                self.backend
                    .builder
                    .build_execution_call(print_fn, &[arg_bits.into()], "")
                    .unwrap();
            }
            let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
            self.finish_borrowed_operands(custody, none, "print_result", |_| {});
            if let Some(&result_id) = op.results.first() {
                self.values.set_value(result_id, none.into());
                self.values
                    .set_repr(result_id, crate::codegen_ssa::Repr::of(&TirType::DynBox));
            }
        } else if call.wire_kind == OpKind::RangeNew {
            // `range(...)` is a dedicated frontend op (`RANGE_NEW`), not a
            // generic builtin lookup. The SSA lifter folds it into
            // `OpCode::CallBuiltin` with `origin = "range_new"`
            // (ssa.rs), but `range` is NOT registered as a runtime
            // intrinsic and `molt_call_builtin` would fall through to the
            // builtins module-cache path — failing at any call site reached
            // before that cache is populated. Lower directly to the
            // dedicated runtime constructor `molt_range_new(start, stop,
            // step)`, exactly as the native and WASM backends do. The
            // frontend (`_parse_range_call`) always materializes all three
            // boxed bounds (start defaults to 0, step to 1), so operands is
            // exactly [start, stop, step], admitted by the shared call view.
            // The constructor borrows its bounds (it copies their values).
            let range_new_fn = self.ensure_runtime_import(RuntimeFn::RangeNew);
            let bounds: Vec<RuntimeArg<'ctx>> = call
                .arguments
                .iter()
                .map(|&bound| RuntimeArg::Operand(bound))
                .collect();
            self.emit_borrowed_runtime_call(
                op,
                range_new_fn,
                &bounds,
                Self::canonical_boxed_return(RuntimeFn::RangeNew, 3),
                "range_new",
                "range_new",
            );
        } else {
            // Build arguments only after a synthesized name is admitted. The
            // dynamic-name lane borrows its existing SSA operand instead.
            let result = match call.target {
                molt_ir::tir::ops::BuiltinCallTarget::Named(name) => self
                    .with_owned_name(name, |this, name_bits| {
                        this.emit_builtin_with_name(
                            RuntimeArg::Word(name_bits.into()),
                            call.arguments,
                        )
                        .into()
                    })
                    .into_int_value(),
                molt_ir::tir::ops::BuiltinCallTarget::Dynamic(name) => {
                    self.emit_builtin_with_name(RuntimeArg::Operand(*name), call.arguments)
                }
            };
            self.bind_owned_runtime_result(op, result.into());
        }
    }

    /// `molt_call_builtin(name, CallArgs(arguments))`, which consumes the
    /// builder. The arguments, and a dynamic name operand, are borrowed through
    /// one custody.
    fn emit_builtin_with_name(
        &mut self,
        name: RuntimeArg<'ctx>,
        arguments: &[ValueId],
    ) -> inkwell::values::IntValue<'ctx> {
        let mut operands = Vec::with_capacity(arguments.len() + 1);
        if let RuntimeArg::Operand(name) = name {
            operands.push(name);
        }
        operands.extend_from_slice(arguments);
        let mut custody = self.begin_borrowed_operands(&operands, "builtin_call");
        let name_bits: inkwell::values::BasicMetadataValueEnum<'ctx> = match name {
            RuntimeArg::Operand(name) => self.borrowed_operand(&mut custody, name).into(),
            RuntimeArg::Word(word) => word,
        };
        let mut arg_bits = Vec::with_capacity(arguments.len());
        for &arg_id in arguments {
            arg_bits.push(self.borrowed_operand(&mut custody, arg_id));
        }
        let call_builtin_fn = self.ensure_runtime_import(RuntimeFn::CallBuiltin);
        let bound = self.with_callargs(&arg_bits, |this, builder| {
            this.backend
                .builder
                .build_execution_call(
                    call_builtin_fn,
                    &[name_bits, builder.into()],
                    "call_builtin",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        });
        self.finish_borrowed_operands(custody, bound, "builtin_call_result", |_| {})
    }
}
