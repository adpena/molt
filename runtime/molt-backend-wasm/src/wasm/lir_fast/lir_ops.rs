use super::lir_context::LirLowerCtx;
use super::lir_runtime_ops::{
    LirSequenceBuilderFinish, emit_lir_alloc, emit_lir_attr, emit_lir_boxed_binary_runtime_call,
    emit_lir_boxed_operands_runtime_call, emit_lir_build_dict, emit_lir_build_set,
    emit_lir_build_slice, emit_lir_closure_load, emit_lir_closure_store, emit_lir_del_index,
    emit_lir_exception_pending, emit_lir_get_iter, emit_lir_index, emit_lir_iter_next,
    emit_lir_membership, emit_lir_object_new_bound, emit_lir_sequence_builder,
    emit_lir_store_index, emit_lir_unsupported_marker,
};
use super::lir_scalar::{
    ArithOp, BitwiseOp, CmpOp, ShiftOp, UnaryOp, emit_get_boxed_for_repr, emit_lir_binary_arith,
    emit_lir_bit_not, emit_lir_bitwise, emit_lir_bool, emit_lir_bool_select, emit_lir_checked_add,
    emit_lir_checked_mul, emit_lir_comparison, emit_lir_identity_comparison, emit_lir_not,
    emit_lir_shift, emit_lir_truthy_cond_builtin, emit_lir_unary_arith, emit_lir_unary_pos,
};
use super::runtime_calls::preserved_copy_runtime_call;
use crate::wasm::body::WasmLirFallbackReason;
use crate::wasm::const_materialization::{WasmConstMaterializationScratch, WasmConstOpPolicy};
use crate::wasm::lir_fast::LirRuntimeCall;
use crate::wasm_abi_generated::{WasmConstLirFastPolicy, WasmConstScalarValue};
use molt_codegen_abi::box_none_bits;
use molt_tir::tir::lir::{LirBlock, LirOp, LirRepr};
use molt_tir::tir::ops::{AttrValue, OpCode};
use wasm_encoder::{Ieee64, Instruction, ValType};

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
        OpCode::CheckedAdd => emit_lir_checked_add(ctx, op),
        OpCode::CheckedMul => emit_lir_checked_mul(ctx, op),
        OpCode::Sub | OpCode::InplaceSub => emit_lir_binary_arith(ctx, op, ArithOp::Sub),
        OpCode::Mul | OpCode::InplaceMul => emit_lir_binary_arith(ctx, op, ArithOp::Mul),
        OpCode::Div => emit_lir_binary_arith(ctx, op, ArithOp::Div),
        OpCode::FloorDiv => emit_lir_binary_arith(ctx, op, ArithOp::FloorDiv),
        OpCode::Mod => emit_lir_binary_arith(ctx, op, ArithOp::Mod),
        OpCode::Pow => emit_lir_boxed_binary_runtime_call(ctx, op, LirRuntimeCall::Pow),
        OpCode::OrdAt => emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::OrdAt, 2),
        OpCode::BuildList => emit_lir_sequence_builder(ctx, op, LirSequenceBuilderFinish::List),
        OpCode::BuildTuple => emit_lir_sequence_builder(ctx, op, LirSequenceBuilderFinish::Tuple),
        OpCode::BuildDict => emit_lir_build_dict(ctx, op),
        OpCode::BuildSet => emit_lir_build_set(ctx, op),
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
        OpCode::Import if !tir_op.operands.is_empty() => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::ModuleImport, 1)
        }
        OpCode::LoadAttr | OpCode::StoreAttr | OpCode::DelAttr => emit_lir_attr(ctx, op),
        OpCode::Alloc => emit_lir_alloc(ctx, op),
        OpCode::ObjectNewBound => emit_lir_object_new_bound(ctx, op),
        OpCode::ClosureLoad => emit_lir_closure_load(ctx, op),
        OpCode::ClosureStore => emit_lir_closure_store(ctx, op),
        OpCode::Copy => emit_lir_copy_or_original_kind(ctx, op),
        OpCode::DeleteVar | OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard => {
            emit_lir_identity_copy(ctx, op)
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
        OpCode::BitNot => emit_lir_bit_not(ctx, op),
        OpCode::Shl => emit_lir_shift(ctx, op, ShiftOp::Left),
        OpCode::Shr => emit_lir_shift(ctx, op, ShiftOp::Right),
        OpCode::Not => emit_lir_not(ctx, op),
        OpCode::And | OpCode::Or => {
            if tir_op.operands.len() >= 2 && !op.result_values.is_empty() {
                emit_lir_bool_select(ctx, op, tir_op.opcode == OpCode::And);
            }
        }
        OpCode::Bool => emit_lir_bool(ctx, op),
        OpCode::CallBuiltin
            if matches!(
                tir_op.attrs.get("lir.truthy_cond"),
                Some(AttrValue::Bool(true))
            ) =>
        {
            emit_lir_truthy_cond_builtin(ctx, op);
        }
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallMethodIc
        | OpCode::CallSuperMethodIc
        | OpCode::CallBuiltin
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

fn emit_lir_identity_copy(ctx: &mut LirLowerCtx, op: &LirOp) {
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        ctx.emit_get(src);
        ctx.emit_set(result.id);
    }
}

fn emit_lir_binding_alias(ctx: &mut LirLowerCtx, op: &LirOp) {
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        emit_get_boxed_for_repr(ctx, src);
        ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
        ctx.emit_get(src);
        ctx.emit_set(result.id);
    }
}

fn emit_lir_copy_or_original_kind(ctx: &mut LirLowerCtx, op: &LirOp) {
    match original_kind(op) {
        Some("binding_alias") => emit_lir_binding_alias(ctx, op),
        Some(kind)
            if crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(kind) =>
        {
            emit_lir_identity_copy(ctx, op)
        }
        Some(kind) if let Some(runtime) = preserved_copy_runtime_call(kind) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, runtime.call, runtime.operand_count)
        }
        Some(_) => emit_lir_unsupported_marker(ctx, op),
        None => emit_lir_identity_copy(ctx, op),
    }
}

fn original_kind(op: &LirOp) -> Option<&str> {
    match op.tir_op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}
