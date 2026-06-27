use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_additive_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    match op.kind.as_str() {
        "add" => {
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
                func.instruction(&Instruction::I64Add);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["add"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["add"],
                    );
                }
            } else {
                // fast_float: check if both operands are plain f64
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::F64Add);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["add"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "inplace_add" => {
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
                func.instruction(&Instruction::I64Add);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_add"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["inplace_add"],
                    );
                }
            } else {
                // fast_float: check if both operands are plain f64
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::F64Add);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_add"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "sub" => {
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
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["sub"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["sub"],
                    );
                }
            } else {
                // fast_float: check if both operands are plain f64
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::F64Sub);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["sub"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "mul" => {
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
                func.instruction(&Instruction::I64Mul);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["mul"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["mul"],
                    );
                }
            } else {
                // fast_float: check if both operands are plain f64
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::F64Mul);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["mul"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "inplace_sub" => {
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
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_sub"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["inplace_sub"],
                    );
                }
            } else {
                // fast_float: check if both operands are plain f64
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::F64Sub);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_sub"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "inplace_mul" => {
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
                func.instruction(&Instruction::I64Mul);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                emit_box_int_from_local_opt(func, tmp_raw, &known_raw_ints);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_mul"]);
                func.instruction(&Instruction::End);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["inplace_mul"],
                    );
                }
            } else {
                // fast_float: check if both operands are plain f64
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Const(48));
                func.instruction(&Instruction::I64ShrU);
                func.instruction(&Instruction::I64Const(0x7FF9));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::I64Const(5));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::I32Eqz);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::F64ReinterpretI64);
                func.instruction(&Instruction::F64Mul);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["inplace_mul"]);
                func.instruction(&Instruction::End);
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
