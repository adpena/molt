//! Block-argument custody on every canonical control arc. A check's payload
//! binds its handler's arguments exactly like a branch argument binds a phi: an
//! owned root moves into one argument, and any other owned binding is retained
//! on its arc, by the check's landing on the exceptional path only. A moved
//! root stays unowned until its definition runs again, and an argument that
//! nothing reads is released on entry. Each case runs concrete paths of the
//! rewritten function through the reference-count model in `mod.rs`, so
//! releasing an object that a live argument still holds, a leak, or an
//! unbalanced retain fails the path wherever the pass placed its operations.

use super::*;

fn function(name: &str) -> TirFunction {
    TirFunction::new(
        name.into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    )
}

fn owned(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::DynBox);
    value
}

fn flag(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::Bool);
    value
}

fn produce(result: ValueId) -> TirOp {
    op(OpCode::Call, vec![], vec![result])
}

fn consume(value: ValueId) -> TirOp {
    op(OpCode::Call, vec![value], vec![])
}

fn observe(label: i64, payload: Vec<ValueId>) -> TirOp {
    let mut check = op(OpCode::CheckException, payload, vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(label));
    check
}

fn protect(label: i64, payload: Vec<ValueId>) -> TirOp {
    let mut start = try_start(label);
    start.operands = payload;
    start
}

fn block(id: BlockId, args: Vec<ValueId>, ops: Vec<TirOp>, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args: args
            .into_iter()
            .map(|id| TirValue {
                id,
                ty: TirType::DynBox,
            })
            .collect(),
        ops,
        terminator,
    }
}

fn jump(target: BlockId, args: Vec<ValueId>) -> Terminator {
    Terminator::Branch { target, args }
}

fn choose(cond: ValueId, then_block: BlockId, else_block: BlockId) -> Terminator {
    Terminator::CondBranch {
        cond,
        then_block,
        then_args: vec![],
        else_block,
        else_args: vec![],
    }
}

fn done() -> Terminator {
    Terminator::Return { values: vec![] }
}

fn retains_in(func: &TirFunction, block: BlockId) -> usize {
    func.blocks[&block]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::IncRef)
        .count()
}

/// A loop's owned value is the payload of every observation in a protected
/// body whose handler re-enters the loop: the public `args_kwargs_eval_order`
/// module shape. The handler argument takes the value and carries it back into
/// the loop, so the handler entry may not also release the loop's own name.
#[test]
fn handler_argument_keeps_the_loop_value_it_receives() {
    let mut func = function("handler_reenters_loop");
    let tuple = owned(&mut func);
    let carried = owned(&mut func);
    let handled = owned(&mut func);
    let more = flag(&mut func);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let handler = func.fresh_block();
    let exit = func.fresh_block();
    func.label_id_map.insert(handler.0, 37);
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![produce(tuple)],
            jump(header, vec![tuple]),
        ),
    );
    func.blocks.insert(
        header,
        block(
            header,
            vec![carried],
            vec![op(OpCode::ConstBool, vec![], vec![more])],
            choose(more, body, exit),
        ),
    );
    func.blocks.insert(
        body,
        block(
            body,
            vec![],
            vec![
                protect(37, vec![carried]),
                op(OpCode::Call, vec![], vec![]),
                observe(37, vec![carried]),
            ],
            jump(header, vec![carried]),
        ),
    );
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![handled],
            vec![consume(handled)],
            jump(header, vec![handled]),
        ),
    );
    func.blocks
        .insert(exit, block(exit, vec![], vec![], done()));

    insert(&mut func);
    let paths: [&[bool]; 5] = [
        &[],
        &[true, false],
        &[true, true],
        &[true, true, true, true],
        &[true, false, true, true],
    ];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// A handler that reads a payload root directly and through its argument holds
