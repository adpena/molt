use super::task_construction::TaskConstruction;
use super::*;
use molt_tir::trampolines::TrampolineTaskKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// Python constructor arguments and the poll's one raw frame parameter
    /// are different ABIs. The shared task kind/layout owns that distinction.
    pub(super) fn ensure_callable_target(
        &self,
        op: &TirOp,
        name: &str,
        arity: usize,
        has_closure: bool,
    ) -> (FunctionValue<'ctx>, FunctionValue<'ctx>) {
        let metadata = match (op.attrs.get("task_kind"), op.attrs.get("task_closure_size")) {
            (None, None) => None,
            (Some(AttrValue::Str(kind)), Some(AttrValue::Int(size))) => Some((
                TrampolineTaskKind::from_constructor_kind(kind)
                    .unwrap_or_else(|| panic!("unknown callable task_kind `{kind}` for {name}")),
                *size,
            )),
            _ => {
                panic!("{name} requires complete callable task_kind and task_closure_size metadata")
            }
        };
        let Some((kind, closure_size)) = metadata else {
            return (
                self.ensure_function_symbol(name, arity, has_closure),
                self.ensure_plain_trampoline(name, has_closure),
            );
        };
        let layout = kind.constructor_layout();
        layout.validate_closure_size(
            closure_size,
            arity,
            has_closure,
            crate::GENERATOR_CONTROL_BYTES,
        );
        let poll = self.ensure_function_symbol(name, 1, false);
        let trampoline_name = format!(
            "{name}__molt_llvm_task_trampoline_{arity}_{}{}_{closure_size}",
            usize::from(has_closure),
            kind.trampoline_kind().symbol_suffix(),
        );
        if let Some(trampoline) = self.backend.module.get_function(&trampoline_name) {
            return (poll, trampoline);
        }
        let i64_ty = self.backend.context.i64_type();
        let trampoline = self.backend.module.add_function(
            &trampoline_name,
            i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false),
            Some(inkwell::module::Linkage::Internal),
        );
        let builder = self.backend.context.create_builder();
        builder.position_at_end(self.backend.context.append_basic_block(trampoline, "entry"));
        let construction = TaskConstruction::begin(
            self.backend,
            &builder,
            trampoline,
            poll.as_global_value()
                .as_pointer_value()
                .const_to_int(i64_ty),
            self.task_constructor_imports(layout),
            closure_size,
            ("task_new", "task_init", "task_ready"),
        );
        let arguments = trampoline.get_nth_param(1).unwrap().into_int_value();
        let arguments = builder
            .build_int_to_ptr(
                arguments,
                self.backend
                    .context
                    .ptr_type(inkwell::AddressSpace::default()),
                "task_arguments",
            )
            .unwrap();
        let retain = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
        let base = layout.payload_base_offset(crate::GENERATOR_CONTROL_BYTES) as u64 / 8;
        for index in 0..arity + usize::from(has_closure) {
            let word = if has_closure && index == 0 {
                trampoline.get_nth_param(0).unwrap().into_int_value()
            } else {
                let argument = index - usize::from(has_closure);
                let address = unsafe {
                    builder
                        .build_gep(
                            i64_ty,
                            arguments,
                            &[i64_ty.const_int(argument as u64, false)],
                            "task_argument_ptr",
                        )
                        .unwrap()
                };
                builder
                    .build_load(i64_ty, address, "task_argument")
                    .unwrap()
                    .into_int_value()
            };
            let destination = unsafe {
                builder
                    .build_gep(
                        i64_ty,
                        construction.payload,
                        &[i64_ty.const_int(base + index as u64, false)],
                        "task_payload_ptr",
                    )
                    .unwrap()
            };
            builder.build_store(destination, word).unwrap();
            builder.build_call(retain, &[word.into()], "").unwrap();
        }
        let result = construction.finish(self.backend, &builder);
        builder.build_return(Some(&result)).unwrap();
        (poll, trampoline)
    }
}
