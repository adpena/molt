use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use molt_ir::OpKind;
use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;

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
                AttrDict::from([(
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
        (normal, vec![marker(OpKind::TraceExit, AttrDict::new())]),
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
    // The stack-exit restore is an exact boxed-ABI kind on the admitted route.
    std::sync::Arc::make_mut(&mut backend.runtime_callable_symbols)
        .insert("molt_exception_stack_exit".into());
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
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::from([("value".into(), AttrValue::Int(100))]),
        source_span: None,
    };
    let runtime = |kind: OpKind, operands, results| TirOp {
        origin: Some(kind),
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results,
        attrs: AttrDict::from([]),
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
    use molt_tir::trampolines::{TaskCompletion, TaskConstructorLayout};

    for (call_async, task_kind) in [
        (false, None),
        (false, Some("future")),
        (false, Some("generator")),
        (false, Some("coroutine")),
        (true, None),
    ] {
        for payload_count in [0, 2] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            backend.function_linkage_abis.insert(
                "opaque_task_body".into(),
                test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
            );
            let layout = if call_async {
                TaskConstructorLayout::for_call_async()
            } else {
                TaskConstructorLayout::for_alloc_kind(task_kind)
            };
            let payload_base = layout.payload_base_offset(crate::GENERATOR_CONTROL_BYTES);
            let mut func = TirFunction::new(
                "task_failure_cleanup".into(),
                vec![TirType::DynBox, TirType::DynBox],
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
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::from([(
                    "s_value".into(),
                    AttrValue::Str("task_failure_cleanup".into()),
                )]),
                source_span: None,
            });
            let mut attrs = AttrDict::from([
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
                dialect: Dialect::Molt,
                opcode: if call_async {
                    OpCode::Copy
                } else {
                    OpCode::AllocTask
                },
                operands: args[..payload_count].to_vec(),
                results: vec![result],
                attrs,
                source_span: None,
            });
            entry.ops.push(TirOp {
                origin: None,
                runtime: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ExceptionPending,
                operands: vec![],
                results: vec![pending],
                attrs: AttrDict::new(),
                source_span: None,
            });
            for arg in args {
                entry.ops.push(TirOp {
                    origin: None,
                    runtime: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::DecRef,
                    operands: vec![arg],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                });
            }
            entry.ops.push(TirOp {
                origin: Some(OpKind::TraceExit),
                runtime: None,
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::from([]),
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
            let init = blocks
                .iter()
                .find(|bb| bb.get_name().to_str().unwrap().starts_with("task_init"))
                .expect("success-only task initializer");
            let ready = blocks
                .iter()
                .find(|bb| bb.get_name().to_str().unwrap().starts_with("task_ready"))
                .expect("ordinary continuation shared by success and failure");
            let allocation = blocks
                .iter()
                .find(|bb| block_ir(bb).contains_execution("@molt_task_new("))
                .expect("runtime allocator block");
            let allocation_ir = block_ir(allocation);
            let init_ir = block_ir(init);
            let ready_ir = block_ir(ready);
            let call_name = if call_async {
                "call_async_task_new"
            } else {
                "task_new"
            };
            assert!(
                allocation_ir.contains_execution(&format!(
                    "icmp ne i64 %{call_name}, {}",
                    nanbox::QNAN | nanbox::TAG_NONE
                )),
                "{ir}"
            );
            assert!(
                allocation_ir.contains_execution(&format!(
                    "br i1 %task_allocated, label %{}, label %{}",
                    init.get_name().to_str().unwrap(),
                    ready.get_name().to_str().unwrap()
                )),
                "allocation failure must directly join cleanup: {ir}"
            );
            assert!(
                !allocation_ir.contains_execution("inttoptr")
                    && !allocation_ir.contains_execution("ret "),
                "{ir}"
            );
            assert!(init_ir.contains_execution("inttoptr"), "{ir}");
            assert_eq!(
                init_ir.matches_execution("store i64").count(),
                payload_count,
                "{ir}"
            );
            assert_eq!(
                init_ir.matches_execution("@__molt_inline_retain(").count(),
                payload_count,
                "{ir}"
            );
            if payload_count != 0 {
                assert!(
                    init_ir.contains_execution(&format!(
                        "getelementptr i64, ptr %task_obj_ptr, i64 {}",
                        payload_base / 8
                    )),
                    "{ir}"
                );
            }
            let registers_token = layout.completion() == TaskCompletion::RegisterCancelToken;
            assert_eq!(
                init_ir.contains_execution("@molt_cancel_token_get_current("),
                registers_token,
                "{ir}"
            );
            assert_eq!(
                init_ir.contains_execution("@molt_task_register_execution("),
                registers_token,
                "{ir}"
            );
            assert!(!init_ir.contains_execution("ret "), "{ir}");
            assert!(
                ready_ir.contains_execution("@molt_exception_pending("),
                "{ir}"
            );
            assert_eq!(
                ready_ir
                    .matches_execution("@__molt_inline_release(")
                    .count(),
                2,
                "{ir}"
            );
            assert!(
                allocation_ir.contains_execution("store i64 %frame_record_enter"),
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
            assert!(
                ready_ir.contains_execution(&format!("ret i64 %{call_name}")),
                "boxed result must survive unchanged: {ir}"
            );
            assert!(
                !ready_ir.contains_execution("inttoptr")
                    && ready_ir.matches_execution("store i64").count() == 1,
                "{ir}"
            );
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
        ("molt_async_sleep_poll", Some(0xFFFF_FF00_0000_0101_u64)),
        ("molt_promise_poll", Some(0xFFFF_FF00_0000_0104_u64)),
        ("ordinary_poll", None),
    ] {
        for call_async in [false, true] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            if expected_key.is_none() {
                backend.function_linkage_abis.insert(
                    symbol.into(),
                    test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
                );
            }
            let mut function = TirFunction::new(
                "task_identity_probe".into(),
                vec![],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let result = function.fresh_value();
            let entry = function.blocks.get_mut(&function.entry_block).unwrap();
            let attrs = AttrDict::from([
                ("s_value".into(), AttrValue::Str(symbol.into())),
                ("value".into(), AttrValue::Int(16)),
                ("task_kind".into(), AttrValue::Str("future".into())),
            ]);
            let origin = call_async.then_some(OpKind::CallAsync);
            entry.ops.push(TirOp {
                origin,
                runtime: None,
                dialect: Dialect::Molt,
                opcode: if call_async {
                    OpCode::Copy
                } else {
                    OpCode::AllocTask
                },
                operands: vec![],
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
            let mut allocations = Vec::new();
            for block in lowered.get_basic_blocks() {
                let mut current = block.get_first_instruction();
                while let Some(inst) = current {
                    current = inst.get_next_instruction();
                    if let Ok(call) = inkwell::values::CallSiteValue::try_from(inst)
                        && call.get_called_fn_value().is_some_and(|callee| {
                            callee.get_name().to_bytes()
                                == crate::llvm_backend::execution_abi::execution_symbol(
                                    "molt_task_new",
                                )
                                .as_bytes()
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
            if expected_key.is_none() {
                let compiled = backend
                    .module
                    .get_execution_function(symbol)
                    .unwrap()
                    .as_global_value()
                    .as_pointer_value()
                    .const_to_int(ctx.i64_type());
                assert_eq!(
                    poll, compiled,
                    "compiled poll must retain its exact function address"
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
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs: {
            let mut m = AttrDict::new();
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
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    exception_pop.origin = Some(OpKind::ExceptionPop);
    entry.ops.push(exception_pop);
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
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

        let mut handler_attrs = AttrDict::new();
        handler_attrs.insert("value".into(), AttrValue::Int(100));
        if poll {
            handler_attrs.insert("async_work_poll".into(), AttrValue::Bool(true));
        }

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Import,
            operands: vec![],
            results: vec![live_value],
            attrs: AttrDict::from([("module".into(), AttrValue::Str("checked_name".into()))]),
            source_span: None,
        });
        let mut check = TirOp {
            origin: None,
            runtime: None,
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
                    dialect: Dialect::Molt,
                    opcode: OpCode::TryStart,
                    operands: vec![],
                    results: vec![],
                    attrs: AttrDict::from([("value".into(), AttrValue::Int(17))]),
                    source_span: None,
                });
            }
            let mut check = TirOp {
                origin: None,
                runtime: None,
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
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
