use melior::{
    Context as MlirContext,
    dialect::arith,
    ir::{
        Block, BlockLike, Location, Type, Value, ValueLike,
        attribute::{FloatAttribute, IntegerAttribute},
        operation::{OperationBuilder, OperationRef},
    },
};
use molt_backend::tir::{
    function::TirFunction,
    ops::{AttrValue, Dialect as TirDialect, OpCode, TirOp},
};

use super::{
    attrs::{extract_bool_attr, extract_float_attr, extract_int_attr},
    opaque_ops::emit_opaque_molt_op,
    values::{ValueMap, resolve_value},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_tir_op<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op: &TirOp,
    value_map: &mut ValueMap<'c, 'a>,
    _tir_func: &TirFunction,
    i64_type: Type<'c>,
    f64_type: Type<'c>,
    i1_type: Type<'c>,
    location: Location<'c>,
) -> Result<(), String> {
    if op.is_async_work_poll() {
        return Err(
            "MLIR backend cannot lower async_work_poll: canonical pending-call/eval-breaker runtime boundary is unavailable"
                .to_string(),
        );
    }

    match (&op.dialect, &op.opcode) {
        // ---- Constants ----
        (_, OpCode::ConstInt) => {
            let val = extract_int_attr(&op.attrs, "value").unwrap_or(0);
            let attr = IntegerAttribute::new(i64_type, val).into();
            let mlir_op = block.append_operation(arith::constant(ctx, attr, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::ConstFloat) => {
            let val = extract_float_attr(&op.attrs, "value").unwrap_or(0.0);
            let attr = FloatAttribute::new(ctx, f64_type, val).into();
            let mlir_op = block.append_operation(arith::constant(ctx, attr, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::ConstBool) => {
            let val = extract_bool_attr(&op.attrs, "value").unwrap_or(false);
            let attr = IntegerAttribute::new(i1_type, if val { 1 } else { 0 }).into();
            let mlir_op = block.append_operation(arith::constant(ctx, attr, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::ConstNone) => {
            // None is represented as i64 zero.
            let attr = IntegerAttribute::new(i64_type, 0).into();
            let mlir_op = block.append_operation(arith::constant(ctx, attr, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::ConstStr | OpCode::ConstBytes) => {
            // String/bytes constants are opaque i64 handles at this lowering level.
            // A real implementation would emit a global string + pointer.
            let attr = IntegerAttribute::new(i64_type, 0).into();
            let mlir_op = block.append_operation(arith::constant(ctx, attr, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        // ---- Integer / float arithmetic ----
        (_, OpCode::Add | OpCode::InplaceAdd) => {
            emit_binary_arith(ctx, block, op, value_map, location, BinaryArithOp::Add)?;
        }
        (_, OpCode::Sub | OpCode::InplaceSub) => {
            emit_binary_arith(ctx, block, op, value_map, location, BinaryArithOp::Sub)?;
        }
        (_, OpCode::Mul | OpCode::InplaceMul) => {
            emit_binary_arith(ctx, block, op, value_map, location, BinaryArithOp::Mul)?;
        }
        (_, OpCode::Div) => {
            emit_binary_arith(ctx, block, op, value_map, location, BinaryArithOp::Div)?;
        }
        (_, OpCode::FloorDiv) => {
            emit_binary_arith(ctx, block, op, value_map, location, BinaryArithOp::FloorDiv)?;
        }
        (_, OpCode::Mod) => {
            emit_binary_arith(ctx, block, op, value_map, location, BinaryArithOp::Mod)?;
        }
        (_, OpCode::Neg) => {
            let operand = resolve_value(value_map, op.operands[0])?;
            let mlir_op = if value_map.is_float_value(op.operands[0], operand, ctx) {
                block.append_operation(arith::negf(operand, location))
            } else {
                // Integer negate: 0 - x
                let zero_attr = IntegerAttribute::new(i64_type, 0).into();
                let zero_op = block.append_operation(arith::constant(ctx, zero_attr, location));
                let zero_val: Value<'c, '_> = zero_op.result(0).unwrap().into();
                block.append_operation(arith::subi(zero_val, operand, location))
            };
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Pos) => {
            // Unary plus is identity. Copy the MLIR Value directly.
            if !op.operands.is_empty() && !op.results.is_empty() {
                let val = *value_map.get(&op.operands[0]).ok_or_else(|| {
                    format!(
                        "TIR ValueId %{} not found in MLIR value map",
                        op.operands[0].0
                    )
                })?;
                value_map.insert(op.results[0], val);
            }
        }

        // ---- Bitwise ----
        (_, OpCode::BitAnd) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::andi(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::BitOr) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::ori(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::BitXor) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::xori(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::BitNot) => {
            // ~x = x ^ (-1)
            let operand = resolve_value(value_map, op.operands[0])?;
            let neg1_attr = IntegerAttribute::new(i64_type, -1).into();
            let neg1_op = block.append_operation(arith::constant(ctx, neg1_attr, location));
            let neg1_val: Value<'c, '_> = neg1_op.result(0).unwrap().into();
            let mlir_op = block.append_operation(arith::xori(operand, neg1_val, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Shl) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::shli(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Shr) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::shrsi(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        // ---- Comparisons ----
        (_, OpCode::Eq) => {
            emit_comparison(
                ctx,
                block,
                op,
                value_map,
                location,
                arith::CmpiPredicate::Eq,
                arith::CmpfPredicate::Oeq,
            )?;
        }
        (_, OpCode::Ne) => {
            emit_comparison(
                ctx,
                block,
                op,
                value_map,
                location,
                arith::CmpiPredicate::Ne,
                arith::CmpfPredicate::One,
            )?;
        }
        (_, OpCode::Lt) => {
            emit_comparison(
                ctx,
                block,
                op,
                value_map,
                location,
                arith::CmpiPredicate::Slt,
                arith::CmpfPredicate::Olt,
            )?;
        }
        (_, OpCode::Le) => {
            emit_comparison(
                ctx,
                block,
                op,
                value_map,
                location,
                arith::CmpiPredicate::Sle,
                arith::CmpfPredicate::Ole,
            )?;
        }
        (_, OpCode::Gt) => {
            emit_comparison(
                ctx,
                block,
                op,
                value_map,
                location,
                arith::CmpiPredicate::Sgt,
                arith::CmpfPredicate::Ogt,
            )?;
        }
        (_, OpCode::Ge) => {
            emit_comparison(
                ctx,
                block,
                op,
                value_map,
                location,
                arith::CmpiPredicate::Sge,
                arith::CmpfPredicate::Oge,
            )?;
        }

        // ---- Boolean ops ----
        // TIR And/Or are logical operations. Normalize dynamic operands to i1
        // before using the integer bitwise primitives.
        (_, OpCode::And) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let lhs = emit_truthy(ctx, block, lhs, location);
            let rhs = emit_truthy(ctx, block, rhs, location);
            let mlir_op = block.append_operation(arith::andi(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Or) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let lhs = emit_truthy(ctx, block, lhs, location);
            let rhs = emit_truthy(ctx, block, rhs, location);
            let mlir_op = block.append_operation(arith::ori(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Not) => {
            let operand = resolve_value(value_map, op.operands[0])?;
            let operand = emit_truthy(ctx, block, operand, location);
            let one_attr = IntegerAttribute::new(i1_type, 1).into();
            let one_op = block.append_operation(arith::constant(ctx, one_attr, location));
            let one_val: Value<'c, '_> = one_op.result(0).unwrap().into();
            let mlir_op = block.append_operation(arith::xori(operand, one_val, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Bool) => {
            let operand = resolve_value(value_map, op.operands[0])?;
            let truthy = emit_truthy(ctx, block, operand, location);
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, truthy);
            }
        }

        // ---- Copy (SSA forwarding) ----
        (_, OpCode::Copy) => {
            let original_kind = match op.attrs.get("_original_kind") {
                Some(AttrValue::Str(kind)) => Some(kind.as_str()),
                _ => None,
            };
            if matches!(original_kind, Some(kind) if kind != "binding_alias") {
                // The TIR mapper preserves not-yet-enumerated SimpleIR kinds as
                // Copy plus _original_kind. They are semantic operations, not
                // SSA copies, and must retain their operands/results and name.
                emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
            } else {
                if op.operands.len() != 1 || op.results.len() != 1 {
                    return Err(format!(
                        "malformed {:?} copy requires one operand and one result",
                        original_kind.unwrap_or("SSA")
                    ));
                }
                let val = resolve_value(value_map, op.operands[0])?;
                if original_kind == Some("binding_alias") {
                    block.append_operation(
                        OperationBuilder::new("molt.inc_ref", location)
                            .add_operands(&[val])
                            .build()
                            .map_err(|e| format!("Failed to build molt.inc_ref: {e}"))?,
                    );
                }
                value_map.insert(op.results[0], val);
            }
        }

        // ---- Identity / pointer comparison ----
        (_, OpCode::Is) => {
            // `is` checks identity (pointer equality). At i64 level: integer eq.
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::cmpi(
                ctx,
                arith::CmpiPredicate::Eq,
                lhs,
                rhs,
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::IsNot) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::cmpi(
                ctx,
                arith::CmpiPredicate::Ne,
                lhs,
                rhs,
                location,
            ));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        (_, OpCode::Pow) => {
            // Python power is a checked, potentially boxed semantic operation.
            // Preserve it in the Molt dialect until the runtime ABI lowering
            // proves a scalar specialization; never substitute multiplication.
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Call ----
        (_, OpCode::Call | OpCode::CallBuiltin | OpCode::CallMethod) => {
            // Python calls are dynamic until a later Molt dialect pass proves a
            // concrete symbol/signature. A func.call with an invented symbol is
            // invalid MLIR and loses method/call-site metadata.
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Runtime ops (box/unbox/refcount/type_guard) ----
        // These are lowered as opaque custom ops that will be resolved by a
        // later molt-specific dialect pass or by the runtime linker.
        (
            _,
            OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard | OpCode::IncRef | OpCode::DecRef,
        ) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Memory ops (lowered as opaque molt ops) ----
        (
            _,
            OpCode::Alloc
            | OpCode::StackAlloc
            | OpCode::Free
            | OpCode::LoadAttr
            | OpCode::StoreAttr
            | OpCode::DelAttr
            | OpCode::Index
            | OpCode::StoreIndex
            | OpCode::DelIndex,
        ) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Container builders ----
        (
            _,
            OpCode::BuildList
            | OpCode::BuildDict
            | OpCode::BuildTuple
            | OpCode::BuildSet
            | OpCode::BuildSlice,
        ) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Iteration ----
        (_, OpCode::GetIter | OpCode::IterNext | OpCode::IterNextUnboxed | OpCode::ForIter) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Generator / coroutine ----
        (
            _,
            OpCode::AllocTask
            | OpCode::StateSwitch
            | OpCode::StateTransition
            | OpCode::StateYield
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::ClosureLoad
            | OpCode::ClosureStore
            | OpCode::Yield
            | OpCode::YieldFrom,
        ) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Exception handling ----
        (
            _,
            OpCode::Raise
            | OpCode::CheckException
            | OpCode::TryStart
            | OpCode::TryEnd
            | OpCode::StateBlockStart
            | OpCode::StateBlockEnd,
        ) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Import ----
        (_, OpCode::Import | OpCode::ImportFrom) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- In / NotIn ----
        (_, OpCode::In | OpCode::NotIn) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- SCF ops (structured control flow) ----
        (TirDialect::Scf, OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield) => {
            // SCF ops are region-based and require special handling.
            // For now, they are emitted as opaque ops. The TIR is already in
            // CFG form (using blocks + terminators) so these rarely appear.
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Diagnostics ----
        (_, OpCode::WarnStderr) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Catch-all for any dialect/opcode combination not handled above ----
        // This covers cases like SCF ops appearing with non-Scf dialects,
        // or future op additions. Emitted as opaque molt ops.
        _ => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }
    }

    Ok(())
}

fn emit_truthy<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    value: Value<'c, 'a>,
    location: Location<'c>,
) -> Value<'c, 'a> {
    let i1_type: Type<'c> = melior::ir::r#type::IntegerType::new(ctx, 1).into();
    if value.r#type() == i1_type {
        return value;
    }
    let comparison = if value.r#type() == Type::float64(ctx) {
        let zero = block.append_operation(arith::constant(
            ctx,
            FloatAttribute::new(ctx, Type::float64(ctx), 0.0).into(),
            location,
        ));
        arith::cmpf(
            ctx,
            arith::CmpfPredicate::One,
            value,
            zero.result(0).unwrap().into(),
            location,
        )
    } else {
        let i64_type: Type<'c> = melior::ir::r#type::IntegerType::new(ctx, 64).into();
        let zero = block.append_operation(arith::constant(
            ctx,
            IntegerAttribute::new(i64_type, 0).into(),
            location,
        ));
        arith::cmpi(
            ctx,
            arith::CmpiPredicate::Ne,
            value,
            zero.result(0).unwrap().into(),
            location,
        )
    };
    block.append_operation(comparison).result(0).unwrap().into()
}

fn emit_math_unary_float<'c, 'a>(
    _ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op_name: &str,
    operand: Value<'c, '_>,
    location: Location<'c>,
) -> Result<OperationRef<'c, 'a>, String> {
    let result_type = operand.r#type();
    let op = OperationBuilder::new(op_name, location)
        .add_operands(&[operand])
        .add_results(&[result_type])
        .build()
        .map_err(|e| format!("Failed to build {op_name}: {e}"))?;
    Ok(block.append_operation(op))
}

/// Binary arithmetic op kind for dispatch.
enum BinaryArithOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
}

/// Emit a binary arithmetic op, choosing integer or float variants based on operand type.
fn emit_binary_arith<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op: &TirOp,
    value_map: &mut ValueMap<'c, 'a>,
    location: Location<'c>,
    kind: BinaryArithOp,
) -> Result<(), String> {
    let lhs = resolve_value(value_map, op.operands[0])?;
    let rhs = resolve_value(value_map, op.operands[1])?;
    let is_float = value_map.is_float_value(op.operands[0], lhs, ctx);

    let mlir_op = match kind {
        BinaryArithOp::Add => {
            if is_float {
                block.append_operation(arith::addf(lhs, rhs, location))
            } else {
                block.append_operation(arith::addi(lhs, rhs, location))
            }
        }
        BinaryArithOp::Sub => {
            if is_float {
                block.append_operation(arith::subf(lhs, rhs, location))
            } else {
                block.append_operation(arith::subi(lhs, rhs, location))
            }
        }
        BinaryArithOp::Mul => {
            if is_float {
                block.append_operation(arith::mulf(lhs, rhs, location))
            } else {
                block.append_operation(arith::muli(lhs, rhs, location))
            }
        }
        BinaryArithOp::Div => {
            if is_float {
                block.append_operation(arith::divf(lhs, rhs, location))
            } else {
                block.append_operation(arith::divsi(lhs, rhs, location))
            }
        }
        BinaryArithOp::FloorDiv => {
            if is_float {
                let div = block.append_operation(arith::divf(lhs, rhs, location));
                let quotient: Value<'c, '_> = div.result(0).unwrap().into();
                emit_math_unary_float(ctx, block, "math.floor", quotient, location)?
            } else {
                block.append_operation(arith::floordivsi(lhs, rhs, location))
            }
        }
        BinaryArithOp::Mod => {
            if is_float {
                block.append_operation(arith::remf(lhs, rhs, location))
            } else {
                block.append_operation(arith::remsi(lhs, rhs, location))
            }
        }
    };

    if let Some(&result_id) = op.results.first() {
        value_map.insert(result_id, mlir_op.result(0).unwrap().into());
    }
    Ok(())
}

/// Emit a comparison op, choosing integer or float variants based on operand type.
fn emit_comparison<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op: &TirOp,
    value_map: &mut ValueMap<'c, 'a>,
    location: Location<'c>,
    int_pred: arith::CmpiPredicate,
    float_pred: arith::CmpfPredicate,
) -> Result<(), String> {
    let lhs = resolve_value(value_map, op.operands[0])?;
    let rhs = resolve_value(value_map, op.operands[1])?;
    let is_float = value_map.is_float_value(op.operands[0], lhs, ctx);

    let mlir_op = if is_float {
        block.append_operation(arith::cmpf(ctx, float_pred, lhs, rhs, location))
    } else {
        block.append_operation(arith::cmpi(ctx, int_pred, lhs, rhs, location))
    };

    if let Some(&result_id) = op.results.first() {
        value_map.insert(result_id, mlir_op.result(0).unwrap().into());
    }
    Ok(())
}
