use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::LoopRole;
use crate::tir::ops::{AttrDict, AttrValue, Dialect};
use crate::tir::verify::verify_function;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp::new(opcode, operands, results)
}

fn const_int(value: i64, result: ValueId) -> TirOp {
    let mut attrs = AttrDict::default();
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
    let stats = run(&mut func, &mut AnalysisManager::new(), false);
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
    run(&mut func, &mut AnalysisManager::new(), false);
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
    let mut seen = HashSet::default();
    let mut work = vec![*then_block];
    while let Some(bid) = work.pop() {
        if !seen.insert(bid) {
            continue;
        }
        let block = &func.blocks[&bid];
        for op in &block.ops {
            assert!(
                !op.operands.contains(&wrapped),
                "block {bid:?} reads the wrapped sum"
            );
        }
        block.terminator.for_each_value(|value| {
            assert_ne!(value, wrapped, "block {bid:?} passes the wrapped sum");
        });
        work.extend(block.terminator.successors());
    }
    assert!(
        func.blocks.values().any(|block| {
            let mut passes_exact = false;
            block
                .terminator
                .for_each_value(|value| passes_exact |= value == exact);
            passes_exact
        }),
        "the exact result flows on"
    );
}

#[test]
fn an_accumulator_seeded_by_a_parameter_is_refused() {
    let Fixture { mut func, .. } = branching_loop(true);
    let before = func.blocks.len();
    let stats = run(&mut func, &mut AnalysisManager::new(), false);
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
    let stats = run(&mut func, &mut AnalysisManager::new(), false);
    assert_eq!(stats.ops_added, 0);
    assert_eq!(func.blocks.len(), before);
}

/// `def f(n: exact int): i = 0; total = 0; while i < n: total += i & 7;
/// i += 1; return total`, in the lifted shape: entry -> header(i, total) ->
/// guard -> body -> header; the guard's exit returns total.
struct BoundedFixture {
    func: TirFunction,
    n: ValueId,
}

