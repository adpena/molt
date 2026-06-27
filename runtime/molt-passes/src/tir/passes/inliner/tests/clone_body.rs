use super::*;

#[test]
fn clone_produces_disjoint_ids() {
    let callee = add_callee();
    let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
    // Two argument values already live in the caller.
    let a = caller.fresh_value();
    let b = caller.fresh_value();
    let before_next_value = caller.next_value;
    let before_next_block = caller.next_block;

    let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[a, b]);

    // The clone minted fresh value + block ids (the add result is fresh).
    assert!(caller.next_value > before_next_value, "value ids advanced");
    assert!(caller.next_block > before_next_block, "block ids advanced");
    // The cloned entry block exists and has NO args (params bound to a, b).
    let entry = &caller.blocks[&cloned.entry];
    assert!(
        entry.args.is_empty(),
        "cloned entry has no args (params bound)"
    );
    // The cloned Add uses the caller's arg values directly (a, b).
    let add = &entry.ops[0];
    assert_eq!(add.opcode, OpCode::Add);
    assert_eq!(add.operands, vec![a, b], "params bound directly to args");
    // The cloned add result is a fresh id, disjoint from a/b.
    assert!(add.results[0] != a && add.results[0] != b);
}

#[test]
fn clone_entry_has_empty_args() {
    let callee = add_callee();
    let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
    let a = caller.fresh_value();
    let b = caller.fresh_value();
    let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[a, b]);
    assert!(caller.blocks[&cloned.entry].args.is_empty());
}

#[test]
fn clone_transfers_all_loop_metadata() {
    // A callee with a header block carrying every loop-metadata kind.
    let mut callee = TirFunction::new("loopfn".into(), vec![], TirType::None);
    let header = callee.fresh_block();
    let end = callee.fresh_block();
    let cond = callee.fresh_block();
    // Give the entry a branch into the header; header/end/cond are trivial
    // blocks so the clone walk has them to remap.
    for bid in [header, end, cond] {
        callee.blocks.insert(
            bid,
            TirBlock {
                id: bid,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    let entry = callee.entry_block;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: header,
        args: vec![],
    };
    // Now wire all four loop maps + a label.
    callee.loop_roles.insert(header, LoopRole::LoopHeader);
    callee.loop_roles.insert(end, LoopRole::LoopEnd);
    callee.loop_pairs.insert(header, end);
    callee
        .loop_break_kinds
        .insert(header, LoopBreakKind::BreakIfTrue);
    callee.loop_cond_blocks.insert(header, cond);
    callee.label_id_map.insert(header.0, 7);

    let mut caller = TirFunction::new("caller".into(), vec![], TirType::None);
    let cloned = clone_function_body_with_fresh_ids(&callee, &mut caller, &[]);

    // All four maps + label_id_map must have one remapped entry each.
    assert_eq!(caller.loop_roles.len(), 2, "loop_roles transferred");
    assert_eq!(caller.loop_pairs.len(), 1, "loop_pairs transferred");
    assert_eq!(
        caller.loop_break_kinds.len(),
        1,
        "loop_break_kinds transferred"
    );
    assert_eq!(
        caller.loop_cond_blocks.len(),
        1,
        "loop_cond_blocks transferred"
    );
    assert_eq!(caller.label_id_map.len(), 1, "label_id_map transferred");
    // None of the transferred keys are the callee's original ids — they were
    // remapped to fresh caller block ids.
    assert!(!caller.loop_roles.contains_key(&header));
    assert!(!caller.loop_pairs.contains_key(&header));
    // The cloned entry is a fresh block (not the callee's BlockId(0)).
    assert!(cloned.entry != callee.entry_block || caller.next_block > callee.next_block);
}

// -- (b) splice ----------------------------------------------------------

#[test]
fn clone_attrs_without_simple_names_drops_only_value_names() {
    // The strip helper drops `_simple_out` and `_simple_result_N` (the
    // collision-prone SimpleIR value-name annotations) but preserves every
    // other attribute verbatim (e.g. the call symbol or a const value).
    let mut attrs = AttrDict::new();
    attrs.insert("_simple_out".into(), AttrValue::Str("x".into()));
    attrs.insert("_simple_result_0".into(), AttrValue::Str("y".into()));
    attrs.insert("_simple_result_1".into(), AttrValue::Str("z".into()));
    attrs.insert("s_value".into(), AttrValue::Str("callee".into()));
    attrs.insert("value".into(), AttrValue::Int(7));
    let stripped = clone_attrs_without_simple_names(&attrs);
    assert!(!stripped.contains_key("_simple_out"), "_simple_out dropped");
    assert!(
        !stripped.contains_key("_simple_result_0"),
        "_simple_result_0 dropped"
    );
    assert!(
        !stripped.contains_key("_simple_result_1"),
        "_simple_result_1 dropped"
    );
    assert_eq!(
        stripped.get("s_value"),
        Some(&AttrValue::Str("callee".into())),
        "s_value preserved"
    );
    assert_eq!(
        stripped.get("value"),
        Some(&AttrValue::Int(7)),
        "value preserved"
    );
}

#[test]
fn inlined_ops_do_not_inherit_callee_simple_out_names() {
    // A callee whose op carries `_simple_out: "collide"` is inlined into a
    // caller that has its OWN op with the SAME `_simple_out: "collide"`. After
    // inlining, the name must appear on exactly ONE op (the caller's
    // original): the cloned callee op must have shed the name, so a name-keyed
    // container-dispatch lookup cannot resolve the inlined value to the
    // caller's kind. Before the strip, the merged body had two ops named
    // "collide" — a latent miscompile.
    let mut callee = TirFunction::new("c".into(), vec![], TirType::I64);
    let cv = callee.fresh_value();
    {
        let entry = callee.entry_block;
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(1));
        attrs.insert("_simple_out".into(), AttrValue::Str("collide".into()));
        let block = callee.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![cv],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![cv] };
    }
    callee.value_types.insert(cv, TirType::I64);

    // caller g(): own = const 9 (named "collide"); r = c(); return own.
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let own = g.fresh_value();
    let call_res = g.fresh_value();
    {
        let entry = g.entry_block;
        let mut own_attrs = AttrDict::new();
        own_attrs.insert("value".into(), AttrValue::Int(9));
        own_attrs.insert("_simple_out".into(), AttrValue::Str("collide".into()));
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str("c".into()));
        let block = g.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![own],
            attrs: own_attrs,
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![call_res],
            attrs: call_attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![own] };
    }
    g.value_types.insert(own, TirType::I64);
    g.value_types.insert(call_res, TirType::I64);

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1, "c() inlined into g");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let collide_count: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.attrs.get("_simple_out") == Some(&AttrValue::Str("collide".into())))
        .count();
    assert_eq!(
        collide_count, 1,
        "only the caller's own op keeps the name; the inlined op shed it"
    );
}
