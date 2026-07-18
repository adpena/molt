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
    let plan = BlockTransportPlan::for_test(
        vec!["float".into(), "word".into()],
        vec![float_var, word_var],
        vec![types::F64, types::I64],
    );
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
    plan.bind_block_params(&mut builder, target);

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
    materialize_label_block(&mut builder, detached_label, &mut is_block_filled, None);

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
    materialize_label_block(&mut builder, resume_block, &mut is_block_filled, None);

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
