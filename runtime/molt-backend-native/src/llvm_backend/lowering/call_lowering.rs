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
}
