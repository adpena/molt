use super::*;

#[test]
fn splice_removes_call_and_passes_verify() {
    let callee = const_callee();
    let mut caller = caller_calling_const("constfn");
    let site = collect_call_sites(&caller, &["constfn".to_string()]);
    assert_eq!(site.len(), 1);
    let did = splice_call_site(&mut caller, &callee, &site[0]);
    assert!(did, "splice succeeded");
    // No Call op remains anywhere.
    let remaining_calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(remaining_calls, 0, "the Call was eliminated");
    // The merged function is valid SSA.
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
}

#[test]
fn splice_void_return() {
    // Callee returns nothing; caller calls it for effect.
    let mut callee = TirFunction::new("eff".into(), vec![], TirType::None);
    let entry = callee.entry_block;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new("g".into(), vec![], TirType::None);
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str("eff".into()));
    let centry = caller.entry_block;
    let block = caller.blocks.get_mut(&centry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };

    let sites = collect_call_sites(&caller, &["eff".to_string()]);
    assert_eq!(sites.len(), 1);
    assert!(splice_call_site(&mut caller, &callee, &sites[0]));
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("void-splice invalid: {e:?}"));
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0);
}

#[test]
fn refcount_guard_refuses_arg_incref() {
    // Caller: IncRef(arg); call f(arg). The guard must refuse the splice.
    let mut callee = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
    let centry = callee.entry_block;
    callee.blocks.get_mut(&centry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new("g".into(), vec![TirType::DynBox], TirType::None);
    let arg = ValueId(0); // the caller's param
    let entry = caller.entry_block;
    let mut call_attrs = AttrDict::new();
    call_attrs.insert("s_value".into(), AttrValue::Str("f".into()));
    let block = caller.blocks.get_mut(&entry).unwrap();
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::IncRef,
        operands: vec![arg],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![arg],
        results: vec![],
        attrs: call_attrs,
        source_span: None,
    });
    block.terminator = Terminator::Return { values: vec![] };

    let sites = collect_call_sites(&caller, &["f".to_string()]);
    assert_eq!(sites.len(), 1);
    assert!(
        !splice_call_site(&mut caller, &callee, &sites[0]),
        "refcount guard must refuse a site with arg IncRef before the call"
    );
    // The call survives intact.
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 1, "refused site keeps its call");
}

// -- is_inlineable gates -------------------------------------------------

/// The set of every label value in `func`'s `label_id_map` plus every
/// exception-op `"value"` label, used to assert collision-freedom.
fn all_labels(func: &TirFunction) -> Vec<i64> {
    function_label_ids(func).into_iter().collect()
}

#[test]
fn splice_observation_callee_remaps_labels_collision_free() {
    // Callee exception label 3; caller ALSO uses label 3 (collision). After
    // splicing, the cloned exit block must carry a FRESH label (not 3), the
    // caller's original label 3 must survive, and no two blocks may share a
    // label value (which would make `exception_label_to_block` ambiguous and
    // emit duplicate `label N` ops in lower_to_simple — a miscompile).
    let callee = observation_callee("obs", 3);
    let mut caller = caller_calling_obs_with_label("c", "obs", 3);

    let sites = collect_call_sites(&caller, &["obs".to_string()]);
    assert_eq!(sites.len(), 1);
    assert!(splice_call_site(&mut caller, &callee, &sites[0]), "spliced");

    // No two blocks share a label value.
    let labels: Vec<i64> = caller.label_id_map.values().copied().collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        labels.len(),
        "every block label is distinct (no collision): {labels:?}"
    );
    // The caller's original label 3 survived.
    assert!(all_labels(&caller).contains(&3), "caller label 3 preserved");

    // Every cloned CheckException's handler label resolves to a block that
    // carries that exact label in label_id_map (the exception edge resolves).
    let label_to_block: std::collections::HashMap<i64, BlockId> = caller
        .label_id_map
        .iter()
        .map(|(b, l)| (*l, BlockId(*b)))
        .collect();
    for block in caller.blocks.values() {
        for op in &block.ops {
            if let Some(label) = exception_label_of(op) {
                assert!(
                    label_to_block.contains_key(&label),
                    "CheckException label {label} resolves to a block"
                );
            }
        }
    }
    // The merged function is valid SSA.
    crate::tir::verify::verify_function(&caller)
        .unwrap_or_else(|e| panic!("merged fn invalid SSA: {e:?}"));
    // The Call is gone.
    let calls: usize = caller
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Call)
        .count();
    assert_eq!(calls, 0, "obs call eliminated");
}

#[test]
fn splice_void_exception_exit_pads_placeholder_for_continuation_type() {
    // The observation callee's exception-exit returns NO value, but the call
    // wants one. The splice must NOT refuse (that would re-dormant the inliner
    // on every value-returning observation callee) — it pads the continuation
    // arg with a representation-matched dead placeholder. The exit branch ends
    // up supplying exactly one continuation arg, and the merged fn verifies.
    for (idx, ty) in [TirType::I64, TirType::Bool, TirType::F64, TirType::DynBox]
        .into_iter()
        .enumerate()
    {
        let callee_name = format!("obs_{idx}");
        let caller_name = format!("c_{idx}");
        let callee = observation_callee_with_type(&callee_name, 30 + idx as i64, ty.clone());
        let mut caller =
            caller_calling_obs_with_label_and_type(&caller_name, &callee_name, 90, ty.clone());

        let sites = collect_call_sites(&caller, std::slice::from_ref(&callee_name));
        assert_eq!(sites.len(), 1);
        assert!(
            splice_call_site(&mut caller, &callee, &sites[0]),
            "value-returning observation callee inlines (not refused) for {ty:?}"
        );
        crate::tir::verify::verify_function(&caller).unwrap_or_else(|e| {
            panic!("merged fn invalid SSA after placeholder pad for {ty:?}: {e:?}")
        });

        let mut placeholder_const_seen = false;
        for block in caller.blocks.values() {
            if let Terminator::Branch { args, .. } = &block.terminator
                && args.len() == 1
                && block
                    .ops
                    .last()
                    .map(|op| {
                        let expected = dead_placeholder_const_for_type(&ty, args[0]);
                        op.opcode == expected.opcode
                            && op.operands == expected.operands
                            && op.results == expected.results
                            && op.attrs == expected.attrs
                    })
                    .unwrap_or(false)
            {
                placeholder_const_seen = true;
            }
        }
        assert!(
            placeholder_const_seen,
            "the void exception-exit branch is padded with a {ty:?} placeholder const"
        );
    }
}
