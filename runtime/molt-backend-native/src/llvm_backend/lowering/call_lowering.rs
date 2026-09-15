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
        let callable_arity = self
            .backend
            .function_linkage_abis
            .get(name)
            .map(|abi| {
                abi.param_types
                    .len()
                    .saturating_sub(usize::from(has_closure))
            })
            .unwrap_or(arity);
        let target_fn = self.ensure_function_symbol(name, callable_arity, has_closure);
        let target_return_tir_ty = self
            .backend
            .function_linkage_abis
            .get(name)
            .and_then(|abi| abi.return_type.clone())
            .unwrap_or(TirType::DynBox);
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
        let param_tir_types = self
            .backend
            .function_linkage_abis
            .get(name)
            .map(|abi| abi.param_types.as_slice());
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
            let arg = match param_tir_types.and_then(|tys| tys.get(param_index)) {
                Some(param_ty) => unbox_dynbox_to_param_ty_with_builder(
                    &builder,
                    self.backend.context,
                    &self.backend.module,
                    arg,
                    param_ty,
                ),
                None => arg,
            };
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
        let ret_bits = materialize_dynbox_bits_with_builder(
            &builder,
            self.backend.context,
            &self.backend.module,
            trampoline_fn,
            result,
            &target_return_tir_ty,
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

    pub(super) fn lower_call_method_ic_op(&mut self, op: &TirOp) -> bool {
        if op.operands.is_empty() {
            return false;
        }
        let Some(method_name) = Self::method_dispatch_name(op) else {
            return false;
        };
        let recv_bits = self.materialize_dynbox_operand(op.operands[0]);
        let extra: Vec<inkwell::values::IntValue<'ctx>> = op.operands[1..]
            .iter()
            .map(|&id| self.materialize_dynbox_operand(id))
            .collect();
        let i64_ty = self.backend.context.i64_type();
        let site_bits = self.next_call_site_bits("call_method_ic");
        let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(&method_name);
        let symbol = match extra.len() {
            0 => "molt_call_method_ic0",
            1 => "molt_call_method_ic1",
            2 => "molt_call_method_ic2",
            3 => "molt_call_method_ic3",
            4 => "molt_call_method_ic4",
            n => panic!(
                "call_method_ic supports at most 4 positional args in LLVM lowering; got {n}"
            ),
        };
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let mut param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            vec![i64_ty.into(), i64_ty.into(), ptr_ty.into(), i64_ty.into()];
        param_types.extend(
            (0..extra.len())
                .map(|_| -> inkwell::types::BasicMetadataTypeEnum<'ctx> { i64_ty.into() }),
        );
        let fn_ty = i64_ty.fn_type(&param_types, false);
        let call_fn =
            declare_fixed_runtime_function(self.backend.context, &self.backend.module, symbol)
                .unwrap_or_else(|| panic!("{symbol} must be a fixed LLVM runtime import"));
        let call_fn = require_llvm_function_type(symbol, call_fn, fn_ty);
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            site_bits.into(),
            recv_bits.into(),
            name_ptr.into(),
            name_len_bits.into(),
        ];
        call_args.extend(
            extra
                .iter()
                .map(|v| -> inkwell::values::BasicMetadataValueEnum<'ctx> { (*v).into() }),
        );
        let result = self
            .backend
            .builder
            .build_call(call_fn, &call_args, symbol)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
        true
    }

    pub(super) fn lower_call_super_method_ic_op(&mut self, op: &TirOp) -> bool {
        if op.operands.len() < 2 {
            return false;
        }
        let Some(method_name) = Self::method_dispatch_name(op) else {
            return false;
        };
        let class_bits = self.materialize_dynbox_operand(op.operands[0]);
        let self_bits = self.materialize_dynbox_operand(op.operands[1]);
        let extra: Vec<inkwell::values::IntValue<'ctx>> = op.operands[2..]
            .iter()
            .map(|&id| self.materialize_dynbox_operand(id))
            .collect();
        let i64_ty = self.backend.context.i64_type();
        let site_bits = self.next_call_site_bits("call_super_method_ic");
        let (name_ptr, name_len_bits) = self.raw_string_const_ptr_and_len(&method_name);
        let symbol = match extra.len() {
            0 => "molt_call_super_method_ic0",
            1 => "molt_call_super_method_ic1",
            2 => "molt_call_super_method_ic2",
            3 => "molt_call_super_method_ic3",
            4 => "molt_call_super_method_ic4",
            n => panic!(
                "call_super_method_ic supports at most 4 positional args in LLVM lowering; got {n}"
            ),
        };
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
            (0..extra.len())
                .map(|_| -> inkwell::types::BasicMetadataTypeEnum<'ctx> { i64_ty.into() }),
        );
        let fn_ty = i64_ty.fn_type(&param_types, false);
        let call_fn =
            declare_fixed_runtime_function(self.backend.context, &self.backend.module, symbol)
                .unwrap_or_else(|| panic!("{symbol} must be a fixed LLVM runtime import"));
        let call_fn = require_llvm_function_type(symbol, call_fn, fn_ty);
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            site_bits.into(),
            class_bits.into(),
            self_bits.into(),
            name_ptr.into(),
            name_len_bits.into(),
        ];
        call_args.extend(
            extra
                .iter()
                .map(|v| -> inkwell::values::BasicMetadataValueEnum<'ctx> { (*v).into() }),
        );
        let result = self
            .backend
            .builder
            .build_call(call_fn, &call_args, symbol)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
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
            let callable = self.resolve(op.operands[0]);
            let result = self.emit_call_func_or_bind_runtime(callable, &op.operands[1..]);
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
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
            let callable_i64 = self.ensure_i64(self.resolve(op.operands[0]));
            let builder_bits = self.ensure_i64(self.resolve(op.operands[1]));
            let site_bits = self.next_call_site_bits(original_kind.unwrap_or("call_bind"));
            let runtime_name = if matches!(original_kind, Some("call_indirect")) {
                "molt_call_indirect_ic"
            } else {
                "molt_call_bind_ic"
            };
            let runtime_fn = self.ensure_runtime_i64_fn(runtime_name, 3);
            let result = self
                .backend
                .builder
                .build_call(
                    runtime_fn,
                    &[site_bits.into(), callable_i64.into(), builder_bits.into()],
                    runtime_name,
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            return;
        }

        if matches!(original_kind, Some("call_guarded"))
            && let Some(callable_id) = guarded_callable
        {
            let callable = self.resolve(callable_id);
            let result = self.emit_call_func_runtime(callable, direct_operands);
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
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
                    let callable = self.resolve(callable_id);
                    let result = self.emit_call_bind_runtime(callable, direct_operands);
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("direct call must be emitted inside a basic block");
                // Direct call — all operands are positional args.
                // Every direct-call argument must be coerced from its
                // SOURCE TirType to the CALLEE's declared param TirType
                // (DynBox = the boxed molt ABI default). This was
                // previously gated on `call_guarded` only — a plain
                // `call`/`call_internal` passed an I64-typed value (or
                // constant) RAW into a NaN-boxed parameter, where the
                // raw bits decode as a garbage float (e.g.
                // `compute(1000000)` received ~4.9e-318 and the loop
                // exited after one iteration). The LLVM-type coercion
                // below is a bitcast-level cast and cannot substitute
                // for representation boxing.
                let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = direct_operands
                    .iter()
                    .enumerate()
                    .map(|(idx, &id)| {
                        let v = self.resolve(id);
                        let source_tir_ty = self
                            .value_types
                            .get(&id)
                            .cloned()
                            .unwrap_or(TirType::DynBox);
                        let target_tir_ty = self
                            .backend
                            .function_linkage_abis
                            .get(target_name.as_str())
                            .and_then(|abi| abi.param_types.get(idx))
                            .cloned()
                            .unwrap_or(TirType::DynBox);
                        let v =
                            self.coerce_to_tir_type(v, &source_tir_ty, &target_tir_ty, current_bb);
                        let target_ty = target_fn
                            .get_nth_param(idx as u32)
                            .map(|param| param.get_type())
                            .unwrap_or_else(|| self.backend.context.i64_type().into());
                        self.coerce_to_type(v, target_ty, current_bb).into()
                    })
                    .collect();
                let call_result = self
                    .backend
                    .builder
                    .build_call(target_fn, &args, "direct_call")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let raw_result =
                        call_result.try_as_basic_value().basic().unwrap_or_else(|| {
                            i64_ty
                                .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                                .into()
                        });
                    let result = if target_return_tir_ty == TirType::DynBox {
                        raw_result
                    } else {
                        materialize_dynbox_bits_with_builder(
                            &self.backend.builder,
                            self.backend.context,
                            &self.backend.module,
                            self.llvm_fn,
                            raw_result,
                            &target_return_tir_ty,
                        )
                        .into()
                    };
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            } else {
                if let Some(callable_id) = guarded_callable {
                    let callable = self.resolve(callable_id);
                    let result = self.emit_call_bind_runtime(callable, direct_operands);
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
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
            let callable = self.resolve(op.operands[0]);
            let result = self.emit_call_bind_runtime(callable, &op.operands[1..]);

            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
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

    pub(super) fn emit_call_method(&mut self, op: &TirOp) {
        // CallMethod: receiver.method(args...).
        // Protocol: molt_call_bind_ic(site, bound_method_bits, args_builder) -> u64.
        let i64_ty = self.backend.context.i64_type();
        if op.operands.is_empty() {
            return;
        }
        let method_bits = self.ensure_i64(self.resolve(op.operands[0]));

        // Build positional args (operands[1..]) for the bound method object.
        let n_args = op.operands.len().saturating_sub(1) as u64;
        let new_fn = self
            .backend
            .module
            .get_function("molt_callargs_new")
            .unwrap();
        let args_builder = self
            .backend
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(n_args, false).into(),
                    i64_ty.const_int(0, false).into(),
                ],
                "cm_args",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self
            .backend
            .module
            .get_function("molt_callargs_push_pos")
            .unwrap();
        for &arg_id in op.operands.get(1..).unwrap_or(&[]) {
            // Method-call args flow through `molt_call_bind_ic` into the
            // bound method's trampoline, which decodes each NaN-boxed
            // `DynBox` into its parameter's raw representation. Box per the
            // value's representation plan rather than passing raw bits (a
            // raw `I64`/`F64` arg would be decoded as a boxed payload —
            // the same carrier miscompile as the plain-call paths).
            let arg_i64 = self.materialize_dynbox_operand(arg_id);
            self.backend
                .builder
                .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cm_push")
                .unwrap();
        }
        let site_bits = self.next_call_site_bits("call_method");
        let call_bind_fn = self.ensure_runtime_i64_fn("molt_call_bind_ic", 3);
        let result = self
            .backend
            .builder
            .build_call(
                call_bind_fn,
                &[site_bits.into(), method_bits.into(), args_builder.into()],
                "call_method_bind",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
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
            // runtime print surface just like the native backend.
            let print_fn = self.ensure_runtime_void_fn("molt_print_obj", 1);
            for &arg_id in call.arguments {
                let arg_i64 = self.materialize_dynbox_operand(arg_id);
                self.backend
                    .builder
                    .build_call(print_fn, &[arg_i64.into()], "print")
                    .unwrap();
            }
            if let Some(&result_id) = op.results.first() {
                let none_val: BasicValueEnum<'ctx> = i64_ty
                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    .into();
                self.values.insert(result_id, none_val);
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
            let range_new_fn = self.ensure_runtime_i64_fn("molt_range_new", 3);
            let start = self.materialize_dynbox_operand(call.arguments[0]).into();
            let stop = self.materialize_dynbox_operand(call.arguments[1]).into();
            let step = self.materialize_dynbox_operand(call.arguments[2]).into();
            let result = self
                .backend
                .builder
                .build_call(range_new_fn, &[start, stop, step], "range_new")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
        } else {
            // Build arguments only after a synthesized name is admitted. The
            // dynamic-name lane borrows its existing SSA operand instead.
            let result = match call.target {
                molt_ir::tir::ops::BuiltinCallTarget::Named(name) => self
                    .with_owned_name(name, |this, name_bits| {
                        this.emit_builtin_with_name(name_bits, call.arguments)
                    }),
                molt_ir::tir::ops::BuiltinCallTarget::Dynamic(name) => {
                    let name_bits = self.materialize_dynbox_operand(*name);
                    self.emit_builtin_with_name(name_bits, call.arguments)
                }
            };
            if let Some(&result_id) = op.results.first() {
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
        }
    }

    fn emit_builtin_with_name(
        &mut self,
        builtin_name_bits: inkwell::values::IntValue<'ctx>,
        arguments: &[ValueId],
    ) -> BasicValueEnum<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let n_args = arguments.len() as u64;
        let new_fn = self
            .backend
            .module
            .get_function("molt_callargs_new")
            .unwrap();
        let args_builder = self
            .backend
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(n_args, false).into(),
                    i64_ty.const_int(0, false).into(),
                ],
                "cb_args",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        let push_fn = self
            .backend
            .module
            .get_function("molt_callargs_push_pos")
            .unwrap();
        for &arg_id in arguments {
            let arg_i64 = self.materialize_dynbox_operand(arg_id);
            self.backend
                .builder
                .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cb_push")
                .unwrap();
        }

        let call_builtin_fn = self
            .backend
            .module
            .get_function("molt_call_builtin")
            .unwrap();
        self.backend
            .builder
            .build_call(
                call_builtin_fn,
                &[builtin_name_bits.into(), args_builder.into()],
                "call_builtin",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }
}