fn bounded_loop() -> BoundedFixture {
    let mut func = TirFunction::new(
        "bounded_loop_fixture".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    func.exact_param_types = vec![TirType::I64];
    let n = func.blocks[&func.entry_block].args[0].id;
    let header = func.fresh_block();
    let guard = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let zero = int(&mut func);
    let one = int(&mut func);
    let seven = int(&mut func);
    let i = int(&mut func);
    let total = int(&mut func);
    let i_read = int(&mut func);
    let compared = func.fresh_value();
    func.value_types.insert(compared, TirType::DynBox);
    let in_range = bool_value(&mut func);
    let total_read = int(&mut func);
    let masked = int(&mut func);
    let added = int(&mut func);
    let next = int(&mut func);
    let result = int(&mut func);
    let entry = func.entry_block;
    func.blocks.insert(
        entry,
        block(
            entry,
            func.blocks[&entry].args.clone(),
            vec![const_int(0, zero), const_int(1, one), const_int(7, seven)],
            Terminator::Branch {
                target: header,
                args: vec![zero, zero],
            },
        ),
    );
    func.blocks.insert(
        header,
        block(
            header,
            vec![arg(i), arg(total)],
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
            // A local read, then the compare's boxed result and its truth, as
            // the lift emits them for a bound not yet known to be an int.
            vec![
                op(OpCode::Copy, vec![i], vec![i_read]),
                op(OpCode::Lt, vec![i_read, n], vec![compared]),
                op(OpCode::Bool, vec![compared], vec![in_range]),
            ],
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
            vec![
                op(OpCode::BitAnd, vec![i, seven], vec![masked]),
                op(OpCode::Copy, vec![total], vec![total_read]),
                op(OpCode::InplaceAdd, vec![total_read, masked], vec![added]),
                op(OpCode::InplaceAdd, vec![i, one], vec![next]),
            ],
            Terminator::Branch {
                target: header,
                args: vec![next, added],
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
    // The lift keeps an unreachable `loop_end -> header` edge as metadata.
    let vestigial = func.fresh_block();
    func.blocks.insert(
        vestigial,
        block(
            vestigial,
            vec![],
            vec![],
            Terminator::Branch {
                target: header,
                args: vec![zero, zero],
            },
        ),
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_cond_blocks.insert(header, guard);
    BoundedFixture { func, n }
}

#[test]
fn a_backend_lane_target_checks_a_bounded_accumulator() {
    let BoundedFixture { mut func, .. } = bounded_loop();
    let stats = run(&mut func, &mut AnalysisManager::new(), false);
    assert!(stats.ops_added > 0);
    verify_function(&func).expect("versioned function verifies");
    // Without final TIR there is no inline-payload guard: the bound stays a
    // word and both adds keep their overflow checks.
    assert_eq!(count(&func, OpCode::IsInlineInt), 0);
    assert_eq!(count(&func, OpCode::CheckedAdd), 2);
}

#[test]
fn an_inline_bound_guards_the_fast_loop_and_drops_its_checks() {
    let BoundedFixture { mut func, n } = bounded_loop();
    let entry = func.entry_block;
    let stats = run(&mut func, &mut AnalysisManager::new(), true);
    assert!(stats.ops_added > 0);
    verify_function(&func).expect("versioned function verifies");
    // One tag test on the entry path, one payload for the fast loop.
    assert_eq!(count(&func, OpCode::IsInlineInt), 1);
    assert_eq!(count(&func, OpCode::UnboxInlineInt), 1);
    let Terminator::CondBranch {
        then_block,
        else_block,
        else_args,
        ..
    } = &func.blocks[&entry].terminator
    else {
        panic!("the loop entry tests the bound");
    };
    assert_eq!(else_args.len(), 2, "a big-int bound enters the slow loop");
    let unbox = &func.blocks[then_block].ops[0];
    assert_eq!(unbox.opcode, OpCode::UnboxInlineInt);
    assert_eq!(unbox.operands, vec![n]);
    let payload = unbox.results[0];
    // The fast loop compares against the payload; the slow loop against the
    // word.
    let compares: Vec<&TirOp> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::Lt)
        .collect();
    assert_eq!(compares.len(), 2);
    assert!(compares.iter().any(|op| op.operands[1] == payload));
    assert!(compares.iter().any(|op| op.operands[1] == n));
    // In the fast loop the counter is an inline int and the sum of at most
    // 2**46 - 1 steps of 0..=7 stays inside i64: no overflow check remains,
    // and the accumulator's add carries the proof.
    assert_eq!(count(&func, OpCode::CheckedAdd), 0);
    let proven: Vec<&TirOp> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.has_i64_exact_proof())
        .collect();
    assert_eq!(proven.len(), 1);
    assert_eq!(proven[0].opcode, OpCode::InplaceAdd);
    // Each loop keeps its two in-place adds: plain machine adds in the fast
    // loop, boxed arithmetic in the slow one.
    assert_eq!(count(&func, OpCode::InplaceAdd), 4);
    // The slow loop starts at a header of its own.
    assert_ne!(
        *else_block,
        func.blocks[then_block].terminator.successors()[0]
    );
}

#[test]
fn a_specialized_clone_of_a_versioned_loop_drops_the_discharged_checks() {
    // The generic body sees an unknown bound and versions with checks; type
    // flow then clones it for an exact-int bound and optimizes the clone
    // again, as the module phase does.
    let BoundedFixture { mut func, n } = bounded_loop();
    func.exact_param_types.clear();
    run(&mut func, &mut AnalysisManager::new(), true);
    verify_function(&func).expect("versioned function verifies");
    assert_eq!(count(&func, OpCode::IsInlineInt), 0);
    assert_eq!(count(&func, OpCode::CheckedAdd), 2);

    func.exact_param_types = vec![TirType::I64];
    let stats = run(&mut func, &mut AnalysisManager::new(), true);
    assert!(stats.ops_added > 0);
    verify_function(&func).expect("re-versioned function verifies");
    assert_eq!(count(&func, OpCode::IsInlineInt), 1);
    let payload = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::UnboxInlineInt)
        .map(|op| {
            assert_eq!(op.operands, vec![n]);
            op.results[0]
        })
        .expect("the fast loop reads the bound's payload");
    assert!(
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|op| op.opcode == OpCode::Lt && op.operands[1] == payload)
    );
    // The fast loop's two checks are discharged; the copy that runs for a
    // big-int bound keeps both.
    assert_eq!(count(&func, OpCode::CheckedAdd), 2);
    let proven = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.has_i64_exact_proof())
        .count();
    assert_eq!(proven, 2);
}

