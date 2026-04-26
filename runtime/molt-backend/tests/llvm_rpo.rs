//! Integration tests for the LLVM backend's reverse-post-order (RPO)
//! computation over TIR CFGs.
//!
//! These tests target [`molt_backend::llvm_backend::lowering::compute_function_rpo`]
//! directly. They construct synthetic TIR CFGs (no ops, just terminators) and
//! assert the dominator-precedes-dominatee invariants that LLVM lowering
//! relies on. We do not pin the result to a single canonical ordering: any
//! traversal that records each block in post-order before reversal is a
//! valid RPO. Instead we assert the relational properties that callers
//! actually depend on.

#![cfg(feature = "llvm")]

use molt_backend::llvm_backend::lowering::{append_terminator_successors, compute_function_rpo};
use molt_backend::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_backend::tir::function::TirFunction;
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;

/// Build a function with `num_blocks` empty blocks (terminators initialized
/// to `Unreachable`; tests overwrite them as needed).
fn make_func_with_blocks(name: &str, num_blocks: u32) -> TirFunction {
    let mut func = TirFunction::new(name.into(), vec![], TirType::I64);
    for _ in 1..num_blocks {
        let bid = func.fresh_block();
        func.blocks.insert(
            bid,
            TirBlock {
                id: bid,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            },
        );
    }
    func
}

fn set_term(func: &mut TirFunction, b: BlockId, term: Terminator) {
    func.blocks.get_mut(&b).unwrap().terminator = term;
}

fn position_of(rpo: &[BlockId], b: BlockId) -> usize {
    rpo.iter()
        .position(|x| *x == b)
        .unwrap_or_else(|| panic!("BlockId {:?} not present in RPO {:?}", b, rpo))
}

#[test]
fn llvm_rpo_diamond_cfg_orders_entry_first_then_arms_then_merge() {
    // CFG:
    //   entry -> A, B   (cond branch)
    //   A     -> merge
    //   B     -> merge
    //   merge -> return
    //
    // Valid RPOs: [entry, A, B, merge] OR [entry, B, A, merge].
    let mut func = make_func_with_blocks("diamond", 4);
    let entry = func.entry_block;
    let a = BlockId(1);
    let b = BlockId(2);
    let merge = BlockId(3);

    let cond = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::CondBranch {
            cond,
            then_block: a,
            then_args: vec![],
            else_block: b,
            else_args: vec![],
        },
    );
    set_term(
        &mut func,
        a,
        Terminator::Branch {
            target: merge,
            args: vec![],
        },
    );
    set_term(
        &mut func,
        b,
        Terminator::Branch {
            target: merge,
            args: vec![],
        },
    );
    set_term(&mut func, merge, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo.len(), 4, "all four blocks must appear in RPO: {:?}", rpo);
    assert_eq!(rpo[0], entry, "entry must be first: {:?}", rpo);
    assert_eq!(rpo[3], merge, "merge must be last: {:?}", rpo);

    let pos_entry = position_of(&rpo, entry);
    let pos_a = position_of(&rpo, a);
    let pos_b = position_of(&rpo, b);
    let pos_merge = position_of(&rpo, merge);

    assert!(pos_entry < pos_a, "entry must precede A: {:?}", rpo);
    assert!(pos_entry < pos_b, "entry must precede B: {:?}", rpo);
    assert!(pos_a < pos_merge, "A must precede merge: {:?}", rpo);
    assert!(pos_b < pos_merge, "B must precede merge: {:?}", rpo);

    // The two valid orderings are exactly these two.
    let valid_a_first = rpo == vec![entry, a, b, merge];
    let valid_b_first = rpo == vec![entry, b, a, merge];
    assert!(
        valid_a_first || valid_b_first,
        "RPO must be one of the two valid diamond orderings, got {:?}",
        rpo
    );
}

#[test]
fn llvm_rpo_simple_loop_orders_entry_before_header_before_body() {
    // CFG:
    //   entry  -> header
    //   header -> body, exit  (cond branch)
    //   body   -> header      (back-edge — does NOT change RPO order)
    //   exit   -> return
    //
    // Required: entry < header < body in RPO. The back-edge body->header
    // is the only edge that runs "backwards" in the resulting layout.
    let mut func = make_func_with_blocks("loop", 4);
    let entry = func.entry_block;
    let header = BlockId(1);
    let body = BlockId(2);
    let exit = BlockId(3);

    let cond = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::Branch {
            target: header,
            args: vec![],
        },
    );
    set_term(
        &mut func,
        header,
        Terminator::CondBranch {
            cond,
            then_block: body,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        },
    );
    set_term(
        &mut func,
        body,
        Terminator::Branch {
            target: header,
            args: vec![],
        },
    );
    set_term(&mut func, exit, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo.len(), 4, "all four blocks must appear in RPO: {:?}", rpo);

    let pos_entry = position_of(&rpo, entry);
    let pos_header = position_of(&rpo, header);
    let pos_body = position_of(&rpo, body);
    let pos_exit = position_of(&rpo, exit);

    assert_eq!(pos_entry, 0, "entry must be first: {:?}", rpo);
    assert!(
        pos_entry < pos_header,
        "entry must precede header: {:?}",
        rpo
    );
    assert!(
        pos_header < pos_body,
        "header must precede body (back-edge does not flip order): {:?}",
        rpo
    );
    assert!(
        pos_header < pos_exit,
        "header must precede exit (then is forward edge): {:?}",
        rpo
    );
}