/// two references on the exceptional path. The observation's landing retains
/// one for the argument, so the normal path pays nothing.
#[test]
fn payload_the_handler_also_reads_is_retained_for_its_argument() {
    let mut func = function("handler_reads_payload_root");
    let root = owned(&mut func);
    let bound = owned(&mut func);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 41);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![produce(root), observe(41, vec![root]), consume(root)],
            done(),
        ),
    );
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![bound],
            vec![consume(bound), consume(root)],
            done(),
        ),
    );

    insert(&mut func);
    assert_eq!(
        retains_in(&func, entry),
        0,
        "the retain belongs to the exceptional path"
    );
    let paths: [&[bool]; 2] = [&[], &[true]];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// One root bound to two handler arguments moves into the first; the second
/// needs its own reference, taken in the landing.
#[test]
fn payload_bound_to_two_arguments_moves_once_and_retains_once() {
    let mut func = function("payload_bound_twice");
    let root = owned(&mut func);
    let first = owned(&mut func);
    let second = owned(&mut func);
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 42);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![produce(root), observe(42, vec![root, root]), consume(root)],
            done(),
        ),
    );
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![first, second],
            vec![consume(first), consume(second)],
            done(),
        ),
    );

    insert(&mut func);
    assert_eq!(
        retains_in(&func, entry),
        0,
        "the retain belongs to the exceptional path"
    );
    let paths: [&[bool]; 2] = [&[], &[true]];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// Two observations of one protected region bind the handler argument to
/// different roots. The earlier root, still used on the normal path, moves on
/// the region start and the first observation, so the handler entry may not
/// release it; the later observation abandons it, so its landing does.
#[test]
fn handler_entry_keeps_a_root_that_an_earlier_observation_moves() {
    let mut func = function("handler_payload_changes");
    let early = owned(&mut func);
    let late = owned(&mut func);
    let bound = owned(&mut func);
    let next = func.fresh_block();
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 43);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![
                produce(early),
                protect(43, vec![early]),
                observe(43, vec![early]),
                produce(late),
                observe(43, vec![late]),
            ],
            jump(next, vec![]),
        ),
    );
    func.blocks.insert(
        next,
        block(next, vec![], vec![consume(early), consume(late)], done()),
    );
    func.blocks.insert(
        handler,
        block(handler, vec![bound], vec![consume(bound)], done()),
    );

    insert(&mut func);
    let paths: [&[bool]; 3] = [&[], &[true], &[false, true]];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// A handler whose argument nothing reads still owns what the check moved into
/// it, so it releases the argument on entry.
#[test]
fn dead_handler_argument_releases_what_its_check_moved() {
    let mut func = function("dead_handler_argument");
    let root = owned(&mut func);
    let bound = owned(&mut func);
    let next = func.fresh_block();
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 50);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![produce(root), observe(50, vec![root])],
            jump(next, vec![]),
        ),
    );
    func.blocks
        .insert(next, block(next, vec![], vec![consume(root)], done()));
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![bound],
            vec![op(OpCode::WarnStderr, vec![], vec![])],
            done(),
        ),
    );

    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), vec![Event::Freed(0)]);
    assert_eq!(
        trace(&func, 0, &[true]),
        vec![Event::Freed(0), Event::Marker],
        "the handler argument dies on entry"
    );
}

/// A join argument that nothing reads owns what each arm moved into it. It is
/// released on entry to the join, once on every path.
#[test]
fn dead_join_argument_releases_what_each_arm_moved() {
    let mut func = function("dead_join_argument");
    let left = owned(&mut func);
    let right = owned(&mut func);
    let joined = owned(&mut func);
    let cond = flag(&mut func);
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let join = func.fresh_block();
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![op(OpCode::ConstBool, vec![], vec![cond])],
            choose(cond, then_block, else_block),
        ),
    );
    func.blocks.insert(
        then_block,
        block(
            then_block,
            vec![],
            vec![produce(left)],
            jump(join, vec![left]),
        ),
    );
    func.blocks.insert(
        else_block,
        block(
            else_block,
            vec![],
            vec![produce(right)],
            jump(join, vec![right]),
        ),
    );
    func.blocks.insert(
        join,
        block(
            join,
            vec![joined],
            vec![op(OpCode::WarnStderr, vec![], vec![])],
            done(),
        ),
    );

    insert(&mut func);
    let paths: [&[bool]; 2] = [&[true], &[false]];
    for path in paths {
        assert_eq!(
            trace(&func, 0, path),
            vec![Event::Freed(0), Event::Marker],
            "along {path:?}: the join argument dies on entry"
        );
    }
}

