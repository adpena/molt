use super::*;

#[test]
fn run_inliner_inlines_const_call() {
    // g() { x = constfn(); return x + 1 }, constfn() = 42.
    // After inlining + re-running the pipeline, the Call is gone, the merged
    // function is valid SSA, and the callee's `const 42` now lives inside g
    // (the call boundary is eliminated). The downstream `const(42)+1 → 43`
    // arithmetic fold across the continuation block-argument is the
    // backend's / a future jump-threading pass's job — verified end-to-end
    // by the differential test, not asserted here (the current per-function
    // pipeline has no single-predecessor block-coalescing pass).
    let callee = const_callee();
    let caller = caller_calling_const("constfn");
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1, "one site inlined");
    assert_eq!(stats.functions_changed, 1, "g changed");
    // No Call op remains in g.
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "constfn call eliminated from g");
    // g is valid SSA after the pipeline re-run.
    crate::tir::verify::verify_function(g)
        .unwrap_or_else(|e| panic!("g invalid after inlining: {e:?}"));
    // The inlined callee's `const 42` is now part of g's body.
    let has_const_42 = g.blocks.values().any(|b| {
        b.ops.iter().any(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(42)))
        })
    });
    assert!(has_const_42, "callee's const 42 inlined into g");
}

#[test]
fn run_inliner_inlines_add_call_with_args() {
    // g(p, q) { return addfn(p, q) }, addfn(a, b) = a + b.
    let callee = add_callee();
    let mut g = TirFunction::new("g".into(), vec![TirType::I64, TirType::I64], TirType::I64);
    let p = ValueId(0);
    let q = ValueId(1);
    let res = g.fresh_value();
    let entry = g.entry_block;
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str("addfn".into()));
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![p, q],
        results: vec![res],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![res] };

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1);
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "addfn call eliminated");
    crate::tir::verify::verify_function(g).unwrap_or_else(|e| panic!("g invalid: {e:?}"));
    // The inlined body's Add (a+b with a=p, b=q) is present and uses the
    // caller's params directly.
    let add_uses_params = g.blocks.values().any(|b| {
        b.ops
            .iter()
            .any(|op| op.opcode == OpCode::Add && op.operands == vec![p, q])
    });
    assert!(add_uses_params, "inlined add uses caller params directly");
}

#[test]
fn run_inliner_two_sites_same_block_both_inlined() {
    // g() { x = constfn(); y = constfn(); return x + y } — two calls to the
    // same inlinable leaf in one block. The reverse-order driver must splice
    // BOTH (a refused/early site must not block the other). After inlining,
    // zero Call ops remain and SSA is valid.
    let callee = const_callee();
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let x = g.fresh_value();
    let y = g.fresh_value();
    let sum = g.fresh_value();
    let entry = g.entry_block;
    let mk_call = |name: &str, out: ValueId| {
        let mut a = AttrDict::new();
        a.insert("s_value".into(), AttrValue::Str(name.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![out],
            attrs: a,
            source_span: None,
        }
    };
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(mk_call("constfn", x));
    block.ops.push(mk_call("constfn", y));
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![x, y],
        results: vec![sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![sum] };

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 2, "both call sites inlined");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "both constfn calls eliminated");
    crate::tir::verify::verify_function(g)
        .unwrap_or_else(|e| panic!("g invalid after 2-site inlining: {e:?}"));
    // Two distinct const-42 ops now live in g (one per inlined site).
    let const_42_count: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| {
            op.opcode == OpCode::ConstInt
                && matches!(op.attrs.get("value"), Some(AttrValue::Int(42)))
        })
        .count();
    assert_eq!(
        const_42_count, 2,
        "each inlined site contributes a const 42"
    );
}

// -- (c) exception-observation inlining ----------------------------------

#[test]
fn run_inliner_inlines_observation_callee_end_to_end() {
    // End-to-end through run_inliner (clone + splice + per-function pipeline
    // re-run): a value-returning observation-only callee is inlined, the Call
    // is gone, and the merged caller is valid SSA with collision-free labels.
    let callee = observation_callee("obs", 3);
    let caller = caller_calling_obs_with_label("c", "obs", 3);
    let mut m = module(vec![caller, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 1, "obs inlined into c");
    let c = m.functions.iter().find(|f| f.name == "c").unwrap();
    let calls: usize = c
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "obs call eliminated from c");
    crate::tir::verify::verify_function(c)
        .unwrap_or_else(|e| panic!("c invalid after observation inlining: {e:?}"));
    // Labels remain collision-free after the pipeline re-run.
    let labels: Vec<i64> = c.label_id_map.values().copied().collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), labels.len(), "labels distinct: {labels:?}");
}
