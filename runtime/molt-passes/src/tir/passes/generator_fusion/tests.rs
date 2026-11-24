use super::*;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn canonical_function_bytes(function: &TirFunction) -> Vec<u8> {
    // Rows named by index cover every callee: equal bodies, equal bytes.
    let rows = function
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(|op| op.callee)
        .map(|id| id.index() + 1)
        .max()
        .unwrap_or(0);
    let names: Vec<String> = (0..rows).map(|index| format!("row{index}")).collect();
    let table = molt_ir::FunctionTable::from_rows(names.iter().map(|name| {
        (
            name.as_str(),
            molt_ir::function_table::FunctionRowKind::Body,
        )
    }));
    crate::tir::serialize::serialize_tir_function(function, &table)
        .expect("test TIR function must serialize canonically")
}

fn only_candidate(poll: &TirFunction, caller: &TirFunction) -> FusionCandidate {
    let module = TirModule::decode_spelled("candidate", vec![poll.clone(), caller.clone()]);
    let call_graph = crate::tir::call_graph::named::NamedCallGraph::build(&module);
    let polls = molt_ir::deterministic_hash::HashMap::from_iter([(
        module.ids[0],
        module.functions[0].clone(),
    )]);
    collect_fusion_candidates(&module.functions[1], &polls, &call_graph)
        .into_iter()
        .next()
        .expect("fixture must expose one fusion candidate")
}

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::default(),
        source_span: None,
    }
}
fn op_v(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>, value: i64) -> TirOp {
    let mut o = op(opcode, operands, results);
    o.attrs.insert("value".into(), AttrValue::Int(value));
    o
}
/// Allocate a fresh i64-typed value id for a constant. The matching
/// `ConstInt` op (carrying `value`) is emitted separately by the caller; the
/// `value` argument documents which constant this id stands for.
fn const_int(f: &mut TirFunction, value: i64) -> ValueId {
    let _ = value;
    let id = f.fresh_value();
    f.value_types.insert(id, TirType::I64);
    id
}

/// The completion report a generator body stores before its normal return
/// (`molt_codegen_abi::GENERATOR_RESUME_RETURNED`).
fn completion_report() -> TirOp {
    let mut report = op(OpCode::StateSet, vec![], vec![]);
    report
        .attrs
        .insert("value".into(), AttrValue::Int(i64::MIN));
    report
}

/// Build a `counter(n)`-shaped single-yield-in-loop generator poll:
///   entry: i=0 (closure_store 56); br header
///   header: i=load56; n=load48; cond = i<n; not; br test
///   test: cond_br not -> exhausted, body
///   body: x = load56; state_yield x,5;
///         (post) i2 = load56 + 1; closure_store 56, i2; br header
///   exhausted: state_set RETURNED; ret None
fn counter_poll() -> TirFunction {
    let mut f = TirFunction::new(
        "counter_poll".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Value,
    );
    // %0 = self
    let header = f.fresh_block();
    let test = f.fresh_block();
    let body = f.fresh_block();
    let exhausted = f.fresh_block();
    let exception_exit = f.fresh_block();

    // entry
    let zero = const_int(&mut f, 0);
    {
        let e = f.blocks.get_mut(&f.entry_block).unwrap();
        e.ops.push(op_v(OpCode::ConstInt, vec![], vec![zero], 0));
        e.ops.push(op_v(
            OpCode::ClosureStore,
            vec![ValueId(0), zero],
            vec![],
            56,
        ));
        e.ops.push(op(OpCode::StateSwitch, vec![], vec![]));
        e.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };
    }
    // header: load i, load n, cmp
    let i_h = f.fresh_value();
    f.value_types.insert(i_h, TirType::DynBox);
    let n_h = f.fresh_value();
    f.value_types.insert(n_h, TirType::DynBox);
    let cond = f.fresh_value();
    f.value_types.insert(cond, TirType::Bool);
    let notc = f.fresh_value();
    f.value_types.insert(notc, TirType::Bool);
    f.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![
                op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![i_h], 56),
                op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![n_h], 48),
                op(OpCode::Lt, vec![i_h, n_h], vec![cond]),
                op(OpCode::Not, vec![cond], vec![notc]),
            ],
            terminator: Terminator::Branch {
                target: test,
                args: vec![],
            },
        },
    );
    // test: cond_br not -> exhausted : body
    f.blocks.insert(
        test,
        TirBlock {
            id: test,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: notc,
                then_block: exhausted,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    // body: x=load56; yield x; post: i2=load56+1; store56; br header
    let x = f.fresh_value();
    f.value_types.insert(x, TirType::DynBox);
    let i_b = f.fresh_value();
    f.value_types.insert(i_b, TirType::DynBox);
    let one = const_int(&mut f, 1);
    let i2 = f.fresh_value();
    f.value_types.insert(i2, TirType::DynBox);
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![x], 56),
                op_v(OpCode::StateYield, vec![x], vec![], 5),
                op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![i_b], 56),
                op_v(OpCode::ConstInt, vec![], vec![one], 1),
                op(OpCode::Add, vec![i_b, one], vec![i2]),
                op_v(OpCode::ClosureStore, vec![ValueId(0), i2], vec![], 56),
                {
                    let mut poll = op_v(OpCode::CheckException, vec![], vec![], 91);
                    poll.mark_async_work_poll();
                    poll
                },
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );
    // exhausted: report completion; ret None
    let none_v = f.fresh_value();
    f.value_types.insert(none_v, TirType::None);
    f.blocks.insert(
        exhausted,
        TirBlock {
            id: exhausted,
            args: vec![],
            ops: vec![
                op(OpCode::ConstNone, vec![], vec![none_v]),
                completion_report(),
            ],
            terminator: Terminator::Return {
                values: vec![none_v],
            },
        },
    );
    f.label_id_map.insert(exception_exit.0, 91);
    f.blocks.insert(
        exception_exit,
        TirBlock {
            id: exception_exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f
}

/// The label of the consumer iteration's Origin check: its handler.
const ITERATION_HANDLER_LABEL: i64 = 92;

/// Build a consumer: `for x in counter(5): ...` at function scope.
///   entry: n5=5; g=AllocTask(counter_poll, function, args=[n5], size=64);
///          it=iter(g); isnone=is(it,None); br guard
///   guard: cond_br isnone -> exit : loophdr
///   loophdr: br cond
///   cond: elem, done = iter_next_unboxed(it); check (Origin) -> handler;
///         cond_br done -> exit : body
///   body: use elem; poll; br loophdr
///   exit: ret
///   handler: ret (an exception pending)
fn consumer() -> TirFunction {
    let mut f = TirFunction::new(
        "consumer".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let guard = f.fresh_block();
    let loophdr = f.fresh_block();
    let condb = f.fresh_block();
    let body = f.fresh_block();
    let exit = f.fresh_block();
    let handler = f.fresh_block();

    let n5 = const_int(&mut f, 5);
    // The generator function (`alloc_task_operands`): the frame context its
    // fused activation's frame states name.
    let function = f.fresh_value();
    f.value_types.insert(function, TirType::DynBox);
    let g = f.fresh_value();
    f.value_types.insert(g, TirType::DynBox);
    let it = f.fresh_value();
    f.value_types.insert(it, TirType::DynBox);
    let nonev = f.fresh_value();
    f.value_types.insert(nonev, TirType::None);
    let isnone = f.fresh_value();
    f.value_types.insert(isnone, TirType::Bool);
    {
        let e = f.blocks.get_mut(&f.entry_block).unwrap();
        e.ops.push(op_v(OpCode::ConstInt, vec![], vec![n5], 5));
        e.ops.push(op(OpCode::ConstNone, vec![], vec![function]));
        let mut at = op(OpCode::AllocTask, vec![function, n5], vec![g]);
        at.attrs
            .insert("s_value".into(), AttrValue::Str("counter_poll".into()));
        at.attrs
            .insert("task_kind".into(), AttrValue::Str("generator".into()));
        at.attrs.insert("value".into(), AttrValue::Int(64));
        e.ops.push(at);
        let mut iter = op(OpCode::Copy, vec![g], vec![it]);
        iter.origin = Some(molt_ir::OpKind::Iter);
        e.ops.push(iter);
        e.ops.push(op(OpCode::ConstNone, vec![], vec![nonev]));
        e.ops.push(op(OpCode::Is, vec![it, nonev], vec![isnone]));
        e.terminator = Terminator::Branch {
            target: guard,
            args: vec![],
        };
    }
    f.blocks.insert(
        guard,
        TirBlock {
            id: guard,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: isnone,
                then_block: exit,
                then_args: vec![],
                else_block: loophdr,
                else_args: vec![],
            },
        },
    );
    f.blocks.insert(
        loophdr,
        TirBlock {
            id: loophdr,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: condb,
                args: vec![],
            },
        },
    );
    let elem = f.fresh_value();
    f.value_types.insert(elem, TirType::DynBox);
    let done = f.fresh_value();
    f.value_types.insert(done, TirType::Bool);
    f.blocks.insert(
        condb,
        TirBlock {
            id: condb,
            args: vec![],
            ops: vec![
                op(OpCode::IterNextUnboxed, vec![it], vec![elem, done]),
                origin_check(ITERATION_HANDLER_LABEL),
            ],
            terminator: Terminator::CondBranch {
                cond: done,
                then_block: exit,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    let elem_use = f.fresh_value();
    f.value_types.insert(elem_use, TirType::DynBox);
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                // a trivial use of elem
                op(OpCode::Copy, vec![elem], vec![elem_use]),
                {
                    let mut poll = op_v(OpCode::CheckException, vec![], vec![], 90);
                    poll.mark_async_work_poll();
                    poll
                },
            ],
            terminator: Terminator::Branch {
                target: loophdr,
                args: vec![],
            },
        },
    );
    f.loop_roles
        .insert(loophdr, crate::tir::blocks::LoopRole::LoopHeader);
    f.loop_cond_blocks.insert(loophdr, condb);
    f.loop_pairs.insert(loophdr, exit);
    f.label_id_map.insert(exit.0, 90);
    f.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f.label_id_map.insert(handler.0, ITERATION_HANDLER_LABEL);
    f.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f
}

