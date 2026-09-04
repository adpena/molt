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
    let mut f = TirFunction::new("counter_poll".into(), vec![TirType::DynBox], TirType::None);
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
    let mut pair_op = op(OpCode::Copy, vec![x, falsev], vec![pair]);
    pair_op
        .attrs
        .insert("_original_kind".into(), AttrValue::Str("tuple_new".into()));
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
    let mut dp = op(OpCode::Copy, vec![none_v, true_v], vec![donepair]);
    dp.attrs
        .insert("_original_kind".into(), AttrValue::Str("tuple_new".into()));
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
    let mut f = TirFunction::new("consumer".into(), vec![], TirType::None);
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

#[test]
fn fusion_rejects_unmaterialized_poll_before_mutating_the_module() {
    let mut unprepared_poll = counter_poll();
    for block in unprepared_poll.blocks.values_mut() {
        block.ops.retain(|op| !op.is_async_work_poll());
    }
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
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_generator_fusion(&mut module, &cg, &TargetInfo::native_release_fast())
    }));
    assert!(
        result.is_err(),
        "unmaterialized target input must fail closed"
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

    // Introduce a second non-entry store for slot 56 in a distinct block. The
    // clone discovers this only after it has allocated every value/block id and
    // inserted earlier cloned blocks into its staging function.
    let entry = poll.entry_block;
    let header = match poll.blocks[&entry].terminator {
        Terminator::Branch { target, .. } => target,
        ref other => panic!("counter entry must branch to its header, got {other:?}"),
    };
    let test = match poll.blocks[&header].terminator {
        Terminator::Branch { target, .. } => target,
        ref other => panic!("counter header must branch to its test, got {other:?}"),
    };
    let stored = poll.blocks[&entry].ops[0].results[0];
    poll.blocks.get_mut(&test).unwrap().ops.push(op_v(
        OpCode::ClosureStore,
        vec![ValueId(0), stored],
        vec![],
        56,
    ));

    let before = canonical_function_bytes(&caller);
    let before_ids = (caller.next_value, caller.next_block);
    let mut stats = FusionStats::default();
    assert!(
        !apply_fusion(&mut caller, &poll, &candidate, &mut stats),
        "multi-block slot stores must conservatively reject fusion"
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
fn wire_late_bail_is_byte_identical_including_cfg_and_ids() {
    let mut poll = counter_poll();
    let mut caller = consumer();
    let candidate = only_candidate(&poll, &caller);

    // A third predecessor into the cloned loop header is rejected during wire,
    // after clone insertion and after header phi arguments have been appended.
    // Keep it unreachable so recognition remains irrelevant to this direct
    // transaction test while the wiring surprise is fully representative.
    let header = match poll.blocks[&poll.entry_block].terminator {
        Terminator::Branch { target, .. } => target,
        ref other => panic!("counter entry must branch to its header, got {other:?}"),
    };
    let third_pred = poll.fresh_block();
    poll.blocks.insert(
        third_pred,
        TirBlock {
            id: third_pred,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );

    let before = canonical_function_bytes(&caller);
    let before_ids = (caller.next_value, caller.next_block);
    let mut stats = FusionStats::default();
    assert!(
        !apply_fusion(&mut caller, &poll, &candidate, &mut stats),
        "a third cloned-loop predecessor must conservatively reject fusion"
    );
    assert_eq!(
        canonical_function_bytes(&caller),
        before,
        "late wire rejection must preserve the complete caller CFG and metadata"
    );
    assert_eq!(
        (caller.next_value, caller.next_block),
        before_ids,
        "failed wiring must not consume deterministic ids"
    );
    assert_eq!(stats, FusionStats::default());
}

#[test]
fn single_yield_in_loop_recognized_and_spliced() {
    let mut module = TirModule {
        name: "m".into(),
        functions: vec![counter_poll(), consumer()],
    };
    let cg = CallGraph::build(&module);
    let tti = TargetInfo::native_release_fast();
    let stats = run_generator_fusion(&mut module, &cg, &tti);
    // Dump the consumer for inspection.
    let cons = module
        .functions
        .iter()
        .find(|f| f.name == "consumer")
        .unwrap();
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
