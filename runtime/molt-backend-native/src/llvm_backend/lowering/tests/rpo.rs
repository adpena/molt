use super::*;

#[test]
fn rpo_diamond_cfg_orders_entry_first_then_arms_then_merge() {
    // CFG:
    //   entry -> A, B   (cond branch)
    //   A     -> merge
    //   B     -> merge
    //   merge -> return
    //
    // Valid RPOs: [entry, A, B, merge] OR [entry, B, A, merge].
    let mut func = make_func_with_blocks("diamond", 4);
    let entry = func.entry_block; // BlockId(0)
    let a = BlockId(1);
    let b = BlockId(2);
    let merge = BlockId(3);

    // We allocate ValueId(0) as the conditional value. We never actually
    // evaluate it — RPO walks terminators, not ops.
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

    assert_eq!(
        rpo.len(),
        4,
        "all four blocks must appear in RPO: {:?}",
        rpo
    );
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
fn rpo_simple_loop_orders_entry_before_header_before_body() {
    // CFG:
    //   entry  -> header
    //   header -> body, exit  (cond branch)
    //   body   -> header      (back-edge — does NOT change RPO order)
    //   exit   -> return
    //
    // Required: entry < header < body in RPO. The back-edge body->header
    // is the only edge that runs "backwards" in the resulting layout.
    let mut func = make_func_with_blocks("loop", 4);
    let entry = func.entry_block; // BlockId(0)
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

    assert_eq!(
        rpo.len(),
        4,
        "all four blocks must appear in RPO: {:?}",
        rpo
    );

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
fn rpo_unreachable_blocks_are_excluded() {
    // CFG:
    //   entry -> exit (return)
    //   dead  -> return  (no predecessor — unreachable)
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
fn rpo_switch_terminator_visits_all_cases_and_default() {
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
fn rpo_deeply_chained_cfg_does_not_overflow_stack() {
    // Build a chain of 5,000 blocks: entry -> b1 -> b2 -> ... -> b4999 -> return.
    // The original recursive implementation overflowed at this depth on
    // default thread stack sizes; the iterative version handles it
    // without issue.
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