/// An Origin `CheckException` whose exceptional successor is `label`.
fn origin_check(label: i64) -> TirOp {
    let mut check = op_v(OpCode::CheckException, vec![], vec![], label);
    check.set_exception_edge_kind(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin);
    check
}

/// `counter_poll` with the latch's shape on a real back edge: an Origin
/// check observes the addition, and the poll after it only polls (Unwind).
fn unwind_polled_counter_poll() -> TirFunction {
    let mut poll = counter_poll();
    for block in poll.blocks.values_mut() {
        if let Some(add) = block.ops.iter().position(|op| op.opcode == OpCode::Add) {
            block.ops.insert(add + 1, origin_check(91));
        }
        for op in &mut block.ops {
            if op.is_async_work_poll() {
                op.set_exception_edge_kind(
                    crate::tir::op_kinds_generated::ExceptionEdgeKind::Unwind,
                );
            }
        }
    }
    poll
}

/// A poll with no Python callback sites: a yield and a completed return. The looping variant requires a latch observation but omits it, so
/// preparation admission can be tested without an unrelated recursion refusal.
fn constant_poll(looping: bool) -> TirFunction {
    let mut f = TirFunction::new(
        "constant_poll".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = f.entry_block;
    let body = if looping { f.fresh_block() } else { entry };
    let none = f.fresh_value();
    let ops = vec![
        op(OpCode::ConstNone, vec![], vec![none]),
        op_v(OpCode::StateYield, vec![none], vec![], 5),
        completion_report(),
    ];
    if looping {
        f.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
        f.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops,
                terminator: Terminator::Branch {
                    target: body,
                    args: vec![],
                },
            },
        );
    } else {
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops = ops;
        block.terminator = Terminator::Return { values: vec![none] };
    }
    crate::tir::type_refine::refine_types(&mut f);
    crate::tir::verify::verify_function(&f).expect("constant poll must be valid TIR");
    f
}

#[test]
fn fusion_rejects_unmaterialized_poll_before_mutating_the_module() {
    let unprepared_poll = constant_poll(true);
    let mut module = TirModule::decode_spelled("m", vec![unprepared_poll, consumer()]);
    let before: Vec<_> = module
        .functions
        .iter()
        .map(crate::tir::printer::print_function)
        .collect();
    let cg = crate::tir::call_graph::named::NamedCallGraph::build(&module);
    assert_eq!(
        poll_fusion_refusal(module.ids[0], &module.functions[0], &cg),
        None
    );
    assert!(!super::super::async_work_poll::is_materialized(
        &module.functions[0]
    ));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast())
    }));
    let panic = result.expect_err("unmaterialized target input must fail closed");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("preparation gate must report its original diagnostic");
    assert!(
        message.contains("generator fusion requires post-pipeline async-work observations in poll"),
        "{message}"
    );
    let after: Vec<_> = module
        .functions
        .iter()
        .map(crate::tir::printer::print_function)
        .collect();
    assert_eq!(
        after, before,
        "the preparation gate must precede all mutation"
    );
}

#[test]
fn clone_late_bail_is_byte_identical_including_id_allocators() {
    let mut poll = counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);

    // Strip the yield's operand. The clone discovers this malformed yield
    // only after it has allocated every value/block id and inserted earlier
    // cloned blocks into its staging function.
    for block in poll.blocks.values_mut() {
        for operation in &mut block.ops {
            if operation.opcode == OpCode::StateYield {
                operation.operands.clear();
            }
        }
    }

    let before = canonical_function_bytes(&caller);
    let before_ids = (caller.next_value, caller.next_block);
    let mut stats = FusionStats::default();
    assert!(
        !apply_fusion(&mut caller, &poll, &candidate, &mut stats, true),
        "a yield without its value must conservatively reject fusion"
    );
    assert_eq!(
        canonical_function_bytes(&caller),
        before,
        "late clone rejection must preserve the complete caller artifact"
    );
    assert_eq!(
        (caller.next_value, caller.next_block),
        before_ids,
        "failed staging must not consume deterministic ids"
    );
    assert_eq!(stats, FusionStats::default());
}

