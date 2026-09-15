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
#[inline]
pub(in crate::native_backend::function_compiler) fn native_int_literal_fits_inline(
    val: i64,
) -> bool {
    molt_codegen_abi::fits_inline_int(val)
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
    representation_plan: &ScalarRepresentationPlan,
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
                    if representation_plan.is_raw_int_carrier_name(out) {
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

    /// Prologue results are frame-owned anchors, independent of every IR
    /// result loaded from them. All exits converge before releasing these.
    pub(in crate::native_backend::function_compiler) fn release_anchors(
        &self,
        builder: &mut FunctionBuilder<'_>,
        local_dec_ref_obj: FuncRef,
    ) {
        for &slot in self
            .const_str_slots
            .values()
            .chain(self.const_bytes_slots.values())
            .chain(self.const_bigint_slots.values())
        {
            let value = builder.ins().stack_load(types::I64, slot, 0);
            builder.ins().call(local_dec_ref_obj, &[value]);
        }
    }
}

/// A constant operation produces an owned IR value on every execution, even
/// when its immutable payload shares a frame anchor with other operations.
#[cfg(feature = "native-backend")]
fn load_owned_literal(
    builder: &mut FunctionBuilder<'_>,
    slot: cranelift_codegen::ir::StackSlot,
    local_inc_ref_obj: FuncRef,
) -> Value {
    let value = builder.ins().stack_load(types::I64, slot, 0);
    emit_inc_ref_obj(builder, value, local_inc_ref_obj);
    value
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
    let slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let none = builder.ins().iconst(types::I64, box_none());
    builder.ins().stack_store(none, slot, 0);
    slot
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
pub(in crate::native_backend::function_compiler) struct LiteralFailureExit {
    pub block: Block,
    pub returns_value: bool,
}

#[cfg(feature = "native-backend")]
impl LiteralFailureExit {
    fn branch_if_failed(self, builder: &mut FunctionBuilder<'_>, failed: Value) {
        let success = builder.create_block();
        let none = builder.ins().iconst(types::I64, box_none());
        let args = if self.returns_value { &[none][..] } else { &[] };
        brif_block(builder, failed, self.block, args, success, &[]);
        switch_to_block_materialized(builder, success);
        builder.seal_block(success);
    }
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
    hoisted_slot: cranelift_codegen::ir::StackSlot,
    failure_exit: LiteralFailureExit,
) {
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
    let tmp_ptr = builder.ins().stack_addr(types::I64, hoisted_slot, 0);
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[ptr, len, tmp_ptr]);
    let status = builder.inst_results(call)[0];
    let failed = builder.ins().icmp_imm(IntCC::NotEqual, status, 0);
    failure_exit.branch_if_failed(builder, failed);
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
    hoisted_slot: cranelift_codegen::ir::StackSlot,
    failure_exit: LiteralFailureExit,
) {
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

    builder.ins().stack_store(val, hoisted_slot, 0);
    let failed = builder.ins().icmp_imm(IntCC::Equal, val, box_none());
    failure_exit.branch_if_failed(builder, failed);
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
    representation_plan: &ScalarRepresentationPlan,
    failure_exit: LiteralFailureExit,
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
                    Some(n) if !representation_plan.is_raw_int_carrier_name(n) => n.clone(),
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

    // Every slot must dominate every constructor failure edge, including
    // anchors whose constructors will never execute after an earlier failure.
    for (values, slots) in [
        (&unique_strs, &mut const_str_slots),
        (&unique_bytes, &mut const_bytes_slots),
        (&unique_bigints, &mut const_bigint_slots),
    ] {
        for (bytes, _) in values {
            slots.insert(bytes.clone(), new_literal_slot(builder));
        }
    }

    for (bytes, ref_name) in &unique_strs {
        hoist_outparam_literal(
            module,
            import_ids,
            data_pool,
            next_data_id,
            builder,
            vars,
            ref_name,
            bytes,
            "molt_string_from_bytes",
            const_str_slots[bytes],
            failure_exit,
        );
    }

    for (bytes, ref_name) in &unique_bytes {
        hoist_outparam_literal(
            module,
            import_ids,
            data_pool,
            next_data_id,
            builder,
            vars,
            ref_name,
            bytes,
            "molt_bytes_from_bytes",
            const_bytes_slots[bytes],
            failure_exit,
        );
    }

    for (bytes, ref_name) in &unique_bigints {
        hoist_bigint_literal(
            module,
            import_ids,
            data_pool,
            next_data_id,
            builder,
            vars,
            ref_name,
            bytes,
            const_bigint_slots[bytes],
            failure_exit,
        );
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

/// Cranelift codegen handlers for constant and literal materialization. This
/// family owns the inline-int range, const_str payload decoding, and heap-literal
/// frame anchors. IR results own references independently of those anchors.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_const_literal_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    hoists: &HeapLiteralHoists,
    local_inc_ref_obj: FuncRef,
) -> OpFlow {
    match op.kind.as_str() {
        "const" => {
            let val = op.value.unwrap_or(0);
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            if representation_plan.is_raw_int_carrier_name(out_name.as_str()) {
                let raw_val = builder.ins().iconst(types::I64, val);
                def_var_named(builder, vars, out_name, raw_val);
            } else if native_int_literal_fits_inline(val) {
                let raw_val = builder.ins().iconst(types::I64, val);
                def_inline_int_value(
                    builder,
                    vars,
                    representation_plan,
                    out_name,
                    raw_val,
                    box_int(val),
                );
            } else {
                let s = val.to_string();
                let bytes = s.as_bytes();
                let slot = hoists
                    .const_bigint_slots
                    .get(bytes)
                    .expect("boxed integer literal must have a frame anchor");
                let boxed = load_owned_literal(builder, *slot, local_inc_ref_obj);
                def_var_named(builder, vars, out_name, boxed);
            }
        }
        "const_bigint" => {
            let s = op.s_value.as_ref().expect("BigInt string not found");
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let bytes = s.as_bytes();
            let slot = hoists
                .const_bigint_slots
                .get(bytes)
                .expect("bigint literal must have a frame anchor");
            let boxed = load_owned_literal(builder, *slot, local_inc_ref_obj);
            def_var_named(builder, vars, out_name, boxed);
        }
        "const_bool" => {
            let val = op.value.unwrap_or(0);
            let boxed = box_bool(val);
            let iconst = builder.ins().iconst(types::I64, boxed);
            if let Some(ref out__) = op.out {
                let raw = builder.ins().iconst(types::I64, val);
                def_bool_result(builder, vars, representation_plan, out__, iconst, Some(raw));
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
                if representation_plan.is_float_unboxed(out__.as_str()) {
                    def_var_named(builder, vars, out__, raw_f64);
                } else {
                    let boxed = box_float(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    def_var_named(builder, vars, out__, iconst);
                }
            }
        }
        "const_str" => {
            let bytes = require_const_str_payload(op);
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let slot = hoists
                .const_str_slots
                .get(bytes)
                .expect("string literal must have a frame anchor");
            let boxed = load_owned_literal(builder, *slot, local_inc_ref_obj);

            def_var_named(builder, vars, out_name, boxed);
        }
        "const_bytes" => {
            let bytes = op.bytes.as_ref().expect("Bytes not found");
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let slot = hoists
                .const_bytes_slots
                .get(bytes)
                .expect("bytes literal must have a frame anchor");
            let boxed = load_owned_literal(builder, *slot, local_inc_ref_obj);

            def_var_named(builder, vars, out_name, boxed);
        }
        kind => panic!("const literal handler received unsupported op kind `{kind}`"),
    }
    OpFlow::Proceed
}
