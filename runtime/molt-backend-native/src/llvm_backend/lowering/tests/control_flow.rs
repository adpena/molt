use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use molt_ir::OpKind;
use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;

#[test]
fn a_virtual_lexical_level_materializes_at_service_and_origin_and_keeps_physical_refusals() {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    let mut f = TirFunction::new(
        "pending_level".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let handler = f.fresh_block();
    f.label_id_map.insert(handler.0, 9);
    let marker = |code| {
        let mut op = TirOp::new(code, vec![], vec![]);
        op.attrs.insert("value".into(), AttrValue::Int(9));
        op
    };
    let level = |kind| TirOp::new(OpCode::Copy, vec![], vec![]).with_origin(kind);
    let mut check = marker(OpCode::CheckException);
    check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
    let mut service = check.clone();
    service.mark_async_work_poll();
    let depth = f.fresh_value();
    f.value_types.insert(depth, TirType::I64);
    let snapshot =
        TirOp::new(OpCode::Copy, vec![], vec![depth]).with_origin(OpKind::ExceptionStackDepth);
    let restore =
        TirOp::new(OpCode::Copy, vec![depth], vec![]).with_origin(OpKind::ExceptionStackSetDepth);
    let mut unwind = check.clone();
    unwind.set_exception_edge_kind(ExceptionEdgeKind::Unwind);
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops = vec![
        snapshot,
        level(OpKind::ExceptionPush),
        marker(OpCode::TryStart),
        check,
        service,
        level(OpKind::ExceptionPop),
        restore.clone(),
        unwind,
        restore,
    ];
    entry.terminator = Terminator::Return { values: vec![] };
    f.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![marker(OpCode::TryEnd), level(OpKind::ExceptionPop)],
            terminator: Terminator::Unwind,
        },
    );

    let compile = |f: &TirFunction, available| {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        for entry in [
            RuntimeFn::ExceptionStackDepth,
            RuntimeFn::ExceptionStackSetDepth,
        ] {
            backend.runtime_callable_symbols.insert(entry);
        }
        if available {
            backend
                .runtime_callable_symbols
                .insert(RuntimeFn::ExceptionPushOrDefer);
        } else {
            backend
                .runtime_callable_symbols
                .remove(RuntimeFn::ExceptionPushOrDefer);
        }
        let machine = lower_tir_to_llvm(f, &backend);
        backend
            .module
            .verify()
            .unwrap_or_else(|err| panic!("{err}\n{}", machine.print_to_string()));
        machine.print_to_string().to_string()
    };
    let ir = compile(&f, true);
    assert_eq!(
        ir.matches_execution("@molt_exception_push_or_defer(")
            .count(),
        1,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_exception_push(").count(),
        3,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_async_work_poll_and_exception_pending(")
            .count(),
        1,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_exception_pop(").count(),
        2,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_exception_stack_set_depth(")
            .count(),
        2,
        "{ir}"
    );
    assert_eq!(ir.matches("xor i64").count(), 1, "{ir}");
    assert!(!compile(&f, false).contains("@molt_exception_push_or_defer"));
    let mut block = "";
    let mut observers = 0;
    let mut guarded_restores = 0;
    for line in ir.lines() {
        if !line.starts_with(' ')
            && let Some((label, _)) = line.split_once(':')
        {
            block = label;
        }
        if line.contains("call ")
            && (line.contains("@molt_async_work_poll_and_exception_pending")
                || line.contains("@molt_exception_frame_origin"))
        {
            assert!(
                block.starts_with("exception_level_ready"),
                "{block}: {line}"
            );
            observers += 1;
        }
        if line.contains("call ") && line.contains("@molt_exception_stack_set_depth") {
            assert!(
                block.starts_with("exception_depth_restore"),
                "{block}: {line}"
            );
            guarded_restores += 1;
        }
    }
    assert_eq!(observers, 3, "{ir}");
    assert_eq!(guarded_restores, 2, "{ir}");

    // The normal and exceptional edges can have the same predecessor block.
    // Its early check reaches the later operations without executing the push.
    let mut bypass = f.clone();
    let later = bypass.fresh_block();
    bypass.label_id_map.insert(later.0, 11);
    let entry = bypass.blocks.get_mut(&bypass.entry_block).unwrap();
    let tail = entry.ops.split_off(3);
    let mut early = marker(OpCode::CheckException);
    early.attrs.insert("value".into(), AttrValue::Int(11));
    early.set_exception_edge_kind(ExceptionEdgeKind::Origin);
    entry.ops.insert(1, early);
    let rest = std::mem::replace(
        &mut entry.terminator,
        Terminator::Branch {
            target: later,
            args: vec![],
        },
    );
    bypass.blocks.insert(
        later,
        TirBlock {
            id: later,
            args: vec![],
            ops: tail,
            terminator: rest,
        },
    );
    assert!(!compile(&bypass, true).contains("@molt_exception_push_or_defer"));
    let result = f.fresh_value();
    f.value_types.insert(result, TirType::DynBox);
    f.blocks.get_mut(&f.entry_block).unwrap().ops.insert(
        3,
        TirOp::new(OpCode::Copy, vec![], vec![result]).with_origin(OpKind::ExceptionActive),
    );
    assert!(
        !compile(&f, true).contains("@molt_exception_push_or_defer"),
        "an intervening context reader needs the actual level"
    );
}

#[test]
fn scoped_frame_record_captures_a_zero_home_owner_and_skips_a_frameless_pop() {
    for enters in [false, true] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "scoped_frame_record".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        func.execution_context = crate::ir::ExecutionContextPolicy::Local;
        let handler = func.fresh_block();
        func.label_id_map.insert(handler.0, 7);
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unwind,
            },
        );
        let mut check = TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(7))]),
            source_span: None,
        };
        check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        if enters {
            entry.ops.push(TirOp {
                origin: Some(OpKind::TraceEnterSlot),
                opcode: OpCode::Copy,
                attrs: AttrDict::from_iter([
                    ("s_value".into(), AttrValue::Str(func.name.clone())),
                    ("value".into(), AttrValue::Int(0)),
                ]),
                ..check.clone()
            });
        }
        entry.ops.push(check);
        entry.terminator = Terminator::Return { values: vec![] };
        let function = lower_tir_to_llvm(&func, &backend);
        backend.module.verify().unwrap();
        let ir = function.print_to_string().to_string();
        let origin = ir
            .lines()
            .find(|line| line.contains_execution("@molt_exception_frame_origin("))
            .expect("the cold edge captures its selected record");
        assert_eq!(origin.contains("i64 0)"), !enters, "{ir}");
        assert_eq!(ir.contains("%owned_frame_base"), enters, "{ir}");
        assert_eq!(
            ir.matches_execution("@__molt_frame_pop_0_up(").count(),
            if enters { 2 } else { 0 },
            "{ir}"
        );
    }
}