#[test]
fn unentered_poll_predecessor_of_a_join_is_pruned() {
    let mut poll = counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);

    // A block the poll never enters branches to its loop header, where the
    // counter slot joins. It holds no slot state: its edge passes placeholders
    // for the join, and the splice prunes it with the other dead blocks.
    let header = match poll.blocks[&poll.entry_block].terminator {
        Terminator::Branch { target, .. } => target,
        ref other => panic!("counter entry must branch to its header, got {other:?}"),
    };
    let unentered = poll.fresh_block();
    poll.blocks.insert(
        unentered,
        TirBlock {
            id: unentered,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );

    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    assert_eq!(stats.frames_elided, 1);
    crate::tir::verify::verify_function(&caller).expect("the fused caller must verify");
}

#[test]
fn fusion_rejects_old_loop_label_and_structural_obligations_before_staging() {
    for opcode in [None, Some(OpCode::CheckException), Some(OpCode::TryEnd)] {
        let poll = counter_poll();
        let mut caller = consumer();
        let candidate = only_candidate(&poll, &caller);
        if let Some(opcode) = opcode {
            caller.label_id_map.insert(candidate.cond_block.0, 12345);
            caller
                .blocks
                .get_mut(&caller.entry_block)
                .unwrap()
                .ops
                .push(op_v(opcode, vec![], vec![], 12345));
        } else {
            caller
                .loop_cond_blocks
                .insert(caller.entry_block, candidate.cond_block);
        }
        let before = canonical_function_bytes(&caller);
        let mut stats = FusionStats::default();
        assert!(!apply_fusion(
            &mut caller,
            &poll,
            &candidate,
            &mut stats,
            true
        ));
        assert_eq!(canonical_function_bytes(&caller), before);
        assert_eq!(stats, FusionStats::default());
    }
}

#[test]
fn fusion_keeps_the_consumer_latch_poll_and_its_exception_transfer() {
    for opcode in [OpCode::Div, OpCode::FloorDiv, OpCode::Mod, OpCode::Call] {
        for split_latch in [false, true] {
            let poll = counter_poll();
            let mut caller = consumer();
            let candidate = only_candidate(&poll, &caller);
            let body = candidate.body_block;
            let zero = const_int(&mut caller, 0);
            let original = {
                let ops = &mut caller.blocks.get_mut(&body).unwrap().ops;
                ops.insert(0, op_v(OpCode::ConstInt, vec![], vec![zero], 0));
                ops[1].opcode = opcode;
                if opcode == OpCode::Call {
                    ops[1].attrs.insert(
                        "s_value".into(),
                        AttrValue::Str("fixture_external_call".into()),
                    );
                } else {
                    ops[1].operands.push(zero);
                }
                let observation = ops.last_mut().unwrap();
                observation.source_span = Some((123, 145));
                observation.clone()
            };
            if split_latch {
                let latch = caller.fresh_block();
                let terminator = std::mem::replace(
                    &mut caller.blocks.get_mut(&body).unwrap().terminator,
                    Terminator::Branch {
                        target: latch,
                        args: vec![],
                    },
                );
                caller.blocks.insert(
                    latch,
                    TirBlock {
                        id: latch,
                        args: vec![],
                        ops: vec![],
                        terminator,
                    },
                );
            }
            let candidate = only_candidate(&poll, &caller);
            let mut stats = FusionStats::default();
            assert!(apply_fusion(
                &mut caller,
                &poll,
                &candidate,
                &mut stats,
                true
            ));
            // The consumer body runs once per fused iteration: its latch poll
            // stays the fused loop's poll, in the consumer's own frame.
            let observation = caller.blocks[&body].ops.last().unwrap();
            let mut expected = original;
            assert_eq!(observation.opcode, OpCode::CheckException, "{opcode:?}");
            let Some(AttrValue::Int(release_label)) = observation.attrs.get("value") else {
                panic!("consumer poll must still transfer exceptional control");
            };
            let release_label = *release_label;
            let release = &caller.blocks[&block_of_label(&caller, release_label)];
            assert!(release.ops.iter().any(|op| {
                op.runtime == Some(molt_ir::runtime_fn_generated::RuntimeFn::SplicedFrameFinish)
            }));
            assert!(
                release
                    .terminator
                    .has_successor(block_of_label(&caller, 90))
            );
            expected
                .attrs
                .insert("value".into(), AttrValue::Int(release_label));
            assert_eq!(
                observation.attrs, expected.attrs,
                "{opcode:?}, split={split_latch}"
            );
            assert_eq!(observation.operands, expected.operands);
            assert_eq!(observation.source_span, expected.source_span);
            assert!(caller.label_id_map.values().any(|label| *label == 90));

            // Only the shared exception oracle may remove a now-unmarked check;
            // all these bodies can raise, so it must preserve the transfer.
            super::super::check_exception_elim::run(&mut caller);
            assert!(
                caller.blocks[&body].ops.iter().any(|op| {
                    op.opcode == OpCode::CheckException
                        && op.attrs.get("value") == Some(&AttrValue::Int(release_label))
                }),
                "{opcode:?}, split={split_latch}"
            );
        }
    }
}

#[test]
fn static_cycle_poll_is_refused_by_module_driver_without_mutation() {
    // A splice copies a body once: only a call cycle makes it unbounded.
    let mut poll = counter_poll();
    let entry = poll.entry_block;
    let mut recursion = op(OpCode::Call, vec![], vec![]);
    recursion
        .attrs
        .insert("s_value".into(), AttrValue::Str("counter_poll".into()));
    poll.blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .insert(0, recursion);
    let mut module = TirModule::decode_spelled("m", vec![poll, consumer()]);
    let before: Vec<_> = module
        .functions
        .iter()
        .map(canonical_function_bytes)
        .collect();
    let cg = crate::tir::call_graph::named::NamedCallGraph::build(&module);
    assert!(cg.in_static_cycle("counter_poll"));
    assert_eq!(
        poll_fusion_refusal(module.ids[0], &module.functions[0], &cg),
        Some(PollRefusal::Recursive)
    );
    let stats = run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast());
    // Nothing fused, and the refusal names its rule.
    assert_eq!(
        stats,
        FusionStats {
            refused_polls: vec![("counter_poll".into(), PollRefusal::Recursive)],
            ..FusionStats::default()
        }
    );
    assert_eq!(
        module
            .functions
            .iter()
            .map(canonical_function_bytes)
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn opaque_call_poll_is_fused_by_module_driver() {
    // The poll's comparison and addition may call Python, so the call graph
    // records an opaque call; that is no cycle, and the poll fuses.
    let mut module = TirModule::decode_spelled("m", vec![counter_poll(), consumer()]);
    let cg = crate::tir::call_graph::named::NamedCallGraph::build(&module);
    assert!(cg.has_opaque_call("counter_poll"));
    assert!(!cg.in_static_cycle("counter_poll"));
    assert_eq!(
        poll_fusion_refusal(module.ids[0], &module.functions[0], &cg),
        None
    );
    let stats = run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast());
    assert_eq!(stats.frames_elided, 1);
    assert_eq!(stats.changed_functions, vec![module.ids[1]]);
    crate::tir::verify::verify_function(&module.functions[1])
        .expect("module-driver fused caller must verify");
}

