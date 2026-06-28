use super::lir_context::LirLowerCtx;
use super::lir_runtime_ops::{
    emit_lir_alloc, emit_lir_boxed_binary_runtime_call, emit_lir_boxed_operands_runtime_call,
    emit_lir_build_slice, emit_lir_closure_load, emit_lir_closure_store, emit_lir_del_index,
    emit_lir_exception_pending, emit_lir_get_iter, emit_lir_index, emit_lir_iter_next,
    emit_lir_membership, emit_lir_object_new_bound, emit_lir_store_index,
};
use super::lir_scalar::{
    emit_get_boxed_for_repr, emit_lir_binary_arith, emit_lir_bitwise, emit_lir_bool_select,
    emit_lir_comparison, emit_lir_i64_binary_or_boxed, emit_lir_identity_comparison,
    emit_lir_truthiness_i32, emit_lir_unary_arith, emit_lir_unary_pos,
};
use crate::wasm::body::WasmLirFallbackReason;
use crate::wasm::const_materialization::{WasmConstMaterializationScratch, WasmConstOpPolicy};
use crate::wasm::lir_fast::LirRuntimeCall;
use crate::wasm_abi_generated::{WasmConstLirFastPolicy, WasmConstScalarValue};
use molt_codegen_abi::box_none_bits;
use molt_tir::tir::lir::{LirBlock, LirOp, LirRepr};
use molt_tir::tir::ops::{AttrValue, OpCode};
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Ieee64, Instruction, ValType};

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

fn emit_checked_mul_overflow_flag(
    ctx: &mut LirLowerCtx,
    lhs: ValueId,
    rhs: ValueId,
    product: ValueId,
) {
    ctx.emit_get(lhs);
    ctx.instructions.push(Instruction::I64Const(0));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I32)));
    ctx.instructions.push(Instruction::I32Const(0));
    ctx.instructions.push(Instruction::Else);

    ctx.emit_get(lhs);
    ctx.instructions.push(Instruction::I64Const(-1));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.emit_get(rhs);
    ctx.instructions.push(Instruction::I64Const(i64::MIN));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.instructions.push(Instruction::I32And);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I32)));
    ctx.instructions.push(Instruction::I32Const(1));
    ctx.instructions.push(Instruction::Else);
    ctx.emit_get(product);
    ctx.emit_get(lhs);
    ctx.instructions.push(Instruction::I64DivS);
    ctx.emit_get(rhs);
    ctx.instructions.push(Instruction::I64Ne);
    ctx.instructions.push(Instruction::End);

    ctx.instructions.push(Instruction::End);
}

pub(super) fn emit_lir_block_ops(ctx: &mut LirLowerCtx, block: &LirBlock) {
    for op in &block.ops {
        emit_lir_op(ctx, op);
    }
}

fn const_policy_for_opcode(opcode: OpCode) -> WasmConstOpPolicy {
    WasmConstOpPolicy::for_tir_opcode(opcode)
        .unwrap_or_else(|| panic!("opcode {opcode:?} is not a WASM const policy opcode"))
}

fn assert_const_lir_fast_policy(
    opcode: OpCode,
    expected: WasmConstLirFastPolicy,
) -> WasmConstOpPolicy {
    let policy = const_policy_for_opcode(opcode);
    assert_eq!(
        policy.lir_fast_policy(),
        expected,
        "generated WASM const LIR-fast policy drifted for {opcode:?}"
    );
    policy
}

fn emit_const_materialization(ctx: &mut LirLowerCtx, op: &LirOp) {
    let policy =
        assert_const_lir_fast_policy(op.tir_op.opcode, WasmConstLirFastPolicy::Materialize);
    let result = op.result_values.first().unwrap_or_else(|| {
        panic!(
            "generated WASM const policy requires a result for {:?}",
            op.tir_op.opcode
        )
    });
    let scratch = policy.needs_literal_scratch().then(|| {
        WasmConstMaterializationScratch::new(
            ctx.alloc_scratch_local(ValType::I64),
            ctx.alloc_scratch_local(ValType::I64),
        )
    });
    let out_local = ctx.get_local(result.id);
    ctx.emit_const_materialization(policy.tir_materialization(&op.tir_op, out_local, scratch));
}

