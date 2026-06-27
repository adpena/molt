use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_bitwise_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    match op.kind.as_str() {
        "bit_or" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    lhs,
                    tmp_lhs,
                    &const_cache,
                    &known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Or);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["bit_or"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["bit_or"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["bit_or"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bit_and" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    lhs,
                    tmp_lhs,
                    &const_cache,
                    &known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64And);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["bit_and"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["bit_and"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["bit_and"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "bit_xor" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    lhs,
                    tmp_lhs,
                    &const_cache,
                    &known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Xor);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["bit_xor"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["bit_xor"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["bit_xor"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "invert" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["invert"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "neg" | "unary_neg" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["neg"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "pos" | "unary_pos" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["pos"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "inplace_bit_or" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    lhs,
                    tmp_lhs,
                    &const_cache,
                    &known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Or);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_bit_or"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["inplace_bit_or"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_bit_or"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "inplace_bit_and" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    lhs,
                    tmp_lhs,
                    &const_cache,
                    &known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64And);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_bit_and"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["inplace_bit_and"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_bit_and"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "inplace_bit_xor" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    lhs,
                    tmp_lhs,
                    &const_cache,
                    &known_raw_ints,
                );
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Xor);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_bit_xor"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["inplace_bit_xor"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_bit_xor"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "lshift" | "shl" | "inplace_lshift" => {
            // `<<` and `<<=`.  Int fast lane identical (builtin int has
            // no __ilshift__); boxed fallback symbol differs —
            // molt_inplace_lshift tries __ilshift__ before the binary
            // chain.
            let boxed_key = if op.kind == "inplace_lshift" {
                "inplace_lshift"
            } else {
                "lshift"
            };
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_opt(func, lhs, tmp_lhs, &const_cache, &known_raw_ints);
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64GeS);
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64Const(64));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64Shl);
                func.instruction(&Instruction::LocalSet(tmp_raw));

                func.instruction(&Instruction::LocalGet(tmp_raw));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64ShrS);
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::I64Eq);
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids[boxed_key]);
                func.instruction(&Instruction::End);

                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids[boxed_key]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids[boxed_key],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids[boxed_key]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "rshift" | "shr" | "inplace_rshift" => {
            // `>>` and `>>=`.  Inplace variant: molt_inplace_rshift
            // tries __irshift__ before the binary chain.
            let boxed_key = if op.kind == "inplace_rshift" {
                "inplace_rshift"
            } else {
                "rshift"
            };
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOrBool,
                );
                let tmp_lhs = locals["__molt_tmp0"];
                let tmp_rhs = locals["__molt_tmp1"];
                let tmp_raw = locals["__molt_tmp2"];
                emit_unbox_int_local_trusted_opt(func, lhs, tmp_lhs, &const_cache, &known_raw_ints);
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64GeS);
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64Const(64));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64ShrS);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids[boxed_key]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids[boxed_key],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids[boxed_key]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        _ => return false,
    }
    true
}