#[test]
fn callback_free_poll_is_fused_by_module_driver() {
    let mut caller = consumer();
    for operation in caller.blocks.values_mut().flat_map(|block| &mut block.ops) {
        if operation.opcode == OpCode::AllocTask {
            operation
                .attrs
                .insert("s_value".into(), AttrValue::Str("constant_poll".into()));
        }
    }
    let mut module = TirModule::decode_spelled("m", vec![constant_poll(false), caller]);
    let cg = crate::tir::call_graph::named::NamedCallGraph::build(&module);
    assert!(!cg.has_opaque_call("constant_poll"));
    assert_eq!(
        poll_fusion_refusal(module.ids[0], &module.functions[0], &cg),
        None
    );
    assert!(super::super::async_work_poll::is_materialized(
        &module.functions[0]
    ));
    let stats = run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast());
    assert_eq!(stats.frames_elided, 1);
    assert_eq!(stats.yield_sites_spliced, 1);
    assert_eq!(stats.changed_functions, vec![module.ids[1]]);
    let caller = &module.functions[1];
    assert!(
        !caller
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|operation| matches!(
                operation.opcode,
                OpCode::AllocTask | OpCode::StateYield | OpCode::IterNextUnboxed
            ))
    );
    crate::tir::verify::verify_function(caller).expect("module-driver fused caller must verify");
}

#[test]
fn fusion_drops_the_generators_location_stores() {
    // A FrameSetLocation index addresses the generator's location table. The
    // splice must not publish it into the consumer's frame (adversarial review,
    // 2026-11-07: a raise inside a fused generator reported a wrong line or an
    // out-of-range index that dropped the consumer from the traceback).
    const GENERATOR_LOCATION: i64 = 4_242;
    let mut poll = counter_poll();
    yield_block(&mut poll).ops.insert(
        0,
        op_v(OpCode::FrameSetLocation, vec![], vec![], GENERATOR_LOCATION),
    );
    let mut caller = consumer();
    let stores = |func: &TirFunction| -> Vec<i64> {
        func.blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .filter(|op| op.opcode == OpCode::FrameSetLocation)
            .filter_map(|op| match op.attrs.get("value") {
                Some(AttrValue::Int(location)) => Some(*location),
                _ => None,
            })
            .collect()
    };
    let before = stores(&caller);
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    assert_eq!(stats.frames_elided, 1);
    assert!(!stores(&caller).contains(&GENERATOR_LOCATION));
    assert_eq!(stores(&caller), before, "the consumer keeps its own stores");
    crate::tir::verify::verify_function(&caller).expect("fused consumer must verify");
}

#[test]
fn single_yield_in_loop_recognized_and_spliced() {
    // The structural splice preserves the latch and ownership. Module-level
    // eligibility separately refuses this callback-bearing poll above.
    let poll = unwind_polled_counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    let cons = &caller;
    eprintln!(
        "=== fused consumer ===\n{}",
        crate::tir::printer::print_function(cons)
    );
    eprintln!("stats: {:?}", stats);
    assert_eq!(
        stats.frames_elided, 1,
        "the single-yield-in-loop generator must fuse"
    );
    // No AllocTask / StateYield / IterNextUnboxed remain.
    let has = |op: OpCode| {
        cons.blocks
            .values()
            .any(|b| b.ops.iter().any(|o| o.opcode == op))
    };
    assert!(!has(OpCode::AllocTask), "AllocTask must be deleted");
    assert!(!has(OpCode::StateYield), "StateYield must be gone");
    assert!(
        !has(OpCode::IterNextUnboxed),
        "IterNextUnboxed must be deleted"
    );
    let poll_sites: Vec<_> = cons
        .blocks
        .iter()
        .flat_map(|(&block, body)| {
            body.ops
                .iter()
                .filter(|op| op.is_async_work_poll())
                .map(move |op| (block, op))
        })
        .collect();
    // The generator loop's pre-authored latch poll and the consumer's latch
    // poll, which runs once per fused iteration in the consumer's own frame.
    assert_eq!(
        poll_sites.len(),
        2,
        "fusion must preserve the generator's latch poll and the consumer's"
    );
    for (_, poll) in &poll_sites {
        let AttrValue::Int(poll_label) = poll.attrs["value"] else {
            panic!("fused latch poll must retain a remapped exception label")
        };
        assert!(
            cons.label_id_map.values().any(|label| *label == poll_label),
            "fused latch poll's remapped label must resolve in the caller"
        );
    }
    let mut analyses = crate::tir::analysis::AnalysisManager::new();
    let loops = analyses
        .get::<crate::tir::analysis::LoopForest>(cons)
        .clone();
    // The generator's poll services work on a cold path (spliced_frames.md
    // 5.2): its block is entered only when work is pending and rejoins the
    // block that carries the backedge.
    let on_backedge = |block: &crate::tir::blocks::BlockId,
                       header: &crate::tir::blocks::BlockId| {
        cons.blocks[block].terminator.has_successor(*header)
    };
    assert!(
        poll_sites
            .iter()
            .any(|(poll_block, _)| loops.headers.iter().any(|header| {
                loops.bodies[header].contains(poll_block)
                    && match &cons.blocks[poll_block].terminator {
                        Terminator::Branch { target, .. } => {
                            on_backedge(poll_block, header) || on_backedge(target, header)
                        }
                        _ => on_backedge(poll_block, header),
                    }
            })),
        "the preserved marker must remain on the actual fused backedge"
    );
    let serviced = poll_sites.iter().any(|(poll_block, _)| {
        cons.blocks.values().any(|block| {
            matches!(
                &block.terminator,
                Terminator::CondBranch { cond, then_block, .. }
                    if then_block == poll_block
                        && block.ops.iter().any(|op| {
                            op.opcode == OpCode::AsyncWorkPending && op.results == [*cond]
                        })
            )
        })
    });
    assert!(
        serviced,
        "the generator's poll must run only when work is pending"
    );
    crate::tir::verify::verify_function(cons).expect("fused consumer must verify");
}

#[test]
fn fusion_rejects_retained_cond_entries_before_staging() {
    for structural in [false, true] {
        let poll = counter_poll();
        let mut caller = consumer();
        let candidate = only_candidate(&poll, &caller);
        let outside = caller.fresh_block();
        caller.blocks.insert(
            outside,
            TirBlock {
                id: outside,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: candidate.cond_block,
                    args: vec![],
                },
            },
        );
        if structural {
            caller.loop_cond_blocks.insert(caller.entry_block, outside);
        }
        crate::tir::verify::verify_function(&caller)
            .expect("retained condition predecessor must be valid before fusion");
        let before = canonical_function_bytes(&caller);
        let mut stats = FusionStats::default();
        assert!(!apply_fusion(
            &mut caller,
            &poll,
            &candidate,
            &mut stats,
            true
        ));
        assert_eq!(canonical_function_bytes(&caller), before);
        assert_eq!(stats, FusionStats::default());
    }
}

