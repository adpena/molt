use super::super::*;
use molt_ir::OpKind;

// Single-source kind authority for [`handle_type_conversion`], consulted by
// `op_family::native_op_family`. Mirror the `match op.kind` arms below.
#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;
    OpKind::BytesFromObj,
    OpKind::BytesFromStr,
    OpKind::BytearrayFromObj,
    OpKind::BytearrayFromStr,
    OpKind::FloatFromObj,
    OpKind::IntFromObj,
    OpKind::IntFromStrOfObj,
    OpKind::OperatorIndex,
    OpKind::ComplexFromObj,
    OpKind::StrFromObj,
    OpKind::ReprFromObj,
    OpKind::AsciiFromObj,
);
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for scalar/sequence type-conversion constructors: `bytes`/`bytearray`/`float`/`int`/`complex`/`str`/`repr`/`ascii` from objects or strings.
///
/// `operator_index` uses the shared operand transaction and owned-result sink.
/// The other arms were extracted from `compile_func_inner`'s per-op dispatch
/// (M1 phase 1), preserving their original bodies; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed `&mut`
/// params). The op-local closure `var_get_boxed_overflow_safe` is reconstructed
/// with the same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_type_conversion(
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
) {
    // Reconstruct the original op-local closure (captures representation_plan +
    // nbc; all other state threads through explicit params) so the moved arm
    // bodies call it exactly as they did inline.
    let var_get_boxed_overflow_safe = |module: &mut dyn Module,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    match op.kind {
        OpKind::BytesFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Bytes source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytes_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*src]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::BytesFromStr => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Bytes source not found");
            let encoding = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Bytes encoding not found");
            let errors = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Bytes errors not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytes_from_str",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*src, *encoding, *errors]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::BytearrayFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Bytearray source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytearray_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*src]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::BytearrayFromStr => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Bytearray source not found");
            let encoding = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Bytearray encoding not found");
            let errors = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Bytearray errors not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytearray_from_str",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*src, *encoding, *errors]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::FloatFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Float source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_float_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*src]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                if representation_plan.is_float_unboxed(out__.as_str()) {
                    let raw_f64 = float_value_from_boxed_extended(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        res,
                    );
                    def_var_named(&mut *builder, vars, out__, raw_f64);
                } else {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            }
        }
        OpKind::IntFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Int value not found");
            let base = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Int base not found");
            let has_base = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Int base flag not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_int_from_obj",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*val, *base, *has_base]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::IntFromStrOfObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Int value not found");
            let base = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Int base not found");
            let has_base = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Int base flag not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_int_from_str_of_obj",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*val, *base, *has_base]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::ComplexFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Complex value not found");
            let imag = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Complex imag not found");
            let has_imag = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Complex flag not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_complex_from_obj",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*val, *imag, *has_imag]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::OperatorIndex => {
            // range()'s bound conversion: `operator.index(x)`, an owned exact int.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            emit_operand_transaction_call(
                op,
                args,
                "molt_operator_index",
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
        }
        OpKind::StrFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Str source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_str_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*src]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::ReprFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Repr source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_repr_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*src]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        OpKind::AsciiFromObj => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Ascii source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_ascii_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*src]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
