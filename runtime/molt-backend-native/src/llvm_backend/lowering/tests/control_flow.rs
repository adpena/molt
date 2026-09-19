use super::*;

#[test]
fn path_local_try_markers_do_not_duplicate_explicit_exception_stack_state() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("path_local_try".into(), vec![TirType::Bool], TirType::None);
    let left = func.fresh_block();
    let right = func.fresh_block();
    let previous_baseline = func.fresh_value();
    let marker = |opcode| TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::from([("value".into(), AttrValue::Int(100))]),
        source_span: None,
    };
    let runtime = |kind: &str, operands, results| TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands,
        results,
        attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
        source_span: None,
    };
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.extend([
        runtime("exception_stack_enter", vec![], vec![previous_baseline]),
        runtime("exception_push", vec![], vec![]),
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
                    runtime("exception_pop", vec![], vec![]),
                    runtime("exception_stack_exit", vec![previous_baseline], vec![]),
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
        ir.matches("@molt_exception_stack_enter(").count(),
        1,
        "{ir}"
    );
    assert_eq!(ir.matches("@molt_exception_push(").count(), 1, "{ir}");
    assert_eq!(ir.matches("@molt_exception_pop(").count(), 2, "{ir}");
    assert_eq!(ir.matches("@molt_exception_stack_exit(").count(), 2, "{ir}");
    assert!(
        !ir.contains("try_baseline"),
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
            );
            let result = func.fresh_value();
            let pending = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let args: Vec<_> = entry.args.iter().map(|arg| arg.id).collect();
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
            if call_async {
                attrs.insert("_original_kind".into(), AttrValue::Str("call_async".into()));
            }
            entry.ops.push(TirOp {
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
                dialect: Dialect::Molt,
                opcode: OpCode::ExceptionPending,
                operands: vec![],
                results: vec![pending],
                attrs: AttrDict::new(),
                source_span: None,
            });
            for arg in args {
                entry.ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::DecRef,
                    operands: vec![arg],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                });
            }
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::from([(
                    "_original_kind".into(),
                    AttrValue::Str("trace_exit".into()),
                )]),
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
                .find(|bb| block_ir(bb).contains("@molt_task_new("))
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
                allocation_ir.contains(&format!(
                    "icmp ne i64 %{call_name}, {}",
                    nanbox::QNAN | nanbox::TAG_NONE
                )),
                "{ir}"
            );
            assert!(
                allocation_ir.contains(&format!(
                    "br i1 %task_allocated, label %{}, label %{}",
                    init.get_name().to_str().unwrap(),
                    ready.get_name().to_str().unwrap()
                )),
                "allocation failure must directly join cleanup: {ir}"
            );
            assert!(
                !allocation_ir.contains("inttoptr") && !allocation_ir.contains("ret "),
                "{ir}"
            );
            assert!(init_ir.contains("inttoptr"), "{ir}");
            assert_eq!(init_ir.matches("store i64").count(), payload_count, "{ir}");
            assert_eq!(
                init_ir.matches("@molt_inc_ref_obj(").count(),
                payload_count,
                "{ir}"
            );
            if payload_count != 0 {
                assert!(
                    init_ir.contains(&format!(
                        "getelementptr i64, ptr %task_obj_ptr, i64 {}",
                        payload_base / 8
                    )),
                    "{ir}"
                );
            }
            let registers_token = layout.completion() == TaskCompletion::RegisterCancelToken;
            assert_eq!(
                init_ir.contains("@molt_cancel_token_get_current("),
                registers_token,
                "{ir}"
            );
            assert_eq!(
                init_ir.contains("@molt_task_register_token_owned("),
                registers_token,
                "{ir}"
            );
            assert!(!init_ir.contains("ret "), "{ir}");
            assert!(ready_ir.contains("@molt_exception_pending("), "{ir}");
            assert_eq!(ready_ir.matches("@molt_dec_ref_obj(").count(), 2, "{ir}");
            assert!(ready_ir.contains("@molt_trace_exit("), "{ir}");
            assert!(
                ready_ir.contains(&format!("ret i64 %{call_name}")),
                "boxed result must survive unchanged: {ir}"
            );
            assert!(
                !ready_ir.contains("inttoptr") && !ready_ir.contains("store i64"),
                "{ir}"
            );
            assert!(
                !ready_ir.contains("@molt_inc_ref_obj(")
                    && !ready_ir.contains("@molt_task_register_token_owned("),
                "{ir}"
            );
            assert!(
                !ir.contains("@molt_exception_clear("),
                "allocation failure must preserve pending exception: {ir}"
            );
        }
    }
}

#[test]
fn lower_const_and_return() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn f() -> i64 { return 42 }
    let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
    let v0 = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
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

    assert!(ir.contains("const_ret"), "function name missing from IR");
    assert!(ir.contains("42"), "constant 42 missing from IR");
    assert!(ir.contains("ret "), "return instruction missing from IR");
}

#[test]
fn lowers_exception_pop_then_dec_ref_from_shared_drop_shape() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("exception_drop".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    let mut exception_pop = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    exception_pop.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("exception_pop".into()),
    );
    entry.ops.push(exception_pop);
    entry.ops.push(TirOp {
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
        .find("molt_exception_pop")
        .unwrap_or_else(|| panic!("LLVM must call molt_exception_pop; IR:\n{ir}"));
    let dec_pos = ir
        .find("molt_dec_ref_obj")
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

    let mut func = TirFunction::new("missing_value".into(), vec![], TirType::I64);
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

    let mut func = TirFunction::new("missing_phi_arg".into(), vec![], TirType::I64);
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

    let mut func = TirFunction::new("dead_phi_pred".into(), vec![], TirType::DynBox);
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

        let mut func = TirFunction::new("check_exception_phi".into(), vec![], TirType::DynBox);
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
            dialect: Dialect::Molt,
            opcode: OpCode::Import,
            operands: vec![],
            results: vec![live_value],
            attrs: AttrDict::from([("module".into(), AttrValue::Str("checked_name".into()))]),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![live_value],
            results: vec![],
            attrs: handler_attrs,
            source_span: None,
        });
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
        assert!(ir.contains(&format!("call i64 @{symbol}()")), "{ir}");
    }
}
