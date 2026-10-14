use super::super::seal_block_once;
use super::*;

#[test]
fn block_transport_plan_emits_typed_args_and_rebinds_values() {
    let sig = Signature::new(CallConv::SystemV);
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let word_var = builder.declare_var(types::I64);
    let float_var = builder.declare_var(types::F64);
    let entry = builder.create_block();
    let target = builder.create_block();
    let plan =
        BlockTransportPlan::for_test(vec![float_var, word_var], vec![types::F64, types::I64]);
    plan.append_block_params(&mut builder, target);

    switch_to_block_materialized(&mut builder, entry);
    let word = builder.ins().iconst(types::I64, 17);
    let float = builder.ins().f64const(3.5);
    builder.def_var(word_var, word);
    builder.def_var(float_var, float);
    let args = plan.edge_args(&mut builder);
    jump_block(&mut builder, target, &args);
    builder.seal_block(entry);

    switch_to_block_materialized(&mut builder, target);
    plan.bind_block_params(&mut builder, target, &empty_ssa_values());

    assert_eq!(builder.use_var(float_var), builder.block_params(target)[0]);
    assert_eq!(builder.use_var(word_var), builder.block_params(target)[1]);
}

#[test]
fn materialize_label_block_defines_unreached_forward_label() {
    let sig = Signature::new(CallConv::SystemV);
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let entry = builder.create_block();
    let later = builder.create_block();
    let detached_label = builder.create_block();

    switch_to_block_materialized(&mut builder, entry);
    builder.ins().jump(later, &[]);
    builder.seal_block(entry);

    let mut is_block_filled = true;
    materialize_label_block(
        &mut builder,
        detached_label,
        &mut is_block_filled,
        None,
        &empty_ssa_values(),
    );

    assert!(
        builder.func.layout.is_block_inserted(detached_label),
        "textual label must materialize its block even before any emitted predecessor reaches it",
    );
    assert_eq!(builder.current_block(), Some(detached_label));
    assert!(
        !is_block_filled,
        "materialized label block must be open for emission"
    );
}

#[test]
fn materialize_label_block_does_not_self_jump_current_resume_block() {
    let sig = Signature::new(CallConv::SystemV);
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let resume_block = builder.create_block();
    switch_to_block_materialized(&mut builder, resume_block);

    let mut is_block_filled = false;
    materialize_label_block(
        &mut builder,
        resume_block,
        &mut is_block_filled,
        None,
        &empty_ssa_values(),
    );

    assert_eq!(builder.current_block(), Some(resume_block));
    assert!(
        !is_block_filled,
        "state_label materialization must leave the current resume block open"
    );
    assert!(
        builder.func.layout.last_inst(resume_block).is_none(),
        "state_label materialization must not emit a self-jump predecessor"
    );
}

// â”€â”€ scan_loop_int_sum_reduction tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn empty_ssa_values() -> NativeSsaValues {
    NativeSsaValues::for_test(
        &[],
        &[],
        &crate::tir::cfg_liveness::SimpleNameTable::default(),
    )
}

#[test]
fn unique_dominating_definition_still_requires_earlier_emission() {
    let initializer = OpIR {
        kind: "const".into(),
        out: Some("immutable".into()),
        ..Default::default()
    };
    let ops = vec![
        OpIR {
            kind: "jump".into(),
            value: Some(9),
            ..Default::default()
        },
        OpIR {
            kind: "label".into(),
            value: Some(7),
            ..Default::default()
        },
        OpIR {
            kind: "ret".into(),
            args: Some(vec!["immutable".into()]),
            ..Default::default()
        },
        OpIR {
            kind: "label".into(),
            value: Some(9),
            ..Default::default()
        },
        initializer,
        OpIR {
            kind: "jump".into(),
            value: Some(7),
            ..Default::default()
        },
    ];
    let names = crate::tir::cfg_liveness::analyze_simple_cfg_liveness_facts(&ops).names;
    let values = NativeSsaValues::for_test(&[], &ops, &names);
    assert!(!values.can_bind_directly(
        names.id("immutable").unwrap(),
        crate::tir::dominators::SimpleProgramPoint::Before(1)
    ));
    assert!(values.can_bind_directly(
        names.id("immutable").unwrap(),
        crate::tir::dominators::SimpleProgramPoint::Before(5)
    ));
}

#[test]
fn internal_single_predecessor_chain_resolves_only_demanded_values() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let mut sealed = BTreeSet::new();
        let entry = builder.create_block();
        switch_to_block_materialized(&mut builder, entry);
        seal_block_once(&mut builder, &mut sealed, entry);
        let mut homes = Vec::new();
        for value in 0..128 {
            let home = builder.declare_var(types::I64);
            let value = builder.ins().iconst(types::I64, value);
            builder.def_var(home, value);
            homes.push((home, value));
        }
        for _ in 0..128 {
            let next = builder.create_block();
            builder.ins().jump(next, &[]);
            switch_to_block_materialized(&mut builder, next);
            seal_block_once(&mut builder, &mut sealed, next);
        }
        let (home, expected) = homes[37];
        assert_eq!(builder.use_var(home), expected);
        builder.ins().return_(&[expected]);
        builder.finalize();
    }
    for block in function.layout.blocks() {
        assert!(function.dfg.block_params(block).is_empty());
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}