#[test]
fn fusion_rejects_altered_candidate_entry_without_claiming_entry_replacement() {
    let poll = counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    // Deliberately alter the entry after recognition: this is a gate-contract
    // probe, not a claim that this altered function has valid SSA dominance.
    // wire_fused_loop must not retire an entry it does not replace.
    caller.entry_block = candidate.loop_header.unwrap();
    let before = canonical_function_bytes(&caller);
    let mut stats = FusionStats::default();
    assert!(!apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    assert_eq!(canonical_function_bytes(&caller), before);
    assert_eq!(stats, FusionStats::default());
}

#[test]
fn fusion_admits_unreachable_predecessor_to_explicitly_rewired_header() {
    let poll = counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let outside = caller.fresh_block();
    caller.blocks.insert(
        outside,
        TirBlock {
            id: outside,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: candidate.loop_header.unwrap(),
                args: vec![],
            },
        },
    );
    crate::tir::verify::verify_function(&caller)
        .expect("unreachable header predecessor must be valid before fusion");
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    assert_eq!(stats.frames_elided, 1);
    crate::tir::verify::verify_function(&caller).expect("declared header rewiring must verify");
}

/// Build `def g(a): yield a`, a straight-line single-yield generator poll whose
/// yield and return share its entry block:
///   entry: switch; x = load48; state_yield x, 5;
///          (post) state_set RETURNED; ret None
fn echo_poll() -> TirFunction {
    let mut f = TirFunction::new(
        "echo_poll".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Value,
    );
    let x = f.fresh_value();
    f.value_types.insert(x, TirType::DynBox);
    let none_v = f.fresh_value();
    f.value_types.insert(none_v, TirType::None);
    let entry = f.entry_block;
    let e = f.blocks.get_mut(&entry).unwrap();
    e.ops = vec![
        op(OpCode::StateSwitch, vec![], vec![]),
        op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![x], 48),
        op_v(OpCode::StateYield, vec![x], vec![], 5),
        op(OpCode::ConstNone, vec![], vec![none_v]),
        completion_report(),
    ];
    e.terminator = Terminator::Return {
        values: vec![none_v],
    };
    f
}

/// The block of `poll` that yields.
fn yield_block(poll: &mut TirFunction) -> &mut TirBlock {
    poll.blocks
        .values_mut()
        .find(|block| {
            block
                .ops
                .iter()
                .any(|operation| operation.opcode == OpCode::StateYield)
        })
        .expect("the poll yields")
}

/// The op of `func` that defines `value`.
fn definition(func: &TirFunction, value: ValueId) -> &TirOp {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|operation| operation.results.contains(&value))
        .unwrap_or_else(|| panic!("{value:?} has no defining op"))
}

/// The op defining the value the fused element copies: the read the poll
/// yielded.
fn yielded_read<'f>(func: &'f TirFunction, candidate: &FusionCandidate) -> &'f TirOp {
    let element = definition(func, candidate.elem_val);
    assert_eq!(element.opcode, OpCode::Copy);
    definition(func, element.operands[0])
}

/// Whether `operation` is an owned alias, a copy whose result holds a reference
/// of its own.
fn is_owned_alias(operation: &TirOp) -> bool {
    operation.opcode == OpCode::Copy && operation.origin == Some(molt_ir::OpKind::BindingAlias)
}

/// The fused element is a copy of the yielded value that keeps the reference
/// the yield hands its consumer, as the eliminated `IterNextUnboxed` returned its
/// element owned. Fusion places no reference operation, and the drop plane
/// releases the element once, after the consumer body's read, and retains it
/// nowhere.
#[test]
fn fused_element_is_owned_once_by_its_yield() {
    // Yield a string: a heap element whatever the counter's facts prove.
    let mut poll = counter_poll();
    let text = poll.fresh_value();
    poll.value_types.insert(text, TirType::DynBox);
    let block = yield_block(&mut poll);
    let yielded = block
        .ops
        .iter_mut()
        .find(|operation| operation.opcode == OpCode::StateYield)
        .expect("the poll yields");
    yielded.operands[0] = text;
    let mut string = op(OpCode::ConstStr, vec![], vec![text]);
    string
        .attrs
        .insert("s_value".into(), AttrValue::Str("x".into()));
    block.ops.insert(0, string);

    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    assert!(
        !caller
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|operation| matches!(operation.opcode, OpCode::IncRef | OpCode::DecRef)),
        "fusion places no reference operation"
    );
    assert_eq!(definition(&caller, candidate.elem_val).opcode, OpCode::Copy);

    crate::tir::passes::drop_insertion::run(
        &mut caller,
        &mut crate::tir::analysis::AnalysisManager::new(),
    );
    let naming_element = |opcode: OpCode| {
        caller
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .filter(|operation| {
                operation.opcode == opcode && operation.operands.contains(&candidate.elem_val)
            })
            .count()
    };
    assert_eq!(
        (
            naming_element(OpCode::IncRef),
            naming_element(OpCode::DecRef)
        ),
        (0, 1),
        "the element's one reference, released once"
    );
}

/// A read after a store in the same iteration sees the stored value. Moving the
/// counter's step before its yield makes the yielded read copy the store's
/// frame reference to the new sum, not the value the iteration started with.
#[test]
fn promoted_read_after_a_store_sees_the_stored_value() {
    let mut poll = counter_poll();
    let block = yield_block(&mut poll);
    // [x, yield, i, one, i + 1, store, check]
    //   -> [i, one, i + 1, store, x, yield, check]
    let ops = std::mem::take(&mut block.ops);
    block.ops = [2, 3, 4, 5, 0, 1, 6]
        .iter()
        .map(|&index| ops[index].clone())
        .collect();

    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    let read = yielded_read(&caller, &candidate);
    assert_eq!(read.opcode, OpCode::Copy, "{read:?}");
    let stored = definition(&caller, read.operands[0]);
    assert_eq!(
        stored.opcode,
        OpCode::Copy,
        "the store's frame reference: {stored:?}"
    );
    assert_eq!(
        definition(&caller, stored.operands[0]).opcode,
        OpCode::Add,
        "the read sees the stored sum"
    );
}

