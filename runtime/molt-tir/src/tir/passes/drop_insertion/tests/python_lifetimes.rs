use super::*;

#[test]
fn finalizer_sensitive_container_releases_at_return_boundary() {
    let mut func = TirFunction::new("finalizer_scope".into(), vec![], TirType::None);
    let item = func.fresh_value();
    let list = func.fresh_value();
    for v in [item, list] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(op(OpCode::BuildList, vec![item], vec![list]));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::BuildList)
        .expect("BuildList op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(list_idx + 1, item), (marker_idx + 1, list)],
        "absorbed producer temp releases at list construction; container owner releases at return"
    );
}

#[test]
fn result_carrying_store_var_keeps_container_owner_to_return_boundary() {
    let mut func = TirFunction::new("finalizer_scope_store_result".into(), vec![], TirType::None);
    let item = func.fresh_value();
    let list = func.fresh_value();
    let stored = func.fresh_value();
    for v in [item, list, stored] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![list],
            vec![stored],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let store_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
        .expect("store_var marker must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert!(
        !dropped
            .iter()
            .any(|(idx, value)| *idx == store_idx + 1 && *value == list),
        "store_var is a no-incref local lifetime marker; it must not release the source owner"
    );
    assert_eq!(
        dropped,
        vec![(list_idx + 1, item), (marker_idx + 1, list)],
        "result-carrying store_var aliases the source owner and defers finalizer-sensitive locals to return"
    );
}

#[test]
fn store_var_boundary_transferred_to_cleanup_block_arg_releases_once() {
    let mut func = TirFunction::new(
        "store_var_cleanup_join_transfers_owner".into(),
        vec![],
        TirType::None,
    );
    let class_obj = func.fresh_value();
    let stored = func.fresh_value();
    let cleanup_arg = func.fresh_value();
    for v in [class_obj, stored, cleanup_arg] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    let cleanup = func.fresh_block();
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_call_bind(class_obj));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![class_obj],
            vec![stored],
        ));
        b.terminator = Terminator::Branch {
            target: cleanup,
            args: vec![stored],
        };
    }
    func.blocks.insert(
        cleanup,
        TirBlock {
            id: cleanup,
            args: vec![TirValue {
                id: cleanup_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::DelBoundary, vec![cleanup_arg], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let entry_drops: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        !entry_drops.contains(&class_obj),
        "store_var transfers the single owner to the cleanup block arg; the source root must not be dropped on the predecessor"
    );

    let cleanup_drops: Vec<ValueId> = func.blocks[&cleanup]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        cleanup_drops,
        vec![cleanup_arg],
        "cleanup block arg is the release authority; original store_var root must not receive a second return-boundary DecRef"
    );
}

#[test]
fn store_var_transfer_phi_live_in_descendant_blocks_old_root_drop() {
    let mut func = TirFunction::new(
        "store_var_transfer_phi_live_in_descendant".into(),
        vec![],
        TirType::None,
    );
    let join = func.fresh_block();
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let ret = func.fresh_block();
    let list = func.fresh_value();
    let stored = func.fresh_value();
    let phi = func.fresh_value();
    let cond = func.fresh_value();
    for v in [list, stored, phi] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);

    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops
            .push(original_copy_with_operands("list_new", vec![], vec![list]));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![list],
            vec![stored],
        ));
        b.terminator = Terminator::Branch {
            target: join,
            args: vec![stored],
        };
    }
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: phi,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block,
                then_args: vec![],
                else_block,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        then_block,
        TirBlock {
            id: then_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        else_block,
        TirBlock {
            id: else_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: ret,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        ret,
        TirBlock {
            id: ret,
            args: vec![],
            ops: vec![op(OpCode::DelBoundary, vec![phi], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    crate::tir::verify::verify_function(&func)
        .expect("drop insertion must preserve SSA after descendant phi-transfer exclusion");

    let entry_increfs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::IncRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        entry_increfs.is_empty(),
        "clean transfer into the phi must not retain a second owner: {entry_increfs:?}"
    );

    let else_drops: Vec<ValueId> = func.blocks[&else_block]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        !else_drops.contains(&list),
        "the source root was transferred into the live phi; descendant edge-dying must not release it"
    );

    let ret_drops: Vec<ValueId> = func.blocks[&ret]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        ret_drops,
        vec![phi],
        "the phi remains the release authority on the descendant return path"
    );
}

#[test]
fn store_var_scope_root_survives_loop_exit_to_return_boundary() {
    let mut func = TirFunction::new(
        "store_var_scope_root_survives_loop_exit".into(),
        vec![],
        TirType::None,
    );
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let owner = func.fresh_value();
    let stored = func.fresh_value();
    let alias = func.fresh_value();
    let cond = func.fresh_value();
    let call_result = func.fresh_value();
    for v in [owner, stored, alias, call_result] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);

    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_call_bind(owner));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![owner],
            vec![stored],
        ));
        b.ops.push(original_copy_with_operands(
            "copy_var",
            vec![owner],
            vec![alias],
        ));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![alias], vec![call_result])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let exit_drops: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        exit_drops,
        vec![owner],
        "a Python-bound store_var root is released at the scope-exit return boundary only; edge-dying must not pre-release it on the loop exit"
    );
}

