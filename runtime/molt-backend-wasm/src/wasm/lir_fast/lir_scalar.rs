use super::lir_context::LirLowerCtx;
use super::lir_ops::{ArithOp, BitwiseOp, CmpOp, UnaryOp};
use super::runtime_calls::LirRuntimeCall;
use crate::wasm_values::push_f64_to_i64_canonical;
use molt_codegen_abi::{
    INLINE_INT_BIAS, INLINE_INT_LIMIT, INT_MASK, INT_MAX_INLINE as INLINE_INT_MAX,
    INT_MIN_INLINE as INLINE_INT_MIN, QNAN, QNAN_TAG_BOOL_I64, QNAN_TAG_INT_I64, QNAN_TAG_MASK_I64,
    TAG_BOOL, box_none_bits,
};
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::AttrValue;
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Instruction, ValType};

fn raw_i64_arith_instruction(arith: ArithOp) -> Instruction<'static> {
    match arith {
        ArithOp::Add => Instruction::I64Add,
        ArithOp::Sub => Instruction::I64Sub,
        ArithOp::Mul => Instruction::I64Mul,
        ArithOp::Div | ArithOp::FloorDiv => Instruction::I64DivS,
        ArithOp::Mod => Instruction::I64RemS,
    }
}

fn boxed_arith_runtime_call(arith: ArithOp) -> LirRuntimeCall {
    match arith {
        ArithOp::Add => LirRuntimeCall::Add,
        ArithOp::Sub => LirRuntimeCall::Sub,
        ArithOp::Mul => LirRuntimeCall::Mul,
        ArithOp::Div => LirRuntimeCall::Div,
        ArithOp::FloorDiv => LirRuntimeCall::FloorDiv,
        ArithOp::Mod => LirRuntimeCall::Mod,
    }
}