/// A handler whose own observation re-enters it binds its argument to itself;
/// the object keeps one owner across every re-entry.
#[test]
fn handler_reentering_itself_keeps_one_owner() {
    let mut func = function("handler_self_reentry");
    let root = owned(&mut func);
    let bound = owned(&mut func);
    let resumed = func.fresh_block();
    let handler = func.fresh_block();
    let recovered = func.fresh_block();
    func.label_id_map.insert(handler.0, 44);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![produce(root), observe(44, vec![root])],
            jump(resumed, vec![]),
        ),
    );
    func.blocks
        .insert(resumed, block(resumed, vec![], vec![consume(root)], done()));
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![bound],
            vec![op(OpCode::Call, vec![], vec![]), observe(44, vec![bound])],
            jump(recovered, vec![]),
        ),
    );
    func.blocks.insert(
        recovered,
        block(recovered, vec![], vec![consume(bound)], done()),
    );

    insert(&mut func);
    let paths: [&[bool]; 4] = [&[], &[true], &[true, true], &[true, true, true]];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// An owner created inside a handler is the payload of a nested observation.
/// The nested handler's argument takes it while the outer handler's normal
/// path still uses it.
#[test]
fn owner_created_in_a_handler_moves_into_a_nested_handler() {
    let mut func = function("nested_handler_owner");
    let created = owned(&mut func);
    let bound = owned(&mut func);
    let outer = func.fresh_block();
    let rejoin = func.fresh_block();
    let inner = func.fresh_block();
    func.label_id_map.insert(outer.0, 45);
    func.label_id_map.insert(inner.0, 46);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(entry, vec![], vec![observe(45, vec![])], done()),
    );
    func.blocks.insert(
        outer,
        block(
            outer,
            vec![],
            vec![produce(created), observe(46, vec![created])],
            jump(rejoin, vec![]),
        ),
    );
    func.blocks.insert(
        rejoin,
        block(rejoin, vec![], vec![consume(created)], done()),
    );
    func.blocks.insert(
        inner,
        block(inner, vec![bound], vec![consume(bound)], done()),
    );

    insert(&mut func);
    let paths: [&[bool]; 3] = [&[], &[true], &[true, true]];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// A named local that a check moves into its handler's argument reaches the
/// scope-exit cleanup through that argument, as frontend lowering emits it.
/// Its own name must not also be released on the handler path, and on both
/// paths the object lives until the cleanup after the last statement.
#[test]
fn local_moved_into_a_handler_argument_is_released_once_at_scope_exit() {
    let mut func = function("local_moved_to_handler");
    let local = owned(&mut func);
    let bound = owned(&mut func);
    let current = owned(&mut func);
    let loaded = owned(&mut func);
    let handler = func.fresh_block();
    let exit = func.fresh_block();
    func.label_id_map.insert(handler.0, 47);
    let mut producer = produce(local);
    producer
        .attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![
                producer,
                original_copy_with_operands("store_var", vec![local], vec![]),
                observe(47, vec![local]),
            ],
            jump(exit, vec![local]),
        ),
    );
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![bound],
            vec![consume(bound)],
            jump(exit, vec![bound]),
        ),
    );
    func.blocks.insert(
        exit,
        block(
            exit,
            vec![current],
            vec![
                op(OpCode::WarnStderr, vec![], vec![]),
                original_copy_with_operands("load_var", vec![current], vec![loaded]),
                op(OpCode::DelBoundary, vec![loaded], vec![]),
            ],
            done(),
        ),
    );

    insert(&mut func);
    let paths: [&[bool]; 2] = [&[], &[true]];
    for path in paths {
        assert_eq!(
            trace(&func, 0, path),
            vec![Event::Marker, Event::Freed(0)],
            "along {path:?}"
        );
    }
}

