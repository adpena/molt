use super::*;

#[derive(Clone, Copy)]
pub(super) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
}

#[derive(Clone, Copy)]
pub(super) enum UnaryOp {
    Neg,
}

#[derive(Clone, Copy)]
pub(super) enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy)]
pub(super) enum BitwiseOp {
    And,
    Or,
    Xor,
}
pub(super) fn emit_lir_block_ops(ctx: &mut LirLowerCtx, block: &LirBlock) {
    for op in &block.ops {
        emit_lir_op(ctx, op);
    }
}

fn emit_lir_op(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    match tir_op.opcode {
        OpCode::ConstInt => {
            let val = match tir_op.attrs.get("value") {
                Some(AttrValue::Int(v)) => *v,
                _ => 0,
            };
            if let Some(result) = op.result_values.first() {
                match result.repr {
                    LirRepr::F64 => ctx
                        .instructions
                        .push(Instruction::F64Const(Ieee64::from(val as f64))),
                    _ => ctx.instructions.push(Instruction::I64Const(val)),
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstFloat => {
            let val = match tir_op
                .attrs
                .get("f_value")
                .or_else(|| tir_op.attrs.get("value"))
            {
                Some(AttrValue::Float(v)) => *v,
                _ => 0.0,
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::F64Const(Ieee64::from(val)));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstBool => {
            let val = match tir_op.attrs.get("value") {
                Some(AttrValue::Bool(v)) => *v,
                _ => false,
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::I32Const(if val { 1 } else { 0 }));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstNone => {
            if let Some(result) = op.result_values.first() {
                ctx.instructions.push(Instruction::I64Const(box_none_bits()));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            if let Some(result) = op.result_values.first() {
                ctx.instructions.push(Instruction::I64Const(0));
                ctx.emit_set(result.id);
            }
        }
        OpCode::Add | OpCode::InplaceAdd => emit_lir_binary_arith(ctx, op, ArithOp::Add),
        OpCode::CheckedAdd => {
            // (sum, flag) = signed-i64 add. A TOTAL function with two lanes:
            //
            // RAW lane (both operands LirRepr::I64): EXACT overflow detection
            // at 2^63 (NOT the 47-bit inline-range triple above — that fires
            // 2^16x too early for the overflow_peel fast loop). WASM has no
            // add-with-overflow instruction; the sign-bit identity
            // ((lhs ^ sum) & (rhs ^ sum)) < 0 is exact: overflow occurred
            // iff both operands share a sign and the sum's sign differs.
            //
            // BOXED lane (any operand unproven — the v1 state on WASM, whose
            // value-keyed RawI64Safe is a 47-bit-window contract that cannot
            // carry an unbounded accumulator): dispatch through the runtime
            // add with both operands NaN-boxed — BigInt-exact, so the sum
            // can never silently wrap and the flag is CONSTANT FALSE (the
            // peel's slow path is correctly dead; same semantics, no speedup
            // until the RawI64Full lattice extension lands).
            assert!(
                tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
                "checked_add requires 2 operands and 2 results"
            );
            let lhs = tir_op.operands[0];
            let rhs = tir_op.operands[1];
            let sum = op.result_values[0].id;
            let flag = op.result_values[1].id;
            if matches!(ctx.repr_of(lhs), LirRepr::I64)
                && matches!(ctx.repr_of(rhs), LirRepr::I64)
            {
                ctx.emit_get(lhs);
                ctx.emit_get(rhs);
                ctx.instructions.push(Instruction::I64Add);
                ctx.emit_set(sum);
                ctx.emit_get(lhs);
                ctx.emit_get(sum);
                ctx.instructions.push(Instruction::I64Xor);
                ctx.emit_get(rhs);
                ctx.emit_get(sum);
                ctx.instructions.push(Instruction::I64Xor);
                ctx.instructions.push(Instruction::I64And);
                ctx.instructions.push(Instruction::I64Const(0));
                ctx.instructions.push(Instruction::I64LtS);
                ctx.emit_set(flag);
            } else {
                emit_get_boxed_for_repr(ctx, lhs);
                emit_get_boxed_for_repr(ctx, rhs);
                ctx.instructions.push(Instruction::Call(0));
                ctx.emit_set(sum);
                ctx.instructions.push(Instruction::I32Const(0));
                ctx.emit_set(flag);
            }
        }
        OpCode::CheckedMul => {
            // (product, flag) = signed-i64 multiply. BOXED-LANE-ONLY v1.
            //
            // WASM has no multiply-with-overflow instruction and no raw
            // 64x64->128 widening primitive, so there is NO sound raw fast
            // lane today (unlike CheckedAdd's sign-bit identity). Rather than
            // fabricate a fake helper or a wrong narrow-range check, every
            // CheckedMul bails this function out of the WASM fast lane via the
            // `Call(0)` BAIL sentinel — the guarded slow path then runs the
            // boxed runtime multiply (`molt_mul`), which is BigInt-exact, so
            // the product can never silently wrap. The overflow flag is set
            // CONSTANT FALSE (the peel's slow path is correctly dead on this
            // bailed lane; same semantics, no speedup). This is a DOCUMENTED
            // target limitation per the Performance Constitution backend
            // scoreboard, retired when the RawI64Full lattice + a 64x64->128
            // overflow helper land.
            assert!(
                tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
                "checked_mul requires 2 operands and 2 results"
            );
            let lhs = tir_op.operands[0];
            let rhs = tir_op.operands[1];
            let product = op.result_values[0].id;
            let flag = op.result_values[1].id;
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.instructions.push(Instruction::Call(0));
            ctx.emit_set(product);
            ctx.instructions.push(Instruction::I32Const(0));
            ctx.emit_set(flag);
        }
        OpCode::Sub | OpCode::InplaceSub => emit_lir_binary_arith(ctx, op, ArithOp::Sub),
        OpCode::Mul | OpCode::InplaceMul => emit_lir_binary_arith(ctx, op, ArithOp::Mul),
        OpCode::Div => emit_lir_binary_arith(ctx, op, ArithOp::Div),
        OpCode::FloorDiv => emit_lir_binary_arith(ctx, op, ArithOp::FloorDiv),
        OpCode::Mod => emit_lir_binary_arith(ctx, op, ArithOp::Mod),
        OpCode::Neg => emit_lir_unary_arith(ctx, op, UnaryOp::Neg),
        OpCode::Pos | OpCode::Copy | OpCode::DeleteVar | OpCode::BoxVal | OpCode::UnboxVal
        | OpCode::TypeGuard => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                if matches!(
                    tir_op.attrs.get("_original_kind"),
                    Some(AttrValue::Str(kind)) if kind == "binding_alias"
                ) {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.emit_runtime_call("inc_ref_obj");
                }
                ctx.emit_get(src);
                ctx.emit_set(result.id);
            }
        }
        OpCode::Eq => emit_lir_comparison(ctx, op, CmpOp::Eq),
        OpCode::Ne => emit_lir_comparison(ctx, op, CmpOp::Ne),
        OpCode::Lt => emit_lir_comparison(ctx, op, CmpOp::Lt),
        OpCode::Le => emit_lir_comparison(ctx, op, CmpOp::Le),
        OpCode::Gt => emit_lir_comparison(ctx, op, CmpOp::Gt),
        OpCode::Ge => emit_lir_comparison(ctx, op, CmpOp::Ge),
        OpCode::BitAnd => emit_lir_bitwise(ctx, op, BitwiseOp::And),
        OpCode::BitOr => emit_lir_bitwise(ctx, op, BitwiseOp::Or),
        OpCode::BitXor => emit_lir_bitwise(ctx, op, BitwiseOp::Xor),
        OpCode::BitNot => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                // `~x` is a bare `x ^ -1` only when `x` is a proven raw i64; an
                // unproven (`DynBox`/`MaybeBigInt`) operand must dispatch through
                // the runtime helper (a raw `I64Xor` on a NaN-boxed word would be
                // a miscompile). On the production fast path the resulting
                // `Call(0)` bails to the guarded slow path.
                if ctx.repr_of(src) == LirRepr::I64 {
                    ctx.emit_get(src);
                    ctx.instructions.push(Instruction::I64Const(-1));
                    ctx.instructions.push(Instruction::I64Xor);
                } else {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.instructions.push(Instruction::Call(0));
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::Shl => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                let result_id = result.id;
                let result_repr = result.repr;
                // Shifts REQUIRE the raw-result proof: a raw `i64.shl` whose
                // count is >= 64 masks mod 64 (wrong value) and a `<<` result can
                // exceed i64. The value-range seed grants `LirRepr::I64` only when
                // the count is range-proven `[0, 63]` and the result fits inline.
                emit_lir_i64_binary_or_boxed(
                    ctx,
                    tir_op.operands[0],
                    tir_op.operands[1],
                    result_id,
                    result_repr,
                    Instruction::I64Shl,
                    true,
                );
            }
        }
        OpCode::Shr => {
            if tir_op.operands.len() >= 2
                && let Some(result) = op.result_values.first()
            {
                let result_id = result.id;
                let result_repr = result.repr;
                emit_lir_i64_binary_or_boxed(
                    ctx,
                    tir_op.operands[0],
                    tir_op.operands[1],
                    result_id,
                    result_repr,
                    Instruction::I64ShrS,
                    true,
                );
            }
        }
        OpCode::Not => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                ctx.emit_get(src);
                ctx.instructions.push(Instruction::I32Eqz);
                ctx.emit_set(result.id);
            }
        }
        OpCode::And | OpCode::Or => {
            if tir_op.operands.len() >= 2 && !op.result_values.is_empty() {
                emit_lir_bool_select(ctx, op, tir_op.opcode == OpCode::And);
            }
        }
        OpCode::Bool => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                match ctx.repr_of(src) {
                    LirRepr::Bool1 => ctx.emit_get(src),
                    LirRepr::F64 => {
                        ctx.emit_get(src);
                        ctx.instructions
                            .push(Instruction::F64Const(Ieee64::from(0.0)));
                        ctx.instructions.push(Instruction::F64Ne);
                    }
                    _ => {
                        ctx.emit_get(src);
                        ctx.instructions.push(Instruction::Call(0));
                    }
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::CallBuiltin
            if matches!(
                tir_op.attrs.get("lir.truthy_cond"),
                Some(AttrValue::Bool(true))
            ) =>
        {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                match ctx.repr_of(src) {
                    LirRepr::Bool1 => ctx.emit_get(src),
                    LirRepr::F64 => {
                        ctx.emit_get(src);
                        ctx.instructions
                            .push(Instruction::F64Const(Ieee64::from(0.0)));
                        ctx.instructions.push(Instruction::F64Ne);
                    }
                    _ => {
                        ctx.emit_get(src);
                        ctx.instructions.push(Instruction::Call(0));
                    }
                }
                ctx.emit_set(result.id);
            }
        }
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallMethodIc
        | OpCode::CallSuperMethodIc
        | OpCode::CallBuiltin
        | OpCode::OrdAt
        | OpCode::BuildList
        | OpCode::BuildDict
        | OpCode::BuildTuple
        | OpCode::BuildSet
        | OpCode::BuildSlice
        | OpCode::LoadAttr
        | OpCode::StoreAttr
        | OpCode::DelAttr
        | OpCode::Index
        | OpCode::StoreIndex
        | OpCode::DelIndex
        | OpCode::Alloc
        | OpCode::StackAlloc
        | OpCode::ObjectNewBound
        | OpCode::ObjectNewBoundStack
        | OpCode::Free
        | OpCode::GetIter
        | OpCode::IterNext
        | OpCode::IterNextUnboxed
        | OpCode::ForIter
        | OpCode::StateSwitch
        | OpCode::StateTransition
        | OpCode::StateYield
        | OpCode::ChanSendYield
        | OpCode::ChanRecvYield
        | OpCode::ClosureLoad
        | OpCode::ClosureStore
        | OpCode::Import
        | OpCode::ImportFrom
        | OpCode::ModuleCacheGet
        | OpCode::ModuleCacheSet
        | OpCode::ModuleCacheDel
        | OpCode::ModuleGetAttr
        | OpCode::ModuleImportFrom
        | OpCode::ModuleGetGlobal
        | OpCode::ModuleGetName
        | OpCode::ModuleSetAttr
        | OpCode::ModuleDelGlobal
        | OpCode::ModuleDelGlobalIfPresent
        | OpCode::Pow
        | OpCode::Is
        | OpCode::IsNot
        | OpCode::In
        | OpCode::NotIn
        | OpCode::Raise
        | OpCode::CheckException
        | OpCode::ExceptionPending
        // Reads a function object's defaults version stamp via a runtime call;
        // the LIR fast lane does not model it, so bail this function to the
        // generic WASM emitter (which has a `function_defaults_version` arm).
        | OpCode::FunctionDefaultsVersion
        | OpCode::AllocTask
        | OpCode::Yield
        | OpCode::YieldFrom
        | OpCode::ScfIf
        | OpCode::ScfFor
        | OpCode::ScfWhile
        | OpCode::ScfYield
        | OpCode::TryStart
        | OpCode::TryEnd
        | OpCode::StateBlockStart
        | OpCode::StateBlockEnd
        | OpCode::WarnStderr
        // ConstBigInt needs a data segment + molt_bigint_from_str, which the
        // LIR fast lane does not model — bail the function to the generic
        // emitter (which handles `const_bigint` natively).
        | OpCode::ConstBigInt => {
            for &operand in &tir_op.operands {
                ctx.emit_get(operand);
            }
            ctx.instructions.push(Instruction::Call(0));
            if let Some(result) = op.result_values.first() {
                ctx.emit_set(result.id);
            }
        }
        // RC drop-insertion ops (design 20, §4.3 Phase 4). `molt_dec_ref_obj` /
        // `molt_inc_ref_obj` take the NaN-boxed value by value and fast-path
        // non-pointers, so passing the operand's boxed form is always safe; the
        // repr filter in the drop pass already excludes raw-scalar carriers, so
        // the operand here is a heap-carrying (NaN-boxed-pointer) value. A NAMED
        // runtime call keeps the function in the LIR fast lane rather than
        // bailing it (`Call(0)`) to the generic emitter — preserving the WASM
        // perf contract for drop-inserted functions. Neither op has a result.
        OpCode::DecRef | OpCode::DelBoundary => {
            if let Some(&operand) = tir_op.operands.first() {
                emit_get_boxed_for_repr(ctx, operand);
                ctx.emit_runtime_call("dec_ref_obj");
            }
        }
        OpCode::IncRef => {
            if let Some(&operand) = tir_op.operands.first() {
                emit_get_boxed_for_repr(ctx, operand);
                ctx.emit_runtime_call("inc_ref_obj");
            }
        }
    }
}
