use super::*;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::call_graph::CallGraph;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn canonical_function_bytes(function: &TirFunction) -> Vec<u8> {
    crate::tir::serialize::serialize_tir_function(function)
        .expect("test TIR function must serialize canonically")
}

fn only_candidate(poll: &TirFunction, caller: &TirFunction) -> FusionCandidate {
    let module = TirModule {
        name: "candidate".into(),
        functions: vec![poll.clone(), caller.clone()],
    };
    let call_graph = CallGraph::build(&module);
    let polls = std::collections::HashMap::from([(poll.name.clone(), poll.clone())]);
    collect_fusion_candidates(caller, &polls, &call_graph)
        .into_iter()
        .next()
        .expect("fixture must expose one fusion candidate")
}

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
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

/// Build a `counter(n)`-shaped single-yield-in-loop generator poll:
///   entry: i=0 (closure_store 56); br header
///   header: i=load56; n=load48; cond = i<n; not; br test
///   test: cond_br not -> exhausted, body
///   body: x = load56; pair=(x,false); state_yield pair,5;
///         (post) i2 = load56 + 1; closure_store 56, i2; br header
///   exhausted: closure_store 16 true; ret (None,True)
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
    // body: x=load56; pair=(x,false); yield; post: i2=load56+1; store56; br header
    let x = f.fresh_value();
    f.value_types.insert(x, TirType::DynBox);
    let falsev = f.fresh_value();
    f.value_types.insert(falsev, TirType::Bool);
    let pair = f.fresh_value();
    f.value_types.insert(pair, TirType::DynBox);
    let i_b = f.fresh_value();
    f.value_types.insert(i_b, TirType::DynBox);
    let one = const_int(&mut f, 1);
    let i2 = f.fresh_value();
    f.value_types.insert(i2, TirType::DynBox);
    let pair_op = op(OpCode::BuildTuple, vec![x, falsev], vec![pair]);
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![x], 56),
                {
                    let mut o = op(OpCode::ConstBool, vec![], vec![falsev]);
                    o.attrs.insert("value".into(), AttrValue::Bool(false));
                    o
                },
                pair_op,
                op_v(OpCode::StateYield, vec![pair], vec![], 5),
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
    // exhausted: store closed; ret (None, True)
    let none_v = f.fresh_value();
    f.value_types.insert(none_v, TirType::None);
    let true_v = f.fresh_value();
    f.value_types.insert(true_v, TirType::Bool);
    let donepair = f.fresh_value();
    f.value_types.insert(donepair, TirType::DynBox);
    let dp = op(OpCode::BuildTuple, vec![none_v, true_v], vec![donepair]);
    f.blocks.insert(
        exhausted,
        TirBlock {
            id: exhausted,
            args: vec![],
            ops: vec![
                op(OpCode::ConstNone, vec![], vec![none_v]),
                {
                    let mut o = op(OpCode::ConstBool, vec![], vec![true_v]);
                    o.attrs.insert("value".into(), AttrValue::Bool(true));
                    o
                },
                op_v(OpCode::ClosureStore, vec![ValueId(0), true_v], vec![], 16),
                dp,
            ],
            terminator: Terminator::Return {
                values: vec![donepair],
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

/// Build a consumer: `for x in counter(5): acc = acc + x` at function scope.
///   entry: n5=5; g=AllocTask(counter_poll, args=[n5], size=64);
///          it=iter(g); isnone=is(it,None); br guard
///   guard: cond_br isnone -> raise : loophdr
///   raise: ... br loophdr  (dead)
///   loophdr: br cond
///   cond: pair=iter_next(it); done=Index(pair,1); cond_br done -> exit : body
///   body: elem=Index(pair,0); ... ; br loophdr
///   exit: ret
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

    let n5 = const_int(&mut f, 5);
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
        let mut at = op(OpCode::AllocTask, vec![n5], vec![g]);
        at.attrs
            .insert("s_value".into(), AttrValue::Str("counter_poll".into()));
        at.attrs
            .insert("task_kind".into(), AttrValue::Str("generator".into()));
        at.attrs.insert("value".into(), AttrValue::Int(64));
        e.ops.push(at);
        let mut iter = op(OpCode::Copy, vec![g], vec![it]);
        iter.attrs
            .insert("_original_kind".into(), AttrValue::Str("iter".into()));
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
    let pair = f.fresh_value();
    f.value_types.insert(pair, TirType::DynBox);
    let one_c = const_int(&mut f, 1);
    let done = f.fresh_value();
    f.value_types.insert(done, TirType::Bool);
    f.blocks.insert(
        condb,
        TirBlock {
            id: condb,
            args: vec![],
            ops: vec![
                op(OpCode::IterNext, vec![it], vec![pair]),
                op_v(OpCode::ConstInt, vec![], vec![one_c], 1),
                {
                    let mut o = op(OpCode::Index, vec![pair, one_c], vec![done]);
                    o.attrs
                        .insert("container_type".into(), AttrValue::Str("tuple".into()));
                    o
                },
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
    let zero_c = const_int(&mut f, 0);
    let elem = f.fresh_value();
    f.value_types.insert(elem, TirType::DynBox);
    let elem_use = f.fresh_value();
    f.value_types.insert(elem_use, TirType::DynBox);
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                op_v(OpCode::ConstInt, vec![], vec![zero_c], 0),
                {
                    let mut o = op(OpCode::Index, vec![pair, zero_c], vec![elem]);
                    o.attrs
                        .insert("container_type".into(), AttrValue::Str("tuple".into()));
                    o
                },
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
    f
}

/// A poll with no Python callback sites: fresh tuples, a yield, and a done
/// return. The looping variant requires a latch observation but omits it, so
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
    let pending = f.fresh_value();
    let pair = f.fresh_value();
    let done = f.fresh_value();
    let done_pair = f.fresh_value();
    let boolean = |value: bool, result: ValueId| {
        let mut operation = op(OpCode::ConstBool, vec![], vec![result]);
        operation
            .attrs
            .insert("value".into(), AttrValue::Bool(value));
        operation
    };
    let ops = vec![
        op(OpCode::ConstNone, vec![], vec![none]),
        boolean(false, pending),
        op(OpCode::BuildTuple, vec![none, pending], vec![pair]),
        op_v(OpCode::StateYield, vec![pair], vec![], 5),
        boolean(true, done),
        op(OpCode::BuildTuple, vec![none, done], vec![done_pair]),
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
        block.terminator = Terminator::Return {
            values: vec![done_pair],
        };
    }
    crate::tir::type_refine::refine_types(&mut f);
    crate::tir::verify::verify_function(&f).expect("constant poll must be valid TIR");
    f
}

#[test]
fn fusion_rejects_unmaterialized_poll_before_mutating_the_module() {
    let unprepared_poll = constant_poll(true);
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![unprepared_poll, consumer()],
    };
    let before: Vec<_> = module
        .functions
        .iter()
        .map(crate::tir::printer::print_function)
        .collect();
    let cg = CallGraph::build(&module);
    assert!(is_poll_fusable(&module.functions[0], &cg));
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

    // Strip the yield's pair operand. The clone discovers this malformed yield
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
        !apply_fusion(&mut caller, &poll, &candidate, &mut stats),
        "a yield without its pair must conservatively reject fusion"
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
    assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
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
        assert!(!apply_fusion(&mut caller, &poll, &candidate, &mut stats));
        assert_eq!(canonical_function_bytes(&caller), before);
        assert_eq!(stats, FusionStats::default());
    }
}

#[test]
fn fusion_retires_latch_role_without_erasing_synchronous_exception_transfer() {
    for opcode in [OpCode::Div, OpCode::FloorDiv, OpCode::Mod, OpCode::Call] {
        for split_latch in [false, true] {
            let poll = counter_poll();
            let mut caller = consumer();
            let candidate = only_candidate(&poll, &caller);
            let body = candidate.body_block;
            let original = {
                let ops = &mut caller.blocks.get_mut(&body).unwrap().ops;
                let zero = ops[0].results[0];
                ops[2].opcode = opcode;
                if opcode == OpCode::Call {
                    ops[2].attrs.insert(
                        "s_value".into(),
                        AttrValue::Str("fixture_external_call".into()),
                    );
                } else {
                    ops[2].operands.push(zero);
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
            assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
            let observation = caller.blocks[&body].ops.last().unwrap();
            let mut expected = original;
            if opcode != OpCode::Call {
                expected.clear_async_work_poll();
            }
            assert_eq!(observation.opcode, OpCode::CheckException, "{opcode:?}");
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
                        && op.attrs.get("value") == Some(&AttrValue::Int(90))
                }),
                "{opcode:?}, split={split_latch}"
            );
        }
    }
}

#[test]
fn callback_poll_is_refused_by_module_driver_without_mutation() {
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![counter_poll(), consumer()],
    };
    let before: Vec<_> = module
        .functions
        .iter()
        .map(canonical_function_bytes)
        .collect();
    let cg = CallGraph::build(&module);
    assert!(cg.has_opaque_call("counter_poll"));
    assert!(!is_poll_fusable(&module.functions[0], &cg));
    let stats = run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast());
    assert_eq!(stats, FusionStats::default());
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
fn callback_free_poll_is_fused_by_module_driver() {
    let mut caller = consumer();
    for operation in caller.blocks.values_mut().flat_map(|block| &mut block.ops) {
        if operation.opcode == OpCode::AllocTask {
            operation
                .attrs
                .insert("s_value".into(), AttrValue::Str("constant_poll".into()));
        }
    }
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![constant_poll(false), caller],
    };
    let cg = CallGraph::build(&module);
    assert!(!cg.has_opaque_call("constant_poll"));
    assert!(is_poll_fusable(&module.functions[0], &cg));
    assert!(super::super::async_work_poll::is_materialized(
        &module.functions[0]
    ));
    let stats = run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast());
    assert_eq!(stats.frames_elided, 1);
    assert_eq!(stats.yield_sites_spliced, 1);
    assert_eq!(stats.changed_functions, vec!["consumer".to_string()]);
    let caller = &module.functions[1];
    assert!(
        !caller
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|operation| matches!(
                operation.opcode,
                OpCode::AllocTask | OpCode::StateYield | OpCode::IterNext
            ))
    );
    crate::tir::verify::verify_function(caller).expect("module-driver fused caller must verify");
}