/// Without a scope-exit cleanup the frame boundary itself releases a named
/// local. Once a check has moved the local into a handler argument, the local
/// no longer owns its object on the handler path to that boundary, and the
/// argument holds the object under the same lexical custody. Each path
/// releases exactly the owner it has, on its own arc into the shared exit.
#[test]
fn frame_boundary_of_a_moved_local_releases_it_only_where_it_is_owned() {
    let mut func = function("moved_local_without_cleanup");
    let local = owned(&mut func);
    let bound = owned(&mut func);
    let handler = func.fresh_block();
    let exit = func.fresh_block();
    func.label_id_map.insert(handler.0, 48);
    let mut producer = produce(local);
    producer
        .attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![
                producer,
                original_copy_with_operands("store_var", vec![local], vec![]),
                observe(48, vec![local]),
            ],
            jump(exit, vec![]),
        ),
    );
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![bound],
            vec![consume(bound)],
            jump(exit, vec![]),
        ),
    );
    func.blocks.insert(
        exit,
        block(
            exit,
            vec![],
            vec![op(OpCode::WarnStderr, vec![], vec![])],
            done(),
        ),
    );

    insert(&mut func);
    let paths: [&[bool]; 2] = [&[], &[true]];
    for path in paths {
        trace(&func, 0, path);
    }
}

/// A poll activation reloads an owned reference from its frame, and a check
/// moves it into a handler argument while the normal continuation still uses
/// it. Resume dispatch and the first entry follow the ordinary rules.
#[test]
fn activation_handler_argument_keeps_a_reloaded_frame_value() {
    let mut func = TirFunction::new(
        "activation_handler_payload".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let frame = func.blocks[&func.entry_block].args[0].id;
    let loaded = owned(&mut func);
    let bound = owned(&mut func);
    let first = func.fresh_block();
    let resume = func.fresh_block();
    let resumed = func.fresh_block();
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 49);
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(9, resume, vec![])],
        default: first,
        default_args: vec![],
    };
    let mut reload = op(OpCode::ClosureLoad, vec![frame], vec![loaded]);
    reload.attrs.insert("value".into(), AttrValue::Int(24));
    func.blocks
        .insert(first, block(first, vec![], vec![], done()));
    func.blocks.insert(
        resume,
        block(
            resume,
            vec![],
            vec![reload, observe(49, vec![loaded])],
            jump(resumed, vec![]),
        ),
    );
    func.blocks.insert(
        resumed,
        block(resumed, vec![], vec![consume(loaded)], done()),
    );
    func.blocks.insert(
        handler,
        block(handler, vec![bound], vec![consume(bound)], done()),
    );

    insert(&mut func);
    let paths: [(i64, &[bool]); 3] = [(9, &[]), (9, &[true]), (0, &[])];
    for (state, path) in paths {
        trace(&func, state, path);
    }
}

#[test]
fn returned_borrow_preserves_both_caller_and_result_owners() {
    let mut func = TirFunction::new(
        "returned_borrow_preserves_both_caller_and_result_owners".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let parameter = entry.args[0].id;
    entry.terminator = Terminator::Return {
        values: vec![parameter],
    };
    assert!(
        execute(&func, 0, &[]).is_err(),
        "an unretained return must fail the ownership oracle"
    );
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), Vec::<Event>::new());
}

#[test]
fn returned_fresh_owner_transfers_without_an_extra_reference() {
    let mut func = TirFunction::new(
        "returned_fresh_owner_transfers_without_an_extra_reference".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_str(result));
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), Vec::<Event>::new());
}