#[test]
fn internal_diamond_preserves_redefinitions_before_and_after_sealing() {
    for seal_before_use in [false, true] {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let mut function = Function::with_name_signature(UserFuncName::default(), sig);
        let mut context = FunctionBuilderContext::new();
        let (left_edge, right_edge, join) = {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            let mut sealed = BTreeSet::new();
            let home = builder.declare_var(types::I64);
            let entry = builder.create_block();
            let left = builder.create_block();
            let right = builder.create_block();
            let join = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            switch_to_block_materialized(&mut builder, entry);
            seal_block_once(&mut builder, &mut sealed, entry);
            let condition = builder.block_params(entry)[0];
            let initial = builder.ins().iconst(types::I64, 11);
            builder.def_var(home, initial);
            builder.ins().brif(condition, left, &[], right, &[]);
            switch_to_block_materialized(&mut builder, left);
            seal_block_once(&mut builder, &mut sealed, left);
            let replaced = builder.ins().iconst(types::I64, 23);
            builder.def_var(home, replaced);
            let final_left = builder.ins().iconst(types::I64, 47);
            builder.def_var(home, final_left);
            let left_edge = builder.ins().jump(join, &[]);
            switch_to_block_materialized(&mut builder, right);
            seal_block_once(&mut builder, &mut sealed, right);
            assert_eq!(builder.use_var(home), initial);
            let right_edge = builder.ins().jump(join, &[]);
            switch_to_block_materialized(&mut builder, join);
            if seal_before_use {
                seal_block_once(&mut builder, &mut sealed, join);
            }
            let result = builder.use_var(home);
            if !seal_before_use {
                seal_block_once(&mut builder, &mut sealed, join);
            }
            assert_eq!(builder.block_params(join), &[result]);
            builder.ins().return_(&[result]);
            builder.finalize();
            (left_edge, right_edge, join)
        };
        // Inspect each actual edge's constant origin, not SSA value numbering
        // or merely the existence of a join parameter. The left arm must carry
        // its final write (47); the right arm must retain the initial write (11).
        for (edge, expected) in [(left_edge, 47), (right_edge, 11)] {
            use cranelift_codegen::ir::{BlockArg, InstructionData, Opcode, ValueDef};
            let destinations = function.dfg.insts[edge]
                .branch_destination(&function.dfg.jump_tables, &function.dfg.exception_tables);
            assert_eq!(destinations.len(), 1);
            assert_eq!(destinations[0].block(&function.dfg.value_lists), join);
            let arguments: Vec<_> = destinations[0].args(&function.dfg.value_lists).collect();
            let [BlockArg::Value(incoming)] = arguments.as_slice() else {
                panic!("diamond edge must pass exactly one SSA value: {arguments:?}");
            };
            let incoming = function.dfg.resolve_aliases(*incoming);
            let ValueDef::Result(definition, _) = function.dfg.value_def(incoming) else {
                panic!("diamond edge must originate in its arm's constant");
            };
            let InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } = function.dfg.insts[definition]
            else {
                panic!("diamond edge must originate in an integer constant");
            };
            assert_eq!(imm.bits(), expected);
        }
        verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
    }
}

#[test]
fn seal_block_once_rejects_unmaterialized_block_without_recording_success() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut function = Function::with_name_signature(UserFuncName::default(), sig);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let mut sealed = BTreeSet::new();
        let home = builder.declare_var(types::I64);
        let entry = builder.create_block();
        let next = builder.create_block();
        switch_to_block_materialized(&mut builder, entry);
        seal_block_once(&mut builder, &mut sealed, entry);
        let initial = builder.ins().iconst(types::I64, 11);
        builder.def_var(home, initial);
        builder.ins().jump(next, &[]);

        let before = sealed.clone();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            seal_block_once(&mut builder, &mut sealed, next);
        }))
        .expect_err("sealing before materialization must fail at Molt's boundary");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .expect("sealing diagnostic must be text");
        assert!(message.contains("cannot seal unmaterialized block"));
        assert_eq!(sealed, before, "failed sealing must preserve the record");
        assert!(!builder.func.layout.is_block_inserted(next));

        // Materializing and retrying must perform the real seal. A false record
        // from the rejected attempt would leave use_var creating a parameter.
        switch_to_block_materialized(&mut builder, next);
        seal_block_once(&mut builder, &mut sealed, next);
        seal_block_once(&mut builder, &mut sealed, next);
        assert!(sealed.contains(&next));
        assert_eq!(builder.use_var(home), initial);
        assert!(builder.block_params(next).is_empty());
        builder.ins().return_(&[initial]);
        builder.finalize();
    }
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}
