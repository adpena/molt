use super::*;

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(crate) fn ensure_trampoline(
        module: &mut ObjectModule,
        trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
        func_name: &str,
        linkage: Linkage,
        spec: TrampolineSpec,
    ) -> cranelift_module::FuncId {
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
        let kind_suffix = match kind {
            TrampolineKind::Plain => "",
            TrampolineKind::Generator => "_gen",
            TrampolineKind::Coroutine => "_coro",
            TrampolineKind::AsyncGen => "_asyncgen",
        };
        let trampoline_name = format!(
            "{func_name}__molt_trampoline_{arity}{closure_suffix}{kind_suffix}{ret_suffix}{import_suffix}"
        );
        let mut ctx = module.make_context();
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
        let nbc = NanBoxConsts::new();

        let closure_bits = builder.block_params(entry_block)[0];
        let args_ptr = builder.block_params(entry_block)[1];
        let _args_len = builder.block_params(entry_block)[2];

        let poll_target = if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            if func_name.ends_with("_poll") {
                func_name.to_string()
            } else {
                format!("{func_name}_poll")
            }
        } else {
            String::new()
        };

        match kind {
            TrampolineKind::Generator => {
                if closure_size < 0 {
                    panic!("generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        args_ptr,
                        arg_offset,
                    );
                    builder.ins().store(
                        MemFlagsData::trusted(),
                        arg_val,
                        obj_ptr,
                        offset + arg_offset,
                    );
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }
                builder.ins().return_(&[obj]);
            }
            TrampolineKind::Coroutine => {
                if closure_size < 0 {
                    panic!("coroutine closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("coroutine closure size too small for trampoline");
                }

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_COROUTINE);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                if payload_slots > 0 {
                    let mut inc_ref_obj_sig = module.make_signature();
                    inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                    let inc_ref_obj_callee = module
                        .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                        .unwrap();
                    let local_inc_ref_obj =
                        module.declare_func_in_func(inc_ref_obj_callee, builder.func);
                    let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                    let mut offset = 0i32;
                    if has_closure {
                        builder
                            .ins()
                            .store(MemFlagsData::trusted(), closure_bits, obj_ptr, offset);
                        builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                        offset += 8;
                    }
                    for idx in 0..arity {
                        let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                        let arg_val = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            args_ptr,
                            arg_offset,
                        );
                        builder.ins().store(
                            MemFlagsData::trusted(),
                            arg_val,
                            obj_ptr,
                            offset + arg_offset,
                        );
                        builder.ins().call(local_inc_ref_obj, &[arg_val]);
                    }
                }

                let mut get_sig = module.make_signature();
                get_sig.returns.push(AbiParam::new(types::I64));
                let get_callee = module
                    .declare_function("molt_cancel_token_get_current", Linkage::Import, &get_sig)
                    .unwrap();
                let get_local = module.declare_func_in_func(get_callee, builder.func);
                let get_call = builder.ins().call(get_local, &[]);
                let current_token = builder.inst_results(get_call)[0];

                let mut reg_sig = module.make_signature();
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.returns.push(AbiParam::new(types::I64));
                let reg_callee = module
                    .declare_function("molt_task_register_token_owned", Linkage::Import, &reg_sig)
                    .unwrap();
                let reg_local = module.declare_func_in_func(reg_callee, builder.func);
                builder.ins().call(reg_local, &[obj, current_token]);

                builder.ins().return_(&[obj]);
            }
            TrampolineKind::AsyncGen => {
                if closure_size < 0 {
                    panic!("async generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("async generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        args_ptr,
                        arg_offset,
                    );
                    builder.ins().store(
                        MemFlagsData::trusted(),
                        arg_val,
                        obj_ptr,
                        offset + arg_offset,
                    );
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }

                let mut asyncgen_sig = module.make_signature();
                asyncgen_sig.params.push(AbiParam::new(types::I64));
                asyncgen_sig.returns.push(AbiParam::new(types::I64));
                let asyncgen_callee = module
                    .declare_function("molt_asyncgen_new", Linkage::Import, &asyncgen_sig)
                    .unwrap();
                let asyncgen_local = module.declare_func_in_func(asyncgen_callee, builder.func);
                let asyncgen_call = builder.ins().call(asyncgen_local, &[obj]);
                let asyncgen_obj = builder.inst_results(asyncgen_call)[0];
                builder.ins().return_(&[asyncgen_obj]);
            }
            TrampolineKind::Plain => {
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
                let target_id = module
                    .declare_function(func_name, Linkage::Import, &target_sig)
                    .unwrap();
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
        builder.finalize();

        let trampoline_id = module
            .declare_function(&trampoline_name, Linkage::Local, &ctx.func.signature)
            .unwrap();
        if let Err(err) = module.define_function(trampoline_id, &mut ctx) {
            panic!("Failed to define trampoline {trampoline_name}: {err:?}");
        }
        trampoline_ids.insert(key, trampoline_id);
        trampoline_id
    }
}
