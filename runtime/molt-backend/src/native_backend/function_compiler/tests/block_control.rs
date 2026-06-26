use super::*;

#[test]
fn live_exception_rebind_vars_skip_future_definitions() {
    let mut vars = BTreeMap::new();
    vars.insert("early".to_string(), Variable::from_u32(0));
    vars.insert("late".to_string(), Variable::from_u32(1));
    vars.insert("dead".to_string(), Variable::from_u32(2));

    let mut transport_last_use = BTreeMap::new();
    transport_last_use.insert("early".to_string(), 10usize);
    transport_last_use.insert("late".to_string(), 10usize);
    transport_last_use.insert("dead".to_string(), 1usize);

    let mut first_defined_at = BTreeMap::new();
    first_defined_at.insert("early".to_string(), 0usize);
    first_defined_at.insert("late".to_string(), 5usize);
    first_defined_at.insert("dead".to_string(), 0usize);

    let live = live_exception_rebind_vars_for_op(&vars, &transport_last_use, &first_defined_at, 3);

    assert!(live.contains_key("early"));
    assert!(!live.contains_key("late"));
    assert!(!live.contains_key("dead"));
}

#[test]
fn switch_to_block_with_rebind_does_not_inflate_merge_params_for_invariant_vars() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let stable_var = builder.declare_var(types::I64);
    let phi_var = builder.declare_var(types::I64);

    let entry = builder.create_block();
    let then_block = builder.create_block();
    let else_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);

    switch_to_block_materialized(&mut builder, entry);
    let stable = builder.ins().iconst(types::I64, 7);
    let cond = builder.ins().iconst(types::I8, 1);
    let then_val = builder.ins().iconst(types::I64, 11);
    let else_val = builder.ins().iconst(types::I64, 13);
    builder.def_var(stable_var, stable);
    builder.ins().brif(cond, then_block, &[], else_block, &[]);
    builder.seal_block(entry);

    switch_to_block_materialized(&mut builder, then_block);
    builder.def_var(phi_var, then_val);
    jump_block(&mut builder, merge_block, &[then_val]);
    builder.seal_block(then_block);

    switch_to_block_materialized(&mut builder, else_block);
    builder.def_var(phi_var, else_val);
    jump_block(&mut builder, merge_block, &[else_val]);
    builder.seal_block(else_block);

    let mut is_block_filled = false;
    switch_to_block_with_rebind(&mut builder, merge_block, &mut is_block_filled, false);

    assert_eq!(
        builder.block_params(merge_block).len(),
        1,
        "merge block should only carry the explicit phi payload param",
    );
}

#[test]
fn switch_to_block_with_rebind_does_not_create_exception_fallthrough_phis_for_invariants() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let stable_var = builder.declare_var(types::I64);

    let entry = builder.create_block();
    let validate_block = builder.create_block();
    let fallthrough = builder.create_block();

    switch_to_block_materialized(&mut builder, entry);
    let stable = builder.ins().iconst(types::I64, 7);
    let cond = builder.ins().iconst(types::I8, 1);
    builder.def_var(stable_var, stable);
    builder
        .ins()
        .brif(cond, validate_block, &[], fallthrough, &[]);
    builder.seal_block(entry);

    switch_to_block_materialized(&mut builder, validate_block);
    builder.ins().jump(fallthrough, &[]);
    builder.seal_block(validate_block);

    let mut is_block_filled = false;
    switch_to_block_with_rebind(&mut builder, fallthrough, &mut is_block_filled, true);

    assert!(
        builder.block_params(fallthrough).is_empty(),
        "exception fallthrough should not synthesize params for invariant vars",
    );
}

#[test]
fn switch_to_block_with_rebind_does_not_create_params_for_plain_label_blocks() {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.returns.push(AbiParam::new(types::I64));
    let mut func = Function::with_name_signature(UserFuncName::default(), sig);
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

    let stable_var = builder.declare_var(types::I64);

    let entry = builder.create_block();
    let label_block = builder.create_block();

    switch_to_block_materialized(&mut builder, entry);
    let stable = builder.ins().iconst(types::I64, 7);
    builder.def_var(stable_var, stable);
    jump_block(&mut builder, label_block, &[]);
    builder.seal_block(entry);

    let mut is_block_filled = false;
    switch_to_block_with_rebind(&mut builder, label_block, &mut is_block_filled, false);

    assert!(
        builder.block_params(label_block).is_empty(),
        "plain label blocks must not gain implicit params from SSA repair",
    );
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
    materialize_label_block(&mut builder, detached_label, &mut is_block_filled);

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
    materialize_label_block(&mut builder, resume_block, &mut is_block_filled);

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
