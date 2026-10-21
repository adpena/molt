use super::super::*;

/// Single-source kind authority for [`handle_callargs_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "callargs_new",
    "callargs_push_pos",
    "callargs_push_method_receiver",
    "callargs_push_kw",
    "callargs_expand_star",
    "callargs_expand_kwstar",
];
use super::OpFlow;

/// CallArgs construction and borrowed push/expand consumers share the native
/// operand transaction: repeated sources keep one identity and a failed mint
/// skips the consumer while releasing earlier temporary boxes.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_callargs_op(
    op: &OpIR,
    module: &mut dyn Module,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) -> OpFlow {
    let names = op.args.as_deref().unwrap_or(&[]);
    let mut operands = NativeOperandTransaction::begin(
        builder,
        representation_plan,
        names.iter().map(String::as_str),
    );
    for name in names {
        operands.operand(
            name,
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            representation_plan,
            nbc,
        );
    }
    operands.enter_consumer(builder, block_tracked_obj, block_tracked_ptr);
    match op.kind.as_str() {
        "callargs_new" => {
            // The source call form picks the builder: a CALL_FUNCTION_EX call
            // site's arguments are its own tuple and mapping.
            let constructor = op
                .call_argument_form()
                .expect("validated callargs_new call form")
                .runtime_constructor();
            let zero = builder.ins().iconst(types::I64, 0);
            let local_callee = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                constructor,
                &[types::I64, types::I64],
                &[types::I64],
            );
            let call = builder.ins().call(local_callee, &[zero, zero]);
            let res = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, res, module, import_ids, builder, vars);
        }
        "callargs_push_method_receiver" => {
            let [callargs, receiver] = names else {
                panic!("validated method receiver prefix takes builder and receiver");
            };
            let builder_word = *operands.word(callargs).expect("Callargs builder not found");
            let receiver_word = *operands.word(receiver).expect("loaded receiver not found");
            // A loaded pair owns its nullable receiver. Move only real Python
            // values into the builder; its retaining push must finish before
            // the original credit ends, including when that push raises.
            let present = builder.ins().icmp_imm_s(
                IntCC::NotEqual,
                receiver_word,
                molt_codegen_abi::loaded_method_null_bits(),
            );
            let push = builder.create_block();
            let join = builder.create_block();
            builder.ins().brif(present, push, &[], join, &[]);
            switch_to_block_materialized(builder, push);
            seal_block_once(builder, sealed_blocks, push);
            let push_fn = import_func_ref(
                module,
                import_ids,
                builder,
                import_refs,
                "molt_callargs_push_pos",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let call = builder.ins().call(push_fn, &[builder_word, receiver_word]);
            let result = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, result, module, import_ids, builder, vars);
            let release = import_func_ref(
                module,
                import_ids,
                builder,
                import_refs,
                "molt_dec_ref_obj",
                &[types::I64],
                &[],
            );
            builder.ins().call(release, &[receiver_word]);
            jump_block(builder, join, &[]);
            switch_to_block_materialized(builder, join);
            seal_block_once(builder, sealed_blocks, join);
        }
        "callargs_push_pos" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let builder_ptr = operands.word(&args[0]).expect("Callargs builder not found");
            let val = operands.word(&args[1]).expect("Callargs value not found");
            let local_callee = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_callargs_push_pos",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let call = builder.ins().call(local_callee, &[*builder_ptr, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "callargs_push_kw" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let builder_ptr = operands.word(&args[0]).expect("Callargs builder not found");
            let name = operands.word(&args[1]).expect("Callargs name not found");
            let val = operands.word(&args[2]).expect("Callargs value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_callargs_push_kw",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*builder_ptr, *name, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "callargs_expand_star" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let builder_ptr = operands.word(&args[0]).expect("Callargs builder not found");
            let iterable = operands
                .word(&args[1])
                .expect("Callargs iterable not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_callargs_expand_star",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*builder_ptr, *iterable]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "callargs_expand_kwstar" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let builder_ptr = operands.word(&args[0]).expect("Callargs builder not found");
            let mapping = operands.word(&args[1]).expect("Callargs mapping not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_callargs_expand_kwstar",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*builder_ptr, *mapping]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
    operands.finish_operation(
        op,
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        representation_plan,
        nbc,
        block_tracked_obj,
        block_tracked_ptr,
    );
    OpFlow::Proceed
}