#[test]
fn store_var_boundary_mixed_return_paths_split_non_transfer_release() {
    let mut func = TirFunction::new("store_var_mixed_return_paths".into(), vec![], TirType::None);
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let ret = func.fresh_block();
    let class_obj = func.fresh_value();
    let stored = func.fresh_value();
    let fallback = func.fresh_value();
    let selected = func.fresh_value();
    let cond = func.fresh_value();
    for v in [class_obj, stored, fallback, selected] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_call_bind(class_obj));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![class_obj],
            vec![stored],
        ));
        b.ops.push(finalizer_object(fallback));
        b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
        b.terminator = Terminator::CondBranch {
            cond,
            then_block,
            then_args: vec![],
            else_block,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        then_block,
        TirBlock {
            id: then_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: ret,
                args: vec![stored],
            },
        },
    );
    func.blocks.insert(
        else_block,
        TirBlock {
            id: else_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: ret,
                args: vec![fallback],
            },
        },
    );
    func.blocks.insert(
        ret,
        TirBlock {
            id: ret,
            args: vec![TirValue {
                id: selected,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::DelBoundary, vec![selected], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ret_drops: Vec<ValueId> = func.blocks[&ret]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        ret_drops,
        vec![selected],
        "return block drops the selected phi only; the original store_var root is path-specific"
    );

    let split_releases_class_obj = func.blocks.iter().any(|(&bid, block)| {
        bid != entry
            && bid != then_block
            && bid != else_block
            && bid != ret
            && block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::DecRef && op.operands == vec![class_obj])
            && matches!(
                &block.terminator,
                Terminator::Branch { target, args }
                    if *target == ret && args == &vec![fallback]
            )
    });
    assert!(
        split_releases_class_obj,
        "the non-transfer edge must release the original store_var root in an edge split"
    );
}

#[test]
fn store_var_rebind_epoch_closes_old_scope_cleanup_candidate() {
    let mut func = TirFunction::new(
        "store_var_rebind_epoch_cleanup".into(),
        vec![],
        TirType::None,
    );
    let rebind = func.fresh_block();
    let keep = func.fresh_block();
    let join = func.fresh_block();
    let cleanup = func.fresh_block();
    let old_owner = func.fresh_value();
    let old_stored = func.fresh_value();
    let new_owner = func.fresh_value();
    let new_stored = func.fresh_value();
    let rebind_current = func.fresh_value();
    let keep_current = func.fresh_value();
    let current_phi = func.fresh_value();
    let cleanup_phi = func.fresh_value();
    let cond = func.fresh_value();
    let old_len = func.fresh_value();
    for v in [
        old_owner,
        old_stored,
        new_owner,
        new_stored,
        rebind_current,
        keep_current,
        current_phi,
        cleanup_phi,
    ] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);
    func.value_types.insert(old_len, TirType::I64);

    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![],
            vec![old_owner],
        ));
        b.ops
            .push(original_store_var("or_clause", old_owner, old_stored));
        b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
        b.terminator = Terminator::CondBranch {
            cond,
            then_block: rebind,
            then_args: vec![old_stored],
            else_block: keep,
            else_args: vec![old_stored],
        };
    }
    func.blocks.insert(
        rebind,
        TirBlock {
            id: rebind,
            args: vec![TirValue {
                id: rebind_current,
                ty: TirType::DynBox,
            }],
            ops: vec![
                original_copy_with_operands("len", vec![rebind_current], vec![old_len]),
                original_copy_with_operands("list_new", vec![], vec![new_owner]),
                original_store_var("or_clause", new_owner, new_stored),
            ],
            terminator: Terminator::Branch {
                target: join,
                args: vec![new_stored],
            },
        },
    );
    func.blocks.insert(
        keep,
        TirBlock {
            id: keep,
            args: vec![TirValue {
                id: keep_current,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join,
                args: vec![keep_current],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: current_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: cleanup,
                args: vec![current_phi],
            },
        },
    );
    func.blocks.insert(
        cleanup,
        TirBlock {
            id: cleanup,
            args: vec![TirValue {
                id: cleanup_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::DelBoundary, vec![cleanup_phi], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    crate::tir::verify::verify_function(&func)
        .expect("drop insertion must preserve SSA through store_var rebind cleanup");

    let old_direct_drops: Vec<(BlockId, usize)> = func
        .blocks
        .iter()
        .flat_map(|(&bid, block)| {
            block.ops.iter().enumerate().filter_map(move |(idx, op)| {
                (op.opcode == OpCode::DecRef && op.operands == vec![old_owner])
                    .then_some((bid, idx))
            })
        })
        .collect();
    assert_eq!(
        old_direct_drops,
        Vec::<(BlockId, usize)>::new(),
        "once the old source owner transfers into local block args, cleanup must release the current epoch carrier, not the original root: {old_direct_drops:?}"
    );

    let rebind_current_drops: Vec<ValueId> = func.blocks[&rebind]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        rebind_current_drops,
        vec![rebind_current],
        "the rebind path must close the previous carried local epoch exactly once"
    );

    let cleanup_drops: Vec<ValueId> = func.blocks[&cleanup]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        cleanup_drops,
        vec![cleanup_phi],
        "scope cleanup must release the current local epoch only"
    );
    assert!(
        !cleanup_drops.contains(&old_owner) && !cleanup_drops.contains(&new_owner),
        "producer roots transferred into the current cleanup phi must not be dropped again"
    );
}

#[test]
fn store_var_origin_carrier_live_to_return_cleanup_suppresses_source_release() {
    let mut func = TirFunction::new(
        "store_var_origin_carrier_live_to_return_cleanup".into(),
        vec![],
        TirType::None,
    );
    let carrier_a_block = func.fresh_block();
    let carrier_b_block = func.fresh_block();
    let return_pred = func.fresh_block();
    let ret = func.fresh_block();
    let owner = func.fresh_value();
    let stored = func.fresh_value();
    let carrier_a = func.fresh_value();
    let carrier_b = func.fresh_value();
    let use_result = func.fresh_value();
    for value in [owner, stored, carrier_a, carrier_b, use_result] {
        func.value_types.insert(value, TirType::DynBox);
    }

    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.push(original_copy_with_operands(
            "object_new",
            vec![],
            vec![owner],
        ));
        block.ops.push(original_store_var("args", owner, stored));
        block.terminator = Terminator::Branch {
            target: carrier_a_block,
            args: vec![stored],
        };
    }
    func.blocks.insert(
        carrier_a_block,
        TirBlock {
            id: carrier_a_block,
            args: vec![TirValue {
                id: carrier_a,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: carrier_b_block,
                args: vec![carrier_a],
            },
        },
    );
    func.blocks.insert(
        carrier_b_block,
        TirBlock {
            id: carrier_b_block,
            args: vec![TirValue {
                id: carrier_b,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: return_pred,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        return_pred,
        TirBlock {
            id: return_pred,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: ret,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        ret,
        TirBlock {
            id: ret,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![carrier_b], vec![use_result])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);
    crate::tir::verify::verify_function(&func)
        .expect("origin-carrier return cleanup must preserve SSA");

    let source_drops: Vec<(BlockId, ValueId)> = func
        .blocks
        .iter()
        .flat_map(|(&bid, block)| {
            block.ops.iter().filter_map(move |op| {
                let &operand = op.operands.first()?;
                (op.opcode == OpCode::DecRef && (operand == owner || operand == stored))
                    .then_some((bid, operand))
            })
        })
        .collect();
    assert_eq!(
        source_drops,
        Vec::<(BlockId, ValueId)>::new(),
        "once a store_var source has moved into a live return-cleanup carrier, the source root is no longer an edge or return release authority"
    );

    let ret_drops: Vec<ValueId> = func.blocks[&ret]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert!(
        ret_drops.contains(&carrier_b),
        "the live carrier remains the cleanup authority at return; got {ret_drops:?}"
    );
}

#[test]
fn owned_root_forwarded_to_three_owned_phis_gets_two_retains() {
    let mut func = TirFunction::new(
        "owned_root_three_phi_retain_multiplicity".into(),
        vec![],
        TirType::None,
    );
    let join = func.fresh_block();
    let owner = func.fresh_value();
    let a = func.fresh_value();
    let b = func.fresh_value();
    let c = func.fresh_value();
    for v in [owner, a, b, c] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let block = func.blocks.get_mut(&entry).unwrap();
        block.ops.push(finalizer_call_bind(owner));
        block.terminator = Terminator::Branch {
            target: join,
            args: vec![owner, owner, owner],
        };
    }
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![
                TirValue {
                    id: a,
                    ty: TirType::DynBox,
                },
                TirValue {
                    id: b,
                    ty: TirType::DynBox,
                },
                TirValue {
                    id: c,
                    ty: TirType::DynBox,
                },
            ],
            ops: vec![
                op(OpCode::DelBoundary, vec![a], vec![]),
                op(OpCode::DelBoundary, vec![b], vec![]),
                op(OpCode::DelBoundary, vec![c], vec![]),
            ],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let entry_increfs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::IncRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        entry_increfs,
        vec![owner, owner],
        "one original owner transfers to the first phi; the other two owned phis need retained references"
    );
}

#[test]
fn store_var_boundary_transferred_through_loop_phi_releases_phi_once() {
    let mut func = TirFunction::new(
        "store_var_loop_phi_transfers_owner".into(),
        vec![],
        TirType::None,
    );
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let set_owner = func.fresh_value();
    let stored = func.fresh_value();
    let current_phi = func.fresh_value();
    let next_owner = func.fresh_value();
    let next_stored = func.fresh_value();
    let cond = func.fresh_value();
    for v in [set_owner, stored, current_phi, next_owner, next_stored] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(original_copy_with_operands(
            "set_new",
            vec![],
            vec![set_owner],
        ));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![set_owner],
            vec![stored],
        ));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![stored],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: current_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: exit,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                original_copy_with_operands("set_new", vec![], vec![next_owner]),
                original_copy_with_operands("store_var", vec![next_owner], vec![next_stored]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next_stored],
            },
        },
    );
    let mut exit_boundary = op(OpCode::DelBoundary, vec![current_phi], vec![]);
    exit_boundary
        .attrs
        .insert("s_value".into(), AttrValue::Str("inherited".into()));
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![exit_boundary],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let exit_drops: Vec<ValueId> = func.blocks[&exit]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        exit_drops,
        vec![current_phi],
        "the set_new owner moved into the loop phi; the phi boundary is the sole release authority"
    );
    assert!(
        !exit_drops.contains(&set_owner),
        "the original producer root must not be return-boundary dropped after a clean phi transfer"
    );
    assert!(
        !exit_drops.contains(&next_owner),
        "the backedge producer root must also transfer into the loop phi instead of dropping beside it at return"
    );
}

