use super::*;

#[test]
fn peephole_collapses_set_get_to_tee() {
    let input = vec![
        Instruction::I64Const(42),
        Instruction::LocalSet(3),
        Instruction::LocalGet(3),
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 3);
    assert!(
        matches!(output[0], Instruction::I64Const(42)),
        "const preserved"
    );
    assert!(
        matches!(output[1], Instruction::LocalTee(3)),
        "set+get collapsed to tee"
    );
    assert!(matches!(output[2], Instruction::End), "end preserved");
}

#[test]
fn peephole_preserves_mismatched_set_get() {
    let input = vec![
        Instruction::LocalSet(1),
        Instruction::LocalGet(2), // different local
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 3);
    assert!(
        matches!(output[0], Instruction::LocalSet(1)),
        "set preserved"
    );
    assert!(
        matches!(output[1], Instruction::LocalGet(2)),
        "get preserved"
    );
}

#[test]
fn peephole_handles_consecutive_tee_chains() {
    // Pattern: set(1) get(1) set(2) get(2) → tee(1) tee(2)
    let input = vec![
        Instruction::I64Const(10),
        Instruction::LocalSet(1),
        Instruction::LocalGet(1),
        Instruction::LocalSet(2),
        Instruction::LocalGet(2),
        Instruction::End,
    ];
    let output = peephole_instrs(input);
    assert_eq!(output.len(), 4);
    assert!(matches!(output[1], Instruction::LocalTee(1)));
    assert!(matches!(output[2], Instruction::LocalTee(2)));
}

#[test]
fn peephole_empty_and_single() {
    assert!(peephole_instrs(vec![]).is_empty());
    let single = vec![Instruction::End];
    assert_eq!(peephole_instrs(single).len(), 1);
}

#[test]
fn peephole_applied_in_const_return() {
    // A const-return function should have tee instead of set+get.
    let func = make_const_return_func(99);
    let output = lower_tir_to_wasm(&func).test_view();

    // After peephole, the pattern: i64.const 99; local.set X; local.get X; return
    // becomes: i64.const 99; local.tee X; return
    let has_tee = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::LocalTee(_)));
    assert!(has_tee, "expected local.tee from peephole optimization");

    // Should have no set+get pairs for the same local.
    for window in output.instructions.windows(2) {
        if let (Instruction::LocalSet(s), Instruction::LocalGet(g)) = (&window[0], &window[1]) {
            assert_ne!(
                s, g,
                "found redundant set+get pair for local {s} that peephole should have eliminated"
            );
        }
    }
}