#[test]
fn llvm_rpo_unreachable_blocks_are_excluded() {
    // CFG:
    //   entry -> exit (return)
    //   dead  -> return  (no predecessor — unreachable from entry)
    let mut func = make_func_with_blocks("dead_block", 3);
    let entry = func.entry_block;
    let exit = BlockId(1);
    let dead = BlockId(2);

    set_term(
        &mut func,
        entry,
        Terminator::Branch {
            target: exit,
            args: vec![],
        },
    );
    set_term(&mut func, exit, Terminator::Return { values: vec![] });
    set_term(&mut func, dead, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo, vec![entry, exit]);
    assert!(
        !rpo.contains(&dead),
        "unreachable block must be excluded from RPO: {:?}",
        rpo
    );
}

#[test]
fn llvm_rpo_switch_terminator_visits_all_cases_and_default() {
    // CFG:
    //   entry -> switch on v: case 0 -> A, case 1 -> B, default -> C
    //   A, B, C -> merge -> return
    let mut func = make_func_with_blocks("switch_cfg", 5);
    let entry = func.entry_block;
    let a = BlockId(1);
    let b = BlockId(2);
    let c = BlockId(3);
    let merge = BlockId(4);

    let v = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::Switch {
            value: v,
            cases: vec![(0, a, vec![]), (1, b, vec![])],
            default: c,
            default_args: vec![],
        },
    );
    for case_block in [a, b, c] {
        set_term(
            &mut func,
            case_block,
            Terminator::Branch {
                target: merge,
                args: vec![],
            },
        );
    }
    set_term(&mut func, merge, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo.len(), 5, "all five blocks must appear: {:?}", rpo);
    assert_eq!(rpo[0], entry);
    assert_eq!(rpo[4], merge);
    for case_block in [a, b, c] {
        let p = position_of(&rpo, case_block);
        assert!(p > 0, "case block must follow entry");
        assert!(p < 4, "case block must precede merge");
    }
}

#[test]
fn llvm_rpo_deeply_chained_cfg_does_not_overflow_stack() {
    // Build a chain of 5,000 blocks: entry -> b1 -> b2 -> ... -> b4999 -> return.
    // A naive recursive DFS overflows the host stack at this depth on default
    // thread stack sizes; the iterative implementation must handle it.
    const N: u32 = 5_000;
    let mut func = make_func_with_blocks("deep_chain", N);
    for i in 0..N - 1 {
        set_term(
            &mut func,
            BlockId(i),
            Terminator::Branch {
                target: BlockId(i + 1),
                args: vec![],
            },
        );
    }
    set_term(
        &mut func,
        BlockId(N - 1),
        Terminator::Return { values: vec![] },
    );

    let rpo = compute_function_rpo(&func);

    assert_eq!(rpo.len(), N as usize);
    for (i, bid) in rpo.iter().enumerate() {
        assert_eq!(
            *bid,
            BlockId(i as u32),
            "deep chain RPO must be entry, b1, b2, ... in order"
        );
    }
}

#[test]
fn llvm_rpo_terminator_successor_helper_preserves_order() {
    // The order in which `append_terminator_successors` records successors
    // is part of the algorithm's contract: it determines tie-breaking when
    // multiple valid RPOs exist. Pin it explicitly.
    let mut buf = Vec::new();

    buf.clear();
    append_terminator_successors(
        &Terminator::Branch {
            target: BlockId(7),
            args: vec![],
        },
        &mut buf,
    );
    assert_eq!(buf, vec![BlockId(7)]);

    buf.clear();
    append_terminator_successors(
        &Terminator::CondBranch {
            cond: ValueId(0),
            then_block: BlockId(11),
            then_args: vec![],
            else_block: BlockId(13),
            else_args: vec![],
        },
        &mut buf,
    );
    assert_eq!(
        buf,
        vec![BlockId(11), BlockId(13)],
        "then must precede else"
    );

    buf.clear();
    append_terminator_successors(
        &Terminator::Switch {
            value: ValueId(0),
            cases: vec![(0, BlockId(20), vec![]), (1, BlockId(21), vec![])],
            default: BlockId(22),
            default_args: vec![],
        },
        &mut buf,
    );
    assert_eq!(
        buf,
        vec![BlockId(20), BlockId(21), BlockId(22)],
        "switch cases in declaration order, then default"
    );

    buf.clear();
    append_terminator_successors(&Terminator::Return { values: vec![] }, &mut buf);
    assert!(buf.is_empty(), "Return has no successors");

    buf.clear();
    append_terminator_successors(&Terminator::Unreachable, &mut buf);
    assert!(buf.is_empty(), "Unreachable has no successors");
}

#[test]
fn llvm_rpo_self_loop_is_not_revisited() {
    // CFG:
    //   entry -> entry (self-loop) OR exit
    //   exit  -> return
    //
    // The self-edge must not cause infinite recursion, and `entry` must
    // appear exactly once.
    let mut func = make_func_with_blocks("self_loop", 2);
    let entry = func.entry_block;
    let exit = BlockId(1);

    let cond = func.fresh_value();
    set_term(
        &mut func,
        entry,
        Terminator::CondBranch {
            cond,
            then_block: entry,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        },
    );
    set_term(&mut func, exit, Terminator::Return { values: vec![] });

    let rpo = compute_function_rpo(&func);
    assert_eq!(rpo, vec![entry, exit]);
}