/// A slot stored on one arm reaches its join as a block argument. The yield
/// block merges the counter slot from an arm that stores it and one that does
/// not, and its read copies the join's argument.
#[test]
fn conditionally_stored_slot_joins_at_a_block_argument() {
    use molt_ir::runtime_fn_generated::RuntimeFn;

    let mut poll = counter_poll();
    let entry = poll.entry_block;
    let zero = poll.blocks[&entry].ops[0].results[0];
    let header = match poll.blocks[&entry].terminator {
        Terminator::Branch { target, .. } => target,
        ref other => panic!("counter entry must branch to its header, got {other:?}"),
    };
    let test = match poll.blocks[&header].terminator {
        Terminator::Branch { target, .. } => target,
        ref other => panic!("counter header must branch to its test, got {other:?}"),
    };
    let (cond, body) = match poll.blocks[&test].terminator {
        Terminator::CondBranch {
            cond, else_block, ..
        } => (cond, else_block),
        ref other => panic!("counter test must branch on its condition, got {other:?}"),
    };
    // test -> choose; choose -> bump | body; bump stores the slot -> body.
    let (choose, bump) = (poll.fresh_block(), poll.fresh_block());
    if let Terminator::CondBranch { else_block, .. } =
        &mut poll.blocks.get_mut(&test).unwrap().terminator
    {
        *else_block = choose;
    }
    poll.blocks.insert(
        choose,
        TirBlock {
            id: choose,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond,
                then_block: bump,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    poll.blocks.insert(
        bump,
        TirBlock {
            id: bump,
            args: vec![],
            ops: vec![op_v(
                OpCode::ClosureStore,
                vec![ValueId(0), zero],
                vec![],
                56,
            )],
            terminator: Terminator::Branch {
                target: body,
                args: vec![],
            },
        },
    );

    // A second, observer-only slot has no initial binding on the other arm.
    let optional_offset = GEN_CONTROL_BYTES + 16;
    poll.blocks.get_mut(&bump).unwrap().ops.push(op_v(
        OpCode::ClosureStore,
        vec![ValueId(0), zero],
        vec![],
        optional_offset,
    ));

    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(
        apply_fusion(&mut caller, &poll, &candidate, &mut stats, true),
        "a conditionally stored slot fuses"
    );
    crate::tir::verify::verify_function(&caller).expect("the fused caller must verify");
    let read = yielded_read(&caller, &candidate);
    assert_eq!(read.opcode, OpCode::Copy, "{read:?}");
    let joined = caller
        .blocks
        .values()
        .find(|block| {
            block
                .ops
                .iter()
                .any(|operation| operation.results == read.results)
        })
        .expect("the read has a block");
    assert_eq!(read.operands, [joined.args[0].id]);
    // The common publication call gets the value and its independently joined
    // presence. Neither None nor a falsey bound value decides whether to bind.
    let mut published = false;
    for op in caller.blocks.values().flat_map(|block| &block.ops) {
        if op.runtime == Some(RuntimeFn::SplicedFrameBind)
            && attr_value_int(definition(&caller, op.operands[1])) == Some(optional_offset)
            && caller.blocks.values().any(|block| {
                block
                    .args
                    .iter()
                    .any(|arg| arg.id == op.operands[3] && arg.ty == TirType::Bool)
            })
        {
            published = true;
        }
    }
    assert!(
        published,
        "a fused snapshot carries the mixed binding's Boolean join"
    );
}

#[test]
fn frame_publication_reads_branch_bindings_without_a_closure_load() {
    use super::slots::{SlotDef, plan_slots};

    let mut poll = TirFunction::new(
        "branch_poll".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = poll.entry_block;
    let (left, right, join) = (poll.fresh_block(), poll.fresh_block(), poll.fresh_block());
    let frame = poll.blocks[&entry].args[0].id;
    let condition = poll.fresh_value();
    poll.value_types.insert(condition, TirType::Bool);
    let body = poll.blocks.get_mut(&entry).unwrap();
    // The frame can escape before either arm. At yield, the surviving
    // reference observes the selected binding even when the poll never loads it.
    body.ops.push(op(OpCode::Call, vec![], vec![]));
    body.ops
        .push(op_v(OpCode::ConstBool, vec![], vec![condition], 1));
    body.terminator = Terminator::CondBranch {
        cond: condition,
        then_block: left,
        then_args: vec![],
        else_block: right,
        else_args: vec![],
    };
    for (id, number) in [(left, 11), (right, 22)] {
        let value = const_int(&mut poll, number);
        poll.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![
                    op_v(OpCode::ConstInt, vec![], vec![value], number),
                    op_v(
                        OpCode::ClosureStore,
                        vec![frame, value],
                        vec![],
                        GEN_CONTROL_BYTES,
                    ),
                ],
                terminator: Terminator::Branch {
                    target: join,
                    args: vec![],
                },
            },
        );
    }
    poll.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![],
            ops: vec![op(OpCode::StateYield, vec![condition], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let plan = plan_slots(&poll, 0).expect("both observer predecessors bind the slot");
    assert!(plan.reads.is_empty());
    assert_eq!(
        plan.entry[&join],
        [Some(SlotDef::Join {
            block: join,
            slot: 0
        })]
    );
    for arm in [left, right] {
        assert_eq!(
            plan.exits[&arm],
            [Some(SlotDef::Stored {
                block: arm,
                index: 1
            })]
        );
    }

    assert!(plan.optional_joins.is_empty());
    poll.blocks.get_mut(&right).unwrap().ops.pop();
    let plan = plan_slots(&poll, 0).expect("a frame can publish a mixed slot");
    assert_eq!(
        plan.optional_joins.into_iter().collect::<Vec<_>>(),
        [(join, 0)]
    );
    assert_eq!(
        plan.exits[&left][0],
        Some(SlotDef::Stored {
            block: left,
            index: 1
        })
    );
    assert_eq!(plan.exits[&right][0], None);
    poll.blocks.get_mut(&entry).unwrap().ops.remove(0);
    let plan = plan_slots(&poll, 0).expect("a yield with no first observer needs no snapshot");
    assert!(plan.joins.is_empty());
    let read = poll.fresh_value();
    poll.value_types.insert(read, TirType::DynBox);
    poll.blocks.get_mut(&join).unwrap().ops.insert(
        0,
        op_v(
            OpCode::ClosureLoad,
            vec![frame],
            vec![read],
            GEN_CONTROL_BYTES,
        ),
    );
    assert!(
        plan_slots(&poll, 0).is_none(),
        "ordinary reads still require every incoming binding"
    );
}

/// A straight-line generator, `def g(a): yield a`, fused over a string. Its
/// parameter slot is the elided frame's own reference to the argument: an
/// owned alias bound in the preheader, so a later rebinding of the caller's
/// name cannot free what the generator still reads. The read keeps the
/// reference the load returned, and the consumer body runs between the yield
/// and the return that shares the yield's block.
#[test]
fn straight_line_parameter_slot_keeps_the_frame_reference() {
    let poll = echo_poll();
    let mut caller = consumer();
    let text = caller.fresh_value();
    caller.value_types.insert(text, TirType::DynBox);
    let entry = caller.entry_block;
    let ops = &mut caller.blocks.get_mut(&entry).unwrap().ops;
    let allocation = ops
        .iter()
        .position(|operation| operation.opcode == OpCode::AllocTask)
        .expect("the consumer allocates its generator");
    let function = ops[allocation].operands[0];
    ops[allocation].operands = vec![function, text];
    ops[allocation]
        .attrs
        .insert("s_value".into(), AttrValue::Str("echo_poll".into()));
    let mut string = op(OpCode::ConstStr, vec![], vec![text]);
    string
        .attrs
        .insert("s_value".into(), AttrValue::Str("x".into()));
    ops.insert(allocation, string);

    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    assert!(
        caller.blocks.contains_key(&candidate.body_block),
        "the consumer body runs"
    );
    let read = yielded_read(&caller, &candidate);
    assert!(
        is_owned_alias(read),
        "the read keeps the load's reference: {read:?}"
    );
    let slot = definition(&caller, read.operands[0]);
    assert!(
        is_owned_alias(slot) && slot.operands == [text],
        "the slot holds the frame's reference to its argument: {slot:?}"
    );
}

// ---------------------------------------------------------------------------
// Spliced frame states (docs/design/foundation/spliced_frames.md)
// ---------------------------------------------------------------------------

fn runtime_of(op: &TirOp) -> Option<molt_ir::runtime_fn_generated::RuntimeFn> {
    op.runtime
}

fn block_of_label(func: &TirFunction, label: i64) -> crate::tir::blocks::BlockId {
    func.label_id_map
        .iter()
        .find_map(|(&block, &defined)| {
            (defined == label).then_some(crate::tir::blocks::BlockId(block))
        })
        .expect("label names a block")
}

/// `counter_poll` with an Origin check after its addition, which may raise,
/// and its latch poll an Origin check too.
fn observable_counter_poll() -> TirFunction {
    let mut poll = counter_poll();
    for block in poll.blocks.values_mut() {
        for op in &mut block.ops {
            if op.opcode == OpCode::CheckException {
                op.set_exception_edge_kind(
                    crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin,
                );
            }
        }
        if let Some(add) = block.ops.iter().position(|op| op.opcode == OpCode::Add) {
            block.ops.insert(add + 1, origin_check(91));
        }
    }
    poll
}

#[test]
fn linearized_poll_resumes_where_its_yield_saves_state() {
    let mut poll = TirFunction::new(
        "resumed_poll".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Value,
    );
    let start = poll.fresh_block();
    let resume = poll.fresh_block();
    let none = poll.fresh_value();
    poll.value_types.insert(none, TirType::None);
    let entry = poll.entry_block;
    poll.blocks.get_mut(&entry).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(8, resume, vec![])],
        default: start,
        default_args: vec![],
    };
    poll.blocks.insert(
        start,
        TirBlock {
            id: start,
            args: vec![],
            ops: vec![
                op(OpCode::ConstNone, vec![], vec![none]),
                op_v(OpCode::StateYield, vec![none], vec![], 8),
            ],
            terminator: Terminator::Unreachable,
        },
    );
    poll.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![],
            ops: vec![completion_report()],
            terminator: Terminator::Return { values: vec![none] },
        },
    );
    let linear = super::linearize::linearize_poll(&poll).expect("one yield, one case");
    assert!(matches!(
        &linear.blocks[&start].terminator,
        Terminator::Branch { target, args } if *target == resume && args.is_empty()
    ));
    assert!(matches!(
        &linear.blocks[&entry].terminator,
        Terminator::Branch { target, args } if *target == start && args.is_empty()
    ));
    // A yield whose state no case resumes is no linear poll.
    let mut unresumed = poll.clone();
    if let Terminator::StateDispatch { cases, .. } =
        &mut unresumed.blocks.get_mut(&entry).unwrap().terminator
    {
        cases[0].0 = 9;
    }
    assert!(super::linearize::linearize_poll(&unresumed).is_none());
}

