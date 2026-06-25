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
/// generic sequence unpacking, specialized `len`, and iterator-next fusion that
/// can replace `iter_next -> index -> unpack_sequence` with zero-allocation
/// direct writes. Keeping those rewrites beside `skip_ops` ownership avoids a
/// second source of truth for iterator consumption.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_sequence_op(
    op: &OpIR,
    ops: &[OpIR],
    op_idx: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    scalarized_tuples: &mut BTreeMap<String, Vec<Value>>,
    skip_ops: &mut BTreeSet<usize>,
    int_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
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
                                       int_primary_vars: &BTreeSet<String>,
                                       float_primary_vars: &BTreeSet<String>|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            int_primary_vars,
            float_primary_vars,
            bool_primary_vars,
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
                        int_primary_vars,
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
                    int_primary_vars,
                    float_primary_vars,
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
                    if int_primary_vars.contains(out__) {
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                        int_primary_vars,
                        float_primary_vars,
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
                        int_primary_vars,
                        float_primary_vars,
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
            // Outlined sequence unpacking: args[0] is the sequence,
            // args[1..] are the output variable names.
            // op.value holds the expected element count.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq_val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Unpack sequence source not found");
            let expected_count = op.value.unwrap_or(0) as usize;

            // Allocate a stack slot for the output array.
            let slot_size = std::cmp::max(expected_count, 1) * 8;
            let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size as u32,
                3, // align_shift: 2^3 = 8-byte alignment
            ));
            let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

            let expected_val = builder.ins().iconst(types::I64, expected_count as i64);

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
            for i in 0..expected_count {
                let elem = builder
                    .ins()
                    .stack_load(types::I64, out_slot, (i * 8) as i32);
                def_var_named(&mut *builder, vars, &args[1 + i], elem);
            }
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
            // TIR-fused iter_next that produces (value, done_flag)
            // directly.  op.args[0] = iterator, op.var = value output,
            // op.out = done_flag output.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let iter = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Iter not found");
            let val_name = op.var.clone().unwrap_or_default();
            let done_name = op.out.clone().unwrap_or_default();

            // Peephole: check if the value output feeds directly into
            // an unpack_sequence with count=2 (e.g., `for k, v in d.items()`).
            // When it does, use molt_iter_next_dict_items to write key
            // and value directly to stack slots - zero tuple allocation.
            let mut unpack_idx_ub = None;
            if !val_name.is_empty() && val_name != "none" {
                let ub_limit = (op_idx + 24).min(ops.len());
                for peek in (op_idx + 1)..ub_limit {
                    if skip_ops.contains(&peek) {
                        continue;
                    }
                    let peek_op = &ops[peek];
                    if peek_op.kind == "unpack_sequence"
                        && peek_op.value == Some(2)
                        && let Some(ref pargs) = peek_op.args
                        && !pargs.is_empty()
                        && pargs[0] == val_name
                    {
                        unpack_idx_ub = Some(peek);
                        break;
                    }
                }
            }

            if let Some(ui) = unpack_idx_ub {
                // === Dict items zero-alloc fast path ===
                let key_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let value_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let key_ptr = builder.ins().stack_addr(types::I64, key_slot, 0);
                let value_ptr = builder.ins().stack_addr(types::I64, value_slot, 0);

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_iter_next_dict_items",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder
                    .ins()
                    .call(local_callee, &[*iter, key_ptr, value_ptr]);
                let done_bits = builder.inst_results(call)[0];

                let loaded_key =
                    builder
                        .ins()
                        .load(types::I64, MemFlagsData::trusted(), key_ptr, 0);
                let loaded_value =
                    builder
                        .ins()
                        .load(types::I64, MemFlagsData::trusted(), value_ptr, 0);

                if !done_name.is_empty() && done_name != "none" {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        int_primary_vars,
                        bool_primary_vars,
                        float_primary_vars,
                        nbc,
                        &done_name,
                        done_bits,
                    );
                }
                // Define val_name for SSA completeness (as key).
                if !val_name.is_empty() && val_name != "none" {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        int_primary_vars,
                        bool_primary_vars,
                        float_primary_vars,
                        nbc,
                        &val_name,
                        loaded_key,
                    );
                }

                // Define unpack outputs directly.
                let unpack_args = ops[ui].args.as_ref().unwrap();
                if unpack_args.len() >= 3 {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        int_primary_vars,
                        bool_primary_vars,
                        float_primary_vars,
                        nbc,
                        &unpack_args[1],
                        loaded_key,
                    );
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        int_primary_vars,
                        bool_primary_vars,
                        float_primary_vars,
                        nbc,
                        &unpack_args[2],
                        loaded_value,
                    );
                }

                skip_ops.insert(ui);
            } else {
                // === Standard unboxed path ===
                let val_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let val_ptr = builder.ins().stack_addr(types::I64, val_slot, 0);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_iter_next_unboxed",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*iter, val_ptr]);
                let done_bits = builder.inst_results(call)[0];
                let loaded_val =
                    builder
                        .ins()
                        .load(types::I64, MemFlagsData::trusted(), val_ptr, 0);

                if !done_name.is_empty() && done_name != "none" {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        int_primary_vars,
                        bool_primary_vars,
                        float_primary_vars,
                        nbc,
                        &done_name,
                        done_bits,
                    );
                }
                if !val_name.is_empty() && val_name != "none" {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        int_primary_vars,
                        bool_primary_vars,
                        float_primary_vars,
                        nbc,
                        &val_name,
                        loaded_val,
                    );
                }
            }
        }
        "iter_next" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let iter = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Iter not found");
            let pair_name = op.out.clone().unwrap();

            // Peephole: detect the iter_next -> index(pair,1) -> ... -> index(pair,0)
            // pattern emitted by for-loops and replace with a single
            // molt_iter_next_unboxed call that avoids the tuple allocation and
            // two molt_index dispatches.
            let mut done_idx = None;
            let mut val_idx = None;
            // Scan ahead for INDEX ops that reference our pair.  Use a
            // wider window (16 ops) to bridge exception-handling
            // boilerplate (check_exception, inc_ref, etc.) that can
            // separate iter_next from its index consumers.
            let scan_limit = (op_idx + 16).min(ops.len());
            for peek in (op_idx + 1)..scan_limit {
                if skip_ops.contains(&peek) {
                    continue;
                }
                let peek_op = &ops[peek];
                if peek_op.kind == "index"
                    && let Some(ref pargs) = peek_op.args
                    && pargs.len() >= 2
                    && pargs[0] == pair_name
                {
                    // Check if the index argument is a const "1" or "0".
                    // The const var names are looked up by scanning
                    // backwards for a const op that defined the arg.
                    let idx_var = &pargs[1];
                    // Find the const op that produced idx_var.
                    if let Some(const_val) = SimpleBackend::resolve_const_int(ops, peek, idx_var) {
                        if const_val == 1 && done_idx.is_none() {
                            done_idx = Some(peek);
                        } else if const_val == 0 && val_idx.is_none() {
                            val_idx = Some(peek);
                        }
                    }
                }
            }

            if let (Some(di), Some(vi)) = (done_idx, val_idx) {
                // Check if the value from iter_next feeds directly into
                // an unpack_sequence with count=2.  When it does, we can
                // use molt_iter_next_dict_items to write key and value
                // directly to stack slots - zero tuple allocation.
                let val_out_name = ops[vi].out.clone().unwrap();
                let mut unpack_idx = None;
                let unpack_limit = (vi + 24).min(ops.len());
                for peek in (vi + 1)..unpack_limit {
                    if skip_ops.contains(&peek) {
                        continue;
                    }
                    let peek_op = &ops[peek];
                    if peek_op.kind == "unpack_sequence"
                        && peek_op.value == Some(2)
                        && let Some(ref pargs) = peek_op.args
                        && !pargs.is_empty()
                        && pargs[0] == val_out_name
                    {
                        unpack_idx = Some(peek);
                        break;
                    }
                }

                if let Some(ui) = unpack_idx {
                    // === Dict items zero-alloc fast path ===
                    let key_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let value_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let key_ptr = builder.ins().stack_addr(types::I64, key_slot, 0);
                    let value_ptr = builder.ins().stack_addr(types::I64, value_slot, 0);

                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_iter_next_dict_items",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*iter, key_ptr, value_ptr]);
                    let done_bits = builder.inst_results(call)[0];

                    let loaded_key =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), key_ptr, 0);
                    let loaded_value =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), value_ptr, 0);

                    // Define done flag.
                    let done_out = ops[di].out.clone().unwrap();
                    def_var_named(&mut *builder, vars, done_out, done_bits);

                    // Define pair variable for exception checks.
                    def_var_named(&mut *builder, vars, pair_name.clone(), done_bits);

                    // Define the value from index(pair,0) for SSA completeness.
                    def_var_named(&mut *builder, vars, val_out_name, loaded_key);

                    // Define the unpack outputs directly from stack slots.
                    let unpack_args = ops[ui].args.as_ref().unwrap();
                    // unpack_args: [source, out1, out2]
                    if unpack_args.len() >= 3 {
                        def_var_named(&mut *builder, vars, &unpack_args[1], loaded_key);
                        def_var_named(&mut *builder, vars, &unpack_args[2], loaded_value);
                    }

                    skip_ops.insert(di);
                    skip_ops.insert(vi);
                    skip_ops.insert(ui);
                } else {
                    // === Unboxed fast path (no unpack detected) ===
                    let val_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let val_ptr = builder.ins().stack_addr(types::I64, val_slot, 0);

                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_iter_next_unboxed",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*iter, val_ptr]);
                    let done_bits = builder.inst_results(call)[0];

                    let loaded_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), val_ptr, 0);

                    let done_out = ops[di].out.clone().unwrap();
                    def_var_named(&mut *builder, vars, done_out, done_bits);

                    let val_out = ops[vi].out.clone().unwrap();
                    def_var_named(&mut *builder, vars, val_out, loaded_val);

                    def_var_named(&mut *builder, vars, pair_name, done_bits);

                    skip_ops.insert(di);
                    skip_ops.insert(vi);
                }
            } else {
                // === Fallback: original boxed path ===
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_iter_next",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*iter]);
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, pair_name, res);
            }
        }
        _ => unreachable!("non-sequence op routed to handle_sequence_op"),
    }
    OpFlow::Proceed
}
