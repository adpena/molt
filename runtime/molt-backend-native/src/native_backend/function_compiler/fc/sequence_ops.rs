use super::super::*;

/// Single-source kind authority for [`handle_sequence_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "len",
    "range_new",
    "tuple_new",
    "unpack_sequence",
    "tuple_count",
    "tuple_index",
    "iter",
    "enumerate",
    "iter_next_unboxed",
    "iter_next",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for sequence and iterator operations.
///
/// This family owns stack-tuple materialization, runtime tuple/range helpers,
/// generic sequence unpacking and specialized `len`. Iterator fusion belongs
/// to shared SSA; this family emits only the admitted operation and its declared
/// results, preserving source effects and generated ownership facts.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_sequence_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    scalarized_tuples: &mut BTreeMap<String, Vec<Value>>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
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

    match op.kind.as_str() {
        "len" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Stack-tuple fast path: length is known at compile time.
            if let Some(elems) = scalarized_tuples.get(&args[0]) {
                if let Some(out__) = op.out.as_ref() {
                    let len = elems.len() as i64;
                    let raw_len = builder.ins().iconst(types::I64, len);
                    def_inline_int_value(
                        &mut *builder,
                        vars,
                        representation_plan,
                        out__,
                        raw_len,
                        box_int(len),
                    );
                }
            } else {
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
                .expect("Len arg not found");
                // Dispatch to specialized fast-path len when container
                // type is known, skipping the 18-type dispatch in molt_len.
                let fn_name = match representation_plan.name_container_kind(&args[0]) {
                    Some(ContainerKind::List) => "molt_len_list",
                    Some(ContainerKind::Str) => "molt_len_str",
                    Some(ContainerKind::Dict) => "molt_len_dict",
                    Some(ContainerKind::Tuple) => "molt_len_tuple",
                    Some(ContainerKind::Set) => "molt_len_set",
                    _ => "molt_len",
                };
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    fn_name,
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*val]);
                let boxed_res = builder.inst_results(call)[0];
                if let Some(out__) = op.out.as_ref() {
                    if representation_plan.is_raw_int_carrier_name(out__) {
                        let raw_res = unbox_int(&mut *builder, boxed_res, nbc);
                        def_var_named(&mut *builder, vars, out__, raw_res);
                    } else {
                        def_var_named(&mut *builder, vars, out__, boxed_res);
                    }
                }
            }
        }
        "range_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Range start not found");
            let stop = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Range stop not found");
            let step = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Range step not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_range_new",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "tuple_new" => {
            let empty_args: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args);
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };

            if op.stack_eligible == Some(true) && args.len() <= 4 {
                let mut elems: Vec<Value> = Vec::with_capacity(args.len());
                for name in args {
                    let val = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    )
                    .expect("Tuple elem not found");
                    elems.push(*val);
                }
                scalarized_tuples.insert(out_name.to_string(), elems);
            }

            let values_ptr = if args.is_empty() {
                builder.ins().iconst(types::I64, 0)
            } else {
                let values_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    (args.len() * 8) as u32,
                    3,
                ));
                for (idx, name) in args.iter().enumerate() {
                    let val = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    )
                    .expect("Tuple elem not found");
                    builder
                        .ins()
                        .stack_store(*val, values_slot, (idx * 8) as i32);
                }
                builder.ins().stack_addr(types::I64, values_slot, 0)
            };
            let len = builder.ins().iconst(types::I64, args.len() as i64);
            let tuple_from_values = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_tuple_from_values",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let tuple_from_values_local =
                module.declare_func_in_func(tuple_from_values, builder.func);
            let tuple_call = builder
                .ins()
                .call(tuple_from_values_local, &[values_ptr, len]);
            let tuple_bits = builder.inst_results(tuple_call)[0];
            def_var_named(&mut *builder, vars, out_name, tuple_bits);
        }
        "unpack_sequence" => {
            // Generated SimpleIR field authority separates the sole sequence
            // source from every output binding.
            // op.value holds the expected element count.
            let mut source = None;
            let mut read_count = 0;
            crate::tir::simple_def_use::visit_simple_ir_reads(op, |read| {
                read_count += 1;
                source.get_or_insert(read.name);
            });
            let mut output_count = 0;
            crate::tir::simple_def_use::visit_simple_ir_result_names(op, |_| {
                output_count += 1;
            });
            let raw_expected = op
                .value
                .expect("unpack_sequence must carry an exact result count");
            let expected_count = usize::try_from(raw_expected)
                .expect("unpack_sequence result count must fit the active target");
            assert_eq!(
                (read_count, output_count),
                (1, expected_count),
                "unpack_sequence must carry one source and its exact result count"
            );
            let seq_val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                source.expect("verified unpack source"),
                representation_plan,
            )
            .expect("Unpack sequence source not found");
            // Allocate a stack slot for the output array.
            let slot_size = std::cmp::max(expected_count, 1)
                .checked_mul(std::mem::size_of::<u64>())
                .and_then(|size| u32::try_from(size).ok())
                .expect("unpack_sequence stack slot exceeds Cranelift's u32 limit");
            let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size,
                3, // align_shift: 2^3 = 8-byte alignment
            ));
            let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

            let expected_val = builder.ins().iconst(
                types::I64,
                i64::try_from(expected_count)
                    .expect("unpack_sequence count must fit the runtime ABI"),
            );

            // Call molt_unpack_sequence(seq_bits, expected_count, output_ptr) -> u64
            let unpack_local = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_unpack_sequence",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            builder
                .ins()
                .call(unpack_local, &[*seq_val, expected_val, out_ptr]);

            // Load each element from the output array into its named variable.
            let mut index: usize = 0;
            crate::tir::simple_def_use::visit_simple_ir_result_names(op, |output| {
                let elem = builder.ins().stack_load(
                    types::I64,
                    out_slot,
                    i32::try_from(index.checked_mul(8).expect("unpack offset overflow"))
                        .expect("unpack offset exceeds Cranelift's i32 limit"),
                );
                def_var_from_boxed_transport(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    output,
                    elem,
                );
                index += 1;
            });
        }
        "tuple_count" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let tuple = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Tuple not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Tuple count value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_tuple_count",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*tuple, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "tuple_index" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let tuple = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Tuple not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Tuple index value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_tuple_index",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*tuple, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "iter" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Iter source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_iter_checked",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "enumerate" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let iterable = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Enumerate iterable not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Enumerate start not found");
            let has_start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Enumerate has_start not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_enumerate",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*iterable, *start, *has_start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "iter_next_unboxed" => {
            // Fusion is represented explicitly by shared SSA. Native lowering
            // consumes exactly the declared value/done outputs; it never scans
            // ahead, skips consumers, or substitutes a key for a tuple.
            let args = op
                .args
                .as_ref()
                .expect("iter_next_unboxed requires an iterator");
            assert_eq!(args.len(), 1, "iter_next_unboxed requires one source");
            let iter = var_get_boxed_overflow_safe(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("iterator source not found");
            let mut results = [None, None];
            let mut result_count = 0;
            crate::tir::simple_def_use::visit_simple_ir_result_names(op, |name| {
                assert!(
                    result_count < results.len(),
                    "iter_next_unboxed requires two results"
                );
                results[result_count] = Some(name);
                result_count += 1;
            });
            assert_eq!(
                result_count, 2,
                "iter_next_unboxed requires value and done results"
            );
            let [Some(value_name), Some(done_name)] = results else {
                unreachable!("verified iterator result pair")
            };

            let value_slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                8,
                3,
            ));
            let value_ptr = builder.ins().stack_addr(types::I64, value_slot, 0);
            let callee = import_func_ref(
                module,
                import_ids,
                builder,
                import_refs,
                "molt_iter_next_unboxed",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let call = builder.ins().call(callee, &[*iter, value_ptr]);
            let done = builder.inst_results(call)[0];
            let value = builder.ins().stack_load(types::I64, value_slot, 0);
            for (name, bits) in [(value_name, value), (done_name, done)] {
                def_var_from_boxed_transport(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    name,
                    bits,
                );
            }
        }
        "iter_next" => {
            let args = op.args.as_ref().expect("iter_next requires an iterator");
            assert_eq!(args.len(), 1, "iter_next requires one source");
            let iter = var_get_boxed_overflow_safe(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("iterator source not found");
            let pair_name = op.out.as_ref().expect("iter_next requires its pair result");
            let callee = import_func_ref(
                module,
                import_ids,
                builder,
                import_refs,
                "molt_iter_next",
                &[types::I64],
                &[types::I64],
            );
            let call = builder.ins().call(callee, &[*iter]);
            let pair = builder.inst_results(call)[0];
            def_var_named(builder, vars, pair_name, pair);
        }
        _ => unreachable!("non-sequence op routed to handle_sequence_op"),
    }
    OpFlow::Proceed
}
