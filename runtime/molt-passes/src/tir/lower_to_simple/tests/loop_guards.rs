use super::*;

/// Regression test: counted loops are normalized into loop-carried
/// store_var/load_var form, and control flow must not re-enter above the
/// first carrier load after loop_start.
#[test]
fn tir_round_trip_keeps_loop_index_start_out_of_backedge_path() {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    let func_ir = FunctionIR {
        name: "counted_loop".into(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".into(),
                value: Some(3),
                out: Some("limit".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(0),
                out: Some("zero".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".into(),
                value: Some(1),
                out: Some("one".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_index_start".into(),
                args: Some(vec!["zero".into()]),
                out: Some("i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "lt".into(),
                args: Some(vec!["i".into(), "limit".into()]),
                out: Some("cond".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_false".into(),
                args: Some(vec!["cond".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".into(),
                args: Some(vec!["i".into(), "one".into()]),
                out: Some("next_i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_index_next".into(),
                args: Some(vec!["next_i".into()]),
                out: Some("i".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = lower_to_tir(&func_ir);
    let round_tripped = lower_to_simple_ir(&tir_func);

    let loop_start_idx = round_tripped
        .iter()
        .position(|op| op.kind == "loop_start")
        .expect("expected loop_start after round-trip");
    let carrier_load_idx = round_tripped
        .iter()
        .position(|op| op.kind == "load_var")
        .expect("expected loop-carried load_var after round-trip");
    assert!(
        round_tripped[loop_start_idx + 1..carrier_load_idx]
            .iter()
            .all(|op| op.kind != "label" && op.kind != "jump" && op.kind != "br_if"),
        "counted loop must not place control-flow re-entry before the carrier load; ops: {:?}",
        round_tripped
            .iter()
            .map(|op| op.kind.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn structured_if_must_not_inline_exception_handler_target_blocks() {
    let mut func = TirFunction::new("eh_handler_if".into(), vec![TirType::Bool], TirType::I64);

    let handler_block = func.fresh_block();
    let else_block = func.fresh_block();
    let handler_value = func.fresh_value();
    let else_value = func.fresh_value();

    let mut handler_attrs = AttrDict::new();
    handler_attrs.insert("value".into(), AttrValue::Int(7));
    let mut else_attrs = AttrDict::new();
    else_attrs.insert("value".into(), AttrValue::Int(9));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut check_exc_attrs = AttrDict::new();
    check_exc_attrs.insert("value".into(), AttrValue::Int(100));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs: check_exc_attrs,
        source_span: None,
    });
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: handler_block,
        then_args: vec![],
        else_block,
        else_args: vec![],
    };

    func.blocks.insert(
        handler_block,
        TirBlock {
            id: handler_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![handler_value],
                attrs: handler_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![handler_value],
            },
        },
    );
    func.blocks.insert(
        else_block,
        TirBlock {
            id: else_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![else_value],
                attrs: else_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![else_value],
            },
        },
    );
    func.label_id_map.insert(handler_block.0, 100);

    let ops = lower_to_simple_ir(&func);

    assert!(
        validate_labels(&ops),
        "exception handler labels must survive lowering: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|op| matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(100)),
        "handler target label 100 must remain materialized: {ops:?}"
    );
}

#[test]
fn emit_guard_raise_path_keeps_cleanup_blocks_after_raise() {
    let mut func = TirFunction::new(
        "emit_guard_raise_path_keeps_cleanup_blocks_after_raise".into(),
        vec![],
        TirType::I64,
    );
    let raise_block = func.fresh_block();
    let cleanup_block = func.fresh_block();
    let raise_value = func.fresh_value();
    let cleanup_value = func.fresh_value();

    let mut raise_attrs = AttrDict::new();
    raise_attrs.insert("value".into(), AttrValue::Int(7));
    func.blocks.insert(
        raise_block,
        TirBlock {
            id: raise_block,
            args: vec![],
            ops: vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![raise_value],
                    attrs: raise_attrs,
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Raise,
                    operands: vec![raise_value],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            ],
            terminator: Terminator::Branch {
                target: cleanup_block,
                args: vec![],
            },
        },
    );

    let mut cleanup_attrs = AttrDict::new();
    cleanup_attrs.insert("value".into(), AttrValue::Int(2));
    func.blocks.insert(
        cleanup_block,
        TirBlock {
            id: cleanup_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![cleanup_value],
                attrs: cleanup_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![cleanup_value],
            },
        },
    );

    let block_param_vars = HashMap::from([(raise_block, Vec::new()), (cleanup_block, Vec::new())]);
    let mut out = Vec::new();
    let labels = HashMap::from([(raise_block, 99_i64), (cleanup_block, 100_i64)]);
    let original_label_to_block = HashMap::from([(99_i64, raise_block), (100_i64, cleanup_block)]);
    let block_label_id = |bid: &BlockId| -> i64 { *labels.get(bid).expect("missing test label") };

    emit_guard_raise_path(
        raise_block,
        &[],
        &HashSet::from([raise_block, cleanup_block]),
        &func,
        &block_param_vars,
        &block_label_id,
        &HashSet::new(),
        &HashMap::new(),
        &original_label_to_block,
        &mut out,
    );

    assert!(
        validate_labels(&out),
        "guard raise path lowering must keep labels reachable after a raise block: {out:?}"
    );
    assert!(
        out.iter()
            .any(|op| matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(100)),
        "cleanup label 100 must remain materialized after a raise-and-branch chain: {out:?}"
    );
}

#[test]
fn explicit_loop_cond_block_is_not_reclassified_as_guard_when_exit_raises() {
    let mut func = TirFunction::new(
        "explicit_loop_cond_block_is_not_reclassified_as_guard_when_exit_raises".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
    );
    let header = func.entry_block;
    let cond = func.fresh_block();
    let exit_raise = func.fresh_block();
    let body = func.fresh_block();
    let nested_cond = func.fresh_block();
    let nested_then = func.fresh_block();
    let nested_join = func.fresh_block();
    let cleanup = func.fresh_block();
    let raise_value = func.fresh_value();

    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_break_kinds
        .insert(header, LoopBreakKind::BreakIfTrue);
    func.loop_cond_blocks.insert(header, cond);

    func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
        target: cond,
        args: vec![],
    };
    func.blocks.insert(
        cond,
        TirBlock {
            id: cond,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: exit_raise,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit_raise,
        TirBlock {
            id: exit_raise,
            args: vec![],
            ops: vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![raise_value],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Raise,
                    operands: vec![raise_value],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            ],
            terminator: Terminator::Branch {
                target: cleanup,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: nested_cond,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        nested_cond,
        TirBlock {
            id: nested_cond,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(1),
                then_block: nested_then,
                then_args: vec![],
                else_block: nested_join,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        nested_then,
        TirBlock {
            id: nested_then,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: nested_join,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        nested_join,
        TirBlock {
            id: nested_join,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        cleanup,
        TirBlock {
            id: cleanup,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);

    assert!(
        validate_labels(&ops),
        "explicit loop condition lowering must not leave dangling labels: {ops:?}"
    );
}

/// Regression for the coroutine/generator `_poll` label-roundtrip panic
/// (compiler-foundation Tier-1 C3).
///
/// A state-machine `_poll` re-enters a loop's CONDITION block from a resume
/// point OUTSIDE the loop region (an explicit `jump <cond_label>` after a
/// yield/await suspension).  The structured-loop reconstruction must NOT
/// consume such a cond block inline — doing so drops its label while the
/// external resume jump still references it, producing
/// `TIR roundtrip emitted invalid labels for '..._poll'`.
///
/// This builds that exact CFG directly: `header → cond` is the loop, but an
/// out-of-region `resume` block (reachable from entry via a switch-like
/// dispatch) branches straight into `cond`.  The fix declines structured
/// reconstruction here and falls back to label-preserving generic lowering,
/// so the cond block keeps its label and every `jump` resolves.
#[test]
fn loop_cond_with_external_reentry_keeps_label_no_dangling() {
    let mut func = TirFunction::new(
        "state_machine_poll__loop_cond_external_reentry".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
    );
    let entry = func.entry_block;
    let header = func.fresh_block();
    let cond = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let resume = func.fresh_block();

    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_break_kinds
        .insert(header, LoopBreakKind::BreakIfTrue);
    func.loop_cond_blocks.insert(header, cond);

    // Record cond's label so the external resume edge references it by the
    // same id the back-conversion will emit — mirroring a real `_poll`
    // resume-dispatch jump.  (label_id_map is keyed by raw block index.)
    func.label_id_map.insert(cond.0, 36);

    // entry: state-dispatch — either fall into the loop header (first poll)
    // or jump to the resume point (re-entry after a suspension).
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: resume,
        then_args: vec![],
        else_block: header,
        else_args: vec![],
    };

    // header → cond (loop entry / back-edge merge point).
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: cond,
                args: vec![],
            },
        },
    );
    // cond: BreakIfTrue → exit on true, body on false.
    func.blocks.insert(
        cond,
        TirBlock {
            id: cond,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(1),
                then_block: exit,
                then_args: vec![],
                else_block: body,
                else_args: vec![],
            },
        },
    );
    // body → header (back-edge).
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );
    // resume: the out-of-region re-entry point — jumps straight to cond.
    func.blocks.insert(
        resume,
        TirBlock {
            id: resume,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: cond,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);

    assert!(
        validate_labels(&ops),
        "external re-entry into a loop cond block must not leave dangling \
             labels (state-machine _poll roundtrip): {ops:?}"
    );
    // The cond block's label (36) must survive: the generic fallback
    // emits it so the resume `jump` resolves.
    assert!(
        ops.iter()
            .any(|op| matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(36)),
        "loop cond label (36) must remain materialized when re-entered \
             from outside the region: {ops:?}"
    );
}

/// Round-9: a body-interior block that is BOTH the loop's back-edge target
/// AND entered directly from outside the region (a shared pre-header/latch:
/// `entry → P → header`, with the back-edge also routing `body → P → header`).
/// `P` lands in the body DFS (it is reached from `body_entry` via the
/// back-edge), so it is consumed into the region, yet it still carries the
/// external entry edge from `entry`. The single-entry-region guard must
/// detect that `P` (≠ header) has an external predecessor and decline
/// structured reconstruction; the generic fallback then emits `P`'s label so
/// the entry's `jump P` resolves. Before the guard generalization this body
/// block's label was merged away and the entry jump dangled — the native
/// `label_blocks[&target]` "no entry found for key" panic and WASM
/// "unknown jump label" miscompile that blocked the native drop flip on
/// `typing._typing_strip_wrapping_parens`.
#[test]
fn loop_shared_preheader_latch_body_keeps_label_no_dangling() {
    let mut func = TirFunction::new(
        "shared_preheader_latch__keeps_label".into(),
        vec![TirType::Bool],
        TirType::None,
    );
    let entry = func.entry_block;
    let latch = func.fresh_block(); // P: pre-header AND back-edge target
    let header = func.fresh_block();
    let cond = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_break_kinds
        .insert(header, LoopBreakKind::BreakIfFalse);
    func.loop_cond_blocks.insert(header, cond);

    // Record latch's label so the external entry edge references it by the
    // same id the back-conversion will emit (label_id_map is keyed by raw
    // block index) — mirroring the real entry `jump <latch_label>`.
    func.label_id_map.insert(latch.0, 62);

    // entry → latch (the external entry edge into the loop's pre-header).
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: latch,
        args: vec![],
    };
    // latch → header (both the pre-header path and the back-edge funnel
    // through here).
    func.blocks.insert(
        latch,
        TirBlock {
            id: latch,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );
    // header → cond.
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: cond,
                args: vec![],
            },
        },
    );
    // cond: BreakIfFalse → body on true, exit on false.
    func.blocks.insert(
        cond,
        TirBlock {
            id: cond,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    // body → latch (the back-edge routes through the shared pre-header/latch,
    // pulling `latch` into the body DFS).
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: latch,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);

    assert!(
        validate_labels(&ops),
        "a shared pre-header/latch body block with an external entry edge \
             must not leave a dangling jump label: {ops:?}"
    );
    // The latch's label (62) must survive: the generic fallback emits it so
    // the entry `jump 62` resolves instead of dangling.
    assert!(
        ops.iter()
            .any(|op| matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(62)),
        "shared pre-header/latch label (62) must remain materialized when \
             entered from outside the loop region: {ops:?}"
    );
}

#[test]
fn loop_guard_raise_chain_keeps_cleanup_handler_label() {
    let mut func = TirFunction::new(
        "loop_guard_raise_chain_keeps_cleanup_handler_label".into(),
        vec![TirType::Bool, TirType::Bool, TirType::Bool],
        TirType::I64,
    );

    let header = func.fresh_block();
    let guard = func.fresh_block();
    let cond_block = func.fresh_block();
    let raise_block = func.fresh_block();
    let body_block = func.fresh_block();
    let exit_block = func.fresh_block();
    let cleanup_block = func.fresh_block();
    let return_block = func.fresh_block();
    let continue_block = func.fresh_block();

    let raise_value = func.fresh_value();
    let exit_value = func.fresh_value();
    let cleanup_value = func.fresh_value();
    let return_value = func.fresh_value();

    let mut handler_attrs = AttrDict::new();
    handler_attrs.insert("value".into(), AttrValue::Int(100));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs: handler_attrs.clone(),
        source_span: None,
    });
    entry.terminator = Terminator::Branch {
        target: header,
        args: vec![],
    };

    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: guard,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        guard,
        TirBlock {
            id: guard,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: raise_block,
                then_args: vec![],
                else_block: cond_block,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        cond_block,
        TirBlock {
            id: cond_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(1),
                then_block: body_block,
                then_args: vec![],
                else_block: exit_block,
                else_args: vec![],
            },
        },
    );

    let mut raise_attrs = AttrDict::new();
    raise_attrs.insert("value".into(), AttrValue::Int(7));
    func.blocks.insert(
        raise_block,
        TirBlock {
            id: raise_block,
            args: vec![],
            ops: vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![raise_value],
                    attrs: raise_attrs,
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Raise,
                    operands: vec![raise_value],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            ],
            terminator: Terminator::Branch {
                target: cleanup_block,
                args: vec![],
            },
        },
    );

    let mut exit_attrs = AttrDict::new();
    exit_attrs.insert("value".into(), AttrValue::Int(0));
    func.blocks.insert(
        exit_block,
        TirBlock {
            id: exit_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![exit_value],
                attrs: exit_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![exit_value],
            },
        },
    );

    func.blocks.insert(
        body_block,
        TirBlock {
            id: body_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: handler_attrs.clone(),
                source_span: None,
            }],
            terminator: Terminator::CondBranch {
                cond: ValueId(2),
                then_block: return_block,
                then_args: vec![],
                else_block: continue_block,
                else_args: vec![],
            },
        },
    );

    let mut return_attrs = AttrDict::new();
    return_attrs.insert("value".into(), AttrValue::Int(1));
    func.blocks.insert(
        return_block,
        TirBlock {
            id: return_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![return_value],
                attrs: return_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![return_value],
            },
        },
    );

    func.blocks.insert(
        continue_block,
        TirBlock {
            id: continue_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: handler_attrs.clone(),
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: header,
                args: vec![],
            },
        },
    );

    let mut cleanup_attrs = AttrDict::new();
    cleanup_attrs.insert("value".into(), AttrValue::Int(2));
    func.blocks.insert(
        cleanup_block,
        TirBlock {
            id: cleanup_block,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![cleanup_value],
                attrs: cleanup_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![cleanup_value],
            },
        },
    );

    func.has_exception_handling = true;
    func.label_id_map.insert(cleanup_block.0, 100);
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_break_kinds
        .insert(header, LoopBreakKind::BreakIfFalse);
    func.loop_cond_blocks.insert(header, cond_block);

    let ops = lower_to_simple_ir(&func);

    assert!(
        validate_labels(&ops),
        "guard raise cleanup handler labels must survive structured loop lowering: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|op| op.kind == "check_exception" && op.value == Some(100)),
        "check_exception must keep targeting handler label 100: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|op| matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(100)),
        "cleanup handler label 100 must remain materialized: {ops:?}"
    );
}