#[test]
fn local_frame_credit_is_guarded_on_early_normal_and_suspension_returns() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "frame_return_edges".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.execution_context = crate::ir::ExecutionContextPolicy::Local;
    let early = func.fresh_block();
    let entered = func.fresh_block();
    let normal = func.fresh_block();
    let suspension = func.fresh_block();
    let args: Vec<_> = func.blocks[&func.entry_block]
        .args
        .iter()
        .map(|arg| arg.id)
        .collect();
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond: args[0],
        then_block: entered,
        then_args: vec![],
        else_block: early,
        else_args: vec![],
    };
    let marker = |kind, attrs| TirOp {
        origin: Some(kind),
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    };
    func.blocks.insert(
        entered,
        TirBlock {
            id: entered,
            args: vec![],
            ops: vec![marker(
                OpKind::TraceEnterSlot,
                AttrDict::from_iter([(
                    "s_value".into(),
                    AttrValue::Str("frame_return_edges".into()),
                )]),
            )],
            terminator: Terminator::CondBranch {
                cond: args[1],
                then_block: normal,
                then_args: vec![],
                else_block: suspension,
                else_args: vec![],
            },
        },
    );
    for (id, ops) in [
        (early, vec![]),
        (normal, vec![marker(OpKind::TraceExit, AttrDict::default())]),
        (suspension, vec![]),
    ] {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops,
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    let function = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().unwrap();
    let ir = function.print_to_string().to_string();
    assert_eq!(
        ir.matches_execution("@molt_frame_record_enter(").count(),
        1,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@__molt_frame_pop_linked(").count(),
        3,
        "every return needs its own guarded closure: {ir}"
    );
    assert_eq!(
        ir.matches_execution("store i64 %frame_record_enter, ptr %owned_frame_base")
            .count(),
        1,
        "only the entry's own record base grants a pop: {ir}"
    );
    assert_eq!(
        ir.matches_execution("store i64 0, ptr %owned_frame_base")
            .count(),
        4,
        "one initial empty base, retired before each potential pop: {ir}"
    );
    // The pop body tests the base, so an early exit does not pop the caller.
    let module_ir = backend.module.print_to_string().to_string();
    let pop_body = module_ir
        .split("define ")
        .find(|body| body.starts_with("internal void @__molt_frame_pop_linked"))
        .expect("frame pop body");
    assert!(
        pop_body.contains("icmp ne i64") && pop_body.contains("br i1 %word_test"),
        "{pop_body}"
    );
    assert_eq!(
        pop_body.matches("molt_frame_record_exit").count(),
        1,
        "{pop_body}"
    );
}

#[test]
fn path_local_try_markers_do_not_duplicate_explicit_exception_stack_state() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    // Both coordinate operations retain an admitted cold runtime path.
    backend
        .runtime_callable_symbols
        .insert(RuntimeFn::ExceptionStackEnter);
    backend
        .runtime_callable_symbols
        .insert(RuntimeFn::ExceptionStackExit);
    let mut func = TirFunction::new(
        "path_local_try".into(),
        vec![TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let left = func.fresh_block();
    let right = func.fresh_block();
    let previous_baseline = func.fresh_value();
    let marker = |opcode| TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(100))]),
        source_span: None,
    };
    let runtime = |kind: OpKind, operands, results| TirOp {
        origin: Some(kind),
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results,
        attrs: AttrDict::from_iter([]),
        source_span: None,
    };
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.extend([
        runtime(OpKind::ExceptionStackEnter, vec![], vec![previous_baseline]),
        runtime(OpKind::ExceptionPush, vec![], vec![]),
        marker(OpCode::TryStart),
    ]);
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: left,
        then_args: vec![],
        else_block: right,
        else_args: vec![],
    };
    for id in [left, right] {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![
                    marker(OpCode::TryEnd),
                    runtime(OpKind::ExceptionPop, vec![], vec![]),
                    runtime(OpKind::ExceptionStackExit, vec![previous_baseline], vec![]),
                ],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    let llvm_fn = try_lower_tir_to_llvm(&func, &backend)
        .expect("alternative region closes must lower independently of block visitation order");
    backend
        .module
        .verify()
        .expect("path-local cleanup must verify");
    let ir = llvm_fn.print_to_string().to_string();
    assert_eq!(
        ir.matches_execution("@molt_exception_stack_enter(").count(),
        1,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_exception_push(").count(),
        1,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_exception_pop(").count(),
        2,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@molt_exception_stack_exit(").count(),
        2,
        "{ir}"
    );
    assert!(
        !ir.contains_execution("try_baseline"),
        "metadata must not allocate a second runtime baseline: {ir}"
    );
}

#[test]
fn task_allocation_failure_skips_initialization_and_rejoins_cleanup() {
    use molt_codegen_abi::task_site::{TaskCompletion, TaskConstructorLayout};

    for (call_async, task_kind, layout) in [
        (false, None, TaskConstructorLayout::future()),
        (false, Some("future"), TaskConstructorLayout::future()),
        (false, Some("generator"), TaskConstructorLayout::generator()),
        (false, Some("coroutine"), TaskConstructorLayout::coroutine()),
        (true, None, TaskConstructorLayout::for_call_async()),
    ] {
        for payload_count in [0, 2] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            let poll_id = molt_ir::FunctionId::from_index(0);
            backend.function_table = molt_ir::FunctionTable::from_rows([(
                "opaque_task_body",
                molt_ir::function_table::FunctionRowKind::Extern,
            )]);
            backend.function_linkage_abis.insert(
                "opaque_task_body".into(),
                test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
            );
            let payload_base = layout.payload_base_offset(crate::GENERATOR_CONTROL_BYTES);
            let mut func = TirFunction::new(
                "task_failure_cleanup".into(),
                // Two payload words, then the task's function.
                vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            func.execution_context = crate::ir::ExecutionContextPolicy::Local;
            let result = func.fresh_value();
            let pending = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let args: Vec<_> = entry.args.iter().map(|arg| arg.id).collect();
            entry.ops.push(TirOp {
                origin: Some(OpKind::TraceEnterSlot),
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::from_iter([(
                    "s_value".into(),
                    AttrValue::Str("task_failure_cleanup".into()),
                )]),
                source_span: None,
            });
            let mut attrs = AttrDict::from_iter([
                ("s_value".into(), AttrValue::Str("opaque_task_body".into())),
                (
                    "value".into(),
                    AttrValue::Int(i64::from(payload_base) + (payload_count as i64) * 8),
                ),
            ]);
            if let Some(kind) = task_kind {
                attrs.insert("task_kind".into(), AttrValue::Str(kind.into()));
            }
            let origin = call_async.then_some(OpKind::CallAsync);
            entry.ops.push(TirOp {
                origin,
                runtime: None,
                callee: Some(poll_id),
                dialect: Dialect::Molt,
                opcode: if call_async {
                    OpCode::Copy
                } else {
                    OpCode::AllocTask
                },
                // alloc_task_operands: the function, then the payload.
                operands: if call_async {
                    args[..payload_count].to_vec()
                } else {
                    std::iter::once(args[2])
                        .chain(args[..payload_count].iter().copied())
                        .collect()
                },
                results: vec![result],
                attrs,
                source_span: None,
            });
            entry.ops.push(TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ExceptionPending,
                operands: vec![],
                results: vec![pending],
                attrs: AttrDict::default(),
                source_span: None,
            });
            for arg in args {
                entry.ops.push(TirOp {
                    origin: None,
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::DecRef,
                    operands: vec![arg],
                    results: vec![],
                    attrs: AttrDict::default(),
                    source_span: None,
                });
            }
            entry.ops.push(TirOp {
                origin: Some(OpKind::TraceExit),
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::from_iter([]),
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![result],
            };

            let llvm_fn = try_lower_tir_to_llvm(&func, &backend)
                .expect("task allocation and its cleanup must lower");
            backend
                .module
                .verify()
                .expect("task failure join must verify");
            let ir = llvm_fn.print_to_string().to_string();
            let blocks = llvm_fn.get_basic_blocks();
            let block_ir = |bb: &inkwell::basic_block::BasicBlock<'_>| {
                let mut lines = Vec::new();
                let mut current = bb.get_first_instruction();
                while let Some(instruction) = current {
                    lines.push(instruction.print_to_string().to_string());
                    current = instruction.get_next_instruction();
                }
                lines.join("\n")
            };
            // The construction runs in the shared task helper
            // (`molt_codegen_abi::task_site`), which this function calls once.
            let helper = backend
                .module
                .get_functions()
                .find(|function| {
                    function
                        .get_name()
                        .to_str()
                        .unwrap()
                        .starts_with("__molt_task_site_")
                })
                .expect("the task construction helper");
            let helper_name = helper.get_name().to_str().unwrap().to_owned();
            assert_eq!(
                ir.matches(&format!("@{helper_name}(")).count(),
                1,
                "the caller constructs through the helper once: {ir}"
            );
            let helper_ir = helper.print_to_string().to_string();
            let helper_blocks = helper.get_basic_blocks();
            let allocation = helper_blocks
                .iter()
                .find(|bb| block_ir(bb).contains_execution("@molt_task_new("))
                .expect("runtime allocator block");
            let init = helper_blocks
                .iter()
                .find(|bb| bb.get_name().to_str().unwrap().starts_with("frame_then"))
                .expect("success-only task initializer");
            let allocation_ir = block_ir(allocation);
            let init_ir = block_ir(init);
            assert!(
                allocation_ir.contains_execution(&format!(
                    "icmp ne i64 %task_new, {}",
                    nanbox::QNAN | nanbox::TAG_NONE
                )),
                "{helper_ir}"
            );
            assert!(
                allocation_ir
                    .contains_execution(&format!("label %{}", init.get_name().to_str().unwrap()))
                    && allocation_ir.contains_execution("br i1"),
                "allocation must branch to the success-only initializer: {helper_ir}"
            );
            // Nothing is stored into the task before the allocation succeeds
            // (the constructor's result local is not the task).
            assert!(
                !allocation_ir
                    .lines()
                    .any(|line| line.contains("store i64") && !line.contains("ptr %frame_local"))
                    && !allocation_ir.contains_execution("ret "),
                "{helper_ir}"
            );
            assert_eq!(
                init_ir.matches_execution("store i64").count(),
                payload_count,
                "{helper_ir}"
            );
            assert_eq!(
                init_ir.matches_execution("@__molt_inline_retain(").count(),
                payload_count,
                "{helper_ir}"
            );
            let registers_token = layout.completion() == TaskCompletion::RegisterCancelToken;
            assert_eq!(
                init_ir.contains_execution("@molt_cancel_token_get_current("),
                registers_token,
                "{helper_ir}"
            );
            assert_eq!(
                init_ir.contains_execution("@molt_task_register_execution("),
                registers_token,
                "{helper_ir}"
            );
            assert!(!init_ir.contains_execution("ret "), "{helper_ir}");
            // The caller's continuation: everything after the helper call but
            // the exception check's cold confirmation.
            let caller_ir = blocks
                .iter()
                .filter(|bb| {
                    !bb.get_name()
                        .to_str()
                        .unwrap()
                        .starts_with("exception_pending_confirm")
                })
                .map(block_ir)
                .collect::<Vec<_>>()
                .join("\n");
            let after_call = caller_ir
                .find(&format!("@{helper_name}("))
                .expect("the caller calls the task helper");
            let ready_ir = caller_ir[after_call..]
                .split_once('\n')
                .map_or(String::new(), |(_, rest)| rest.to_owned());
            // Invariant J1: the continuation tests the record's pending byte
            // inline; only a set byte reaches the runtime reader.
            assert!(
                ready_ir.contains_execution("load i8, ptr %record_word"),
                "{ir}"
            );
            assert!(
                !ready_ir.contains_execution("@molt_exception_pending("),
                "{ir}"
            );
            assert!(
                blocks.iter().any(|bb| {
                    bb.get_name()
                        .to_str()
                        .unwrap()
                        .starts_with("exception_pending_confirm")
                        && block_ir(bb).contains_execution("@molt_exception_pending(")
                }),
                "{ir}"
            );
            // Cleanup releases each of the three parameters (two payload words
            // and the task's function).
            assert_eq!(
                ready_ir
                    .matches_execution("@__molt_inline_release(")
                    .count(),
                3,
                "{ir}"
            );
            let record_base = caller_ir
                .find("store i64 %frame_record_enter")
                .expect("the frame's record base is stored");
            assert!(
                record_base < after_call,
                "the frame's record base must precede allocation: {ir}"
            );
            let retire = ready_ir
                .find("store i64 0, ptr %owned_frame_base")
                .expect("cleanup retires the record base");
            let pop = ready_ir
                .find("@__molt_frame_pop_linked")
                .expect("cleanup must reach the guarded frame exit");
            assert!(
                retire < pop,
                "retire the record base before finalizers: {ir}"
            );
            // Nothing but the return can consume the allocated task. The caller
            // still reads its own thread record when checking for an exception.
            let allocation = blocks
                .iter()
                .flat_map(|block| block.get_instructions())
                .find(|inst| {
                    inkwell::values::CallSiteValue::try_from(*inst)
                        .is_ok_and(|call| call.get_called_fn_value() == Some(helper))
                })
                .expect("the task allocation result");
            let use_ = allocation.get_first_use().expect("the result is returned");
            assert!(
                use_.get_next_use().is_none(),
                "the task has a second user: {ir}"
            );
            assert_eq!(
                use_.get_user().into_instruction_value().get_opcode(),
                inkwell::values::InstructionOpcode::Return,
                "{ir}"
            );
            assert_eq!(ready_ir.matches_execution("store i64").count(), 1, "{ir}");
            assert!(
                !ready_ir.contains_execution("@__molt_inline_retain(")
                    && !ready_ir.contains_execution("@molt_task_register_execution("),
                "{ir}"
            );
            assert!(
                !ir.contains_execution("@molt_exception_clear("),
                "allocation failure must preserve pending exception: {ir}"
            );
        }
    }
}

#[test]
fn llvm_task_producers_emit_runtime_keys_and_compiled_code_addresses() {
    for (symbol, expected_key) in [
        (
            "molt_async_sleep_poll",
            Some(molt_codegen_abi::runtime_callable_key(
                "molt_async_sleep_poll",
            )),
        ),
        (
            "molt_promise_poll",
            Some(molt_codegen_abi::runtime_callable_key("molt_promise_poll")),
        ),
        ("ordinary_poll", None),
    ] {
        for call_async in [false, true] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            let callee = expected_key.is_none().then(|| {
                backend.function_table = molt_ir::FunctionTable::from_rows([(
                    symbol,
                    molt_ir::function_table::FunctionRowKind::Extern,
                )]);
                molt_ir::FunctionId::from_index(0)
            });
            if expected_key.is_none() {
                backend.function_linkage_abis.insert(
                    symbol.into(),
                    test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
                );
            }
            let mut function = TirFunction::new(
                "task_identity_probe".into(),
                // The task's function word (None for a runtime-native poll).
                vec![TirType::DynBox],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let result = function.fresh_value();
            let entry = function.blocks.get_mut(&function.entry_block).unwrap();
            let attrs = AttrDict::from_iter([
                ("s_value".into(), AttrValue::Str(symbol.into())),
                ("value".into(), AttrValue::Int(16)),
                ("task_kind".into(), AttrValue::Str("future".into())),
            ]);
            let origin = call_async.then_some(OpKind::CallAsync);
            entry.ops.push(TirOp {
                origin,
                runtime: RuntimeFn::from_symbol(symbol),
                callee,
                dialect: Dialect::Molt,
                opcode: if call_async {
                    OpCode::Copy
                } else {
                    OpCode::AllocTask
                },
                operands: if call_async {
                    vec![]
                } else {
                    vec![entry.args[0].id]
                },
                results: vec![result],
                attrs,
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![result],
            };
            let lowered = try_lower_tir_to_llvm(&function, &backend).unwrap();
            backend
                .module
                .verify()
                .expect("task poll identity module must verify");
            // The probe constructs through the shared task helper; the poll
            // identity is the helper call's constant poll argument (the task
            // helper's first word after the execution context).
            let mut allocations = Vec::new();
            for block in lowered.get_basic_blocks() {
                let mut current = block.get_first_instruction();
                while let Some(inst) = current {
                    current = inst.get_next_instruction();
                    if let Ok(call) = inkwell::values::CallSiteValue::try_from(inst)
                        && call.get_called_fn_value().is_some_and(|callee| {
                            callee
                                .get_name()
                                .to_str()
                                .is_ok_and(|name| name.starts_with("__molt_task_site_"))
                        })
                    {
                        allocations.push(inst);
                    }
                }
            }
            assert_eq!(allocations.len(), 1);
            let poll = allocations[0]
                .get_operand(1)
                .unwrap()
                .value()
                .unwrap()
                .into_int_value();
            assert_eq!(
                poll.get_zero_extended_constant(),
                expected_key,
                "call_async={call_async}/{symbol}: {}",
                lowered.print_to_string()
            );
            let size_symbol = molt_ir::code_slots::stateful_frame_size_symbol(symbol);
            assert_eq!(
                backend.module.get_global(&size_symbol).is_some(),
                expected_key.is_none(),
                "a provider contributes a frame-size relocation only for a compiled poll"
            );
            if expected_key.is_none() {
                let compiled = backend
                    .module
                    .get_compiled_function(symbol)
                    .unwrap()
                    .as_global_value()
                    .as_pointer_value()
                    .const_to_int(ctx.i64_type());
                assert_eq!(
                    poll, compiled,
                    "compiled poll must retain its exact function address"
                );
                // A relocatable symbol still cannot be invoked as a poll if its
                // provider returns a raw float through the host floating-point ABI.
                backend
                    .function_linkage_abis
                    .get_mut(symbol)
                    .unwrap()
                    .return_type = Some(TirType::F64);
                function.name = "invalid_poll_result".into();
                let error = try_lower_tir_to_llvm(&function, &backend)
                    .expect_err("a task cannot enter a float-returning poll");
                assert!(
                    error
                        .to_string()
                        .contains("must have ABI (context, task) -> i64")
                );
            }
        }
    }
}

#[test]
fn lower_const_and_return() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn f() -> i64 { return 42 }
    let mut func = TirFunction::new(
        "const_ret".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let v0 = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs: {
            let mut m = AttrDict::default();
            m.insert("value".into(), AttrValue::Int(42));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v0] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains_execution("const_ret"),
        "function name missing from IR"
    );
    assert!(ir.contains_execution("42"), "constant 42 missing from IR");
    assert!(
        ir.contains_execution("ret "),
        "return instruction missing from IR"
    );
}

#[test]
fn lowers_exception_pop_then_dec_ref_from_shared_drop_shape() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new(
        "exception_drop".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    let mut exception_pop = TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::default(),
        source_span: None,
    };
    exception_pop.origin = Some(OpKind::ExceptionPop);
    entry.ops.push(exception_pop);
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::default(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    let pop_pos = ir
        .find_execution("molt_exception_pop")
        .unwrap_or_else(|| panic!("LLVM must call molt_exception_pop; IR:\n{ir}"));
    let dec_pos = ir
        .find_execution("__molt_inline_release")
        .unwrap_or_else(|| panic!("LLVM must call molt_dec_ref_obj; IR:\n{ir}"));
    assert!(
        pop_pos < dec_pos,
        "shared ExceptionRegion drops must lower after the owning exception_pop; IR:\n{ir}"
    );
}

#[test]
fn missing_value_id_is_fatal_lowering_error() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new(
        "missing_value".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Return {
        values: vec![ValueId(99)],
    };

    let err = match try_lower_tir_to_llvm(&func, &backend) {
        Ok(_) => panic!("malformed TIR unexpectedly lowered successfully"),
        Err(err) => err,
    };
    assert_lowering_error_contains(&err, "ValueId %99 was used before being defined");
}

#[test]
fn missing_phi_argument_is_fatal_lowering_error() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new(
        "missing_phi_arg".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let join_id = func.fresh_block();
    let join_arg = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: join_id,
        args: vec![],
    };
    func.blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );

    let err = match try_lower_tir_to_llvm(&func, &backend) {
        Ok(_) => panic!("malformed phi unexpectedly lowered successfully"),
        Err(err) => err,
    };
    assert_lowering_error_contains(&err, "phi argument index 0 is required");
}

