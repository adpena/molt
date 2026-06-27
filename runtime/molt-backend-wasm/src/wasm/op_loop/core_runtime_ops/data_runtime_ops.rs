use super::super::result_sink::store_non_none_result_or_drop;
use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_data_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    match op.kind.as_str() {
        "json_parse" => {
            emit_scalar_parse_op(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                "json_parse_scalar",
                "json_parse_scalar_obj",
            );
        }
        "msgpack_parse" => {
            emit_scalar_parse_op(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                "msgpack_parse_scalar",
                "msgpack_parse_scalar_obj",
            );
        }
        "cbor_parse" => {
            emit_scalar_parse_op(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                "cbor_parse_scalar",
                "cbor_parse_scalar_obj",
            );
        }
        "ord" | "chr" | "string_lower" | "string_upper" | "string_capitalize"
        | "bytes_from_obj" | "bytearray_from_obj" | "float_from_obj" | "intarray_from_seq"
        | "memoryview_new" | "memoryview_tobytes" | "str_from_obj" | "repr_from_obj"
        | "ascii_from_obj" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                1,
            );
        }
        "ord_at"
        | "string_join"
        | "string_split"
        | "string_split_validate"
        | "string_strip"
        | "string_lstrip"
        | "string_rstrip"
        | "bytes_split"
        | "bytearray_split"
        | "buffer2d_matmul"
        | "bytes_find"
        | "bytearray_find"
        | "string_find"
        | "string_startswith"
        | "bytes_startswith"
        | "bytearray_startswith"
        | "string_endswith"
        | "bytes_endswith"
        | "bytearray_endswith"
        | "string_count"
        | "bytes_count"
        | "bytearray_count" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                2,
            );
        }
        "string_format" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                "format_builtin",
                2,
            );
        }
        "string_split_field"
        | "string_split_field_len"
        | "string_split_max"
        | "bytes_split_max"
        | "bytearray_split_max"
        | "bytes_from_str"
        | "bytearray_from_str"
        | "int_from_obj"
        | "int_from_str_of_obj"
        | "complex_from_obj"
        | "buffer2d_new"
        | "buffer2d_get"
        | "string_split_ws_dict_inc"
        | "taq_ingest_line" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                3,
            );
        }
        "string_split_field_eq"
        | "string_split_field_len_from_bounds"
        | "bytes_replace"
        | "string_replace"
        | "bytearray_replace"
        | "buffer2d_set"
        | "memoryview_cast"
        | "string_split_sep_dict_inc" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                4,
            );
        }
        "string_split_field_start"
        | "string_split_field_end"
        | "string_split_field_is_ascii"
        | "string_split_field_to_int" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                3,
            );
        }
        "string_split_field_ord_at_bounds" | "statistics_mean_slice" | "statistics_stdev_slice" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                5,
            );
        }
        "bytes_find_slice"
        | "bytearray_find_slice"
        | "string_find_slice"
        | "string_startswith_slice"
        | "bytes_startswith_slice"
        | "bytearray_startswith_slice"
        | "string_endswith_slice"
        | "bytes_endswith_slice"
        | "bytearray_endswith_slice"
        | "string_count_slice"
        | "bytes_count_slice"
        | "bytearray_count_slice" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                6,
            );
        }
        "bytearray_fill_range" => {
            emit_fixed_arg_import(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                op.kind.as_str(),
                4,
            );
        }
        _ => return false,
    }
    true
}

fn emit_fixed_arg_import(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    import_key: &str,
    arg_count: usize,
) {
    let args = op.args.as_ref().unwrap();
    assert!(
        args.len() >= arg_count,
        "wasm runtime op '{}' expected at least {arg_count} args, got {}",
        op.kind,
        args.len()
    );
    for arg in &args[..arg_count] {
        func.instruction(&Instruction::LocalGet(locals[arg]));
    }
    emit_call(func, reloc_enabled, import_ids[import_key]);
    store_non_none_result_or_drop(func, op, locals);
}

fn emit_scalar_parse_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    scalar_import: &str,
    object_import: &str,
) {
    let args = op.args.as_ref().unwrap();
    let arg_name = &args[0];
    let out_ptr = locals[op.out.as_ref().unwrap()];
    if let Some(scratch) = locals.try_literal_scratch(arg_name) {
        let tmp_rc = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);

        func.instruction(&Instruction::I64Const(8));
        emit_call(func, reloc_enabled, import_ids["alloc"]);
        func.instruction(&Instruction::LocalSet(out_ptr));

        func.instruction(&Instruction::LocalGet(scratch.ptr_local()));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(scratch.len_local()));
        func.instruction(&Instruction::LocalGet(out_ptr));
        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
        emit_call(func, reloc_enabled, import_ids[scalar_import]);
        func.instruction(&Instruction::I64ExtendI32U);
        func.instruction(&Instruction::LocalSet(tmp_rc));

        func.instruction(&Instruction::LocalGet(tmp_rc));
        func.instruction(&Instruction::I64Eqz);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(out_ptr));
        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::LocalSet(out_ptr));
        func.instruction(&Instruction::Else);
        func.instruction(&Instruction::LocalGet(locals[arg_name]));
        emit_call(func, reloc_enabled, import_ids[object_import]);
        func.instruction(&Instruction::LocalSet(out_ptr));
        func.instruction(&Instruction::End);
    } else {
        func.instruction(&Instruction::LocalGet(locals[arg_name]));
        emit_call(func, reloc_enabled, import_ids[object_import]);
        func.instruction(&Instruction::LocalSet(out_ptr));
    }
}