#[test]
fn result_carrying_store_var_later_container_absorb_keeps_owner_to_return_boundary() {
    let mut func = TirFunction::new(
        "finalizer_scope_store_result_later_absorb".into(),
        vec![],
        TirType::None,
    );
    let list = func.fresh_value();
    let stored = func.fresh_value();
    let item = func.fresh_value();
    for v in [list, stored, item] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops
            .push(original_copy_with_operands("list_new", vec![], vec![list]));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![list],
            vec![stored],
        ));
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_append",
            vec![stored, item],
            vec![],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let store_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
        .expect("store_var marker must survive");
    let append_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_append"))
        .expect("list_append op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert!(
        !dropped
            .iter()
            .any(|(idx, value)| *idx == store_idx + 1 && *value == list),
        "a result-carrying store_var must not release an empty container before later mutation through its alias"
    );
    assert_eq!(
        dropped,
        vec![(append_idx + 1, item), (marker_idx + 1, list)],
        "later container absorption makes the Python-bound owner finalizer-sensitive without moving its release before return"
    );
    assert!(
        !dropped
            .iter()
            .any(|(idx, value)| *idx == list_idx + 1 && *value == list),
        "empty container owner must survive past construction once bound to a Python local"
    );
}

#[test]
fn copy_list_new_finalizer_sensitive_container_releases_at_return_boundary() {
    let mut func = TirFunction::new("finalizer_scope_copy_list".into(), vec![], TirType::None);
    let item = func.fresh_value();
    let list = func.fresh_value();
    for v in [item, list] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(list_idx + 1, item), (marker_idx + 1, list)],
        "Copy-preserved list_new must release the producer temp at the absorption boundary"
    );
}