#[test]
fn unreachable_predecessor_does_not_feed_phi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new(
        "dead_phi_pred".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let join_id = func.fresh_block();
    let dead_id = func.fresh_block();
    let live_value = func.fresh_value();
    let join_arg = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(live_value));
    entry.terminator = Terminator::Branch {
        target: join_id,
        args: vec![live_value],
    };
    func.blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );
    func.blocks.insert(
        dead_id,
        TirBlock {
            id: dead_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(999)],
            },
        },
    );

    try_lower_tir_to_llvm(&func, &backend)
        .expect("dead TIR predecessor must not contribute to LLVM phi incoming values");
    backend
        .module
        .verify()
        .expect("dead predecessor phi lowering should verify");
}

#[test]
fn check_exception_edge_feeds_handler_phi() {
    for poll in [false, true] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);

        let mut func = TirFunction::new(
            "check_exception_phi".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let exit_id = func.fresh_block();
        let handler_id = func.fresh_block();
        let live_value = func.fresh_value();
        let exit_value = func.fresh_value();
        let handler_arg = func.fresh_value();

        let mut handler_attrs = AttrDict::default();
        handler_attrs.insert("value".into(), AttrValue::Int(100));
        if poll {
            handler_attrs.insert("async_work_poll".into(), AttrValue::Bool(true));
        }

        let module_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(module_id, 1));
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::ModuleImportClosed,
            operands: vec![module_id],
            results: vec![live_value],
            attrs: AttrDict::default(),
            source_span: None,
        });
        let mut check = TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![live_value],
            results: vec![],
            attrs: handler_attrs,
            source_span: None,
        };
        check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
        entry.ops.push(check);
        entry.terminator = Terminator::Branch {
            target: exit_id,
            args: vec![],
        };
        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![const_none_def(exit_value)],
                terminator: Terminator::Return {
                    values: vec![exit_value],
                },
            },
        );
        func.blocks.insert(
            handler_id,
            TirBlock {
                id: handler_id,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![handler_arg],
                },
            },
        );
        func.has_exception_handling = true;
        func.label_id_map.insert(handler_id.0, 100);

        let llvm_fn = try_lower_tir_to_llvm(&func, &backend)
            .expect("check_exception operands must feed handler block phi args");
        backend
            .module
            .verify()
            .expect("check_exception handler phi lowering should verify");
        let ir = llvm_fn.print_to_string().to_string();
        let symbol = if poll {
            "molt_async_work_poll_and_exception_pending"
        } else {
            "molt_exception_pending"
        };
        assert!(
            ir.contains_execution(&format!("call i64 @{symbol}()")),
            "{ir}"
        );
    }
}

#[test]
fn check_exception_requires_explicit_target_inside_and_outside_try() {
    for inside_try in [false, true] {
        for poll in [false, true] {
            let ctx = Context::create();
            let backend = make_backend(&ctx);
            let mut function = TirFunction::new(
                "missing_exception_target".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let handler = function.fresh_block();
            function.label_id_map.insert(handler.0, 17);
            function.has_exception_handling = inside_try;
            let entry = function.blocks.get_mut(&function.entry_block).unwrap();
            if inside_try {
                entry.ops.push(TirOp {
                    origin: None,
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::TryStart,
                    operands: vec![],
                    results: vec![],
                    attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(17))]),
                    source_span: None,
                });
            }
            let mut check = TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::default(),
                source_span: None,
            };
            check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
            if poll {
                check.mark_async_work_poll();
            }
            entry.ops.push(check);
            entry.terminator = Terminator::Return { values: vec![] };
            function.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            let error = try_lower_tir_to_llvm(&function, &backend)
                .expect_err("a surrounding handler cannot supply an omitted exceptional edge");
            assert!(
                error.to_string().contains_execution(
                    "check_exception requires an explicit exceptional successor label"
                ),
                "inside_try={inside_try}, poll={poll}: {error}"
            );
        }
    }
}