#[test]
fn a_bound_the_header_carries_unchanged_is_tested_once() {
    // A fused generator's range stop: the loop header carries the bound in a
    // parameter whose entry and back edges pass the bound itself, under
    // value-preserving copies. The parameter is the invariant every
    // iteration, so the entry tests it once and the fast loop reads its
    // payload.
    let BoundedFixture { mut func, n } = bounded_loop();
    let entry = func.entry_block;
    let Terminator::Branch { target: header, .. } = func.blocks[&entry].terminator.clone() else {
        panic!("the entry enters the loop");
    };
    let entered = int(&mut func);
    let carried = int(&mut func);
    let again = int(&mut func);
    let entry_block = func.blocks.get_mut(&entry).unwrap();
    entry_block
        .ops
        .push(op(OpCode::Copy, vec![n], vec![entered]));
    if let Terminator::Branch { args, .. } = &mut entry_block.terminator {
        args.push(entered);
    }
    func.blocks
        .get_mut(&header)
        .unwrap()
        .args
        .push(arg(carried));
    let mut latches = Vec::new();
    for block in func.blocks.values() {
        if block.id != entry && block.terminator.successors().contains(&header) {
            latches.push(block.id);
        }
    }
    for latch in latches {
        let block = func.blocks.get_mut(&latch).unwrap();
        let back_edge = !block.ops.is_empty();
        if back_edge {
            block.ops.push(op(OpCode::Copy, vec![carried], vec![again]));
        }
        if let Terminator::Branch { args, .. } = &mut block.terminator {
            args.push(if back_edge { again } else { args[0] });
        }
    }
    for block in func.blocks.values_mut() {
        for operation in &mut block.ops {
            if operation.opcode == OpCode::Lt {
                operation.operands[1] = carried;
            }
        }
    }
    verify_function(&func).expect("fixture verifies");
    let stats = run(&mut func, &mut AnalysisManager::new(), true);
    assert!(stats.ops_added > 0);
    verify_function(&func).expect("versioned function verifies");
    assert_eq!(count(&func, OpCode::IsInlineInt), 1);
    let payload = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::UnboxInlineInt)
        .map(|op| {
            assert_eq!(op.operands, vec![entered]);
            op.results[0]
        })
        .expect("the fast loop reads the bound's payload");
    assert!(
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .any(|op| op.opcode == OpCode::Lt && op.operands[1] == payload)
    );
}

#[test]
fn an_invariant_only_shifted_keeps_its_word() {
    // `total += i << n` under `i < 1000`: with both operands raw the shift
    // would box both and call the runtime each iteration unless its result
    // were proven inline, so the fast loop reads an invariant's payload only
    // in comparisons and selected arithmetic.
    let BoundedFixture { mut func, n } = bounded_loop();
    let limit = int(&mut func);
    let entry = func.entry_block;
    func.blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .push(const_int(1000, limit));
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            if op.opcode == OpCode::Lt {
                op.operands[1] = limit;
            }
            if op.opcode == OpCode::BitAnd {
                op.opcode = OpCode::Shl;
                op.operands[1] = n;
            }
        }
    }
    run(&mut func, &mut AnalysisManager::new(), true);
    verify_function(&func).expect("function verifies");
    assert_eq!(count(&func, OpCode::IsInlineInt), 0);
    assert_eq!(count(&func, OpCode::UnboxInlineInt), 0);
}

#[test]
fn a_bounded_subtraction_takes_the_exact_form() {
    // `total += i * 3 - 1` under an inline bound: `i * 3` and `i * 3 - 1`
    // leave the inline window but stay inside i64, so both are plain machine
    // ops with the proof; the unbounded sum keeps its check.
    let BoundedFixture { mut func, .. } = bounded_loop();
    let three = int(&mut func);
    let tripled = int(&mut func);
    let entry = func.entry_block;
    func.blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .push(const_int(3, three));
    let one = func.blocks[&entry].ops[1].results[0];
    for block in func.blocks.values_mut() {
        let Some(at) = block.ops.iter().position(|op| op.opcode == OpCode::BitAnd) else {
            continue;
        };
        let masked = block.ops[at].results[0];
        let i = block.ops[at].operands[0];
        block.ops[at] = op(OpCode::Mul, vec![i, three], vec![tripled]);
        block
            .ops
            .insert(at + 1, op(OpCode::Sub, vec![tripled, one], vec![masked]));
    }
    run(&mut func, &mut AnalysisManager::new(), true);
    verify_function(&func).expect("versioned function verifies");
    let proven: Vec<OpCode> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.has_i64_exact_proof())
        .map(|op| op.opcode)
        .collect();
    assert!(proven.contains(&OpCode::Mul), "{proven:?}");
    assert!(proven.contains(&OpCode::Sub), "{proven:?}");
    // The fast loop checks only the unbounded accumulator.
    assert_eq!(count(&func, OpCode::CheckedAdd), 1);
}