#[test]
fn copy_class_def_descriptor_temp_releases_at_class_construction_boundary() {
    let mut func = TirFunction::new(
        "finalizer_scope_copy_class_def".into(),
        vec![],
        TirType::None,
    );
    let name = func.fresh_value();
    let descriptor = func.fresh_value();
    let class_obj = func.fresh_value();
    for v in [name, descriptor, class_obj] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(name));
        b.ops.push(finalizer_object(descriptor));
        b.ops.push(original_copy_with_operands(
            "class_def",
            vec![name, descriptor],
            vec![class_obj],
        ));
        b.ops.push(original_copy_with_operands(
            "store_var",
            vec![class_obj],
            vec![],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let class_def_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("class_def"))
        .expect("class_def op must survive");
    let store_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
        .expect("store_var op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let tracked_drops: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .filter_map(|(idx, op)| {
            let dropped = op.operands[0];
            [descriptor, class_obj]
                .contains(&dropped)
                .then_some((idx, dropped))
        })
        .collect();
    let descriptor_drop_idx = tracked_drops
        .iter()
        .find_map(|(idx, dropped)| (*dropped == descriptor).then_some(*idx))
        .expect("descriptor temp must be dropped");
    let class_drop_idx = tracked_drops
        .iter()
        .find_map(|(idx, dropped)| (*dropped == class_obj).then_some(*idx))
        .expect("class owner must be dropped");
    assert!(
        class_def_idx < descriptor_drop_idx && descriptor_drop_idx < store_idx,
        "descriptor temp must release at the class construction boundary before the class owner is used"
    );
    assert_eq!(
        class_drop_idx,
        marker_idx + 1,
        "class owner should remain live until the Python boundary"
    );
}

