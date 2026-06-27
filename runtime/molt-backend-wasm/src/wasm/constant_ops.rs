use super::context::CompileFuncContext;
use super::*;

pub(super) struct ConstantOpContext<'a, 'ctx> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) ctx: &'a CompileFuncContext<'ctx>,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
}

pub(super) fn needs_literal_pointer_locals(kind: &str) -> bool {
    matches!(kind, "const_str" | "const_bytes" | "const_bigint")
}

pub(super) fn const_seed_bits(op: &OpIR) -> Option<i64> {
    match op.kind.as_str() {
        "const" => op.value.map(box_int),
        "const_bool" => op.value.map(box_bool),
        "const_float" => op.f_value.map(box_float),
        "const_none" => Some(box_none()),
        _ => None,
    }
}

pub(super) fn needs_seeded_runtime_const(kind: &str) -> bool {
    matches!(
        kind,
        "const_str" | "const_bytes" | "const_bigint" | "const_not_implemented" | "const_ellipsis"
    )
}

pub(super) fn emit_constant_op(
    context: ConstantOpContext<'_, '_>,
    func: &mut Function,
    op: &OpIR,
    known_raw_ints: &mut BTreeMap<u32, i64>,
) -> bool {
    let ConstantOpContext {
        backend,
        ctx,
        import_ids,
        locals,
        const_cache,
        func_index,
        reloc_enabled,
    } = context;

    match op.kind.as_str() {
        "const" => {
            let val = op.value.unwrap();
            func.instruction(&Instruction::I64Const(box_int(val)));
            let local_idx = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(local_idx));
            known_raw_ints.insert(local_idx, val);
        }
        "const_bool" => {
            let val = op.value.unwrap();
            func.instruction(&Instruction::I64Const(box_bool(val)));
            let local_idx = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(local_idx));
            known_raw_ints.remove(&local_idx);
        }
        "const_float" => {
            let val = op.f_value.expect("Float value not found");
            func.instruction(&Instruction::I64Const(box_float(val)));
            let local_idx = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(local_idx));
            known_raw_ints.remove(&local_idx);
        }
        "const_none" => {
            const_cache.emit_none(func);
            let local_idx = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(local_idx));
            known_raw_ints.remove(&local_idx);
        }
        "const_not_implemented" => {
            emit_call(func, reloc_enabled, import_ids["not_implemented"]);
            forget_output_raw_int(op, locals, known_raw_ints);
            let local_idx = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_ellipsis" => {
            emit_call(func, reloc_enabled, import_ids["ellipsis"]);
            forget_output_raw_int(op, locals, known_raw_ints);
            let local_idx = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_str" => {
            forget_output_raw_int(op, locals, known_raw_ints);
            emit_const_str(
                backend,
                func,
                op,
                locals,
                func_index,
                reloc_enabled,
                import_ids,
                ctx.const_str_scratch_segment,
            );
        }
        "const_bigint" => {
            forget_output_raw_int(op, locals, known_raw_ints);
            emit_const_bigint(
                backend,
                func,
                op,
                locals,
                func_index,
                reloc_enabled,
                import_ids,
            );
        }
        "const_bytes" => {
            forget_output_raw_int(op, locals, known_raw_ints);
            emit_const_bytes(
                backend,
                func,
                op,
                locals,
                func_index,
                reloc_enabled,
                import_ids,
                ctx.const_str_scratch_segment,
            );
        }
        _ => return false,
    }
    true
}

pub(super) fn emit_seeded_runtime_const_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    const_str_scratch_segment: DataSegmentRef,
) {
    match op.kind.as_str() {
        "const_not_implemented" => {
            emit_call(func, reloc_enabled, import_ids["not_implemented"]);
            let local_idx = locals[op.out.as_ref().expect("const_not_implemented out")];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_ellipsis" => {
            emit_call(func, reloc_enabled, import_ids["ellipsis"]);
            let local_idx = locals[op.out.as_ref().expect("const_ellipsis out")];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_str" => emit_const_str(
            backend,
            func,
            op,
            locals,
            func_index,
            reloc_enabled,
            import_ids,
            const_str_scratch_segment,
        ),
        "const_bigint" => emit_const_bigint(
            backend,
            func,
            op,
            locals,
            func_index,
            reloc_enabled,
            import_ids,
        ),
        "const_bytes" => emit_const_bytes(
            backend,
            func,
            op,
            locals,
            func_index,
            reloc_enabled,
            import_ids,
            const_str_scratch_segment,
        ),
        _ => panic!("unsupported seeded runtime const op {}", op.kind),
    }
}