#[test]
fn borrowed_phi_retains_run_only_on_the_selected_arc() {
    for (then_count, else_count) in [(1, 0), (2, 1)] {
        let mut func = TirFunction::new(
            "borrowed_phi_selected_arc".into(),
            vec![TirType::DynBox],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = func.entry_block;
        let borrowed = func.blocks[&entry].args[0].id;
        let condition = flag(&mut func);
        let then_block = func.fresh_block();
        let else_block = func.fresh_block();
        let then_values: Vec<_> = (0..then_count).map(|_| owned(&mut func)).collect();
        let else_values: Vec<_> = (0..else_count).map(|_| owned(&mut func)).collect();
        let body = func.blocks.get_mut(&entry).unwrap();
        body.ops
            .push(op(OpCode::ConstBool, vec![], vec![condition]));
        body.terminator = Terminator::CondBranch {
            cond: condition,
            then_block,
            then_args: vec![borrowed; then_count],
            else_block,
            else_args: vec![borrowed; else_count],
        };
        for (target, values) in [(then_block, then_values), (else_block, else_values)] {
            let uses = values.iter().copied().map(consume).collect();
            func.blocks
                .insert(target, block(target, values, uses, done()));
        }
        insert(&mut func);
        for selected in [true, false] {
            assert_eq!(trace(&func, 0, &[selected]), Vec::<Event>::new());
        }
        assert_eq!(
            retains_in(&func, entry),
            0,
            "a conditional edge cannot retain owners for its unselected sibling"
        );
    }
}

/// SSA binds a handler argument at the region's entry to its variable's value
/// there, which is `None` for a variable that the protected body assigns first.
/// The registration never raises into the handler, so it binds no argument:
/// only the check does, moving its payload into an argument that nothing
/// reads, and the handler releases that argument on entry. A `Bool` stands in
/// for the `None` placeholder: both are non-heap carriers to the pass, and the
/// oracle models only `Bool` and `I64` results as raw.
#[test]
fn dead_handler_argument_unbound_at_region_entry_releases_what_its_check_moved() {
    let mut func = function("dead_handler_argument_unbound_at_region_entry");
    let unbound = flag(&mut func);
    let root = owned(&mut func);
    let bound = owned(&mut func);
    let next = func.fresh_block();
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 51);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            vec![],
            vec![
                op(OpCode::ConstBool, vec![], vec![unbound]),
                protect(51, vec![unbound]),
                produce(root),
                observe(51, vec![root]),
            ],
            jump(next, vec![]),
        ),
    );
    func.blocks
        .insert(next, block(next, vec![], vec![consume(root)], done()));
    func.blocks.insert(
        handler,
        block(
            handler,
            vec![bound],
            vec![op(OpCode::WarnStderr, vec![], vec![])],
            done(),
        ),
    );

    insert(&mut func);
    assert_eq!(trace(&func, 0, &[]), vec![Event::Freed(0)]);
    assert_eq!(
        trace(&func, 0, &[true]),
        vec![Event::Freed(0), Event::Marker],
        "the handler argument dies on entry whatever the region entry bound"
    );
}