pub(super) fn emit_lir_binary_arith(ctx: &mut LirLowerCtx, op: &LirOp, arith: ArithOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let dst = op.result_values[0].id;
    if matches!(
        tir_op.attrs.get("lir.checked_overflow"),
        Some(AttrValue::Bool(true))
    ) {
        let main = op.result_values[0].id;
        let overflow_box = op.result_values[1].id;
        let overflow_flag = op.result_values[2].id;

        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(raw_i64_arith_instruction(arith));
        ctx.emit_set(main);

        ctx.emit_get(main);
        ctx.instructions.push(Instruction::I64Const(INLINE_INT_MIN));
        ctx.instructions.push(Instruction::I64GeS);
        ctx.emit_get(main);
        ctx.instructions.push(Instruction::I64Const(INLINE_INT_MAX));
        ctx.instructions.push(Instruction::I64LeS);
        ctx.instructions.push(Instruction::I32And);
        ctx.instructions.push(Instruction::If(BlockType::Empty));
        emit_box_none(ctx);
        ctx.emit_set(overflow_box);
        ctx.instructions.push(Instruction::I32Const(0));
        ctx.emit_set(overflow_flag);
        ctx.instructions.push(Instruction::Else);
        // Inline boxing is sound here: the checked-triple gate
        // (`lowers_to_checked_i64_arithmetic`) only fires when BOTH operands
        // are value-range-proven inside the 47-bit inline window. The boxed
        // side channel uses the same arithmetic opcode as the raw main result,
        // so Add/Sub/Mul cannot drift from the generated checked-triple fact.
        emit_box_inline_i64(ctx, lhs);
        emit_box_inline_i64(ctx, rhs);
        ctx.emit_runtime_call(boxed_arith_runtime_call(arith));
        ctx.emit_set(overflow_box);
        ctx.instructions.push(Instruction::I32Const(1));
        ctx.emit_set(overflow_flag);
        ctx.instructions.push(Instruction::End);
        return;
    }
    let lhs_repr = ctx.repr_of(lhs);
    let rhs_repr = ctx.repr_of(rhs);
    // LIR-lowering marked this op as requiring the boxed runtime dispatch
    // (raw-i64 operands without the inline-window proof — a bare machine op
    // could wrap at 2^63). Honor it before any repr-keyed arm.
    let boxed_dispatch = matches!(
        tir_op.attrs.get("lir.boxed_dispatch"),
        Some(AttrValue::Bool(true))
    );
    // Phase 1 introduces *mixed* reprs (e.g. a proven `RawI64Safe` operand and an
    // unproven `MaybeBigInt`/`DynBox` operand). The boxed fallthrough dispatches
    // through the BigInt-correct runtime helper, which expects NaN-boxed
    // operands — so operands must be pushed *per-arm*, raw only for the
    // homogeneous unboxed arms and BOXED for the runtime-call arm. Pushing raw
    // before the match (the pre-Phase-1 shape) would feed a raw i64 word to
    // `molt_add` on the mixed case → a hard miscompile.
    let result_repr = op.result_values[0].repr;
    match (lhs_repr, rhs_repr) {
        // Bare machine arithmetic requires the RESULT to be a raw carrier too.
        // Raw carriers may include full-i64 `RawI64FullDeopt` CheckedAdd/
        // CheckedMul results. When the result is unproven (boxed repr), a bare
        // op would silently wrap at 2^63 AND deposit a raw word in a
        // DynBox-typed local; such ops take the boxed runtime dispatch below
        // instead. `boxed_dispatch` (proof-driven, set at LIR-lowering)
        // likewise forces the runtime path.
        (LirRepr::I64, LirRepr::I64) if result_repr == LirRepr::I64 && !boxed_dispatch => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(raw_i64_arith_instruction(arith));
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            match arith {
                ArithOp::Add => ctx.instructions.push(Instruction::F64Add),
                ArithOp::Sub => ctx.instructions.push(Instruction::F64Sub),
                ArithOp::Mul => ctx.instructions.push(Instruction::F64Mul),
                ArithOp::Div => ctx.instructions.push(Instruction::F64Div),
                ArithOp::FloorDiv => {
                    // Python // on floats: floor(a / b)
                    ctx.instructions.push(Instruction::F64Div);
                    ctx.instructions.push(Instruction::F64Floor);
                    // Result already on stack, fall through to emit_set.
                }
                ArithOp::Mod => {
                    // Python fmod: a - floor(a / b) * b
                    // Stack: [lhs, rhs]. We need both values twice.
                    // Allocate scratch locals for the operands.
                    let scratch_a = ctx.alloc_scratch_local(ValType::F64);
                    let scratch_b = ctx.alloc_scratch_local(ValType::F64);
                    // Pop rhs, pop lhs into scratches.
                    ctx.instructions.push(Instruction::LocalSet(scratch_b));
                    ctx.instructions.push(Instruction::LocalSet(scratch_a));
                    // Compute: a - floor(a / b) * b
                    ctx.instructions.push(Instruction::LocalGet(scratch_a));
                    ctx.instructions.push(Instruction::LocalGet(scratch_a));
                    ctx.instructions.push(Instruction::LocalGet(scratch_b));
                    ctx.instructions.push(Instruction::F64Div);
                    ctx.instructions.push(Instruction::F64Floor);
                    ctx.instructions.push(Instruction::LocalGet(scratch_b));
                    ctx.instructions.push(Instruction::F64Mul);
                    ctx.instructions.push(Instruction::F64Sub);
                    // Result on stack, fall through to emit_set.
                }
            }
        }
        _ => {
            // Heterogeneous / boxed operands: dispatch through the runtime
            // helper with both operands NaN-boxed. A named runtime call keeps
            // the function in the LIR fast lane; typed bail markers are reserved
            // for operations that must fall back to the generic emitter.
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(boxed_arith_runtime_call(arith));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

/// Push operand `v` onto the WASM stack in **NaN-boxed** form, ready for a
/// runtime helper call (`molt_add`/`molt_lt`/...). A raw-i64-repr operand is
/// boxed through the overflow-safe path because it may be a full-i64
/// `RawI64FullDeopt` carrier; a `Bool1` is widened to a boxed bool; an `F64` is
/// boxed via the runtime float-box; a `DynBox`/`Ref64` operand is already a
/// NaN-box word and passes through unchanged.
///
/// This is the Phase-1 fix for `emit_lir_binary_arith`'s (and the comparison's)
/// boxed fallthrough: before Phase 1 every int operand was `LirRepr::I64`, so the
/// boxed arm only fired for homogeneous `DynBox`; now a proven `I64` operand can
/// share an op with an unproven `DynBox` operand, and the raw one MUST be boxed
/// before the call.
pub(super) fn emit_get_boxed_for_repr(ctx: &mut LirLowerCtx, v: ValueId) {
    match ctx.repr_of(v) {
        // OVERFLOW-SAFE: raw-i64 carriers may include full-i64
        // `RawI64FullDeopt` CheckedAdd/CheckedMul results; the unchecked inline
        // box truncates mod 2^47.
        LirRepr::I64 => emit_box_i64_overflow_safe(ctx, v),
        LirRepr::Bool1 => {
            ctx.emit_get(v);
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.instructions
                .push(Instruction::I64Const(QNAN_TAG_BOOL_I64));
            ctx.instructions.push(Instruction::I64Or);
        }
        LirRepr::F64 => {
            ctx.emit_get(v);
            let scratch = ctx.alloc_scratch_local(ValType::I64);
            push_f64_to_i64_canonical(|instruction| ctx.instructions.push(instruction), scratch);
        }
        LirRepr::DynBox | LirRepr::Ref64 => ctx.emit_get(v),
    }
}

pub(super) fn emit_lir_unary_arith(ctx: &mut LirLowerCtx, op: &LirOp, _unary: UnaryOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.is_empty() || op.result_values.is_empty() {
        return;
    }
    let src = tir_op.operands[0];
    let dst = op.result_values[0].id;
    match ctx.repr_of(src) {
        LirRepr::I64 => {
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Sub);
        }
        LirRepr::F64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::F64Neg);
        }
        _ => {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(LirRuntimeCall::Neg);
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

pub(super) fn emit_lir_unary_pos(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.is_empty() || op.result_values.is_empty() {
        return;
    }
    let src = tir_op.operands[0];
    let dst = op.result_values[0].id;
    match (ctx.repr_of(src), op.result_values[0].repr) {
        (LirRepr::I64, LirRepr::I64) | (LirRepr::F64, LirRepr::F64) => ctx.emit_get(src),
        _ => {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(LirRuntimeCall::Pos);
        }
    }
    ctx.emit_set(dst);
}

pub(super) fn emit_lir_truthiness_i32(ctx: &mut LirLowerCtx, src: ValueId) {
    match ctx.repr_of(src) {
        LirRepr::Bool1 => ctx.emit_get(src),
        LirRepr::I64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.instructions.push(Instruction::I64Ne);
        }
        LirRepr::F64 => {
            ctx.emit_get(src);
            ctx.instructions
                .push(Instruction::F64Const(wasm_encoder::Ieee64::from(0.0)));
            ctx.instructions.push(Instruction::F64Ne);
        }
        LirRepr::DynBox | LirRepr::Ref64 => {
            ctx.emit_get(src);
            ctx.instructions
                .push(Instruction::I64Const(QNAN_TAG_MASK_I64));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions
                .push(Instruction::I64Const((QNAN | TAG_BOOL) as i64));
            ctx.instructions.push(Instruction::I64Eq);
            ctx.instructions
                .push(Instruction::If(BlockType::Result(ValType::I32)));
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I32WrapI64);
            ctx.instructions.push(Instruction::I32Const(1));
            ctx.instructions.push(Instruction::I32And);
            ctx.instructions.push(Instruction::Else);
            ctx.emit_get(src);
            ctx.emit_runtime_call(LirRuntimeCall::IsTruthy);
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.instructions.push(Instruction::I64Ne);
            ctx.instructions.push(Instruction::End);
        }
    }
}