fn forget_output_raw_int(
    op: &OpIR,
    locals: &WasmFrameLocals,
    known_raw_ints: &mut BTreeMap<u32, i64>,
) {
    if let Some(out) = op.out.as_ref()
        && let Some(local_idx) = locals.get(out)
    {
        known_raw_ints.remove(local_idx);
    }
}

fn const_str_bytes(op: &OpIR) -> (&str, &[u8]) {
    let out_name = op.out.as_ref().expect("const_str out");
    let bytes = op
        .bytes
        .as_deref()
        .unwrap_or_else(|| op.s_value.as_ref().expect("const_str bytes").as_bytes());
    (out_name, bytes)
}

fn const_bigint_bytes(op: &OpIR) -> (&str, &[u8]) {
    let out_name = op.out.as_ref().expect("const_bigint out");
    let bytes = op.s_value.as_ref().expect("const_bigint string").as_bytes();
    (out_name, bytes)
}

fn const_bytes_bytes(op: &OpIR) -> (&str, &[u8]) {
    (
        op.out.as_ref().expect("const_bytes out"),
        op.bytes.as_deref().expect("const_bytes bytes"),
    )
}

fn emit_literal_ptr_len(
    backend: &mut WasmBackend,
    func: &mut Function,
    out_name: &str,
    bytes: &[u8],
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
) {
    let data = backend.add_data_segment(reloc_enabled, bytes);
    let ptr_local = locals[&format!("{out_name}_ptr")];
    let len_local = locals[&format!("{out_name}_len")];
    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::LocalSet(ptr_local));
    func.instruction(&Instruction::I64Const(bytes.len() as i64));
    func.instruction(&Instruction::LocalSet(len_local));
}

fn emit_scratch_materialized_bytes(
    backend: &mut WasmBackend,
    func: &mut Function,
    out_name: &str,
    bytes: &[u8],
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
) {
    emit_literal_ptr_len(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
    );
    let ptr_local = locals[&format!("{out_name}_ptr")];
    let len_local = locals[&format!("{out_name}_len")];
    func.instruction(&Instruction::LocalGet(ptr_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(len_local));
    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
    emit_call(func, reloc_enabled, import_id);
    func.instruction(&Instruction::Drop);

    let out_local = locals[out_name];
    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
        align: 3,
        offset: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::LocalSet(out_local));
}

fn emit_const_str(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    scratch_segment: DataSegmentRef,
) {
    let (out_name, bytes) = const_str_bytes(op);
    emit_scratch_materialized_bytes(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
        import_ids["string_from_bytes"],
        scratch_segment,
    );
}

fn emit_const_bigint(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
) {
    let (out_name, bytes) = const_bigint_bytes(op);
    emit_literal_ptr_len(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
    );
    let ptr_local = locals[&format!("{out_name}_ptr")];
    let len_local = locals[&format!("{out_name}_len")];
    func.instruction(&Instruction::LocalGet(ptr_local));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::LocalGet(len_local));
    emit_call(func, reloc_enabled, import_ids["bigint_from_str"]);
    let out_local = locals[out_name];
    func.instruction(&Instruction::LocalSet(out_local));
}

fn emit_const_bytes(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    scratch_segment: DataSegmentRef,
) {
    let (out_name, bytes) = const_bytes_bytes(op);
    emit_scratch_materialized_bytes(
        backend,
        func,
        out_name,
        bytes,
        locals,
        func_index,
        reloc_enabled,
        import_ids["bytes_from_bytes"],
        scratch_segment,
    );
}
