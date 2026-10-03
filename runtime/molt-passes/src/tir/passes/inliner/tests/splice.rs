use super::*;

#[test]
fn splice_removes_call_and_passes_verify() {
    let callee = const_callee();
    let mut caller = caller_calling_const("constfn");
    let site = collect_call_sites(&caller, &["constfn".to_string()]);
    assert_eq!(site.len(), 1);
    let did = splice_call_site(&mut caller, &callee, &site[0], &mut Activations::default());
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
    let mut callee = TirFunction::new(
        "eff".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let entry = callee.entry_block;
    callee.blocks.get_mut(&entry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new(
        "g".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
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
    let spliced = splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default());
    assert!(spliced);
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
    let mut callee = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let centry = callee.entry_block;
    callee.blocks.get_mut(&centry).unwrap().terminator = Terminator::Return { values: vec![] };

    let mut caller = TirFunction::new(
        "g".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
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
    let spliced = splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default());
    assert!(
        !spliced,
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
    // emit duplicate `label N` ops in lower_to_simple - a miscompile).
    let callee = observation_callee("obs", 3);
    let mut caller = caller_calling_obs_with_label("c", "obs", 3);

    let sites = collect_call_sites(&caller, &["obs".to_string()]);
    assert_eq!(sites.len(), 1);
    let spliced = splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default());
    assert!(spliced, "spliced");

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
fn splice_void_exception_exit_branches_directly_to_post_call_exception_target() {
    // The observation callee's exception-exit returns NO value, but the call
    // wants one. The splice must NOT refuse (that would re-dormant the inliner
    // on every value-returning observation callee). With a post-call
    // CheckException at the continuation start, the exception-exit branch goes
    // directly to the caller handler, and the merged fn verifies.
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
        let spliced =
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default());
        assert!(
            spliced,
            "value-returning observation callee inlines (not refused) for {ty:?}"
        );
        crate::tir::verify::verify_function(&caller).unwrap_or_else(|e| {
            panic!("merged fn invalid SSA after direct exception branch for {ty:?}: {e:?}")
        });

        let handler = caller
            .label_id_map
            .iter()
            .find_map(|(block, label)| (*label == 90).then_some(BlockId(*block)))
            .expect("caller exception handler label survives");
        let direct_exception_branches = caller
            .blocks
            .values()
            .filter(|block| {
                matches!(
                    &block.terminator,
                    Terminator::Branch { target, args } if *target == handler && args.is_empty()
                )
            })
            .count();
        assert!(
            direct_exception_branches >= 1,
            "void exception-exit branches directly to the caller handler for {ty:?}"
        );
    }
}

/// The spliced activation binds each parameter by its custody. A callee that
/// owns its parameter binds the caller's temporary through one owned
/// `binding_alias` where the call was; its return captures that binding first,
/// and each exit, the exception exit included, releases the binding before it
/// leaves. A borrowed parameter whose argument is the caller's own parameter
/// binds directly, and nothing in the activation releases it.
#[test]
fn splice_binds_parameters_by_custody_and_clears_them_at_every_exit() {
    use molt_ir::ParameterCustody;
    for (custody, pass_parameter) in [
        (ParameterCustody::Transferred, false),
        (ParameterCustody::Borrowed, true),
    ] {
        // obs(a) -> a, with a CheckException whose exception exit returns void.
        let mut callee = observation_callee_with_type("obs_custody", 7, TirType::DynBox);
        callee.set_parameter_custody(&[custody]);
        // c(p) { t = make(); r = obs(p or t); check; return r }
        let mut caller = TirFunction::new(
            "c_custody".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        caller.has_exception_handling = true;
        let parameter = ValueId(0);
        let temporary = caller.fresh_value();
        let result = caller.fresh_value();
        let handler = caller.fresh_block();
        let argument = if pass_parameter { parameter } else { temporary };
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("s_value".into(), AttrValue::Str("obs_custody".into()));
        let mut call = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![argument],
            results: vec![result],
            attrs: call_attrs,
            source_span: Some((3, 9)),
        };
        call.set_argument_custody(&[custody]);
        let mut check_attrs = AttrDict::new();
        check_attrs.insert("value".into(), AttrValue::Int(90));
        let entry = caller.entry_block;
        let block = caller.blocks.get_mut(&entry).unwrap();
        block.ops = vec![
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![temporary],
                attrs: AttrDict::new(),
                source_span: None,
            },
            call,
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: check_attrs,
                source_span: None,
            },
        ];
        block.terminator = Terminator::Return {
            values: vec![result],
        };
        caller.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        caller.label_id_map.insert(handler.0, 90);
        caller.value_types.insert(temporary, TirType::DynBox);
        caller.value_types.insert(result, TirType::DynBox);

        let sites = collect_call_sites(&caller, &["obs_custody".to_string()]);
        assert_eq!(sites.len(), 1);
        let spliced =
            splice_call_site(&mut caller, &callee, &sites[0], &mut Activations::default());
        assert!(spliced, "{custody:?}");
        crate::tir::verify::verify_function(&caller)
            .unwrap_or_else(|e| panic!("{custody:?}: merged fn invalid SSA: {e:?}"));

        let ops: Vec<&TirOp> = caller.blocks.values().flat_map(|b| &b.ops).collect();
        let aliases_of = |source: ValueId| {
            ops.iter()
                .copied()
                .filter(|op| {
                    op.opcode == OpCode::Copy
                        && op.operands == [source]
                        && matches!(
                            op.attrs.get("_original_kind"),
                            Some(AttrValue::Str(kind)) if kind == "binding_alias"
                        )
                })
                .collect::<Vec<_>>()
        };
        let releases_of = |value: ValueId| {
            ops.iter()
                .filter(|op| op.opcode == OpCode::DelBoundary && op.operands == [value])
                .count()
        };
        let bindings = aliases_of(argument);
        if pass_parameter {
            assert!(bindings.is_empty(), "the caller's parameter binds directly");
            assert_eq!(releases_of(argument), 0, "the activation releases no borrow");
            continue;
        }
        assert_eq!(bindings.len(), 1, "one owned binding where the call was");
        assert_eq!(bindings[0].source_span, Some((3, 9)));
        let binding = bindings[0].results[0];
        assert_eq!(releases_of(binding), 2, "both exits clear the frame");
        let captures = aliases_of(binding);
        assert_eq!(captures.len(), 1, "the returned binding is captured");
        let capture = captures[0].results[0];
        let continuation_args: Vec<ValueId> = caller
            .blocks
            .values()
            .filter_map(|block| match &block.terminator {
                Terminator::Branch { args, .. } => Some(args.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            continuation_args,
            [capture],
            "the result carries the capture, never the binding"
        );
    }
}