#[test]
fn single_yield_in_loop_recognized_and_spliced() {
    // The structural splice preserves the latch and ownership. Module-level
    // eligibility separately refuses this callback-bearing poll above.
    let poll = counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
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
    // No AllocTask / StateYield / IterNext remain.
    let has = |op: OpCode| {
        cons.blocks
            .values()
            .any(|b| b.ops.iter().any(|o| o.opcode == op))
    };
    assert!(!has(OpCode::AllocTask), "AllocTask must be deleted");
    assert!(!has(OpCode::StateYield), "StateYield must be gone");
    assert!(!has(OpCode::IterNext), "IterNext must be deleted");
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
    assert_eq!(
        poll_sites.len(),
        1,
        "fusion must preserve exactly the generator loop's pre-authored latch transfer"
    );
    let (poll_block, poll) = poll_sites[0];
    let AttrValue::Int(poll_label) = poll.attrs["value"] else {
        panic!("fused latch poll must retain a remapped exception label")
    };
    assert!(
        cons.label_id_map.values().any(|label| *label == poll_label),
        "fused latch poll's remapped label must resolve in the caller"
    );
    let mut analyses = crate::tir::analysis::AnalysisManager::new();
    let loops = analyses
        .get::<crate::tir::analysis::LoopForest>(cons)
        .clone();
    assert!(
        loops.headers.iter().any(|header| {
            loops.bodies[header].contains(&poll_block)
                && cons.blocks[&poll_block].terminator.has_successor(*header)
        }),
        "the preserved marker must remain on the actual fused backedge"
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
        assert!(!apply_fusion(&mut caller, &poll, &candidate, &mut stats));
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
    assert!(!apply_fusion(&mut caller, &poll, &candidate, &mut stats));
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
    assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
    assert_eq!(stats.frames_elided, 1);
    crate::tir::verify::verify_function(&caller).expect("declared header rewiring must verify");
}

/// Build `def g(a): yield a`, a straight-line single-yield generator poll whose
/// yield and return share its entry block:
///   entry: switch; x = load48; pair = (x, false); state_yield pair, 5;
///          (post) closed = true; ret (None, True)
fn echo_poll() -> TirFunction {
    let mut f = TirFunction::new(
        "echo_poll".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Value,
    );
    let x = f.fresh_value();
    f.value_types.insert(x, TirType::DynBox);
    let falsev = f.fresh_value();
    f.value_types.insert(falsev, TirType::Bool);
    let pair = f.fresh_value();
    f.value_types.insert(pair, TirType::DynBox);
    let none_v = f.fresh_value();
    f.value_types.insert(none_v, TirType::None);
    let true_v = f.fresh_value();
    f.value_types.insert(true_v, TirType::Bool);
    let done_pair = f.fresh_value();
    f.value_types.insert(done_pair, TirType::DynBox);
    let tuple =
        |operands: Vec<ValueId>, result: ValueId| op(OpCode::BuildTuple, operands, vec![result]);
    let boolean = |value: bool, result: ValueId| {
        let mut o = op(OpCode::ConstBool, vec![], vec![result]);
        o.attrs.insert("value".into(), AttrValue::Bool(value));
        o
    };
    let entry = f.entry_block;
    let e = f.blocks.get_mut(&entry).unwrap();
    e.ops = vec![
        op(OpCode::StateSwitch, vec![], vec![]),
        op_v(OpCode::ClosureLoad, vec![ValueId(0)], vec![x], 48),
        boolean(false, falsev),
        tuple(vec![x, falsev], pair),
        op_v(OpCode::StateYield, vec![pair], vec![], 5),
        op(OpCode::ConstNone, vec![], vec![none_v]),
        boolean(true, true_v),
        op_v(OpCode::ClosureStore, vec![ValueId(0), true_v], vec![], 16),
        tuple(vec![none_v, true_v], done_pair),
    ];
    e.terminator = Terminator::Return {
        values: vec![done_pair],
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

/// The op defining the first element of the pair the fused element comes from:
/// the read the poll yielded.
fn yielded_read<'f>(func: &'f TirFunction, candidate: &FusionCandidate) -> &'f TirOp {
    let index = definition(func, candidate.elem_val);
    assert_eq!(index.opcode, OpCode::Index);
    let pair = definition(func, index.operands[0]);
    definition(func, pair.operands[0])
}

/// Whether `operation` is an owned alias, a copy whose result holds a reference
/// of its own.
fn is_owned_alias(operation: &TirOp) -> bool {
    operation.opcode == OpCode::Copy
        && operation.attrs.get("_original_kind") == Some(&AttrValue::Str("binding_alias".into()))
}

/// The fused element is the result of `Index(pair, 0)`, which the runtime
/// returns owned, as it did the eliminated `IterNext` pair's element. Fusion
/// places no reference operation, and the drop plane releases the element
/// once, after the consumer body's read, and retains it nowhere.
#[test]
fn fused_element_is_owned_once_by_its_index() {
    // Yield a string: a heap element whatever the counter's facts prove.
    let mut poll = counter_poll();
    let text = poll.fresh_value();
    poll.value_types.insert(text, TirType::DynBox);
    let block = yield_block(&mut poll);
    let pair = block
        .ops
        .iter_mut()
        .find(|operation| operation.opcode == OpCode::BuildTuple)
        .expect("the yield builds its pair");
    pair.operands[0] = text;
    let mut string = op(OpCode::ConstStr, vec![], vec![text]);
    string
        .attrs
        .insert("s_value".into(), AttrValue::Str("x".into()));
    block.ops.insert(0, string);

    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
    assert!(
        !caller
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|operation| matches!(operation.opcode, OpCode::IncRef | OpCode::DecRef)),
        "fusion places no reference operation"
    );
    assert_eq!(
        definition(&caller, candidate.elem_val).opcode,
        OpCode::Index
    );

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
    // [x, false, pair, yield, i, one, i + 1, store, check]
    //   -> [i, one, i + 1, store, x, false, pair, yield, check]
    let ops = std::mem::take(&mut block.ops);
    block.ops = [4, 5, 6, 7, 0, 1, 2, 3, 8]
        .iter()
        .map(|&index| ops[index].clone())
        .collect();

    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
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

    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);
    let mut stats = FusionStats::default();
    assert!(
        apply_fusion(&mut caller, &poll, &candidate, &mut stats),
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
    assert_eq!(
        joined.args.iter().map(|arg| arg.id).collect::<Vec<_>>(),
        read.operands,
        "the read copies its block's join argument"
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
    ops[allocation].operands = vec![text];
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
    assert!(apply_fusion(&mut caller, &poll, &candidate, &mut stats));
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
