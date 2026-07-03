use super::*;
use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::call_graph::CallGraph;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

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
    crate::tir::verify::verify_function(cons).expect("fused consumer must verify");
}