#[test]
fn observable_poll_keeps_its_frame_on_every_exception_edge() {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    let poll = observable_counter_poll();
    let mut caller = consumer();
    // The body never reads the second argument. It remains a frame binding
    // when the caller observes an exception from the generator.
    let unread = const_int(&mut caller, 42);
    let ops = &mut caller.blocks.get_mut(&caller.entry_block).unwrap().ops;
    let allocation = ops
        .iter()
        .position(|operation| operation.opcode == OpCode::AllocTask)
        .expect("the consumer allocates its generator");
    ops[allocation].operands.push(unread);
    ops[allocation]
        .attrs
        .insert("value".into(), AttrValue::Int(72));
    ops.insert(allocation, op_v(OpCode::ConstInt, vec![], vec![unread], 42));
    let candidate = only_candidate(&poll, &caller);
    let function = candidate.function;
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    crate::tir::verify::verify_function(&caller).expect("fused caller verifies");
    let handler = block_of_label(&caller, ITERATION_HANDLER_LABEL);
    // The poll's Origin check now leaves through a cold block that links the
    // generator's record (its function, then its slot bindings), inserts its
    // entry and reaches the iteration's handler.
    let mut origin_edges = 0;
    for block in caller.blocks.values() {
        for check in block.ops.iter().filter(|op| {
            op.opcode == OpCode::CheckException
                && op.exception_edge_kind()
                    == Some(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin)
        }) {
            let Some(AttrValue::Int(label)) = check.attrs.get("value") else {
                panic!("a check names its successor");
            };
            // The consumer's own checks keep their targets; a generator poll
            // leaves through a block whose records its bracket linked.
            if *label == ITERATION_HANDLER_LABEL || *label == 90 || check.is_async_work_poll() {
                continue;
            }
            let cold = &caller.blocks[&block_of_label(&caller, *label)];
            let calls: Vec<_> = cold.ops.iter().filter_map(runtime_of).collect();
            assert_eq!(
                calls.first(),
                Some(&RuntimeFn::SplicedFrameLink),
                "{}",
                crate::tir::printer::print_function(&caller)
            );
            assert_eq!(calls.last(), Some(&RuntimeFn::SplicedFrameOrigin));
            assert!(calls.contains(&RuntimeFn::SplicedFrameBind));
            let link = cold
                .ops
                .iter()
                .find(|op| op.runtime == Some(RuntimeFn::SplicedFrameLink))
                .unwrap();
            assert_eq!(link.operands[0], function);
            assert!(
                cold.ops.iter().any(|op| {
                    op.runtime == Some(RuntimeFn::SplicedFrameBind)
                        && definition(&caller, op.operands[1]).attrs.get("value")
                            == Some(&AttrValue::Int(GEN_CONTROL_BYTES + 8))
                }),
                "the unread argument still has a frame home"
            );
            let Terminator::Branch { target, args } = &cold.terminator else {
                panic!("the origin must leave through its release landing");
            };
            assert!(args.is_empty());
            let release = &caller.blocks[target];
            assert!(release.ops.iter().any(|op| {
                op.runtime == Some(RuntimeFn::SplicedFrameFinish)
                    && op.operands == [link.operands[2]]
            }));
            assert!(matches!(
                &release.terminator,
                Terminator::Branch { target, args } if *target == handler && args.is_empty()
            ));
            origin_edges += 1;
        }
    }
    assert!(origin_edges > 0, "the poll's Origin edge keeps its frame");
    // A poll with an Origin edge also observes the exception of the operation
    // before it, so it never moves behind the pending-work test.
    for block in caller.blocks.values() {
        if let Terminator::CondBranch {
            cond, then_block, ..
        } = &block.terminator
            && block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::AsyncWorkPending && op.results == [*cond])
        {
            assert!(
                !caller.blocks[then_block].ops.iter().any(|op| {
                    op.is_async_work_poll()
                        && op.exception_edge_kind()
                            == Some(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin)
                }),
                "{}",
                crate::tir::printer::print_function(&caller)
            );
        }
    }
    // The comparison and the addition on untyped slots may call Python: each
    // runs inside the generator's linked record.
    let bracketed = |opcode: OpCode| {
        caller.blocks.values().any(|block| {
            block.ops.iter().enumerate().any(|(index, op)| {
                op.opcode == opcode
                    && block.ops[..index]
                        .iter()
                        .rev()
                        .find(|prior| prior.runtime.is_some())
                        .is_some_and(|prior| {
                            matches!(
                                prior.runtime,
                                Some(RuntimeFn::SplicedFrameLink | RuntimeFn::SplicedFrameBind)
                            )
                        })
                    && block.ops[index + 1..]
                        .iter()
                        .find(|later| later.runtime.is_some())
                        .is_some_and(|later| later.runtime == Some(RuntimeFn::SplicedFrameUnlink))
            })
        })
    };
    assert!(
        bracketed(OpCode::Lt),
        "{}",
        crate::tir::printer::print_function(&caller)
    );
    assert!(bracketed(OpCode::Add));
}