pub(super) fn emit_lir_bool_select(ctx: &mut LirLowerCtx, op: &LirOp, is_and: bool) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let result = &op.result_values[0];
    let dst = result.id;
    if ctx.repr_of(lhs) == LirRepr::Bool1
        && ctx.repr_of(rhs) == LirRepr::Bool1
        && result.repr == LirRepr::Bool1
    {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(if is_and {
            Instruction::I32And
        } else {
            Instruction::I32Or
        });
        ctx.emit_set(dst);
        return;
    }

    assert!(
        matches!(result.repr, LirRepr::DynBox | LirRepr::Ref64),
        "boxed Python boolean selection must produce a boxed result, got {:?}",
        result.repr
    );
    assert!(
        crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
            tir_op.opcode
        ),
        "boxed Python boolean selection must mint an owned selected operand"
    );

    emit_get_boxed_for_repr(ctx, lhs);
    ctx.emit_runtime_call(LirRuntimeCall::IsTruthy);
    ctx.instructions.push(Instruction::I64Const(0));
    ctx.instructions.push(Instruction::I64Ne);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    if is_and {
        emit_get_boxed_for_repr(ctx, rhs);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
    }
    ctx.instructions.push(Instruction::Else);
    if is_and {
        emit_get_boxed_for_repr(ctx, lhs);
    } else {
        emit_get_boxed_for_repr(ctx, rhs);
    }
    ctx.instructions.push(Instruction::End);
    ctx.instructions
        .push(Instruction::LocalTee(ctx.get_local(dst)));
    ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
}

