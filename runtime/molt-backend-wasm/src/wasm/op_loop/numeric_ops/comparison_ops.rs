use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_comparison_numeric_op(
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