#[test]
fn call_bind_list_new_finalizer_sensitive_container_releases_at_return_boundary() {
    let mut func = TirFunction::new(
        "finalizer_scope_call_bind_list".into(),
        vec![],
        TirType::None,
    );
    let item = func.fresh_value();
    let list = func.fresh_value();
    for v in [item, list] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_call_bind(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(list_idx + 1, item), (marker_idx + 1, list)],
        "call_bind-created finalizer temps release at list_new while the container owner defers"
    );
}

#[test]
fn unbound_finalizer_container_call_arg_releases_at_call_boundary() {
    let mut func = TirFunction::new(
        "finalizer_scope_unbound_call_arg".into(),
        vec![],
        TirType::None,
    );
    let item = func.fresh_value();
    let list = func.fresh_value();
    for v in [item, list] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops.push(op(OpCode::Call, vec![list], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let call_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Call && op.operands == vec![list])
        .expect("call op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(list_idx + 1, item), (call_idx + 1, list)],
        "unbound finalizer-sensitive expression temps die at their last use, not at frame return"
    );
    assert!(
        call_idx < marker_idx,
        "fixture must keep a later side effect after the call boundary"
    );
}

#[test]
fn call_bind_check_exception_list_new_finalizer_releases_at_return_boundary() {
    let mut func = TirFunction::new(
        "finalizer_scope_real_call_bind_list".into(),
        vec![],
        TirType::None,
    );
    let callee = func.fresh_value();
    let builder = func.fresh_value();
    let item = func.fresh_value();
    let list = func.fresh_value();
    for v in [callee, builder, item, list] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(op(OpCode::ModuleGetAttr, vec![], vec![callee]));
        b.ops.push(original_copy_with_operands(
            "callargs_new",
            vec![],
            vec![builder],
        ));
        let mut call = finalizer_call_bind(item);
        call.operands = vec![callee, builder];
        b.ops.push(call);
        b.ops.push(op(OpCode::CheckException, vec![], vec![]));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(op(OpCode::CheckException, vec![], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert!(
        dropped.contains(&(list_idx + 1, item)),
        "call result temp must release at the list_new absorption boundary: {dropped:?}"
    );
    assert!(
        dropped.contains(&(marker_idx + 1, list)),
        "absorbing list owner must still release at return boundary: {dropped:?}"
    );
}

#[test]
fn list_append_absorbed_temp_releases_at_append_boundary() {
    let mut func = TirFunction::new("finalizer_scope_list_append".into(), vec![], TirType::None);
    let list = func.fresh_value();
    let item = func.fresh_value();
    for v in [list, item] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops
            .push(original_copy_with_operands("list_new", vec![], vec![list]));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_append",
            vec![list, item],
            vec![],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let append_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_append"))
        .expect("list_append op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(append_idx + 1, item), (marker_idx + 1, list)],
        "list_append absorbs the producer temp but the container owner stays boundary-deferred"
    );
}

#[test]
fn module_set_attr_releases_absorbed_value_before_later_borrowed_use() {
    let mut func = TirFunction::new(
        "finalizer_scope_module_set_attr".into(),
        vec![],
        TirType::None,
    );
    let module = func.fresh_value();
    let name = func.fresh_value();
    let item = func.fresh_value();
    let list = func.fresh_value();
    let popped = func.fresh_value();
    for v in [module, name, item, list, popped] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops
            .push(op(OpCode::ModuleSetAttr, vec![module, name, list], vec![]));
        b.ops.push(original_copy_with_operands(
            "list_pop",
            vec![list],
            vec![popped],
        ));
        b.ops
            .push(op(OpCode::ModuleDelGlobal, vec![module, name], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let store_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::ModuleSetAttr)
        .expect("module_set_attr op must survive");
    let pop_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_pop"))
        .expect("list_pop op must survive");
    let tracked_drops: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .filter_map(|(idx, op)| {
            let dropped = op.operands[0];
            [item, list, popped]
                .contains(&dropped)
                .then_some((idx, dropped))
        })
        .collect();
    assert_eq!(
        tracked_drops,
        vec![
            (list_idx + 1, item),
            (store_idx + 1, list),
            (pop_idx + 1, popped),
        ],
        "module_set_attr owns the Python-visible global lifetime, so the compiler-owned list ref must release at the storage boundary"
    );
}

#[test]
fn generic_attr_store_releases_absorbed_defaults_tuple_before_later_borrowed_use() {
    let mut func = TirFunction::new(
        "finalizer_scope_generic_attr_defaults".into(),
        vec![],
        TirType::None,
    );
    let item = func.fresh_value();
    let func_obj = func.fresh_value();
    let defaults = func.fresh_value();
    let version = func.fresh_value();
    for v in [item, func_obj, defaults, version] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(finalizer_object(func_obj));
        b.ops.push(original_copy_with_operands(
            "tuple_new",
            vec![item],
            vec![defaults],
        ));
        let mut store = op(OpCode::StoreAttr, vec![func_obj, defaults], vec![]);
        store.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("set_attr_generic_obj".into()),
        );
        store
            .attrs
            .insert("s_value".into(), AttrValue::Str("__defaults__".into()));
        b.ops.push(store);
        b.ops.push(op(
            OpCode::FunctionDefaultsVersion,
            vec![func_obj],
            vec![version],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let tuple_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("tuple_new"))
        .expect("tuple_new op must survive");
    let store_idx = ops
        .iter()
        .position(|op| {
            op.opcode == OpCode::StoreAttr && original_kind(op) == Some("set_attr_generic_obj")
        })
        .expect("generic attr store must survive");
    let version_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::FunctionDefaultsVersion)
        .expect("later borrowed function read must survive");
    let tracked_drops: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .filter_map(|(idx, op)| {
            let dropped = op.operands[0];
            [item, defaults, version]
                .contains(&dropped)
                .then_some((idx, dropped))
        })
        .collect();
    let drop_index = |value| {
        tracked_drops
            .iter()
            .find_map(|(idx, dropped)| (*dropped == value).then_some(*idx))
            .expect("tracked owned value must be released")
    };
    assert_eq!(drop_index(item), tuple_idx + 1);
    assert_eq!(
        drop_index(defaults),
        store_idx + 1,
        "generic attr storage retains the value, so the compiler-owned defaults tuple must release at the store boundary before later borrowed function reads"
    );
    assert!(drop_index(version) > version_idx);
    assert!(
        drop_index(defaults) < version_idx,
        "the compiler-owned defaults ref must be released before the later borrowed defaults-version read"
    );
}

#[test]
fn discarded_list_pop_result_releases_at_pop_boundary() {
    let mut func = TirFunction::new("finalizer_scope_list_pop".into(), vec![], TirType::None);
    let item = func.fresh_value();
    let list = func.fresh_value();
    let popped = func.fresh_value();
    for v in [item, list, popped] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(original_copy_with_operands(
            "list_pop",
            vec![list],
            vec![popped],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let pop_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_pop"))
        .expect("list_pop op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![
            (list_idx + 1, item),
            (pop_idx + 1, popped),
            (marker_idx + 1, list),
        ],
        "discarded list_pop result releases at pop boundary while list owner defers"
    );
}

#[test]
fn named_local_absorbed_into_list_is_not_released_at_absorption_boundary() {
    let mut func = TirFunction::new(
        "finalizer_scope_named_local_list".into(),
        vec![],
        TirType::None,
    );
    let item = func.fresh_value();
    let list = func.fresh_value();
    for v in [item, list] {
        func.value_types.insert(v, TirType::DynBox);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops
            .push(original_copy_with_operands("store_var", vec![item], vec![]));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let list_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("list_new"))
        .expect("list_new op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert!(
        !dropped.contains(&(list_idx + 1, item)),
        "Python-bound local must not drop at the list absorption statement: {dropped:?}"
    );
    assert_eq!(
        dropped,
        vec![(list_idx + 1, list), (marker_idx + 1, item)],
        "the expression container releases at statement last use while the Python-bound local root waits for the frame boundary"
    );
}

#[test]
fn non_finalizer_local_store_releases_at_last_use_not_return_boundary() {
    let mut func = TirFunction::new("ordinary_local_scope".into(), vec![], TirType::None);
    let list = func.fresh_value();
    func.value_types.insert(list, TirType::DynBox);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops
            .push(original_copy_with_operands("list_new", vec![], vec![list]));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let store_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::Copy && original_kind(op) == Some("store_var"))
        .expect("store_var marker must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(store_idx + 1, list)],
        "ordinary local stores must not create a second return-boundary cleanup"
    );
    assert!(
        store_idx < marker_idx,
        "fixture keeps a side-effect marker after the local store"
    );
}

#[test]
fn edge_dying_skips_finalizer_boundary_owned_local_root() {
    let mut func = TirFunction::new("finalizer_boundary_edge_exit".into(), vec![], TirType::None);
    let gate = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let item = func.fresh_value();
    let list = func.fresh_value();
    let cond = func.fresh_value();
    let body_arg = func.fresh_value();
    let body_use = func.fresh_value();
    for v in [item, list, body_arg, body_use] {
        func.value_types.insert(v, TirType::DynBox);
    }
    func.value_types.insert(cond, TirType::Bool);
    {
        let b = func.blocks.get_mut(&func.entry_block).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(original_copy_with_operands(
            "list_new",
            vec![item],
            vec![list],
        ));
        b.ops
            .push(original_copy_with_operands("store_var", vec![list], vec![]));
        b.terminator = Terminator::Branch {
            target: gate,
            args: vec![],
        };
    }
    func.blocks.insert(
        gate,
        TirBlock {
            id: gate,
            args: vec![],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![list],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![TirValue {
                id: body_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![op(OpCode::Copy, vec![body_arg], vec![body_use])],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![op(OpCode::WarnStderr, vec![], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let exit_ops = &func.blocks[&exit].ops;
    let marker_idx = exit_ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("exit marker must survive");
    let dropped: Vec<(usize, ValueId)> = exit_ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert_eq!(
        dropped,
        vec![(marker_idx + 1, list)],
        "edge-dying must not release a finalizer-sensitive local before its return boundary"
    );
}

#[test]
fn explicit_decref_is_the_finalizer_del_boundary() {
    let mut func = TirFunction::new("finalizer_del".into(), vec![], TirType::None);
    let item = func.fresh_value();
    func.value_types.insert(item, TirType::DynBox);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(finalizer_object(item));
        b.ops.push(op(OpCode::DecRef, vec![item], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let decrefs: Vec<ValueId> = func.blocks[&entry]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::DecRef)
        .map(|op| op.operands[0])
        .collect();
    assert_eq!(
        decrefs,
        vec![item],
        "explicit DecRef/`del` consumes the finalizer boundary and must not be duplicated at return"
    );
}

#[test]
fn delete_var_releases_old_slot_at_delete_boundary() {
    let mut func = TirFunction::new(
        "delete_var_finalizer_boundary".into(),
        vec![],
        TirType::None,
    );
    let missing = func.fresh_value();
    let item = func.fresh_value();
    let deleted = func.fresh_value();
    func.value_types.insert(missing, TirType::None);
    func.value_types.insert(item, TirType::DynBox);
    func.value_types.insert(deleted, TirType::None);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        let mut missing_op = op(OpCode::ConstNone, vec![], vec![missing]);
        missing_op
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("missing".into()));
        b.ops.push(missing_op);
        b.ops.push(finalizer_object(item));
        let mut delete = op(OpCode::DeleteVar, vec![missing, item], vec![deleted]);
        delete
            .attrs
            .insert("_var".into(), AttrValue::Str("item".into()));
        b.ops.push(delete);
        b.ops.push(op(OpCode::WarnStderr, vec![], vec![]));
        b.terminator = Terminator::Return { values: vec![] };
    }

    let mut am = AnalysisManager::new();
    run(&mut func, &mut am);

    let ops = &func.blocks[&entry].ops;
    let delete_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::DeleteVar)
        .expect("delete_var op must survive");
    let marker_idx = ops
        .iter()
        .position(|op| op.opcode == OpCode::WarnStderr)
        .expect("marker op must survive");
    let dropped: Vec<(usize, ValueId)> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| op.opcode == OpCode::DecRef)
        .map(|(idx, op)| (idx, op.operands[0]))
        .collect();
    assert!(
        dropped.contains(&(delete_idx + 1, item)),
        "delete_var must drop the old occupant immediately after storing missing: {dropped:?}"
    );
    assert!(
        !dropped
            .iter()
            .any(|(idx, value)| *value == item && *idx > marker_idx),
        "old slot occupant must not be deferred past later side effects: {dropped:?}"
    );
}
