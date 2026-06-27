use super::*;

#[test]
fn recursive_not_inlined() {
    // f calls f → recursive.
    let mut f = TirFunction::new("f".into(), vec![], TirType::None);
    let entry = f.entry_block;
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("f".into()));
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
}

#[test]
fn too_large_not_inlined() {
    // A callee with op_count > budget.
    let mut f = TirFunction::new("big".into(), vec![], TirType::I64);
    let entry = f.entry_block;
    let tti = TargetInfo::native_release_fast();
    let budget = tti.inline_budget("big");
    // Allocate value ids first (avoid overlapping borrows).
    let vals: Vec<ValueId> = (0..budget + 5).map(|_| f.fresh_value()).collect();
    let block = f.blocks.get_mut(&entry).unwrap();
    for v in &vals {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(1));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![*v],
            attrs,
            source_span: None,
        });
    }
    block.terminator = Terminator::Return {
        values: vec![vals[0]],
    };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    assert!(
        !is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "callee over budget is not inlinable"
    );
}

#[test]
fn generator_not_inlined() {
    let mut f = TirFunction::new("gen".into(), vec![], TirType::None);
    let entry = f.entry_block;
    let block = f.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Yield,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
}

#[test]
fn entry_predecessor_callee_not_inlined() {
    // A callee whose entry block is a branch target (a back-edge to entry)
    // cannot be spliced by the direct-param-binding model — refuse it.
    let mut f = TirFunction::new("looper".into(), vec![], TirType::None);
    let body = f.fresh_block();
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![],
            // body branches BACK to the entry → entry has a predecessor.
            terminator: Terminator::Branch {
                target: f.entry_block,
                args: vec![],
            },
        },
    );
    let entry = f.entry_block;
    f.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
        target: body,
        args: vec![],
    };
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(
        !is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "callee with entry-block predecessor is not inlinable this arc"
    );
}

#[test]
fn handler_bearing_callee_not_inlined() {
    // A callee with a REAL exception handler region (TryStart/TryEnd) is
    // excluded — splicing across a handler boundary needs handler-label
    // re-targeting this arc does not perform.
    let mut f = TirFunction::new("guarded".into(), vec![], TirType::None);
    f.has_exception_handling = true;
    let entry = f.entry_block;
    {
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryStart,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryEnd,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
    }
    assert!(f.has_exception_handlers(), "TryStart/TryEnd => handlers");
    let m = module(vec![f]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(!is_inlineable(&m.functions[0], &cg, &sm, &tti));
}

#[test]
fn observation_only_callee_is_inlineable() {
    // An OBSERVATION-only callee (CheckException, no handler region) IS
    // inlinable even though `has_exception_handling` is set: it has no real
    // handler, so `has_exception_handlers()` is false.
    let callee = observation_callee("obs", 3);
    assert!(
        callee.has_exception_handling,
        "CheckException sets has_exception_handling"
    );
    assert!(
        !callee.has_exception_handlers(),
        "no TryStart/TryEnd/StateBlock => no handler region"
    );
    let caller = caller_calling_obs("c", "obs");
    let m = module(vec![callee, caller]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let obs = m.functions.iter().find(|f| f.name == "obs").unwrap();
    assert!(
        is_inlineable(obs, &cg, &sm, &tti),
        "observation-only callee is inlinable"
    );
}

#[test]
fn closure_callee_not_inlined() {
    // task #44: a closure (first param == __molt_closure__) must NOT be
    // inlinable. The direct param->operand splice cannot bind the captured
    // env (it would bind the call's leading function-value operand instead),
    // so `is_inlineable` refuses it — conservative-correct exclusion.
    let callee = closure_callee("__main____add");
    assert!(
        is_closure(&callee),
        "first param is the env marker => closure"
    );
    let m = module(vec![callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(
        !is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "closure callee must be refused (task #44 miscompile gate)"
    );
}

#[test]
fn non_closure_same_arity_still_inlineable() {
    // The WIN must survive: a NON-closure 2-param callee (param_names do NOT
    // start with the env marker) is still inlinable. The closure gate keys on
    // the marker, not on arity, so a legitimate same-arity function is never
    // de-inlined by the fix.
    let callee = add_callee(); // params ["p0", "p1"] — not a closure
    assert!(
        !is_closure(&callee),
        "add_callee's first param is not the env marker"
    );
    let m = module(vec![callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    assert!(
        is_inlineable(&m.functions[0], &cg, &sm, &tti),
        "non-closure same-arity callee stays inlinable (perf win preserved)"
    );
}

#[test]
fn run_inliner_refuses_closure_call_site() {
    // End-to-end through the production chokepoint: a caller that calls a
    // closure must NOT have the call spliced away. The Call op survives and
    // the closure body is NOT cloned into the caller (no Index over the
    // function value), so the miscompile cannot occur.
    let callee = closure_callee("__main____add");
    // caller g(): r = __main____add(<func>, 10); return r.
    // Operands model the real call ABI [callee_value, arg0].
    let mut g = TirFunction::new("g".into(), vec![], TirType::I64);
    let func_val = g.fresh_value();
    let ten = g.fresh_value();
    let res = g.fresh_value();
    let entry = g.entry_block;
    let mut fattrs = AttrDict::new();
    fattrs.insert("value".into(), AttrValue::Int(0)); // stand-in producer
    let mut tattrs = AttrDict::new();
    tattrs.insert("value".into(), AttrValue::Int(10));
    let mut call_attrs = AttrDict::new();
    call_attrs.insert(
        "s_value".into(),
        AttrValue::Str("__main____add".to_string()),
    );
    let block = g.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![func_val],
        attrs: fattrs,
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![ten],
        attrs: tattrs,
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![func_val, ten],
        results: vec![res],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![res] };
    g.value_types.insert(res, TirType::I64);

    let mut m = module(vec![g, callee]);
    let (cg, sm) = analysis(&m);
    let tti = TargetInfo::native_release_fast();
    let stats = run_inliner(&mut m, &cg, &sm, &tti, &HashSet::new());
    assert_eq!(stats.sites_inlined, 0, "closure call site is NOT inlined");
    let g = m.functions.iter().find(|f| f.name == "g").unwrap();
    let calls: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 1, "the Call op survives (closure not spliced)");
    // No Index op leaked into the caller (the closure body was not cloned).
    let indexes: usize = g
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Index)
        .count();
    assert_eq!(indexes, 0, "closure body not cloned into caller");
}

// -- run_inliner end-to-end ----------------------------------------------