#[test]
fn proven_operations_lose_their_brackets_after_reoptimization() {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    let mut caller = consumer();
    let poll = observable_counter_poll();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    let tti = TargetInfo::native_release_fast();
    let _ = super::super::reoptimize(&mut caller, &tti);
    // The counter starts at 0 and adds 1; its bound is the constant 5. Every
    // operand is an exact int, so no bracketed operation can call Python.
    let pruned = super::super::spliced_frame::prune_brackets(&mut caller);
    assert!(
        pruned > 0,
        "{}",
        crate::tir::printer::print_function(&caller)
    );
    let _ = super::super::reoptimize(&mut caller, &tti);
    crate::tir::verify::verify_function(&caller).expect("pruned caller verifies");
    // The generator's poll keeps its bracket for signal handlers. A yield
    // snapshots the frame only when an observer already materialized it.
    let unlinks = caller
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.runtime == Some(RuntimeFn::SplicedFrameUnlink))
        .count();
    let polls = caller
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.is_async_work_poll())
        .count();
    let snapshots = caller
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::SplicedFrameObserved)
        .count();
    assert!(
        unlinks <= polls + snapshots,
        "{}",
        crate::tir::printer::print_function(&caller)
    );
}

#[test]
fn exposed_poll_with_a_bracket_stays_unfused() {
    // Where a PEP 667 frame-locals writer may run, Python code inside a
    // bracket could rebind the generator's locals, which the bracket's
    // snapshot would drop: the poll stays a heap task (spliced_frames.md 5.1).
    let mut poll = counter_poll();
    poll.attrs.insert(
        crate::tir::function::FRAME_LOCALS_EXPOSED_ATTR.into(),
        AttrValue::Bool(true),
    );
    let mut module = TirModule::decode_spelled("m", vec![poll, consumer()]);
    let unfused = crate::tir::printer::print_function(&module.functions[1]);
    let cg = crate::tir::call_graph::named::NamedCallGraph::build(&module);
    let stats = run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast());
    assert_eq!(stats.frames_elided, 0);
    assert!(stats.changed_functions.is_empty());
    assert!(
        stats
            .refused_polls
            .iter()
            .any(|(name, refusal)| name == "counter_poll" && *refusal == PollRefusal::MutableFrame)
    );
    assert_eq!(
        crate::tir::printer::print_function(&module.functions[1]),
        unfused
    );
}

/// `consumer` whose loop carries an accumulator through its header, and
/// whose iteration check passes it to the handler.
fn accumulating_consumer() -> (TirFunction, ValueId) {
    let mut f = consumer();
    let header = *f.loop_pairs.keys().next().unwrap();
    let cond = f.loop_cond_blocks[&header];
    let handler = block_of_label(&f, ITERATION_HANDLER_LABEL);
    let body = match &f.blocks[&cond].terminator {
        Terminator::CondBranch { else_block, .. } => *else_block,
        _ => unreachable!(),
    };
    let zero = const_int(&mut f, 0);
    let acc = f.fresh_value();
    f.value_types.insert(acc, TirType::DynBox);
    let next = f.fresh_value();
    f.value_types.insert(next, TirType::DynBox);
    let entry = f.entry_block;
    f.blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .insert(0, op_v(OpCode::ConstInt, vec![], vec![zero], 0));
    // Every edge into the header carries the accumulator.
    for block in f.blocks.values_mut() {
        let source = block.id;
        block.terminator.for_each_edge_mut(|target, args| {
            if *target == header {
                args.push(if source == body { next } else { zero });
            }
        });
    }
    f.blocks
        .get_mut(&header)
        .unwrap()
        .args
        .push(crate::tir::values::TirValue {
            id: acc,
            ty: TirType::DynBox,
        });
    let elem = f.blocks[&body].ops[0].operands[0];
    let body_ops = &mut f.blocks.get_mut(&body).unwrap().ops;
    body_ops.insert(1, op(OpCode::Add, vec![acc, elem], vec![next]));
    // The iteration's check hands the accumulator to its handler.
    for op in &mut f.blocks.get_mut(&cond).unwrap().ops {
        if op.opcode == OpCode::CheckException {
            op.operands.push(acc);
        }
    }
    let caught = f.fresh_value();
    f.value_types.insert(caught, TirType::DynBox);
    f.blocks
        .get_mut(&handler)
        .unwrap()
        .args
        .push(crate::tir::values::TirValue {
            id: caught,
            ty: TirType::DynBox,
        });
    (f, acc)
}

#[test]
fn carried_consumer_values_thread_through_the_fused_loop() {
    let (mut caller, acc) = accumulating_consumer();
    crate::tir::verify::verify_function(&caller).expect("fixture verifies");
    let poll = observable_counter_poll();
    let candidate = only_candidate(&poll, &caller);
    assert_eq!(candidate.handler_args, vec![acc]);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(
        &mut caller,
        &poll,
        &candidate,
        &mut stats,
        true
    ));
    crate::tir::verify::verify_function(&caller).expect("fused caller verifies");
    // The generator's own exception edges hand the handler the accumulator
    // that reaches them, as the iteration's check did.
    let handler = block_of_label(&caller, ITERATION_HANDLER_LABEL);
    let into_handler: Vec<&Vec<ValueId>> = caller
        .blocks
        .values()
        .filter_map(|block| match &block.terminator {
            Terminator::Branch { target, args } if *target == handler => Some(args),
            _ => None,
        })
        .collect();
    assert!(!into_handler.is_empty());
    assert!(into_handler.iter().all(|args| args.len() == 1));
}
