use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn ensure_function_symbol(
        &self,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> FunctionValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            if let Some(param_types) = self.backend.function_param_types.get(name) {
                param_types
                    .iter()
                    .map(|ty| lower_type(self.backend.context, ty).into())
                    .collect()
            } else {
                let param_count = arity + usize::from(has_closure);
                (0..param_count).map(|_| i64_ty.into()).collect()
            };
        let return_ty = self
            .backend
            .function_return_types
            .get(name)
            .map(|ty| lower_type(self.backend.context, ty))
            .unwrap_or_else(|| i64_ty.into());
        let fn_ty = return_ty.fn_type(&params, false);
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
            .function_param_types
            .get(name)
            .map(|tys| tys.len().saturating_sub(usize::from(has_closure)))
            .unwrap_or(arity);
        let target_fn = self.ensure_function_symbol(name, callable_arity, has_closure);
        let target_return_tir_ty = self
            .backend
            .function_return_types
            .get(name)
            .cloned()
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
        let param_tir_types = self.backend.function_param_types.get(name);
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
            // param needs its 47-bit payload sign-extended back, a `Bool` its
            // low payload bit. `F64`/reference params are already the raw bits.
            let param_index = idx + usize::from(has_closure);
            let arg = match param_tir_types.and_then(|tys| tys.get(param_index)) {
                Some(param_ty) => unbox_dynbox_to_param_ty_with_builder(
                    &builder,
                    self.backend.context,
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
        let site_bits = self.next_call_site_bits("call_method_ic");
        let (name_ptr_bits, name_len_bits) = self.raw_string_const_ptr_len(&method_name);
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
        let call_fn = self.ensure_runtime_i64_fn(symbol, 4 + extra.len());
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            site_bits.into(),
            recv_bits.into(),
            name_ptr_bits.into(),
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
        let site_bits = self.next_call_site_bits("call_super_method_ic");
        let (name_ptr_bits, name_len_bits) = self.raw_string_const_ptr_len(&method_name);
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
        let call_fn = self.ensure_runtime_i64_fn(symbol, 5 + extra.len());
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            site_bits.into(),
            class_bits.into(),
            self_bits.into(),
            name_ptr_bits.into(),
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
}