fn emit_lir_op(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    match tir_op.opcode {
        OpCode::ConstInt => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            let val = match policy.required_tir_scalar_value(tir_op) {
                WasmConstScalarValue::Int(value) => value,
                other => panic!(
                    "generated WASM const policy produced {other:?} for {:?}",
                    tir_op.opcode
                ),
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
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            let val = match policy.required_tir_scalar_value(tir_op) {
                WasmConstScalarValue::Float(value) => value,
                other => panic!(
                    "generated WASM const policy produced {other:?} for {:?}",
                    tir_op.opcode
                ),
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::F64Const(Ieee64::from(val)));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstBool => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            let val = match policy.required_tir_scalar_value(tir_op) {
                WasmConstScalarValue::Bool(value) => value,
                other => panic!(
                    "generated WASM const policy produced {other:?} for {:?}",
                    tir_op.opcode
                ),
            };
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::I32Const(if val { 1 } else { 0 }));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstNone => {
            let policy = assert_const_lir_fast_policy(tir_op.opcode, WasmConstLirFastPolicy::Lower);
            assert_eq!(
                policy.required_tir_scalar_value(tir_op),
                WasmConstScalarValue::NoneValue,
                "generated WASM const policy must classify ConstNone as NoneValue"
            );
            if let Some(result) = op.result_values.first() {
                ctx.instructions
                    .push(Instruction::I64Const(box_none_bits()));
                ctx.emit_set(result.id);
            }
        }
        OpCode::ConstStr | OpCode::ConstBytes => {
            match const_policy_for_opcode(tir_op.opcode).lir_fast_policy() {
                WasmConstLirFastPolicy::Materialize => emit_const_materialization(ctx, op),
                WasmConstLirFastPolicy::Lower => {
                    panic!(
                        "generated WASM const policy requires direct LIR lowering for {:?}",
                        tir_op.opcode
                    );
                }
            }
        }
        OpCode::ConstBigInt => match const_policy_for_opcode(tir_op.opcode).lir_fast_policy() {
            WasmConstLirFastPolicy::Materialize => emit_const_materialization(ctx, op),
            WasmConstLirFastPolicy::Lower => {
                panic!(
                    "generated WASM const policy requires direct LIR lowering for {:?}",
                    tir_op.opcode
                );
            }
        },
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
            if matches!(ctx.repr_of(lhs), LirRepr::I64) && matches!(ctx.repr_of(rhs), LirRepr::I64)
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
                ctx.emit_bail_to_generic_path(WasmLirFallbackReason::BoxedCheckedArithmetic);
                ctx.emit_set(sum);
                ctx.instructions.push(Instruction::I32Const(0));
                ctx.emit_set(flag);
            }
        }
        OpCode::CheckedMul => {
            // (product, flag) = signed-i64 multiply.
            //
            // RAW lane (both operands and the product carrier are raw i64):
            // emit the wrapping product and an exact overflow predicate using
            // the identity `product / lhs == rhs` when `lhs != 0`, with the
            // `-1 * i64::MIN` trap case classified before the division. The
            // wrapped product is observable only when the flag is false.
            //
            // BOXED/unproven lane: do not pretend a boxed runtime `mul` can
            // populate the raw full-i64 product carrier. That carrier mismatch
            // still belongs to the explicit guarded generic fallback until the
            // upstream representation plan admits a boxed checked-result lane.
            assert!(
                tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
                "checked_mul requires 2 operands and 2 results"
            );
            let lhs = tir_op.operands[0];
            let rhs = tir_op.operands[1];
            let product = op.result_values[0].id;
            let flag = op.result_values[1].id;
            if matches!(ctx.repr_of(lhs), LirRepr::I64)
                && matches!(ctx.repr_of(rhs), LirRepr::I64)
                && op.result_values[0].repr == LirRepr::I64
            {
                ctx.emit_get(lhs);
                ctx.emit_get(rhs);
                ctx.instructions.push(Instruction::I64Mul);
                ctx.emit_set(product);
                emit_checked_mul_overflow_flag(ctx, lhs, rhs, product);
                ctx.emit_set(flag);
            } else {
                emit_get_boxed_for_repr(ctx, lhs);
                emit_get_boxed_for_repr(ctx, rhs);
                ctx.emit_bail_to_generic_path(WasmLirFallbackReason::BoxedCheckedArithmetic);
                ctx.emit_set(product);
                ctx.instructions.push(Instruction::I32Const(0));
                ctx.emit_set(flag);
            }
        }
        OpCode::Sub | OpCode::InplaceSub => emit_lir_binary_arith(ctx, op, ArithOp::Sub),
        OpCode::Mul | OpCode::InplaceMul => emit_lir_binary_arith(ctx, op, ArithOp::Mul),
        OpCode::Div => emit_lir_binary_arith(ctx, op, ArithOp::Div),
        OpCode::FloorDiv => emit_lir_binary_arith(ctx, op, ArithOp::FloorDiv),
        OpCode::Mod => emit_lir_binary_arith(ctx, op, ArithOp::Mod),
        OpCode::Pow => emit_lir_boxed_binary_runtime_call(ctx, op, LirRuntimeCall::Pow),
        OpCode::OrdAt => emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::OrdAt, 2),
        OpCode::Neg => emit_lir_unary_arith(ctx, op, UnaryOp::Neg),
        OpCode::Pos => emit_lir_unary_pos(ctx, op),
        OpCode::Index => emit_lir_index(ctx, op),
        OpCode::StoreIndex => emit_lir_store_index(ctx, op),
        OpCode::DelIndex => emit_lir_del_index(ctx, op),
        OpCode::BuildSlice => emit_lir_build_slice(ctx, op),
        OpCode::GetIter => emit_lir_get_iter(ctx, op),
        OpCode::IterNext => emit_lir_iter_next(ctx, op),
        OpCode::In => emit_lir_membership(ctx, op, false),
        OpCode::NotIn => emit_lir_membership(ctx, op, true),
        OpCode::ExceptionPending => emit_lir_exception_pending(ctx, op),
        OpCode::FunctionDefaultsVersion => emit_lir_boxed_operands_runtime_call(
            ctx,
            op,
            LirRuntimeCall::FunctionDefaultsVersion,
            1,
        ),
        OpCode::ModuleCacheGet => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleCacheGet, 1)
        }
        OpCode::ModuleCacheSet => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleCacheSet, 2)
        }
        OpCode::ModuleCacheDel => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleCacheDel, 1)
        }
        OpCode::ModuleGetAttr => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleGetAttr, 2)
        }
        OpCode::ModuleImportFrom => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleImportFrom, 2)
        }
        OpCode::ModuleGetGlobal => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleGetGlobal, 2)
        }
        OpCode::ModuleGetName => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleGetName, 2)
        }
        OpCode::ModuleSetAttr => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleSetAttr, 3)
        }
        OpCode::ModuleDelGlobal => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleDelGlobal, 2)
        }
        OpCode::ModuleDelGlobalIfPresent => emit_lir_boxed_operands_runtime_call(
            ctx,
            op,
            LirRuntimeCall::ModuleDelGlobalIfPresent,
            2,
        ),
        OpCode::Alloc => emit_lir_alloc(ctx, op),
        OpCode::ObjectNewBound => emit_lir_object_new_bound(ctx, op),
        OpCode::ClosureLoad => emit_lir_closure_load(ctx, op),
        OpCode::ClosureStore => emit_lir_closure_store(ctx, op),
        OpCode::Copy
        | OpCode::DeleteVar
        | OpCode::BoxVal
        | OpCode::UnboxVal
        | OpCode::TypeGuard => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                if matches!(
                    tir_op.attrs.get("_original_kind"),
                    Some(AttrValue::Str(kind)) if kind == "binding_alias"
                ) {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
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
        OpCode::Is => emit_lir_identity_comparison(ctx, op, false),
        OpCode::IsNot => emit_lir_identity_comparison(ctx, op, true),
        OpCode::BitAnd => emit_lir_bitwise(ctx, op, BitwiseOp::And),
        OpCode::BitOr => emit_lir_bitwise(ctx, op, BitwiseOp::Or),
        OpCode::BitXor => emit_lir_bitwise(ctx, op, BitwiseOp::Xor),
        OpCode::BitNot => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                // `~x` is a bare `x ^ -1` only when `x` is a proven raw i64; an
                // unproven (`DynBox`/`MaybeBigInt`) operand must dispatch through
                // the runtime helper (a raw `I64Xor` on a NaN-boxed word would be
                // a miscompile). On the production fast path the typed bail
                // call dispatches through the BigInt-correct runtime helper.
                if ctx.repr_of(src) == LirRepr::I64 {
                    ctx.emit_get(src);
                    ctx.instructions.push(Instruction::I64Const(-1));
                    ctx.instructions.push(Instruction::I64Xor);
                } else {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.emit_runtime_call(LirRuntimeCall::Invert);
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
                    LirRuntimeCall::LShift,
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
                    LirRuntimeCall::RShift,
                );
            }
        }
        OpCode::Not => {
            if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first())
            {
                if result.repr == LirRepr::Bool1 {
                    emit_lir_truthiness_i32(ctx, src);
                    ctx.instructions.push(Instruction::I32Eqz);
                } else {
                    emit_get_boxed_for_repr(ctx, src);
                    ctx.emit_runtime_call(LirRuntimeCall::Not);
                }
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
                emit_lir_truthiness_i32(ctx, src);
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
                emit_lir_truthiness_i32(ctx, src);
                ctx.emit_set(result.id);
            }
        }
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallMethodIc
        | OpCode::CallSuperMethodIc
        | OpCode::CallBuiltin
        | OpCode::BuildList
        | OpCode::BuildDict
        | OpCode::BuildTuple
        | OpCode::BuildSet
        | OpCode::LoadAttr
        | OpCode::StoreAttr
        | OpCode::DelAttr
        | OpCode::StackAlloc
        | OpCode::ObjectNewBoundStack
        | OpCode::Free
        | OpCode::IterNextUnboxed
        | OpCode::ForIter
        | OpCode::StateSwitch
        | OpCode::StateTransition
        | OpCode::StateYield
        | OpCode::ChanSendYield
        | OpCode::ChanRecvYield
        | OpCode::Import
        | OpCode::ImportFrom
        | OpCode::Raise
        | OpCode::CheckException
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
        | OpCode::WarnStderr => {
            for &operand in &tir_op.operands {
                ctx.emit_get(operand);
            }
            ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
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
        // bailing it to the generic emitter, preserving the WASM perf contract
        // for drop-inserted functions. Neither op has a result.
        OpCode::DecRef | OpCode::DelBoundary => {
            if let Some(&operand) = tir_op.operands.first() {
                emit_get_boxed_for_repr(ctx, operand);
                ctx.emit_runtime_call(LirRuntimeCall::DecRefObj);
            }
        }
        OpCode::IncRef => {
            if let Some(&operand) = tir_op.operands.first() {
                emit_get_boxed_for_repr(ctx, operand);
                ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
            }
        }
    }
}