#[test]
fn equivalent_exception_cleanups_share_code_but_keep_each_edges_values() {
    for observations in [2, 32] {
        for forward_borrowed_twice in [false, true] {
            let mut func = TirFunction::new(
                "parameterized_exception_cleanup".into(),
                if forward_borrowed_twice {
                    vec![TirType::DynBox]
                } else {
                    vec![]
                },
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let entry = func.entry_block;
            let forwarded = if forward_borrowed_twice {
                vec![func.blocks[&entry].args[0].id; 2]
            } else {
                vec![]
            };
            let handler = func.fresh_block();
            func.label_id_map.insert(handler.0, 99);
            let handler_args: Vec<_> = (0..forwarded.len()).map(|_| owned(&mut func)).collect();
            let mut handler_ops: Vec<_> = handler_args.iter().copied().map(consume).collect();
            handler_ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            func.blocks
                .insert(handler, block(handler, handler_args, handler_ops, done()));
            let mut sources = vec![entry];
            for _ in 1..observations {
                let id = func.fresh_block();
                func.blocks.insert(id, block(id, vec![], vec![], done()));
                sources.push(id);
            }
            for (index, &source) in sources.iter().enumerate() {
                let first = owned(&mut func);
                let second = owned(&mut func);
                let body = func.blocks.get_mut(&source).unwrap();
                body.ops = vec![
                    produce(first),
                    produce(second),
                    observe(99, forwarded.clone()),
                    consume(first),
                    consume(second),
                ];
                body.terminator = sources
                    .get(index + 1)
                    .map_or_else(done, |&next| jump(next, vec![]));
            }

            insert(&mut func);
            let labels: HashSet<_> = sources
                .iter()
                .flat_map(|source| {
                    func.blocks[source].ops.iter().filter_map(|op| {
                        if op.opcode == OpCode::CheckException {
                            match op.attrs.get("value") {
                                Some(AttrValue::Int(label)) => Some(*label),
                                _ => panic!("observation lost its handler"),
                            }
                        } else {
                            None
                        }
                    })
                })
                .collect();
            assert_eq!(
                labels.len(),
                1,
                "SSA names must not duplicate identical cleanup code"
            );
            let first_object = usize::from(forward_borrowed_twice);
            let normal: Vec<_> = (first_object..first_object + 2 * observations)
                .map(Event::Freed)
                .collect();
            assert_eq!(trace(&func, 0, &[]), normal);
            for failure in 0..observations {
                let mut choices = vec![false; failure];
                choices.push(true);
                let mut expected = normal[..2 * failure].to_vec();
                expected.extend([
                    Event::Freed(first_object + 2 * failure + 1),
                    Event::Freed(first_object + 2 * failure),
                    Event::Marker,
                ]);
                assert_eq!(trace(&func, 0, &choices), expected);
            }
        }
    }
}

/// Growing construction prefixes share exact-value unwind suffixes. Every
/// possible failure must free only initialized owners, newest first; handler
/// payload retains remain per-entry, including repeated borrowed arguments.
#[test]
fn construction_prefix_cleanup_is_linear_and_preserves_every_failure_path() {
    for count in [2, 8, 64] {
        for forward_borrowed_twice in [false, true] {
            let mut func = TirFunction::new(
                "construction_prefix_cleanup".into(),
                if forward_borrowed_twice {
                    vec![TirType::DynBox]
                } else {
                    vec![]
                },
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let entry = func.entry_block;
            let forwarded = if forward_borrowed_twice {
                vec![func.blocks[&entry].args[0].id; 2]
            } else {
                vec![]
            };
            let handler = func.fresh_block();
            func.label_id_map.insert(handler.0, 91);
            let handler_args: Vec<_> = (0..forwarded.len()).map(|_| owned(&mut func)).collect();
            let mut handler_ops: Vec<_> = handler_args.iter().copied().map(consume).collect();
            handler_ops.push(op(OpCode::WarnStderr, vec![], vec![]));
            func.blocks
                .insert(handler, block(handler, handler_args, handler_ops, done()));
            let values: Vec<_> = (0..count).map(|_| owned(&mut func)).collect();
            let mut ops = vec![observe(91, forwarded.clone())];
            for &value in &values {
                ops.push(produce(value));
                ops.push(observe(91, forwarded.clone()));
            }
            ops.extend(values.iter().copied().map(consume));
            func.blocks.get_mut(&entry).unwrap().ops = ops;
            func.blocks.get_mut(&entry).unwrap().terminator = done();
            insert(&mut func);
            super::point_availability::assert_rc_operands_available(&func);
            let op_count: usize = func.blocks.values().map(|block| block.ops.len()).sum();
            assert!(
                op_count <= 8 * count + 16,
                "quadratic cleanup code: {op_count}"
            );
            let payload_count: usize = func.blocks[&entry]
                .ops
                .iter()
                .filter(|op| op.opcode == OpCode::CheckException)
                .map(|op| op.operands.len())
                .sum();
            assert!(
                payload_count <= (forwarded.len() + 1) * (count + 1),
                "quadratic exceptional transport: {payload_count}"
            );
            let first_object = usize::from(forward_borrowed_twice);
            let normal: Vec<_> = (first_object..first_object + count)
                .map(Event::Freed)
                .collect();
            assert_eq!(trace(&func, 0, &[]), normal);
            for initialized in 0..=count {
                let mut choices = vec![false; initialized];
                choices.push(true);
                let mut expected: Vec<_> = (first_object..first_object + initialized)
                    .rev()
                    .map(Event::Freed)
                    .collect();
                expected.push(Event::Marker);
                assert_eq!(
                    trace(&func, 0, &choices),
                    expected,
                    "failure after {initialized} of {count} initializations"
                );
            }
        }
    }
}
