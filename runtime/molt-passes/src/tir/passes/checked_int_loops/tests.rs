use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::LoopRole;
use crate::tir::ops::{AttrDict, AttrValue, Dialect};
use crate::tir::verify::verify_function;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp::new(opcode, operands, results)
}

fn const_int(value: i64, result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        attrs,
        ..TirOp::new(OpCode::ConstInt, vec![], vec![result])
    }
}

fn int(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::I64);
    value
}

fn bool_value(func: &mut TirFunction) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, TirType::Bool);
    value
}

fn block(id: BlockId, args: Vec<TirValue>, ops: Vec<TirOp>, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args,
        ops,
        terminator,
    }
}

fn arg(id: ValueId) -> TirValue {
    TirValue {
        id,
        ty: TirType::I64,
    }
}

/// `total = 0; for i in range(1000): if i < 500: total += i; return total`
/// in the shape the lift produces: entry -> header(total, i) -> guard ->
/// body (branching) -> join -> latch -> header; guard exit returns total.
/// With `seed_from_param`, total starts from the (non-exact) parameter.
struct Fixture {
    func: TirFunction,
    exit: BlockId,
}

fn branching_loop(seed_from_param: bool) -> Fixture {
    let mut func = TirFunction::new(
        "checked_loop_fixture".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let param = func.blocks[&func.entry_block].args[0].id;
    let header = func.fresh_block();
    let guard = func.fresh_block();
    let body = func.fresh_block();
    let then_block = func.fresh_block();
    let join = func.fresh_block();
    let exit = func.fresh_block();

    let n = int(&mut func);
    let half = int(&mut func);
    let zero = int(&mut func);
    let one = int(&mut func);
    let total = int(&mut func);
    let i = int(&mut func);
    let in_range = bool_value(&mut func);
    let small = bool_value(&mut func);
    let added = int(&mut func);
    let merged = int(&mut func);
    let next = int(&mut func);
    let result = int(&mut func);

    let entry = func.entry_block;
    let init = if seed_from_param { param } else { zero };
    func.blocks.insert(
        entry,
        block(
            entry,
            func.blocks[&entry].args.clone(),
            vec![
                const_int(1000, n),
                const_int(500, half),
                const_int(0, zero),
                const_int(1, one),
            ],
            Terminator::Branch {
                target: header,
                args: vec![init, zero],
            },
        ),
    );
    func.blocks.insert(
        header,
        block(
            header,
            vec![arg(total), arg(i)],
            vec![],
            Terminator::Branch {
                target: guard,
                args: vec![],
            },
        ),
    );
    func.blocks.insert(
        guard,
        block(
            guard,
            vec![],
            vec![op(OpCode::Lt, vec![i, n], vec![in_range])],
            Terminator::CondBranch {
                cond: in_range,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        ),
    );
    func.blocks.insert(
        body,
        block(
            body,
            vec![],
            vec![op(OpCode::Lt, vec![i, half], vec![small])],
            Terminator::CondBranch {
                cond: small,
                then_block,
                then_args: vec![],
                else_block: join,
                else_args: vec![total],
            },
        ),
    );
    func.blocks.insert(
        then_block,
        block(
            then_block,
            vec![],
            vec![op(OpCode::InplaceAdd, vec![total, i], vec![added])],
            Terminator::Branch {
                target: join,
                args: vec![added],
            },
        ),
    );
    func.blocks.insert(
        join,
        block(
            join,
            vec![arg(merged)],
            vec![op(OpCode::Add, vec![i, one], vec![next])],
            Terminator::Branch {
                target: header,
                args: vec![merged, next],
            },
        ),
    );
    func.blocks.insert(
        exit,
        block(
            exit,
            vec![],
            vec![op(OpCode::Copy, vec![total], vec![result])],
            Terminator::Return {
                values: vec![result],
            },
        ),
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_cond_blocks.insert(header, guard);
    Fixture { func, exit }
}

fn count(func: &TirFunction, opcode: OpCode) -> usize {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == opcode)
        .count()
}

#[test]
fn a_branching_accumulator_loop_is_versioned() {
    let Fixture { mut func, exit } = branching_loop(false);
    verify_function(&func).expect("fixture verifies");
    let stats = run(&mut func, &mut AnalysisManager::new());
    assert!(stats.ops_added > 0, "the accumulator loop is versioned");
    verify_function(&func).expect("versioned function verifies");
    // The fast loop checks the accumulator; the slow loop and the transfer
    // keep plain boxed arithmetic, BigInt-exact.
    assert_eq!(count(&func, OpCode::CheckedAdd), 1);
    assert_eq!(count(&func, OpCode::InplaceAdd), 2);
    // The counter is inline-safe and is not checked.
    assert_eq!(count(&func, OpCode::Add), 2);
    // Code after the loop merges the fast and slow accumulators.
    assert_eq!(func.blocks[&exit].args.len(), 1);
    let merged = func.blocks[&exit].args[0].id;
    assert_eq!(func.blocks[&exit].ops[0].operands, vec![merged]);
}

#[test]
fn the_overflow_path_reads_the_exact_result() {
    let Fixture { mut func, .. } = branching_loop(false);
    run(&mut func, &mut AnalysisManager::new());
    let checked = func
        .blocks
        .values()
        .find(|block| block.ops.iter().any(|op| op.opcode == OpCode::CheckedAdd))
        .expect("a checked add exists");
    let (wrapped, flag) = {
        let op = checked.ops.last().unwrap();
        (op.results[0], op.results[1])
    };
    let Terminator::CondBranch {
        cond, then_block, ..
    } = &checked.terminator
    else {
        panic!("the checked add ends its block with the overflow branch");
    };
    assert_eq!(*cond, flag);
    let transfer = &func.blocks[then_block];
    assert_eq!(transfer.ops.len(), 1);
    assert_eq!(transfer.ops[0].opcode, OpCode::InplaceAdd);
    let exact = transfer.ops[0].results[0];
    // No block reachable from the transfer reads the wrapped value.
    let mut seen = HashSet::new();
    let mut work = vec![*then_block];
    while let Some(bid) = work.pop() {
        if !seen.insert(bid) {
            continue;
        }
        let block = &func.blocks[&bid];
        for op in &block.ops {
            assert!(!op.operands.contains(&wrapped), "block {bid:?} reads the wrapped sum");
        }
        block.terminator.for_each_value(|value| {
            assert_ne!(value, wrapped, "block {bid:?} passes the wrapped sum");
        });
        work.extend(block.terminator.successors());
    }
    assert!(
        func.blocks.values().any(|block| {
            let mut passes_exact = false;
            block.terminator.for_each_value(|value| passes_exact |= value == exact);
            passes_exact
        }),
        "the exact result flows on"
    );
}

#[test]
fn an_accumulator_seeded_by_a_parameter_is_refused() {
    let Fixture { mut func, .. } = branching_loop(true);
    let before = func.blocks.len();
    let stats = run(&mut func, &mut AnalysisManager::new());
    assert_eq!(stats.ops_added, 0);
    assert_eq!(func.blocks.len(), before);
    assert_eq!(count(&func, OpCode::CheckedAdd), 0);
}

#[test]
fn a_loop_with_a_handler_region_is_refused() {
    let Fixture { mut func, .. } = branching_loop(false);
    let body = func
        .blocks
        .values()
        .find(|block| block.ops.iter().any(|op| op.opcode == OpCode::InplaceAdd))
        .map(|block| block.id)
        .unwrap();
    let mut try_start = TirOp::new(OpCode::TryStart, vec![], vec![]);
    try_start.attrs.insert("value".into(), AttrValue::Int(99));
    func.blocks.get_mut(&body).unwrap().ops.insert(0, try_start);
    let before = func.blocks.len();
    let stats = run(&mut func, &mut AnalysisManager::new());
    assert_eq!(stats.ops_added, 0);
    assert_eq!(func.blocks.len(), before);
}
