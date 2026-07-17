mod const_ops;
mod copy_ops;
mod fallback;
mod generated_calls;
mod numeric_selection;
mod refcount_ops;

use self::const_ops::emit_lir_const;
use self::copy_ops::{emit_lir_copy_or_original_kind, emit_lir_identity_copy};
use self::fallback::emit_lir_unsupported_fallback;
use self::generated_calls::emit_lir_generated_fixed_runtime_call;
use self::numeric_selection::numeric_selection_for_opcode;
use self::refcount_ops::emit_lir_refcount_op;
use super::lir_context::LirLowerCtx;
use super::lir_runtime_ops::{
    LirSequenceBuilderFinish, emit_lir_alloc, emit_lir_attr, emit_lir_boxed_operands_runtime_call,
    emit_lir_build_dict, emit_lir_build_set, emit_lir_build_slice, emit_lir_closure_load,
    emit_lir_closure_store, emit_lir_del_index, emit_lir_exception_pending, emit_lir_get_iter,
    emit_lir_index, emit_lir_iter_next, emit_lir_membership, emit_lir_object_new_bound,
    emit_lir_sequence_builder, emit_lir_store_index, emit_lir_unpack_sequence,
};
use super::lir_scalar::{
    emit_lir_binary_arith, emit_lir_bit_not, emit_lir_bitwise, emit_lir_bool, emit_lir_bool_select,
    emit_lir_checked_add, emit_lir_checked_mul, emit_lir_comparison, emit_lir_identity_comparison,
    emit_lir_not, emit_lir_shift, emit_lir_truthy_cond_builtin, emit_lir_unary_arith,
    emit_lir_unary_pos,
};
use super::runtime_calls::numeric_lir_runtime_call;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::lir::{LirBlock, LirOp};
use molt_tir::tir::ops::{AttrValue, OpCode};

pub(super) fn emit_lir_block_ops(ctx: &mut LirLowerCtx, block: &LirBlock) {
    for op in &block.ops {
        emit_lir_op(ctx, op);
    }
}

fn emit_lir_op(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    match tir_op.opcode {
        OpCode::ConstInt
        | OpCode::ConstFloat
        | OpCode::ConstBool
        | OpCode::ConstNone
        | OpCode::ConstStr
        | OpCode::ConstBytes
        | OpCode::ConstBigInt => emit_lir_const(ctx, op),
        OpCode::Add | OpCode::InplaceAdd => {
            emit_lir_binary_arith(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
        OpCode::CheckedAdd => emit_lir_checked_add(ctx, op),
        OpCode::CheckedMul => emit_lir_checked_mul(ctx, op),
        OpCode::Sub | OpCode::InplaceSub => {
            emit_lir_binary_arith(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
        OpCode::Mul | OpCode::InplaceMul => {
            emit_lir_binary_arith(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
        OpCode::Div | OpCode::FloorDiv | OpCode::Mod => {
            emit_lir_binary_arith(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
        OpCode::Pow => {
            let selection = numeric_selection_for_opcode(tir_op.opcode);
            emit_lir_boxed_operands_runtime_call(ctx, op, numeric_lir_runtime_call(selection));
        }
        OpCode::OrdAt => emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::OrdAt),
        OpCode::BuildList => emit_lir_sequence_builder(ctx, op, LirSequenceBuilderFinish::List),
        OpCode::BuildTuple => emit_lir_sequence_builder(ctx, op, LirSequenceBuilderFinish::Tuple),
        OpCode::BuildDict => emit_lir_build_dict(ctx, op),
        OpCode::BuildSet => emit_lir_build_set(ctx, op),
        OpCode::Neg => emit_lir_unary_arith(ctx, op, numeric_selection_for_opcode(tir_op.opcode)),
        OpCode::Pos => emit_lir_unary_pos(ctx, op, numeric_selection_for_opcode(tir_op.opcode)),
        OpCode::Index => emit_lir_index(ctx, op),
        OpCode::StoreIndex => emit_lir_store_index(ctx, op),
        OpCode::DelIndex => emit_lir_del_index(ctx, op),
        OpCode::BuildSlice => emit_lir_build_slice(ctx, op),
        OpCode::GetIter => emit_lir_get_iter(ctx, op),
        OpCode::IterNext => emit_lir_iter_next(ctx, op),
        OpCode::UnpackSequence => emit_lir_unpack_sequence(ctx, op),
        OpCode::In => emit_lir_membership(ctx, op, false),
        OpCode::NotIn => emit_lir_membership(ctx, op, true),
        OpCode::ExceptionPending => emit_lir_exception_pending(ctx, op),
        OpCode::FunctionDefaultsVersion
        | OpCode::ModuleCacheGet
        | OpCode::ModuleCacheSet
        | OpCode::ModuleCacheDel
        | OpCode::ModuleGetAttr
        | OpCode::ModuleImportFrom
        | OpCode::ModuleGetGlobal
        | OpCode::ModuleGetName
        | OpCode::ModuleSetAttr
        | OpCode::ModuleDelGlobal
        | OpCode::ModuleDelGlobalIfPresent => emit_lir_generated_fixed_runtime_call(ctx, op),
        OpCode::Import if !tir_op.operands.is_empty() => {
            emit_lir_generated_fixed_runtime_call(ctx, op)
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
        OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => {
            emit_lir_comparison(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
        OpCode::Is => emit_lir_identity_comparison(ctx, op, false),
        OpCode::IsNot => emit_lir_identity_comparison(ctx, op, true),
        OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor => {
            emit_lir_bitwise(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
        OpCode::BitNot => emit_lir_bit_not(ctx, op, numeric_selection_for_opcode(tir_op.opcode)),
        OpCode::Shl | OpCode::Shr => {
            emit_lir_shift(ctx, op, numeric_selection_for_opcode(tir_op.opcode))
        }
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
            emit_lir_unsupported_fallback(ctx, op);
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
            emit_lir_refcount_op(ctx, op, LirRuntimeCall::DecRefObj)
        }
        OpCode::IncRef => emit_lir_refcount_op(ctx, op, LirRuntimeCall::IncRefObj),
    }
}
