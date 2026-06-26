use super::*;

#[allow(unused_variables)]
pub(super) fn emit_numeric_op(
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
        "vec_sum_int"
        | "vec_sum_int_trusted"
        | "vec_sum_int_range_iter"
        | "vec_sum_int_range_iter_trusted"
        | "vec_sum_int_range"
        | "vec_sum_int_range_trusted"
        | "vec_sum_float"
        | "vec_sum_float_trusted"
        | "vec_sum_float_range_iter"
        | "vec_sum_float_range_iter_trusted"
        | "vec_sum_float_range"
        | "vec_sum_float_range_trusted"
        | "vec_prod_int"
        | "vec_prod_int_trusted"
        | "vec_prod_int_range"
        | "vec_prod_int_range_trusted"
        | "vec_min_int"
        | "vec_min_int_trusted"
        | "vec_min_int_range"
        | "vec_min_int_range_trusted"
        | "vec_max_int"
        | "vec_max_int_trusted"
        | "vec_max_int_range"
        | "vec_max_int_range_trusted" => {
            let args_names = op.args.as_ref().unwrap();
            let arg_locals: Vec<u32> = args_names.iter().map(|n| locals[n]).collect();
            let out = locals[op.out.as_ref().unwrap()];
            emit_simple_call(
                func,
                reloc_enabled,
                import_ids[op.kind.as_str()],
                &arg_locals,
                out,
            );
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
        "matmul" | "inplace_matmul" => {
            // `@` and `@=`.  No int/float fast lane; the boxed symbol
            // changes — molt_inplace_matmul tries __imatmul__ before
            // the binary __matmul__/__rmatmul__ chain.
            let boxed_key = if op.kind == "inplace_matmul" {
                "inplace_matmul"
            } else {
                "matmul"
            };
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::LocalGet(rhs));
            emit_call(func, reloc_enabled, import_ids[boxed_key]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "div" | "inplace_div" => {
            // `/` and `/=`.  Int/float fast lanes identical (builtin
            // numerics have no __itruediv__); boxed fallback symbol
            // changes — molt_inplace_div tries __itruediv__ before the
            // binary __truediv__/__rtruediv__ chain.
            let boxed_key = if op.kind == "inplace_div" {
                "inplace_div"
            } else {
                "div"
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
                emit_unbox_int_local_trusted_opt(func, lhs, tmp_lhs, &const_cache, &known_raw_ints);
                emit_unbox_int_local_trusted_tee_opt(
                    func,
                    rhs,
                    tmp_rhs,
                    &const_cache,
                    &known_raw_ints,
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::F64ConvertI64S);
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::F64ConvertI64S);
                func.instruction(&Instruction::F64Div);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
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
                func.instruction(&Instruction::F64Div);
                emit_f64_to_i64_canonical(func, locals["__molt_tmp3"]);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids[boxed_key]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "floordiv" | "inplace_floordiv" => {
            // `//` and `//=`.  Int/float fast lanes identical (builtin
            // numerics have no __ifloordiv__); boxed fallback symbol
            // changes — molt_inplace_floordiv tries __ifloordiv__
            // before the binary __floordiv__/__rfloordiv__ chain.
            let boxed_key = if op.kind == "inplace_floordiv" {
                "inplace_floordiv"
            } else {
                "floordiv"
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
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64DivS);
                func.instruction(&Instruction::LocalSet(tmp_raw));

                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64RemS);
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::I32Xor);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(tmp_raw));
                func.instruction(&Instruction::I64Const(1));
                func.instruction(&Instruction::I64Sub);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                func.instruction(&Instruction::End);

                emit_inline_int_range_check(func, tmp_raw, &const_cache);
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
        "mod" | "inplace_mod" => {
            // `%` and `%=`.  Int/float fast lanes identical (builtin
            // numerics have no __imod__); boxed fallback symbol
            // changes — molt_inplace_mod tries __imod__ before the
            // binary __mod__/__rmod__ chain.
            let boxed_key = if op.kind == "inplace_mod" {
                "inplace_mod"
            } else {
                "mod"
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
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64RemS);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                func.instruction(&Instruction::LocalGet(tmp_raw));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::LocalGet(tmp_lhs));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64LtS);
                func.instruction(&Instruction::I32Xor);
                func.instruction(&Instruction::I32And);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(tmp_raw));
                func.instruction(&Instruction::LocalGet(tmp_rhs));
                func.instruction(&Instruction::I64Add);
                func.instruction(&Instruction::LocalSet(tmp_raw));
                func.instruction(&Instruction::End);
                emit_inline_int_range_check(func, tmp_raw, &const_cache);
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
        "pow" | "inplace_pow" => {
            // `**` and `**=`.  No int/float fast lane in WASM; the
            // boxed symbol changes — molt_inplace_pow tries __ipow__
            // before the binary __pow__/__rpow__ chain.
            let boxed_key = if op.kind == "inplace_pow" {
                "inplace_pow"
            } else {
                "pow"
            };
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::LocalGet(rhs));
            emit_call(func, reloc_enabled, import_ids[boxed_key]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "pow_mod" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            let modulus = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::LocalGet(rhs));
            func.instruction(&Instruction::LocalGet(modulus));
            emit_call(func, reloc_enabled, import_ids["pow_mod"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "round" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            let ndigits = locals[&args[1]];
            let has_ndigits = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(ndigits));
            func.instruction(&Instruction::LocalGet(has_ndigits));
            emit_call(func, reloc_enabled, import_ids["round"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "trunc" => {
            let args = op.args.as_ref().unwrap();
            let val = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["trunc"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "lt" => {
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
                func.instruction(&Instruction::I64LtS);
                emit_box_bool_from_i32(func);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["lt"],
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
                func.instruction(&Instruction::F64Lt);
                emit_box_bool_from_i32(func);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["lt"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "le" => {
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
                func.instruction(&Instruction::I64LeS);
                emit_box_bool_from_i32(func);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["le"],
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
                func.instruction(&Instruction::F64Le);
                emit_box_bool_from_i32(func);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["le"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "gt" => {
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
                func.instruction(&Instruction::I64GtS);
                emit_box_bool_from_i32(func);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["gt"],
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
                func.instruction(&Instruction::F64Gt);
                emit_box_bool_from_i32(func);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["gt"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "ge" => {
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
                func.instruction(&Instruction::I64GeS);
                emit_box_bool_from_i32(func);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["ge"],
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
                func.instruction(&Instruction::F64Ge);
                emit_box_bool_from_i32(func);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["ge"]);
                func.instruction(&Instruction::End);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "eq" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOnly,
                );
                // Box/unbox elimination: when both operands are
                // known NaN-boxed integers, equality of the boxed
                // representations implies equality of the raw
                // values (same tag prefix).  Skip unbox entirely.
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Eq);
                emit_box_bool_from_i32(func);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["eq"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["eq"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "ne" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                let guarded = emit_trusted_int_fast_path_guard_open(
                    func,
                    &[lhs, rhs],
                    &known_raw_ints,
                    IntFastLane::IntOnly,
                );
                // Box/unbox elimination: compare NaN-boxed values
                // directly — same tag means ne(boxed) iff ne(raw).
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                func.instruction(&Instruction::I64Ne);
                emit_box_bool_from_i32(func);
                if guarded {
                    emit_trusted_int_fast_path_guard_close(
                        func,
                        reloc_enabled,
                        &[lhs, rhs],
                        import_ids["ne"],
                    );
                }
            } else {
                func.instruction(&Instruction::LocalGet(lhs));
                func.instruction(&Instruction::LocalGet(rhs));
                emit_call(func, reloc_enabled, import_ids["ne"]);
            }
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "string_eq" => {
            let args = op.args.as_ref().unwrap();
            let lhs = locals[&args[0]];
            let rhs = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(lhs));
            func.instruction(&Instruction::LocalGet(rhs));
            emit_call(func, reloc_enabled, import_ids["string_eq"]);
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