#[test]
fn a_range_stop_of_any_magnitude_takes_the_guard() {
    // `for i in range(n)` compares against `n.__index__()`, an exact int of
    // any magnitude (BigInt-typed): it takes the same guard.
    let BoundedFixture { mut func, n } = bounded_loop();
    func.exact_param_types = vec![TirType::BigInt];
    let _ = n;
    run(&mut func, &mut AnalysisManager::new(), true);
    verify_function(&func).expect("versioned function verifies");
    assert_eq!(count(&func, OpCode::IsInlineInt), 1);
    assert_eq!(count(&func, OpCode::UnboxInlineInt), 1);
}

// Captured from bench_etl_orders.parse_int. The CSV and wide CSV parsers
// have the same loop. Keep real frame stores and exception observers.
#[test]
fn short_ascii_region_preserves_the_generic_parser_and_observers() {
    let ir: molt_ir::FunctionIR = serde_json::from_str(include_str!("parse_int.json")).unwrap();
    let target = crate::tir::target_info::TargetInfo::native_release_fast();
    let mut func = crate::tir::lower_from_simple::lower_to_tir_for_target(&ir, &target);
    let polls = |f: &TirFunction| {
        f.blocks
            .values()
            .flat_map(|b| &b.ops)
            .filter(|op| op.is_async_work_poll())
            .map(|op| {
                (
                    op.attrs["value"].clone(),
                    op.attrs["_source_op_idx"].clone(),
                    op.source_span,
                )
            })
            .collect::<Vec<_>>()
    };
    let mut exposed = func.clone();
    exposed
        .attrs
        .insert("frame_locals_exposed".into(), AttrValue::Bool(true));
    let pipeline = crate::tir::pass_manager::build_default_pipeline(target);
    let count_admissions = |func: &TirFunction| {
        func.blocks
            .values()
            .flat_map(|b| &b.ops)
            .filter(|op| {
                op.runtime == Some(molt_ir::runtime_fn_generated::RuntimeFn::ShortAsciiSnapshot)
            })
            .count()
    };
    // The admission lives inside the ordinary pass pipeline, including the
    // preceding type refinement and exception analysis.
    pipeline.run(&mut func);
    assert_eq!(
        count_admissions(&func),
        1,
        "{}",
        crate::tir::printer::print_function(&func)
    );
    verify_function(&func).expect("versioned captured parser verifies");
    assert_eq!(count(&func, OpCode::OrdAt), 1, "generic ordinal retained");
    let ranges = <ValueRange as crate::tir::analysis::Analysis>::compute(&func);
    let raw = crate::representation_facts::raw_i64_safe_values_for(&func, &ranges);
    let (block, shift) = func
        .blocks
        .values()
        .flat_map(|block| block.ops.iter().map(move |op| (block.id, op)))
        .find(|(_, op)| op.opcode == OpCode::Shr && ranges.const_int_of(op.operands[1]).is_none())
        .expect("packed byte selection uses a variable shift");
    assert!(
        raw.contains(&shift.results[0]),
        "variable byte extraction must stay raw (shift count {:?}, output {:?})",
        ranges.range_at(block, shift.operands[1]),
        ranges.range_of(shift.results[0])
    );
    pipeline.run(&mut exposed);
    assert_eq!(count_admissions(&exposed), 0);
    let generic_polls = polls(&exposed);
    assert_eq!(
        generic_polls.len(),
        2,
        "existing integer versioning keeps a backedge observation in each loop"
    );
    assert_eq!(
        polls(&func),
        generic_polls,
        "both arms retain the generic observer's target and source span"
    );
}
