use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;

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
        crate::llvm_backend::lowering::linked_compiled_function(self.backend, name)
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
    /// classifies the symbol from the same generated callable ABI. Returns the
    /// callable's catalog key (its identity, HF-881), its target and trampoline.
    pub(super) fn ensure_builtin_callable(
        &mut self,
        name: &str,
        runtime: Option<RuntimeFn>,
        arity: usize,
    ) -> Option<(u64, FunctionValue<'ctx>, FunctionValue<'ctx>)> {
        use molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi;
        // The typed runtime entry decoded at TIR ingest (HF-821 B1).
        let Some((function, key, trampoline_abi)) = runtime.and_then(|function| {
            let callable = function.callable()?;
            Some((function, callable.key, callable.trampoline?))
        }) else {
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
        Some((key, target_fn, trampoline))
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
        // The trampoline is a function of its own, with its own state.
        let trampoline_state = super::codegen_builder::LlvmFunctionState::new(true);
        let mut codegen = super::LlvmCodegen {
            words: super::LlvmWords {
                context: self.backend.context,
                builder: &builder,
            },
            backend: self.backend,
            params: &self.block_params,
            state: &trampoline_state,
        };
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
        // (`codegen_ssa::control::coerce`).
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
            let arg = crate::codegen_ssa::scalar::unbox_to_param_word(
                &mut codegen,
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
        let ret_bits = crate::codegen_ssa::scalar::box_value(
            &mut codegen,
            result,
            return_tir_type.unwrap_or(&TirType::DynBox),
        );
        builder.build_return(Some(&ret_bits)).unwrap();
        trampoline_fn
    }

    /// CPython's CALL after a method load: `[callable, self_or_null, args...]`,
    /// a source call that adopts every operand. `molt_call_loaded_method{N}`
    /// (or the spilled entry above four arguments) takes every word over: it
    /// enters the selected function with `self` prepended, or calls the
    /// attribute when `self_or_null` is the null sentinel. Failed boxing skips
    /// the entry and releases the instruction's inputs.
    ///
    /// Stays in LLVM until the walker builder can read the thread record
    /// (`thread_record_word`, the W3 switch lane's): the site call body
    /// passes it.
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

    /// `Call` (`FunctionCx::call_op`); a direct call of linked compiled code
    /// stays here until the direct-call slice.
    pub(super) fn emit_call(&mut self, op: &TirOp) {
        let target = self.call_target(op);
        let linked = target
            .as_deref()
            .is_some_and(|name| self.backend.function_linkage_abis.contains_key(name));
        let mut lowered = true;
        self.walker_op(|cx, b| lowered = cx.call_op(b, op, target.as_deref(), linked));
        if lowered {
            return;
        }
        let (name, target_fn, return_ty) = self
            .guarded_target_fn(op)
            .expect("a linked direct call has a compiled target");
        self.emit_direct_compiled_call(op, &name, target_fn, &op.operands, 0, &return_ty);
    }

    /// The linker symbol a call names: its typed callee, else `_var`.
    fn call_target(&self, op: &TirOp) -> Option<String> {
        self.callee_symbol(op)
            .or_else(|| match op.attrs.get("_var") {
                Some(AttrValue::Str(symbol)) if !symbol.is_empty() => Some(symbol.clone()),
                _ => None,
            })
    }

    /// The compiled target a guarded call or its late-TIR ops name, when its
    /// linkage row exists.
    fn guarded_target_fn(&self, op: &TirOp) -> Option<(String, FunctionValue<'ctx>, TirType)> {
        let name = self.call_target(op)?;
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
        self.values.set_repr(
            op.results[0],
            crate::codegen_ssa::HeldCarrier::of(&TirType::Bool),
        );
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
            // A boxed parameter takes the operand's custody word; any other
            // takes the operand converted to the parameter's carrier.
            let value: BasicValueEnum<'ctx> = if boxes_for_parameter(idx) {
                self.borrowed_operand(&mut custody, id).into()
            } else {
                let v = self.resolve(id);
                if adopted(idx)
                    && Self::tir_type_is_dynbox_like(&source_tir_types[idx])
                    && !Self::tir_type_is_dynbox_like(&param_tir_types[idx])
                {
                    extracted_owners.push(self.ensure_i64(v));
                }
                self.coerce_value(v, self.held(id), Some(id), &param_tir_types[idx])
            };
            args.push(value.into());
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
                crate::codegen_ssa::scalar::box_value(
                    &mut self.codegen(),
                    raw_result,
                    target_return_tir_ty,
                )
            };
            let result =
                self.finish_borrowed_operands(custody, result, "direct_call_result", |_| {});
            self.values.set_value(result_id, result.into());
            self.values.set_repr(
                result_id,
                crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
            );
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
}