pub(super) fn emit_lir_comparison(ctx: &mut LirLowerCtx, op: &LirOp, cmp: CmpOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let dst = op.result_values[0].id;
    // Same per-arm operand push as `emit_lir_binary_arith` (finding #3): the
    // homogeneous unboxed arms push raw operands; the boxed runtime-dispatch arm
    // must push BOTH operands NaN-boxed, so a proven `RawI64Safe` operand sharing
    // a compare with an unproven `DynBox` operand is boxed before the call.
    match (ctx.repr_of(lhs), ctx.repr_of(rhs)) {
        (LirRepr::I64, LirRepr::I64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(match cmp {
                CmpOp::Eq => Instruction::I64Eq,
                CmpOp::Ne => Instruction::I64Ne,
                CmpOp::Lt => Instruction::I64LtS,
                CmpOp::Le => Instruction::I64LeS,
                CmpOp::Gt => Instruction::I64GtS,
                CmpOp::Ge => Instruction::I64GeS,
            });
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(match cmp {
                CmpOp::Eq => Instruction::F64Eq,
                CmpOp::Ne => Instruction::F64Ne,
                CmpOp::Lt => Instruction::F64Lt,
                CmpOp::Le => Instruction::F64Le,
                CmpOp::Gt => Instruction::F64Gt,
                CmpOp::Ge => Instruction::F64Ge,
            });
        }
        _ => {
            // Boxed dispatch through the named runtime comparison keeps the
            // function in the LIR fast lane.
            // The helper returns a NaN-BOXED bool (i64); a Bool1 destination
            // local is i32, so extract bit 0 and wrap.
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(match cmp {
                CmpOp::Eq => LirRuntimeCall::Eq,
                CmpOp::Ne => LirRuntimeCall::Ne,
                CmpOp::Lt => LirRuntimeCall::Lt,
                CmpOp::Le => LirRuntimeCall::Le,
                CmpOp::Gt => LirRuntimeCall::Gt,
                CmpOp::Ge => LirRuntimeCall::Ge,
            });
            if op.result_values[0].repr == LirRepr::Bool1 {
                ctx.instructions.push(Instruction::I64Const(1));
                ctx.instructions.push(Instruction::I64And);
                ctx.instructions.push(Instruction::I32WrapI64);
            }
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

pub(super) fn emit_lir_identity_comparison(ctx: &mut LirLowerCtx, op: &LirOp, invert: bool) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let result = &op.result_values[0];

    emit_get_boxed_for_repr(ctx, lhs);
    emit_get_boxed_for_repr(ctx, rhs);
    ctx.emit_runtime_call(LirRuntimeCall::Is);

    match result.repr {
        LirRepr::Bool1 => {
            ctx.instructions.push(Instruction::I64Const(1));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions.push(Instruction::I32WrapI64);
            if invert {
                ctx.instructions.push(Instruction::I32Eqz);
            }
        }
        LirRepr::DynBox | LirRepr::Ref64 | LirRepr::I64 => {
            if invert {
                ctx.emit_runtime_call(LirRuntimeCall::Not);
            }
        }
        LirRepr::F64 => {
            panic!("identity comparison cannot materialize an f64 result");
        }
    }
    ctx.emit_set(result.id);
}

pub(super) fn emit_lir_bitwise(ctx: &mut LirLowerCtx, op: &LirOp, bw: BitwiseOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let instr = match bw {
        BitwiseOp::And => Instruction::I64And,
        BitwiseOp::Or => Instruction::I64Or,
        BitwiseOp::Xor => Instruction::I64Xor,
    };
    let runtime_call = match bw {
        BitwiseOp::And => LirRuntimeCall::BitAnd,
        BitwiseOp::Or => LirRuntimeCall::BitOr,
        BitwiseOp::Xor => LirRuntimeCall::BitXor,
    };
    // `&`/`|`/`^` never overflow and the raw machine op is always defined for
    // any i64 operands, so the operand proof alone authorizes the raw lane
    // (require_raw_result = false) — no perf regression on the proven-operand
    // bitwise path.
    emit_lir_i64_binary_or_boxed(
        ctx,
        tir_op.operands[0],
        tir_op.operands[1],
        op.result_values[0].id,
        op.result_values[0].repr,
        instr,
        false,
        runtime_call,
    );
}

/// Emit a bare two-operand `i64` machine instruction (`I64And`/`I64Shl`/…)
/// **only** when both operands are proven raw-i64 carriers (`LirRepr::I64`).
/// Otherwise — a `MaybeBigInt`/`DynBox` operand — dispatch through the runtime
/// helper with both operands NaN-boxed (finding #3: a bare `I64*` on a NaN-boxed
/// word is a miscompile). The typed body distinguishes named runtime dispatch
/// from generic-path bail markers, so production never infers semantics from a
/// synthetic call index.
///
/// `require_raw_result` additionally gates the raw lane on the **result** being a
/// raw carrier (`LirRepr::I64`). `I64And`/`I64Or`/`I64Xor` never overflow and the
/// machine op is always defined, so they pass `false` (operand proof suffices).
/// `I64Shl`/`I64ShrS` MUST pass `true`: a `<<` result can exceed i64, and a raw
/// shift whose count is `>= 64` is a silent wrong-value mask-mod-64 on wasm. The
/// shared value-range seed grants a shift result `RawI64Safe` (→ `LirRepr::I64`)
/// ONLY when its count is range-proven in `[0, 63]` and the result fits the inline
/// window, so gating on the result repr here routes every other shift to the
/// boxed `molt_lshift`/`molt_rshift` runtime (BigInt- and exception-correct),
/// exactly as the LLVM `emit_bitwise` gate and the native backend do.
pub(super) fn emit_lir_i64_binary_or_boxed(
    ctx: &mut LirLowerCtx,
    lhs: ValueId,
    rhs: ValueId,
    dst: ValueId,
    dst_repr: LirRepr,
    bare_i64_instr: Instruction<'static>,
    require_raw_result: bool,
    boxed_runtime_call: LirRuntimeCall,
) {
    let raw_lane_ok = ctx.repr_of(lhs) == LirRepr::I64
        && ctx.repr_of(rhs) == LirRepr::I64
        && (!require_raw_result || dst_repr == LirRepr::I64);
    if raw_lane_ok {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(bare_i64_instr);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
        emit_get_boxed_for_repr(ctx, rhs);
        ctx.emit_runtime_call(boxed_runtime_call);
    }
    ctx.emit_set(dst);
}

pub(super) fn emit_box_inline_i64(ctx: &mut LirLowerCtx, src: ValueId) {
    ctx.emit_get(src);
    ctx.instructions
        .push(Instruction::I64Const(INT_MASK as i64));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions
        .push(Instruction::I64Const(QNAN_TAG_INT_I64));
    ctx.instructions.push(Instruction::I64Or);
}

/// Box a raw-i64 carrier OVERFLOW-SAFELY: fits-inline-47 fast path (the
/// band/bor NaN box) with a cold `int_from_i64` runtime call (heap BigInt)
/// for values outside `[-2^46, 2^46)`.
///
/// This is the wasm twin of native `ensure_boxed_overflow_safe` /
/// `box_raw_i64_value_overflow_safe` and the LLVM
/// `box_i64_overflow_safe_with_builder`. It exists because raw-i64 carriers may
/// be full-i64 `RawI64FullDeopt` checked results; the unchecked
/// [`emit_box_inline_i64`] silently truncates mod 2^47 -- the
/// silent-integer-miscompile class -- and is only sound when the value-range
/// analysis proves the inline window.
pub(super) fn emit_box_i64_overflow_safe(ctx: &mut LirLowerCtx, src: ValueId) {
    // fits = (src + 2^46) <u 2^47
    ctx.emit_get(src);
    ctx.instructions
        .push(Instruction::I64Const(INLINE_INT_BIAS));
    ctx.instructions.push(Instruction::I64Add);
    ctx.instructions
        .push(Instruction::I64Const(INLINE_INT_LIMIT));
    ctx.instructions.push(Instruction::I64LtU);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    ctx.emit_get(src);
    ctx.instructions
        .push(Instruction::I64Const(INT_MASK as i64));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions
        .push(Instruction::I64Const(QNAN_TAG_INT_I64));
    ctx.instructions.push(Instruction::I64Or);
    ctx.instructions.push(Instruction::Else);
    ctx.emit_get(src);
    ctx.emit_runtime_call(LirRuntimeCall::IntFromI64);
    ctx.instructions.push(Instruction::End);
}

pub(super) fn emit_box_none(ctx: &mut LirLowerCtx) {
    ctx.instructions
        .push(Instruction::I64Const(box_none_bits()));
}

pub(super) fn emit_return_boxed_i64(ctx: &mut LirLowerCtx, value: ValueId) {
    match ctx.repr_of(value) {
        // OVERFLOW-SAFE: return-value boxing of a full-range raw carrier
        // (see emit_get_boxed_for_repr).
        LirRepr::I64 => emit_box_i64_overflow_safe(ctx, value),
        LirRepr::DynBox | LirRepr::Ref64 => ctx.emit_get(value),
        LirRepr::Bool1 => {
            ctx.emit_get(value);
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.instructions
                .push(Instruction::I64Const(QNAN_TAG_BOOL_I64));
            ctx.instructions.push(Instruction::I64Or);
        }
        LirRepr::F64 => {
            ctx.emit_get(value);
            let scratch = ctx.alloc_scratch_local(ValType::I64);
            push_f64_to_i64_canonical(|instruction| ctx.instructions.push(instruction), scratch);
        }
    }
}
