use super::*;

#[test]
fn zero_insertion_borrowed_param_function_still_marks_drop_inserted() {
    let mut func = TirFunction::new(
        "borrowed_param_no_owned_temps".into(),
        vec![TirType::DynBox],
        TirType::None,
    );
    let param = func.blocks[&func.entry_block].args[0].id;
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![op(OpCode::Call, vec![param], vec![])];
        entry.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(
        stats.ops_added, 0,
        "borrowed-param-only functions need no physical drops"
    );
    assert_eq!(count_decrefs(&func), 0);
    assert_eq!(count_increfs(&func), 0);
    assert!(
        matches!(
            func.attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ),
        "zero-insertion full analysis must still disable native legacy RC cleanup"
    );
}

#[test]
fn exception_region_match_release_inserts_before_handler_full_drop() {
    let mut func = TirFunction::new("split_exception_cleanup".into(), vec![], TirType::None);
    let clean = func.fresh_block();
    let handler = func.fresh_block();
    let handler_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 4);
    let exc = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: clean,
        args: vec![],
    };
    func.blocks.insert(
        clean,
        TirBlock {
            id: clean,
            args: vec![],
            ops: vec![original_copy("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original_copy("exception_last_pending", vec![exc])],
            terminator: Terminator::Branch {
                target: handler_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_pop,
        TirBlock {
            id: handler_pop,
            args: vec![],
            ops: vec![original_copy("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 1);
    assert!(matches!(
        func.attrs.get(EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    ));
    assert!(
        matches!(
            func.attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ),
        "handler functions now run on full shared DropInsertion ownership"
    );
    assert_eq!(
        func.blocks[&clean]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .count(),
        0,
        "the sibling normal cleanup pop must not own the handler match ref"
    );
    let handler_ops = &func.blocks[&handler_pop].ops;
    assert_eq!(handler_ops[0].opcode, OpCode::Copy);
    assert_eq!(handler_ops[1].opcode, OpCode::DecRef);
    assert_eq!(handler_ops[1].operands, vec![exc]);

    let stats = run(&mut func, &mut am);
    assert_eq!(stats.ops_added, 0);
    assert_eq!(
        func.blocks[&handler_pop]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .count(),
        1,
        "full drop_inserted marker must make the handler ownership slice idempotent"
    );
}

#[test]
fn exception_creation_ref_releases_at_raise_with_handler_full_drop() {
    let mut func = TirFunction::new("raise_creation_cleanup".into(), vec![], TirType::None);
    let handler = func.fresh_block();
    let exc = func.fresh_value();
    func.label_id_map.insert(handler.0, 4);

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![
        try_start(4),
        original_copy("exception_new_builtin_one", vec![exc]),
        op(OpCode::Raise, vec![exc], vec![]),
    ];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original_copy("exception_pop", vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 1);
    assert!(matches!(
        func.attrs.get(EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    ));
    assert!(
        matches!(
            func.attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ),
        "raise-path CreationRef release composes with full handler DropInsertion"
    );
    let entry_ops = &func.blocks[&func.entry_block].ops;
    assert_eq!(entry_ops[2].opcode, OpCode::Raise);
    assert_eq!(entry_ops[3].opcode, OpCode::DecRef);
    assert_eq!(entry_ops[3].operands, vec![exc]);
}

#[test]
fn exception_edge_borrowed_payload_retains_for_owned_handler_arg() {
    let mut func = TirFunction::new(
        "exception_edge_borrowed_payload".into(),
        vec![TirType::DynBox],
        TirType::None,
    );
    let handler = func.fresh_block();
    let handler_arg = func.fresh_value();
    func.value_types.insert(handler_arg, TirType::DynBox);
    func.label_id_map.insert(handler.0, 4);

    let param = func.blocks[&func.entry_block].args[0].id;
    let mut check = op(OpCode::CheckException, vec![param], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(4));
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![try_start(4), check];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::Call, vec![handler_arg], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(
        stats.ops_added, 3,
        "borrowed payload retain+normal release plus handler arg release"
    );
    let entry_ops = &func.blocks[&func.entry_block].ops;
    let check_idx = entry_ops
        .iter()
        .position(|op| op.opcode == OpCode::CheckException)
        .expect("check_exception survives");
    assert_eq!(entry_ops[check_idx - 1].opcode, OpCode::IncRef);
    assert_eq!(entry_ops[check_idx - 1].operands, vec![param]);
    assert_eq!(entry_ops[check_idx + 1].opcode, OpCode::DecRef);
    assert_eq!(entry_ops[check_idx + 1].operands, vec![param]);

    let handler_ops = &func.blocks[&handler].ops;
    assert_eq!(handler_ops[0].opcode, OpCode::Call);
    assert_eq!(handler_ops[1].opcode, OpCode::DecRef);
    assert_eq!(handler_ops[1].operands, vec![handler_arg]);
}

#[test]
fn try_start_edge_borrowed_payload_retains_for_owned_handler_arg() {
    let mut func = TirFunction::new(
        "try_start_edge_borrowed_payload".into(),
        vec![TirType::DynBox],
        TirType::None,
    );
    let handler = func.fresh_block();
    let handler_arg = func.fresh_value();
    func.value_types.insert(handler_arg, TirType::DynBox);
    func.label_id_map.insert(handler.0, 4);

    let param = func.blocks[&func.entry_block].args[0].id;
    let mut start = op(OpCode::TryStart, vec![param], vec![]);
    start.attrs.insert("value".into(), AttrValue::Int(4));
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![start];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::Call, vec![handler_arg], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(
        stats.ops_added, 3,
        "try_start borrowed payload retain+normal release plus handler arg release"
    );
    let entry_ops = &func.blocks[&func.entry_block].ops;
    let start_idx = entry_ops
        .iter()
        .position(|op| op.opcode == OpCode::TryStart)
        .expect("try_start survives");
    assert_eq!(entry_ops[start_idx - 1].opcode, OpCode::IncRef);
    assert_eq!(entry_ops[start_idx - 1].operands, vec![param]);
    assert_eq!(entry_ops[start_idx + 1].opcode, OpCode::DecRef);
    assert_eq!(entry_ops[start_idx + 1].operands, vec![param]);

    let handler_ops = &func.blocks[&handler].ops;
    assert_eq!(handler_ops[0].opcode, OpCode::Call);
    assert_eq!(handler_ops[1].opcode, OpCode::DecRef);
    assert_eq!(handler_ops[1].operands, vec![handler_arg]);
}

#[test]
fn exception_creation_ref_release_is_path_local_for_alternative_raises() {
    let mut func = TirFunction::new("raise_creation_diamond".into(), vec![], TirType::None);
    let then_raise = func.fresh_block();
    let else_raise = func.fresh_block();
    let cond = func.fresh_value();
    let exc = func.fresh_value();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            op(OpCode::ConstBool, vec![], vec![cond]),
            original_copy("exception_new_builtin_one", vec![exc]),
        ];
        entry.terminator = Terminator::CondBranch {
            cond,
            then_block: then_raise,
            then_args: vec![],
            else_block: else_raise,
            else_args: vec![],
        };
    }
    for block in [then_raise, else_raise] {
        func.blocks.insert(
            block,
            TirBlock {
                id: block,
                args: vec![],
                ops: vec![op(OpCode::Raise, vec![exc], vec![])],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 2);
    for block in [then_raise, else_raise] {
        let ops = &func.blocks[&block].ops;
        assert_eq!(ops[0].opcode, OpCode::Raise);
        assert_eq!(ops[1].opcode, OpCode::DecRef);
        assert_eq!(ops[1].operands, vec![exc]);
    }
    assert_eq!(
        count_decrefs(&func),
        2,
        "CreationRef release is path-local: each mutually exclusive raise edge must release the shared SSA ref on the path that actually raises"
    );
}

#[test]
fn handler_match_ref_is_not_released_at_reraise() {
    let mut func = TirFunction::new("reraise_match_ref_cleanup".into(), vec![], TirType::None);
    let handler = func.fresh_block();
    let exc = func.fresh_value();
    func.label_id_map.insert(handler.0, 4);

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![
                original_copy("exception_last_pending", vec![exc]),
                op(OpCode::Raise, vec![exc], vec![]),
                original_copy("exception_pop", vec![]),
            ],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 1);
    let handler_ops = &func.blocks[&handler].ops;
    assert_eq!(handler_ops[1].opcode, OpCode::Raise);
    assert_eq!(
        handler_ops[2].opcode,
        OpCode::Copy,
        "the reraise itself must not consume the handler MatchRef"
    );
    assert_eq!(handler_ops[3].opcode, OpCode::DecRef);
    assert_eq!(handler_ops[3].operands, vec![exc]);
}

#[test]
fn exception_region_match_release_splits_shared_pop_by_dominating_edge() {
    let mut func = TirFunction::new("shared_exception_pop".into(), vec![], TirType::None);
    let normal = func.fresh_block();
    let shared_pop = func.fresh_block();
    let after_pop = func.fresh_block();
    let handler = func.fresh_block();
    let handler_body = func.fresh_block();
    func.label_id_map.insert(handler.0, 4);
    let exc = func.fresh_value();
    let matched = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: normal,
        args: vec![],
    };
    func.blocks.insert(
        normal,
        TirBlock {
            id: normal,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: shared_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        shared_pop,
        TirBlock {
            id: shared_pop,
            args: vec![],
            ops: vec![original_copy("exception_pop", vec![])],
            terminator: Terminator::Branch {
                target: after_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        after_pop,
        TirBlock {
            id: after_pop,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![
                original_copy("exception_last_pending", vec![exc]),
                op(OpCode::Copy, vec![exc], vec![matched]),
            ],
            terminator: Terminator::Branch {
                target: handler_body,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_body,
        TirBlock {
            id: handler_body,
            args: vec![],
            ops: vec![op(OpCode::Copy, vec![matched], vec![])],
            terminator: Terminator::Branch {
                target: shared_pop,
                args: vec![],
            },
        },
    );

    let mut am = AnalysisManager::new();
    let before_blocks = func.blocks.len();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 1);
    assert!(
        func.blocks.len() >= before_blocks + 2,
        "shared pop release must split out a post-pop continuation and a handler pop clone"
    );
    assert_eq!(
        func.blocks[&shared_pop]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .count(),
        0,
        "normal path must not see a handler-only MatchRef DecRef"
    );
    let handler_successor = match &func.blocks[&handler_body].terminator {
        Terminator::Branch { target, .. } => *target,
        other => panic!("handler edge should remain unconditional, got {other:?}"),
    };
    assert_ne!(
        handler_successor, shared_pop,
        "handler edge should be retargeted to a split pop block"
    );
    let split_ops = &func.blocks[&handler_successor].ops;
    assert_eq!(split_ops[0].opcode, OpCode::Copy);
    assert_eq!(split_ops[1].opcode, OpCode::DecRef);
    assert_eq!(split_ops[1].operands, vec![exc]);
    crate::tir::verify::verify_function(&func)
        .expect("path-specific exception MatchRef release must preserve SSA dominance");
}

#[test]
fn exception_region_match_release_splits_shared_pop_with_block_args() {
    let mut func = TirFunction::new(
        "shared_exception_pop_with_arg".into(),
        vec![],
        TirType::None,
    );
    let normal = func.fresh_block();
    let handler = func.fresh_block();
    let handler_body = func.fresh_block();
    let shared_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 4);
    let exc = func.fresh_value();
    let normal_arg = func.fresh_value();
    let handler_arg = func.fresh_value();
    let pop_arg = func.fresh_value();
    let tail_value = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: normal,
        args: vec![],
    };
    func.blocks.insert(
        normal,
        TirBlock {
            id: normal,
            args: vec![],
            ops: vec![const_str(normal_arg)],
            terminator: Terminator::Branch {
                target: shared_pop,
                args: vec![normal_arg],
            },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![
                original_copy("exception_last_pending", vec![exc]),
                const_str(handler_arg),
            ],
            terminator: Terminator::Branch {
                target: handler_body,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_body,
        TirBlock {
            id: handler_body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: shared_pop,
                args: vec![handler_arg],
            },
        },
    );
    func.blocks.insert(
        shared_pop,
        TirBlock {
            id: shared_pop,
            args: vec![TirValue {
                id: pop_arg,
                ty: TirType::Str,
            }],
            ops: vec![
                original_copy("exception_pop", vec![]),
                op(OpCode::Copy, vec![pop_arg], vec![tail_value]),
            ],
            terminator: Terminator::Return {
                values: vec![tail_value],
            },
        },
    );

    let mut am = AnalysisManager::new();
    let before_blocks = func.blocks.len();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 1);
    assert_eq!(
        func.blocks.len(),
        before_blocks + 2,
        "block-arg shared pop needs exactly one continuation and one handler split"
    );
    assert_eq!(
        func.blocks[&shared_pop]
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef)
            .count(),
        0,
        "normal path must not see a handler-only MatchRef DecRef"
    );

    let continuation = match &func.blocks[&shared_pop].terminator {
        Terminator::Branch { target, args } => {
            assert_eq!(
                args,
                &vec![pop_arg],
                "the original pop block must forward its incoming phi payload"
            );
            *target
        }
        other => panic!("shared pop must branch to a continuation, got {other:?}"),
    };
    let continuation_block = &func.blocks[&continuation];
    assert_eq!(continuation_block.args.len(), 1);
    let continuation_arg = continuation_block.args[0].id;
    assert_ne!(
        continuation_arg, pop_arg,
        "the moved tail must own a fresh block arg instead of reusing the pre-split phi"
    );
    assert_eq!(continuation_block.ops[0].opcode, OpCode::Copy);
    assert_eq!(continuation_block.ops[0].operands, vec![continuation_arg]);

    let handler_successor = match &func.blocks[&handler_body].terminator {
        Terminator::Branch { target, .. } => *target,
        other => panic!("handler edge should remain unconditional, got {other:?}"),
    };
    assert_ne!(
        handler_successor, shared_pop,
        "handler edge should be retargeted to a path-specific pop clone"
    );
    let split = &func.blocks[&handler_successor];
    assert_eq!(split.ops[0].opcode, OpCode::Copy);
    assert_eq!(split.ops[1].opcode, OpCode::DecRef);
    assert_eq!(split.ops[1].operands, vec![exc]);
    match &split.terminator {
        Terminator::Branch { target, args } => {
            assert_eq!(*target, continuation);
            assert_eq!(
                args,
                &vec![handler_arg],
                "the handler split must forward the original handler edge payload"
            );
        }
        other => panic!("handler split must branch to continuation, got {other:?}"),
    }

    crate::tir::verify::verify_function(&func)
        .expect("block-arg path-specific exception release must preserve SSA dominance");
}

#[test]
fn exception_region_match_release_remaps_dominated_successor_uses() {
    let mut func = TirFunction::new(
        "shared_exception_pop_successor_uses_arg".into(),
        vec![],
        TirType::None,
    );
    let normal = func.fresh_block();
    let handler = func.fresh_block();
    let handler_body = func.fresh_block();
    let shared_pop = func.fresh_block();
    let after_pop = func.fresh_block();
    func.label_id_map.insert(handler.0, 4);

    let exc = func.fresh_value();
    let normal_arg = func.fresh_value();
    let handler_arg = func.fresh_value();
    let pop_arg = func.fresh_value();
    let pop_alias = func.fresh_value();
    let tail_value = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().ops = vec![try_start(4)];
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: normal,
        args: vec![],
    };
    func.blocks.insert(
        normal,
        TirBlock {
            id: normal,
            args: vec![],
            ops: vec![const_str(normal_arg)],
            terminator: Terminator::Branch {
                target: shared_pop,
                args: vec![normal_arg],
            },
        },
    );
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![
                original_copy("exception_last_pending", vec![exc]),
                const_str(handler_arg),
            ],
            terminator: Terminator::Branch {
                target: handler_body,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        handler_body,
        TirBlock {
            id: handler_body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: shared_pop,
                args: vec![handler_arg],
            },
        },
    );
    func.blocks.insert(
        shared_pop,
        TirBlock {
            id: shared_pop,
            args: vec![TirValue {
                id: pop_arg,
                ty: TirType::Str,
            }],
            ops: vec![
                original_copy_with_operands("load_var", vec![pop_arg], vec![pop_alias]),
                original_copy("exception_pop", vec![]),
            ],
            terminator: Terminator::Branch {
                target: after_pop,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        after_pop,
        TirBlock {
            id: after_pop,
            args: vec![],
            ops: vec![op(OpCode::Copy, vec![pop_alias], vec![tail_value])],
            terminator: Terminator::Return {
                values: vec![tail_value],
            },
        },
    );

    let mut am = AnalysisManager::new();
    let before_blocks = func.blocks.len();
    let stats = run(&mut func, &mut am);

    assert_eq!(stats.ops_added, 1);
    assert_eq!(
        func.blocks.len(),
        before_blocks + 2,
        "shared pop needs a continuation plus the handler-specific release split"
    );

    let continuation = match &func.blocks[&shared_pop].terminator {
        Terminator::Branch { target, args } => {
            assert_eq!(args, &vec![pop_arg]);
            *target
        }
        other => panic!("shared pop must branch to a continuation, got {other:?}"),
    };
    let continuation_arg = func.blocks[&continuation].args[0].id;
    assert_ne!(continuation_arg, pop_arg);
    assert_eq!(
        func.blocks[&after_pop].ops[0].operands,
        vec![continuation_arg],
        "dominated successor must read the post-split continuation arg, not the stale pre-split phi"
    );

    crate::tir::verify::verify_function(&func)
        .expect("post-pop split must preserve SSA dominance through dominated successors");
}
