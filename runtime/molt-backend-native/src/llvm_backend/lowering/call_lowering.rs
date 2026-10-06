use super::*;

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
        let fn_ty = match linkage_abi.return_type.as_ref() {
            Some(return_type) => {
                lower_type(self.backend.context, return_type).fn_type(&params, false)
            }
            None => self.backend.context.void_type().fn_type(&params, false),
        };
        if let Some(func) = self.backend.module.get_function(name) {
            return require_llvm_function_type(name, func, fn_ty);
        }
        self.backend
            .module
            .add_function(name, fn_ty, Some(inkwell::module::Linkage::External))
    }

    pub(super) fn ensure_plain_trampoline(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let target_fn = self.ensure_function_symbol(name, arity, has_closure);
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
        arity: usize,
    ) -> Option<(FunctionValue<'ctx>, FunctionValue<'ctx>)> {
        use molt_ir::runtime_callable_abi_generated::{
            RuntimeCallableTrampolineAbi, runtime_callable_abi,
        };
        let Some(abi) = runtime_callable_abi(name) else {
            self.record_fatal(format!(
                "builtin_func target `{name}` has no runtime callable ABI; use func_new for compiled functions"
            ));
            return None;
        };
        if abi.arity != arity {
            self.record_fatal(format!(
                "builtin_func arity mismatch for `{name}`: manifest requires {}, got {arity}",
                abi.arity
            ));
            return None;
        }
        let target_fn = self.ensure_runtime_i64_fn(name, abi.arity);
        let trampoline = match abi.trampoline_abi {
            // This runtime entry already consumes (closure, argv, argc).
            RuntimeCallableTrampolineAbi::CallFrame => target_fn,
            RuntimeCallableTrampolineAbi::UnpackArgs => self.ensure_typed_trampoline(
                name,
                target_fn,
                false,
                &vec![TirType::DynBox; abi.arity],
                Some(&TirType::DynBox),
                &vec![crate::ir::ParameterCustody::Borrowed; abi.arity],
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
        if let Some(func) = self.backend.module.get_function(&trampoline_name) {
            return func;
        }

        let i64_ty = self.backend.context.i64_type();
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let trampoline_fn = self.backend.module.add_function(
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
            .get_nth_param(0)
            .expect("trampoline closure param missing")
            .into_int_value();
        let args_bits = trampoline_fn
            .get_nth_param(1)
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
                .get_nth_param(0)
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
                .get_nth_param(param_index as u32)
                .map(|param| param.get_type())
                .unwrap_or_else(|| i64_ty.into());
            call_args.push(coerce_trampoline_arg(
                arg,
                target_ty,
                &format!("trampoline_arg_cast_{idx}"),
            ));
        }

        let result = builder
            .build_call(target_fn, &call_args, "trampoline_call")
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

    pub(super) fn method_dispatch_name(op: &TirOp) -> Option<String> {
        op.attrs
            .get("method")
            .or_else(|| op.attrs.get("s_value"))
            .and_then(|v| match v {
                AttrValue::Str(s) => Some(s.clone()),
                _ => None,
            })
    }

    /// A fused method call instruction that adopted its receiver (or `self`)
    /// and its arguments: `molt_call_method_ic_owned(site, receiver, name,
    /// len, args, nargs)`, or the super form, whose class stays borrowed. The
    /// owned entry takes every adopted word over, ending the receiver at
    /// attribute resolution when the attribute does not bind it as `self`.
    fn lower_owned_method_ic(&mut self, op: &TirOp, method_name: &str, super_form: bool) -> bool {
        let (lane, symbol, first_arg) = if super_form {
            ("call_super_method_ic", "molt_call_super_method_ic_owned", 2)
        } else {
            ("call_method_ic", "molt_call_method_ic_owned", 1)
        };
        if op.operands.len() < first_arg {
            return false;
        }
        let call_fn =
            declare_fixed_runtime_function(self.backend.context, &self.backend.module, symbol)
                .unwrap_or_else(|| panic!("{symbol} must be a fixed LLVM runtime import"));
        let site_bits = self.next_call_site_bits(lane);
        let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(method_name);
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![site_bits.into()];
        let adopted = &op.operands[first_arg - 1..];
        let borrowed = &op.operands[..first_arg - 1];
        let mut custody = self.begin_call_operands(&op.operands, adopted, None, lane);
        if super_form {
            call_args.push(self.borrowed_operand(&mut custody, op.operands[0]).into());
        }
        let receiver = self.borrowed_operand(&mut custody, op.operands[first_arg - 1]);
        let args: Vec<inkwell::values::IntValue<'ctx>> = op.operands[first_arg..]
            .iter()
            .map(|&arg| self.borrowed_operand(&mut custody, arg))
            .collect();
        let (args_ptr, nargs) = self.spill_call_words(&args, &format!("{lane}_owned_args"));
        call_args.extend_from_slice(&[
            receiver.into(),
            name_ptr.into(),
            name_len_bits.into(),
            args_ptr.into(),
            nargs.into(),
        ]);
        self.surrender_borrowed_owners(&custody, adopted, borrowed);
        let result = self
            .backend
            .builder
            .build_call(call_fn, &call_args, symbol)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();
        let result =
            self.finish_borrowed_operands(custody, result, &format!("{lane}_result"), |_| {});
        self.bind_owned_runtime_result(op, result.into());
        true
    }

    pub(super) fn lower_call_method_ic_op(&mut self, op: &TirOp) -> bool {
        if op.operands.is_empty() {
            return false;
        }
        let Some(method_name) = Self::method_dispatch_name(op) else {
            return false;
        };
        if op.argument_custody().is_some() {
            return self.lower_owned_method_ic(op, &method_name, false);
        }
        let extra = op.operands.len() - 1;
        let symbol = match extra {
            0 => "molt_call_method_ic0",
            1 => "molt_call_method_ic1",
            2 => "molt_call_method_ic2",
            3 => "molt_call_method_ic3",
            4 => "molt_call_method_ic4",
            n => panic!(
                "call_method_ic supports at most 4 positional args in LLVM lowering; got {n}"
            ),
        };
        let i64_ty = self.backend.context.i64_type();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let mut param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            vec![i64_ty.into(), i64_ty.into(), ptr_ty.into(), i64_ty.into()];
        param_types.extend(
            (0..extra).map(|_| -> inkwell::types::BasicMetadataTypeEnum<'ctx> { i64_ty.into() }),
        );
        let fn_ty = i64_ty.fn_type(&param_types, false);
        let call_fn =
            declare_fixed_runtime_function(self.backend.context, &self.backend.module, symbol)
                .unwrap_or_else(|| panic!("{symbol} must be a fixed LLVM runtime import"));
        let call_fn = require_llvm_function_type(symbol, call_fn, fn_ty);
        let site_bits = self.next_call_site_bits("call_method_ic");
        let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(&method_name);
        // The receiver and positional arguments are borrowed by the fast path
        // and retained into CallArgs by the slow path (`call_method_ic_dispatch`).
        let mut args = vec![
            RuntimeArg::Word(site_bits.into()),
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Word(name_ptr.into()),
            RuntimeArg::Word(name_len_bits.into()),
        ];
        args.extend(
            op.operands[1..]
                .iter()
                .map(|&operand| RuntimeArg::Operand(operand)),
        );
        self.emit_borrowed_runtime_call(
            op,
            call_fn,
            &args,
            RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
            "call_method_ic",
            symbol,
        );
        true
    }

    pub(super) fn lower_call_super_method_ic_op(&mut self, op: &TirOp) -> bool {
        if op.operands.len() < 2 {
            return false;
        }
        let Some(method_name) = Self::method_dispatch_name(op) else {
            return false;
        };
        if op.argument_custody().is_some() {
            return self.lower_owned_method_ic(op, &method_name, true);
        }
        let extra = op.operands.len() - 2;
        let symbol = match extra {
            0 => "molt_call_super_method_ic0",
            1 => "molt_call_super_method_ic1",
            2 => "molt_call_super_method_ic2",
            3 => "molt_call_super_method_ic3",
            4 => "molt_call_super_method_ic4",
            n => panic!(
                "call_super_method_ic supports at most 4 positional args in LLVM lowering; got {n}"
            ),
        };
        let i64_ty = self.backend.context.i64_type();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let mut param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = vec![
            i64_ty.into(),
            i64_ty.into(),
            i64_ty.into(),
            ptr_ty.into(),
            i64_ty.into(),
        ];
        param_types.extend(
            (0..extra).map(|_| -> inkwell::types::BasicMetadataTypeEnum<'ctx> { i64_ty.into() }),
        );
        let fn_ty = i64_ty.fn_type(&param_types, false);
        let call_fn =
            declare_fixed_runtime_function(self.backend.context, &self.backend.module, symbol)
                .unwrap_or_else(|| panic!("{symbol} must be a fixed LLVM runtime import"));
        let call_fn = require_llvm_function_type(symbol, call_fn, fn_ty);
        let site_bits = self.next_call_site_bits("call_super_method_ic");
        let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(&method_name);
        let mut args = vec![
            RuntimeArg::Word(site_bits.into()),
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
            RuntimeArg::Word(name_ptr.into()),
            RuntimeArg::Word(name_len_bits.into()),
        ];
        args.extend(
            op.operands[2..]
                .iter()
                .map(|&operand| RuntimeArg::Operand(operand)),
        );
        self.emit_borrowed_runtime_call(
            op,
            call_fn,
            &args,
            RuntimeResultCustody::Boxed(RuntimeBoxedReturn::OwnedValue),
            "call_super_method_ic",
            symbol,
        );
        true
    }

    pub(super) fn emit_call(&mut self, op: &TirOp) {
        let i64_ty = self.backend.context.i64_type();
        let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        });

        if matches!(original_kind, Some("call_func") | Some("call_function"))
            && !op.operands.is_empty()
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
        let direct_operands: &[ValueId] = if matches!(original_kind, Some("call_guarded")) {
            op.operands.get(1..).unwrap_or(&[])
        } else {
            &op.operands
        };
        let guarded_callable = if matches!(original_kind, Some("call_guarded")) {
            op.operands.first().copied()
        } else {
            None
        };

        if matches!(original_kind, Some("call_bind") | Some("call_indirect"))
            && op.operands.len() >= 2
        {
            let (lane, runtime_name) = if matches!(original_kind, Some("call_indirect")) {
                ("call_indirect", "molt_call_indirect_ic")
            } else {
                ("call_bind", "molt_call_bind_ic")
            };
            // An ordinary call (a stack-form builder) adopted its callable as
            // well as the builder; one owned entry serves both spellings.
            let owned_callable = op.operand_custody(0) == crate::ir::ParameterCustody::Transferred;
            let runtime_name = if owned_callable {
                "molt_call_bind_ic_owned"
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

        if matches!(original_kind, Some("call_guarded"))
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
                let expected_params = target_fn.count_params() as usize;
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
                    Some(kind) => kind == "call",
                    None => {
                        crate::tir::call_targets::direct_call_symbol_for_op(op)
                            == Some(target_name.as_str())
                    }
                };
                if runtime_role
                    && let Some(abi) = runtime_boxed_abi(target_name, direct_operands.len())
                {
                    self.emit_boxed_runtime_call(op, abi);
                    return;
                }
                self.record_fatal(format!(
                    "direct call target `{target_name}` has no exact native linkage ABI; refusing to invent an i64 signature"
                ));
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(
                        result_id,
                        i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into(),
                    );
                    self.value_types.insert(result_id, TirType::DynBox);
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
                self.values.insert(result_id, none_val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
        }
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
        let source_tir_types: Vec<TirType> = operands
            .iter()
            .map(|id| self.value_types.get(id).cloned().unwrap_or(TirType::DynBox))
            .collect();
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
        let adopted =
            |idx: usize| op.operand_custody(idx) == crate::ir::ParameterCustody::Transferred;
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
                )
            };
            let target_ty = target_fn
                .get_nth_param(idx as u32)
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
            .build_call(target_fn, &args, "direct_call")
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
            self.values.insert(result_id, result.into());
            self.value_types.insert(result_id, TirType::DynBox);
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
        runtime_name: &str,
        owned_callable: bool,
    ) -> inkwell::values::IntValue<'ctx> {
        let builder_ty = self
            .value_types
            .get(&builder)
            .cloned()
            .unwrap_or(TirType::DynBox);
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
        let runtime_fn = self.ensure_runtime_i64_fn(runtime_name, 3);
        let bound = self
            .backend
            .builder
            .build_call(
                runtime_fn,
                &[site_bits.into(), callable_bits.into(), builder_bits.into()],
                runtime_name,
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
        let call_bind_fn = self.ensure_runtime_i64_fn("molt_call_bind_ic", 3);
        let bound = self.with_callargs(&arg_bits, |this, builder| {
            this.backend
                .builder
                .build_call(
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

    pub(super) fn emit_call_method_ic(&mut self, op: &TirOp) {
        if !self.lower_call_method_ic_op(op) {
            self.record_fatal(
                "malformed CallMethodIc op: expected receiver operand and method attr",
            );
        }
    }

    pub(super) fn emit_call_super_method_ic(&mut self, op: &TirOp) {
        if !self.lower_call_super_method_ic_op(op) {
            self.record_fatal(
                "malformed CallSuperMethodIc op: expected class/self operands and method attr",
            );
        }
    }

    pub(super) fn emit_call_builtin(&mut self, op: &TirOp) {
        // IR owns executable identity and argument roles. Named calls carry
        // arguments only; dynamic lookup alone consumes operand zero as name.
        let i64_ty = self.backend.context.i64_type();
        let Some(call) = op.builtin_call() else {
            self.record_fatal("malformed CallBuiltin identity or argument contract");
            return;
        };

        if matches!(call.wire_kind, "print" | "builtin_print") {
            // PRINT is a dedicated frontend op. By the time it reaches
            // backend IR, multi-argument CPython semantics have already
            // been normalized into a single joined display string plus
            // explicit newline behavior. Lower it directly to the
            // runtime print surface just like the native backend. Each
            // argument is borrowed by its print call; a failed box skips
            // every later print.
            let print_fn = self.ensure_runtime_void_fn("molt_print_obj", 1);
            let mut custody = self.begin_borrowed_operands(call.arguments, "print");
            for &arg_id in call.arguments {
                let arg_bits = self.borrowed_operand(&mut custody, arg_id);
                self.backend
                    .builder
                    .build_call(print_fn, &[arg_bits.into()], "")
                    .unwrap();
            }
            let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
            self.finish_borrowed_operands(custody, none, "print_result", |_| {});
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, none.into());
                self.value_types.insert(result_id, TirType::DynBox);
            }
        } else if call.wire_kind == "range_new" {
            // `range(...)` is a dedicated frontend op (`RANGE_NEW`), not a
            // generic builtin lookup. The SSA lifter folds it into
            // `OpCode::CallBuiltin` with `_original_kind = "range_new"`
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
            let range_new_fn = self.ensure_runtime_i64_fn("molt_range_new", 3);
            let bounds: Vec<RuntimeArg<'ctx>> = call
                .arguments
                .iter()
                .map(|&bound| RuntimeArg::Operand(bound))
                .collect();
            self.emit_borrowed_runtime_call(
                op,
                range_new_fn,
                &bounds,
                Self::canonical_boxed_return("molt_range_new", 3),
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
        let call_builtin_fn = self.ensure_runtime_i64_fn("molt_call_builtin", 2);
        let bound = self.with_callargs(&arg_bits, |this, builder| {
            this.backend
                .builder
                .build_call(
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
