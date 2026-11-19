use super::*;
use molt_ir::runtime_fn_generated::RuntimeFn;

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(crate) fn ensure_trampoline(
        module: &mut dyn Module,
        trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
        import_ids: &mut RuntimeImportIds,
        func_name: &str,
        runtime_target: Option<RuntimeFn>,
        linkage: Linkage,
        spec: TrampolineSpec,
    ) -> cranelift_module::FuncId {
        Self::ensure_trampoline_with_inspector(
            module,
            trampoline_ids,
            import_ids,
            func_name,
            runtime_target,
            linkage,
            spec,
            |_| {},
        )
    }

    fn ensure_trampoline_with_inspector<F>(
        module: &mut dyn Module,
        trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
        import_ids: &mut RuntimeImportIds,
        func_name: &str,
        runtime_target: Option<RuntimeFn>,
        linkage: Linkage,
        spec: TrampolineSpec,
        inspect: F,
    ) -> cranelift_module::FuncId
    where
        F: FnOnce(&Context),
    {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret,
        } = spec;
        let is_import = matches!(linkage, Linkage::Import);
        let key = TrampolineKey {
            name: func_name.to_string(),
            arity,
            has_closure,
            is_import,
            kind,
            closure_size,
            target_has_ret,
        };
        if let Some(id) = trampoline_ids.get(&key) {
            return *id;
        }
        let closure_suffix = if has_closure { "_closure" } else { "" };
        let import_suffix = if is_import { "_import" } else { "" };
        let ret_suffix = if target_has_ret { "" } else { "_void" };
        let kind_suffix = kind.symbol_suffix();
        let trampoline_name = format!(
            "{func_name}__molt_trampoline_{arity}{closure_suffix}{kind_suffix}{ret_suffix}{import_suffix}"
        );
        let mut ctx = module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        let execution_context = builder.block_params(entry_block)[0];
        let closure_bits = builder.block_params(entry_block)[1];
        let args_ptr = builder.block_params(entry_block)[2];
        let args_len = builder.block_params(entry_block)[3];
        let behavior = kind.behavior();
        // The invocation's refcount admission: a thread-record word read once;
        // a trampoline with no refcount site leaves the trusted loads dead.
        let admission = thread_record_word(
            &mut builder,
            execution_context,
            molt_codegen_abi::thread_record::ThreadRecordWord::RefcountAdmission,
        );

        match behavior {
            TrampolineBehavior::Task(task_kind) => {
                let layout = crate::trampoline_task_layout(task_kind);
                layout.validate_closure_size(
                    closure_size,
                    arity,
                    has_closure,
                    GENERATOR_CONTROL_BYTES,
                );
                use molt_codegen_abi::inline_refcount::RefcountOperation;
                use molt_codegen_abi::task_site::{TaskFrameContext, TaskPayload};
                let poll_addr =
                    Self::task_poll_identity(module, &mut builder, func_name, Linkage::Import);
                let size_val = crate::native_backend::load_stateful_frame_size(
                    module,
                    &mut builder,
                    func_name,
                );
                // The trampoline borrows its closure and argument array; the
                // task reads them only once it exists.
                let payload = TaskPayload::BorrowedArguments {
                    closure: has_closure.then_some(closure_bits),
                    arguments: args_ptr,
                    count: arity,
                };
                let mut import = |function| {
                    let callee = Self::import_runtime_func_id_split(module, import_ids, function);
                    module.declare_func_in_func(callee, builder.func)
                };
                let refcount_site = |slow, operation| NativeRefcountSite {
                    slow,
                    admission,
                    operation,
                };
                let sites = NativeFrameSites {
                    retain: refcount_site(import(RuntimeFn::IncRefObj), RefcountOperation::Retain),
                    release: refcount_site(
                        import(RuntimeFn::DecRefObj),
                        RefcountOperation::Release,
                    ),
                    enter: import(RuntimeFn::FrameRecordEnter),
                    exit: import(RuntimeFn::FrameRecordExit),
                };
                let tasks = NativeTaskSites::with_imports(import);
                // The caller published the function it calls as this thread's
                // pending invocation; the task takes its frame context from it.
                let thread = thread_record_address(&mut builder, execution_context);
                let mut sealed = BTreeSet::new();
                let result = emit_task_construction(
                    &mut builder,
                    &mut sealed,
                    sites,
                    tasks,
                    layout,
                    poll_addr,
                    size_val,
                    TaskFrameContext::PendingInvocation { thread },
                    payload,
                );
                builder.ins().return_(&[result]);
            }
            TrampolineBehavior::ForwardCallFrame => {
                let mut target_sig = module.make_signature();
                target_sig.params.push(AbiParam::new(types::I64));
                target_sig.params.push(AbiParam::new(types::I64));
                target_sig.params.push(AbiParam::new(types::I64));
                if target_has_ret {
                    target_sig.returns.push(AbiParam::new(types::I64));
                }
                // A runtime callable's target is its typed runtime entry; every
                // other target is a compiled function.
                let target_id = match runtime_target {
                    Some(function) => import_ids.func_id(module, function),
                    None => declare_compiled_execution_function(
                        module,
                        func_name,
                        Linkage::Import,
                        &target_sig,
                    )
                    .unwrap(),
                };
                let target_ref = module.declare_func_in_func(target_id, builder.func);
                let call = builder
                    .ins()
                    .call(target_ref, &[closure_bits, args_ptr, args_len]);
                if target_has_ret {
                    let result = builder.inst_results(call)[0];
                    builder.ins().return_(&[result]);
                } else {
                    let none_val = builder.ins().iconst(types::I64, box_none());
                    builder.ins().return_(&[none_val]);
                }
            }
            TrampolineBehavior::UnpackArgs => {
                let mut call_args = Vec::with_capacity(arity + if has_closure { 1 } else { 0 });
                if has_closure {
                    call_args.push(closure_bits);
                }
                for idx in 0..arity {
                    let offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), args_ptr, offset);
                    call_args.push(arg_val);
                }

                let mut target_sig = module.make_signature();
                if has_closure {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                for _ in 0..arity {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                if target_has_ret {
                    target_sig.returns.push(AbiParam::new(types::I64));
                }
                // Always use Import for the target function inside
                // trampolines: the target is defined by its own
                // compile_func call (Export), and in batched compilation
                // the target may be in a different batch .o file.
                // A runtime callable's target is its typed runtime entry; every
                // other target is a compiled function.
                let target_id = match runtime_target {
                    Some(function) => import_ids.func_id(module, function),
                    None => declare_compiled_execution_function(
                        module,
                        func_name,
                        Linkage::Import,
                        &target_sig,
                    )
                    .unwrap(),
                };
                let target_ref = module.declare_func_in_func(target_id, builder.func);
                let call = builder.ins().call(target_ref, &call_args);
                if target_has_ret {
                    let res = builder.inst_results(call)[0];
                    builder.ins().return_(&[res]);
                } else {
                    let none_val = builder.ins().iconst(types::I64, box_none());
                    builder.ins().return_(&[none_val]);
                }
            }
        }

        builder.seal_all_blocks();
        builder.finalize(module.target_config());
        complete_attached_calls(module, &mut ctx.func, execution_context);
        inspect(&ctx);

        let trampoline_id = module
            .declare_function(
                &compiled_execution_symbol(&trampoline_name),
                Linkage::Local,
                &ctx.func.signature,
            )
            .unwrap();
        if let Err(err) = module.define_function(trampoline_id, &mut ctx) {
            panic!("Failed to define trampoline {trampoline_name}: {err:?}");
        }
        trampoline_ids.insert(key, trampoline_id);
        trampoline_id
    }

    #[cfg(test)]
    pub(crate) fn task_trampoline_clif_for_test(
        module: &mut dyn Module,
        import_ids: &mut RuntimeImportIds,
        func_name: &str,
        spec: TrampolineSpec,
    ) -> cranelift_codegen::ir::Function {
        let mut trampoline_ids = BTreeMap::new();
        let mut clif = None;
        Self::ensure_trampoline_with_inspector(
            module,
            &mut trampoline_ids,
            import_ids,
            func_name,
            None,
            Linkage::Import,
            spec,
            |ctx| clif = Some(ctx.func.clone()),
        );
        clif.expect("new task trampoline must expose generated CLIF")
    }
}
