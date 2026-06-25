use super::super::*;
use super::OpFlow;

/// Single-source kind authority for [`handle_const_literal_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "const",
    "const_bigint",
    "const_bool",
    "const_none",
    "const_not_implemented",
    "const_ellipsis",
    "const_float",
    "const_str",
    "const_bytes",
];

#[cfg(feature = "native-backend")]
const NATIVE_INLINE_INT_MIN: i64 = -(1_i64 << 46);
#[cfg(feature = "native-backend")]
const NATIVE_INLINE_INT_MAX: i64 = (1_i64 << 46) - 1;

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn native_int_literal_fits_inline(
    val: i64,
) -> bool {
    (NATIVE_INLINE_INT_MIN..=NATIVE_INLINE_INT_MAX).contains(&val)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn require_const_str_payload(op: &OpIR) -> &[u8] {
    op.bytes.as_deref().unwrap_or_else(|| {
        op.s_value
            .as_deref()
            .unwrap_or_else(|| {
                panic!(
                    "const_str missing bytes or string payload for output `{}`",
                    op.out.as_deref().unwrap_or("<missing>")
                )
            })
            .as_bytes()
    })
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn op_uses_heap_literal_data_segment(
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "const_str" | "const_bytes" | "const_bigint" => true,
        "const" => op
            .value
            .is_some_and(|val| !native_int_literal_fits_inline(val)),
        _ => false,
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn collect_loop_entry_const_defs(
    func_ir: &FunctionIR,
    int_primary_vars: &BTreeSet<String>,
) -> BTreeMap<String, i64> {
    func_ir
        .ops
        .iter()
        .filter(|op| op.kind == "const" || op.kind == "const_bool" || op.kind == "const_none")
        .filter_map(|op| {
            let out = op.out.as_ref()?;
            match op.kind.as_str() {
                "const" => {
                    let val = op.value.unwrap_or(0);
                    if int_primary_vars.contains(out) {
                        return Some((out.clone(), val));
                    }
                    if native_int_literal_fits_inline(val) {
                        Some((out.clone(), box_int(val)))
                    } else {
                        None
                    }
                }
                "const_bool" => {
                    let val = op.value.unwrap_or(0);
                    Some((out.clone(), box_bool(val)))
                }
                "const_none" => Some((out.clone(), box_none())),
                _ => None,
            }
        })
        .collect()
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct HeapLiteralHoists {
    const_str_slots: BTreeMap<Vec<u8>, cranelift_codegen::ir::StackSlot>,
    const_bytes_slots: BTreeMap<Vec<u8>, cranelift_codegen::ir::StackSlot>,
    const_bigint_slots: BTreeMap<Vec<u8>, cranelift_codegen::ir::StackSlot>,
    str_output_slots: BTreeMap<String, cranelift_codegen::ir::StackSlot>,
}

#[cfg(feature = "native-backend")]
impl HeapLiteralHoists {
    pub(in crate::native_backend::function_compiler) fn str_output_slots(
        &self,
    ) -> &BTreeMap<String, cranelift_codegen::ir::StackSlot> {
        &self.str_output_slots
    }
}

#[cfg(feature = "native-backend")]
fn declare_literal_data(
    module: &mut ObjectModule,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    bytes: &[u8],
) -> (Value, Value) {
    let data_id = SimpleBackend::intern_data_segment(module, data_pool, next_data_id, bytes);
    let global_ptr = module.declare_data_in_func(data_id, builder.func);
    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
    let len = builder.ins().iconst(types::I64, bytes.len() as i64);
    (ptr, len)
}

#[cfg(feature = "native-backend")]
fn new_literal_slot(builder: &mut FunctionBuilder<'_>) -> cranelift_codegen::ir::StackSlot {
    builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3))
}

#[cfg(feature = "native-backend")]
fn hoist_outparam_literal(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    ref_name: &str,
    bytes: &[u8],
    runtime_func: &'static str,
) -> cranelift_codegen::ir::StackSlot {
    let (ptr, len) = declare_literal_data(module, data_pool, next_data_id, builder, bytes);
    def_var_named(builder, vars, format!("{}_ptr", ref_name), ptr);
    def_var_named(builder, vars, format!("{}_len", ref_name), len);

    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        runtime_func,
        &[types::I64, types::I64, types::I64],
        &[types::I32],
    );
    let tmp_slot = new_literal_slot(builder);
    let tmp_ptr = builder.ins().stack_addr(types::I64, tmp_slot, 0);
    let local_callee = module.declare_func_in_func(callee, builder.func);
    builder.ins().call(local_callee, &[ptr, len, tmp_ptr]);

    let hoisted_slot = new_literal_slot(builder);
    let val = builder.ins().stack_load(types::I64, tmp_slot, 0);
    builder.ins().stack_store(val, hoisted_slot, 0);
    hoisted_slot
}

#[cfg(feature = "native-backend")]
fn hoist_bigint_literal(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    ref_name: &str,
    bytes: &[u8],
) -> cranelift_codegen::ir::StackSlot {
    let (ptr, len) = declare_literal_data(module, data_pool, next_data_id, builder, bytes);
    def_var_named(builder, vars, format!("{}_ptr", ref_name), ptr);
    def_var_named(builder, vars, format!("{}_len", ref_name), len);

    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_bigint_from_str",
        &[types::I64, types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[ptr, len]);
    let val = builder.inst_results(call)[0];

    let hoisted_slot = new_literal_slot(builder);
    builder.ins().stack_store(val, hoisted_slot, 0);
    hoisted_slot
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn hoist_heap_literals(
    func_ir: &FunctionIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
) -> HeapLiteralHoists {
    let mut const_str_slots: BTreeMap<Vec<u8>, cranelift_codegen::ir::StackSlot> = BTreeMap::new();
    let mut const_bytes_slots: BTreeMap<Vec<u8>, cranelift_codegen::ir::StackSlot> =
        BTreeMap::new();
    let mut const_bigint_slots: BTreeMap<Vec<u8>, cranelift_codegen::ir::StackSlot> =
        BTreeMap::new();

    let mut unique_strs: Vec<(Vec<u8>, String)> = Vec::new();
    let mut unique_bytes: Vec<(Vec<u8>, String)> = Vec::new();
    let mut unique_bigints: Vec<(Vec<u8>, String)> = Vec::new();
    let mut seen_str_bytes: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    let mut seen_bytes_bytes: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    let mut seen_bigint_bytes: std::collections::HashSet<Vec<u8>> =
        std::collections::HashSet::new();

    for op in &func_ir.ops {
        match op.kind.as_str() {
            "const_str" => {
                let bytes = require_const_str_payload(op).to_vec();
                let out_name = match &op.out {
                    Some(n) => n.clone(),
                    None => continue,
                };
                if seen_str_bytes.insert(bytes.clone()) {
                    unique_strs.push((bytes, out_name));
                }
            }
            "const_bytes" => {
                let bytes = op.bytes.as_ref().expect("Bytes not found").clone();
                let out_name = match &op.out {
                    Some(n) => n.clone(),
                    None => continue,
                };
                if seen_bytes_bytes.insert(bytes.clone()) {
                    unique_bytes.push((bytes, out_name));
                }
            }
            "const_bigint" => {
                let bytes = op
                    .s_value
                    .as_ref()
                    .expect("BigInt string not found")
                    .as_bytes()
                    .to_vec();
                let out_name = match &op.out {
                    Some(n) => n.clone(),
                    None => continue,
                };
                if seen_bigint_bytes.insert(bytes.clone()) {
                    unique_bigints.push((bytes, out_name));
                }
            }
            "const" => {
                let val = op.value.unwrap_or(0);
                let out_name = match &op.out {
                    Some(n) if !int_primary_vars.contains(n) => n.clone(),
                    _ => continue,
                };
                if native_int_literal_fits_inline(val) {
                    continue;
                }
                let bytes = val.to_string().into_bytes();
                if seen_bigint_bytes.insert(bytes.clone()) {
                    unique_bigints.push((bytes, out_name));
                }
            }
            _ => {}
        }
    }

    for (bytes, ref_name) in &unique_strs {
        let hoisted_slot = hoist_outparam_literal(
            module,
            import_ids,
            data_pool,
            next_data_id,
            builder,
            vars,
            ref_name,
            bytes,
            "molt_string_from_bytes",
        );
        const_str_slots.insert(bytes.clone(), hoisted_slot);
    }

    for (bytes, ref_name) in &unique_bytes {
        let hoisted_slot = hoist_outparam_literal(
            module,
            import_ids,
            data_pool,
            next_data_id,
            builder,
            vars,
            ref_name,
            bytes,
            "molt_bytes_from_bytes",
        );
        const_bytes_slots.insert(bytes.clone(), hoisted_slot);
    }

    for (bytes, ref_name) in &unique_bigints {
        let hoisted_slot = hoist_bigint_literal(
            module,
            import_ids,
            data_pool,
            next_data_id,
            builder,
            vars,
            ref_name,
            bytes,
        );
        const_bigint_slots.insert(bytes.clone(), hoisted_slot);
    }

    let mut str_output_slots = BTreeMap::new();
    for op in &func_ir.ops {
        if op.kind == "const_str" {
            let bytes = require_const_str_payload(op);
            if let Some(ref out) = op.out
                && let Some(&slot) = const_str_slots.get(bytes)
            {
                str_output_slots.insert(out.clone(), slot);
            }
        }
    }

    HeapLiteralHoists {
        const_str_slots,
        const_bytes_slots,
        const_bigint_slots,
        str_output_slots,
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn emit_unhoisted_outparam_literal(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    out_name: &str,
    bytes: &[u8],
    runtime_func: &'static str,
) -> Value {
    let (ptr, len) = declare_literal_data(module, data_pool, next_data_id, builder, bytes);
    def_var_named(builder, vars, format!("{}_ptr", out_name), ptr);
    def_var_named(builder, vars, format!("{}_len", out_name), len);

    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        runtime_func,
        &[types::I64, types::I64, types::I64],
        &[types::I32],
    );
    let out_slot = new_literal_slot(builder);
    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
    let local_callee = module.declare_func_in_func(callee, builder.func);
    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
    builder.ins().stack_load(types::I64, out_slot, 0)
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn emit_unhoisted_bigint_literal(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    out_name: &str,
    bytes: &[u8],
) -> Value {
    let (ptr, len) = declare_literal_data(module, data_pool, next_data_id, builder, bytes);
    def_var_named(builder, vars, format!("{}_ptr", out_name), ptr);
    def_var_named(builder, vars, format!("{}_len", out_name), len);

    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_bigint_from_str",
        &[types::I64, types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[ptr, len]);
    builder.inst_results(call)[0]
}

/// Cranelift codegen handlers for constant and literal materialization. This
/// family owns the inline-int range, const_str payload fallback, heap-literal
/// prologue hoists, and the `rc_skip_dec` adjustment for hoisted heap constants.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_const_literal_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
    next_data_id: &mut u64,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    hoists: &HeapLiteralHoists,
    rc_skip_dec: &mut std::collections::HashSet<String>,
) -> OpFlow {
    match op.kind.as_str() {
        "const" => {
            let val = op.value.unwrap_or(0);
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            if int_primary_vars.contains(out_name.as_str()) {
                let raw_val = builder.ins().iconst(types::I64, val);
                def_var_named(builder, vars, out_name, raw_val);
            } else if native_int_literal_fits_inline(val) {
                let raw_val = builder.ins().iconst(types::I64, val);
                def_inline_int_value(
                    builder,
                    vars,
                    int_primary_vars,
                    out_name,
                    raw_val,
                    box_int(val),
                );
            } else {
                let s = val.to_string();
                let bytes = s.as_bytes();
                let boxed = if let Some(slot) = hoists.const_bigint_slots.get(bytes) {
                    builder.ins().stack_load(types::I64, *slot, 0)
                } else {
                    emit_unhoisted_bigint_literal(
                        module,
                        import_ids,
                        data_pool,
                        next_data_id,
                        builder,
                        vars,
                        out_name,
                        bytes,
                    )
                };
                def_var_named(builder, vars, out_name, boxed);
                rc_skip_dec.insert(out_name.clone());
            }
        }
        "const_bigint" => {
            let s = op.s_value.as_ref().expect("BigInt string not found");
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let bytes = s.as_bytes();
            let boxed = if let Some(slot) = hoists.const_bigint_slots.get(bytes) {
                builder.ins().stack_load(types::I64, *slot, 0)
            } else {
                emit_unhoisted_bigint_literal(
                    module,
                    import_ids,
                    data_pool,
                    next_data_id,
                    builder,
                    vars,
                    out_name,
                    bytes,
                )
            };
            def_var_named(builder, vars, out_name, boxed);
            rc_skip_dec.insert(out_name.clone());
        }
        "const_bool" => {
            let val = op.value.unwrap_or(0);
            let boxed = box_bool(val);
            let iconst = builder.ins().iconst(types::I64, boxed);
            if let Some(ref out__) = op.out {
                let raw = builder.ins().iconst(types::I64, val);
                def_bool_result(builder, vars, bool_primary_vars, out__, iconst, Some(raw));
            }
        }
        "const_none" => {
            let iconst = builder.ins().iconst(types::I64, box_none());
            if let Some(out__) = op.out.as_ref() {
                def_var_named(builder, vars, out__, iconst);
            }
        }
        "const_not_implemented" => {
            let callee = SimpleBackend::import_func_id_split(
                module,
                import_ids,
                "molt_not_implemented",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(builder, vars, out__, res);
            }
        }
        "const_ellipsis" => {
            let callee = SimpleBackend::import_func_id_split(
                module,
                import_ids,
                "molt_ellipsis",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(builder, vars, out__, res);
            }
        }
        "const_float" => {
            let val = op.f_value.expect("Float value not found");
            let raw_f64 = builder.ins().f64const(val);
            if let Some(ref out__) = op.out {
                if float_primary_vars.contains(out__.as_str()) {
                    def_var_named(builder, vars, out__, raw_f64);
                } else {
                    let boxed = box_float(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    def_var_named(builder, vars, out__, iconst);
                }
            }
        }
        "const_str" => {
            let bytes = require_const_str_payload(op).to_vec();
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let boxed = if let Some(slot) = hoists.const_str_slots.get(&bytes) {
                builder.ins().stack_load(types::I64, *slot, 0)
            } else {
                emit_unhoisted_outparam_literal(
                    module,
                    import_ids,
                    data_pool,
                    next_data_id,
                    builder,
                    vars,
                    out_name,
                    &bytes,
                    "molt_string_from_bytes",
                )
            };

            def_var_named(builder, vars, out_name, boxed);
            rc_skip_dec.insert(out_name.clone());
        }
        "const_bytes" => {
            let bytes = op.bytes.as_ref().expect("Bytes not found");
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let boxed = if let Some(slot) = hoists.const_bytes_slots.get(bytes) {
                builder.ins().stack_load(types::I64, *slot, 0)
            } else {
                emit_unhoisted_outparam_literal(
                    module,
                    import_ids,
                    data_pool,
                    next_data_id,
                    builder,
                    vars,
                    out_name,
                    bytes,
                    "molt_bytes_from_bytes",
                )
            };

            def_var_named(builder, vars, out_name, boxed);
            rc_skip_dec.insert(out_name.clone());
        }
        kind => panic!("const literal handler received unsupported op kind `{kind}`"),
    }
    OpFlow::Proceed
}
