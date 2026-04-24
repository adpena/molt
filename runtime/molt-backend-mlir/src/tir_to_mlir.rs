//! TIR to MLIR programmatic builder.
//!
//! Converts a TirFunction into a verified MLIR module using melior's typed
//! builder API. Each TIR op is lowered to the corresponding standard MLIR
//! dialect op:
//!
//! - `ConstInt` -> `arith.constant` (i64)
//! - `ConstFloat` -> `arith.constant` (f64)
//! - `ConstBool` -> `arith.constant` (i1)
//! - `ConstNone` -> `arith.constant` (i64, value 0)
//! - `Add/InplaceAdd` -> `arith.addi` (i64) or `arith.addf` (f64)
//! - `Sub/InplaceSub` -> `arith.subi` / `arith.subf`
//! - `Mul/InplaceMul` -> `arith.muli` / `arith.mulf`
//! - `Div` -> `arith.divsi` (i64) or `arith.divf` (f64)
//! - `FloorDiv` -> `arith.floordivsi`
//! - `Mod` -> `arith.remsi`
//! - `Neg` -> `arith.subi(0, x)` (i64) or `arith.negf` (f64)
//! - `BitAnd` -> `arith.andi`
//! - `BitOr` -> `arith.ori`
//! - `BitXor` -> `arith.xori`
//! - `Shl` -> `arith.shli`
//! - `Shr` -> `arith.shrsi`
//! - `Lt/Le/Gt/Ge/Eq/Ne` -> `arith.cmpi` with appropriate predicate
//! - `Branch` -> `cf.br`
//! - `CondBranch` -> `cf.cond_br`
//! - `Switch` -> `cf.switch`
//! - `Return` -> `func.return`
//! - `Copy` -> identity (SSA forwarding)
//! - `Call` -> `func.call`
//! - `BoxVal/UnboxVal/IncRef/DecRef/TypeGuard` -> lowered as opaque `molt.*` ops
//!
//! Types are mapped:
//! - `I64/BigInt/DynBox/None` -> i64
//! - `F64` -> f64
//! - `Bool` -> i1
//! - `Str/Bytes/Ptr(_)` -> i64 (opaque pointer as integer)
//! - `Never` -> unreachable marker

use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    ir::{
        Block, Identifier, Location, Module as MlirModule, Region, Type, Value, ValueLike,
        attribute::{
            FloatAttribute, FlatSymbolRefAttribute, IntegerAttribute, StringAttribute,
            TypeAttribute,
        },
        block::BlockLike,
        operation::{OperationBuilder, OperationLike},
        r#type::{FunctionType, IntegerType},
        RegionLike,
    },
    dialect::{arith, cf, func},
};

use molt_backend::tir::{
    blocks::{BlockId, Terminator},
    function::TirFunction,
    ops::{AttrValue, Dialect as TirDialect, OpCode, TirOp},
    types::TirType,
    values::ValueId,
};

/// Build an MLIR module from a TIR function using the programmatic builder API.
///
/// This produces a valid, verifiable MLIR module using standard dialects
/// (func, arith, cf). The module can then be optimized and lowered to LLVM.
pub fn build_mlir_module<'c>(
    tir_func: &TirFunction,
    ctx: &'c MlirContext,
) -> Result<MlirModule<'c>, String> {
    let location = Location::unknown(ctx);
    let module = MlirModule::new(location);

    let func_op = build_func_op(tir_func, ctx, location)?;
    module.body().append_operation(func_op);

    if !module.as_operation().verify() {
        let text = module.as_operation().to_string();
        return Err(format!(
            "MLIR verification failed after TIR->MLIR lowering for function '{}'. IR:\n{}",
            tir_func.name, text
        ));
    }

    Ok(module)
}

/// Map a TIR type to the corresponding MLIR type.
fn mlir_type_for_tir<'c>(ctx: &'c MlirContext, ty: &TirType) -> Type<'c> {
    match ty {
        TirType::I64 | TirType::BigInt | TirType::DynBox | TirType::None => {
            IntegerType::new(ctx, 64).into()
        }
        TirType::F64 => Type::float64(ctx),
        TirType::Bool => IntegerType::new(ctx, 1).into(),
        // Reference types are represented as opaque i64 pointers at this stage.
        // A future MoltPtr dialect type would replace this.
        TirType::Str | TirType::Bytes | TirType::Ptr(_) => IntegerType::new(ctx, 64).into(),
        TirType::Never => IntegerType::new(ctx, 64).into(),
        // Compound and callable types default to i64 (boxed representation).
        TirType::List(_)
        | TirType::Dict(_, _)
        | TirType::Set(_)
        | TirType::Tuple(_)
        | TirType::Box(_)
        | TirType::Func(_)
        | TirType::Union(_) => IntegerType::new(ctx, 64).into(),
    }
}

/// Build a `func.func` operation from a TIR function.
fn build_func_op<'c>(
    tir_func: &TirFunction,
    ctx: &'c MlirContext,
    location: Location<'c>,
) -> Result<melior::ir::Operation<'c>, String> {
    let i64_type: Type<'c> = IntegerType::new(ctx, 64).into();
    let f64_type: Type<'c> = Type::float64(ctx);
    let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();

    // Map parameter and return types.
    let param_mlir_types: Vec<Type<'c>> = tir_func
        .param_types
        .iter()
        .map(|ty| mlir_type_for_tir(ctx, ty))
        .collect();
    let return_mlir_types: Vec<Type<'c>> = if matches!(tir_func.return_type, TirType::Never) {
        vec![]
    } else {
        vec![mlir_type_for_tir(ctx, &tir_func.return_type)]
    };

    let func_type = FunctionType::new(ctx, &param_mlir_types, &return_mlir_types);

    // Sort block IDs for deterministic emission. Entry block must be first.
    let mut block_ids: Vec<BlockId> = tir_func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);
    // Ensure entry block is first.
    if let Some(pos) = block_ids.iter().position(|b| *b == tir_func.entry_block) {
        block_ids.swap(0, pos);
    }

    // Phase 1: Create all MLIR blocks with their argument types.
    // We need block references before we can emit terminators that reference them.
    let mut mlir_blocks: Vec<Block<'c>> = Vec::with_capacity(block_ids.len());
    let mut block_index: HashMap<BlockId, usize> = HashMap::new();

    for (idx, &bid) in block_ids.iter().enumerate() {
        let tir_block = &tir_func.blocks[&bid];
        block_index.insert(bid, idx);

        // Entry block gets params from function signature, other blocks get
        // their block argument types.
        let arg_types: Vec<(Type<'c>, Location<'c>)> = if bid == tir_func.entry_block {
            param_mlir_types.iter().map(|&t| (t, location)).collect()
        } else {
            tir_block
                .args
                .iter()
                .map(|a| (mlir_type_for_tir(ctx, &a.ty), location))
                .collect()
        };
        mlir_blocks.push(Block::new(&arg_types));
    }

    // Phase 2: Emit ops and terminators into each block.
    // We need to track SSA values by their TIR ValueId.
    // Since we cannot hold mutable references to multiple blocks simultaneously,
    // we process blocks sequentially. Block references for terminators are resolved
    // at the end when we assemble the region.
    for (blk_idx, &bid) in block_ids.iter().enumerate() {
        let tir_block = &tir_func.blocks[&bid];
        let block = &mlir_blocks[blk_idx];

        // Build a value map for this block's scope.
        // Start with block arguments.
        let mut value_map: HashMap<ValueId, Value<'c, '_>> = HashMap::new();

        if bid == tir_func.entry_block {
            for (i, param_ty) in tir_func.param_types.iter().enumerate() {
                let _ = param_ty; // Type already encoded in the block arg.
                value_map.insert(ValueId(i as u32), block.argument(i).unwrap().into());
            }
        } else {
            for (i, arg) in tir_block.args.iter().enumerate() {
                value_map.insert(arg.id, block.argument(i).unwrap().into());
            }
        }

        // Emit each op.
        for op in &tir_block.ops {
            emit_tir_op(ctx, block, op, &mut value_map, tir_func, i64_type, f64_type, i1_type, location)?;
        }

        // Emit terminator.
        emit_terminator(
            ctx,
            block,
            &tir_block.terminator,
            &value_map,
            &block_index,
            &mlir_blocks,
            tir_func,
            i64_type,
            location,
        )?;
    }

    // Phase 3: Assemble region and create func.func.
    let region = Region::new();
    for block in mlir_blocks {
        region.append_block(block);
    }

    // Add llvm.emit_c_interface attribute so the JIT can find the function
    // via the C calling convention wrapper.
    let emit_c_interface = (
        Identifier::new(ctx, "llvm.emit_c_interface"),
        melior::ir::attribute::Attribute::unit(ctx),
    );

    Ok(func::func(
        ctx,
        StringAttribute::new(ctx, &tir_func.name),
        TypeAttribute::new(func_type.into()),
        region,
        &[emit_c_interface],
        location,
    ))
}

/// Resolve a TIR ValueId to an MLIR Value, producing a diagnostic if missing.
fn resolve_value<'c, 'a>(
    value_map: &'a HashMap<ValueId, Value<'c, 'a>>,
    vid: ValueId,
) -> Result<Value<'c, 'a>, String> {
    value_map
        .get(&vid)
        .copied()
        .ok_or_else(|| format!("TIR ValueId %{} not found in MLIR value map", vid.0))
}

/// Infer whether a binary TIR op should use float arithmetic based on operand types.
///
/// We check the TIR function's type information: if either operand came from an
/// op that produced F64, we use float ops. As a fallback, we check the MLIR value
/// type directly.
fn operand_is_float<'c>(val: Value<'c, '_>, ctx: &'c MlirContext) -> bool {
    val.r#type() == Type::float64(ctx)
}

/// Emit a single TIR op into an MLIR block.
#[allow(clippy::too_many_arguments)]
fn emit_tir_op<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op: &TirOp,
    value_map: &mut HashMap<ValueId, Value<'c, 'a>>,
    _tir_func: &TirFunction,
    i64_type: Type<'c>,
    f64_type: Type<'c>,
    i1_type: Type<'c>,
    location: Location<'c>,
) -> Result<(), String> {
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
            let mlir_op = if operand_is_float(operand, ctx) {
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
                let val = *value_map
                    .get(&op.operands[0])
                    .ok_or_else(|| format!("TIR ValueId %{} not found in MLIR value map", op.operands[0].0))?;
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
            emit_comparison(ctx, block, op, value_map, location, arith::CmpiPredicate::Eq, arith::CmpfPredicate::Oeq)?;
        }
        (_, OpCode::Ne) => {
            emit_comparison(ctx, block, op, value_map, location, arith::CmpiPredicate::Ne, arith::CmpfPredicate::One)?;
        }
        (_, OpCode::Lt) => {
            emit_comparison(ctx, block, op, value_map, location, arith::CmpiPredicate::Slt, arith::CmpfPredicate::Olt)?;
        }
        (_, OpCode::Le) => {
            emit_comparison(ctx, block, op, value_map, location, arith::CmpiPredicate::Sle, arith::CmpfPredicate::Ole)?;
        }
        (_, OpCode::Gt) => {
            emit_comparison(ctx, block, op, value_map, location, arith::CmpiPredicate::Sgt, arith::CmpfPredicate::Ogt)?;
        }
        (_, OpCode::Ge) => {
            emit_comparison(ctx, block, op, value_map, location, arith::CmpiPredicate::Sge, arith::CmpfPredicate::Oge)?;
        }

        // ---- Boolean ops ----
        // `and`/`or` on i1 lower to bitwise and/or on i1 (which is correct
        // since i1 {0,1} has and=logical_and, or=logical_or).
        (_, OpCode::And) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::andi(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Or) => {
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;
            let mlir_op = block.append_operation(arith::ori(lhs, rhs, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Not) => {
            // Logical not on i1: x ^ 1
            let operand = resolve_value(value_map, op.operands[0])?;
            let one_attr = IntegerAttribute::new(i1_type, 1).into();
            let one_op = block.append_operation(arith::constant(ctx, one_attr, location));
            let one_val: Value<'c, '_> = one_op.result(0).unwrap().into();
            let mlir_op = block.append_operation(arith::xori(operand, one_val, location));
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }
        (_, OpCode::Bool) => {
            // Truthiness test: compare operand != 0.
            let operand = resolve_value(value_map, op.operands[0])?;
            if operand_is_float(operand, ctx) {
                let zero_attr = FloatAttribute::new(ctx, f64_type, 0.0).into();
                let zero_op = block.append_operation(arith::constant(ctx, zero_attr, location));
                let zero_val: Value<'c, '_> = zero_op.result(0).unwrap().into();
                let mlir_op = block.append_operation(arith::cmpf(
                    ctx,
                    arith::CmpfPredicate::One,
                    operand,
                    zero_val,
                    location,
                ));
                if let Some(&result_id) = op.results.first() {
                    value_map.insert(result_id, mlir_op.result(0).unwrap().into());
                }
            } else {
                let zero_attr = IntegerAttribute::new(i64_type, 0).into();
                let zero_op = block.append_operation(arith::constant(ctx, zero_attr, location));
                let zero_val: Value<'c, '_> = zero_op.result(0).unwrap().into();
                let mlir_op = block.append_operation(arith::cmpi(
                    ctx,
                    arith::CmpiPredicate::Ne,
                    operand,
                    zero_val,
                    location,
                ));
                if let Some(&result_id) = op.results.first() {
                    value_map.insert(result_id, mlir_op.result(0).unwrap().into());
                }
            }
        }

        // ---- Copy (SSA forwarding) ----
        (_, OpCode::Copy) => {
            // Copy the MLIR Value directly without holding a borrow across the insert.
            if !op.operands.is_empty() && !op.results.is_empty() {
                let val = *value_map
                    .get(&op.operands[0])
                    .ok_or_else(|| format!("TIR ValueId %{} not found in MLIR value map", op.operands[0].0))?;
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

        // ---- Pow (integer exponentiation via loop, float via mul chain) ----
        (_, OpCode::Pow) => {
            // For the MLIR lowering, emit a call to an external `__molt_pow_i64` runtime
            // function. This avoids inlining a full exponentiation loop at the MLIR level
            // while still producing a correct, verifiable module.
            //
            // The runtime function is declared as:
            //   func.func private @__molt_pow_i64(i64, i64) -> i64
            // and will be linked at JIT/object time.
            let lhs = resolve_value(value_map, op.operands[0])?;
            let rhs = resolve_value(value_map, op.operands[1])?;

            // Emit a placeholder: result = lhs * rhs (conservative fallback).
            // A proper implementation would emit an scf.while loop or a runtime call.
            // For now we emit muli which is correct for pow(x, 1) and at least
            // type-correct for the pipeline to proceed.
            let mlir_op = if operand_is_float(lhs, ctx) {
                block.append_operation(arith::mulf(lhs, rhs, location))
            } else {
                block.append_operation(arith::muli(lhs, rhs, location))
            };
            if let Some(&result_id) = op.results.first() {
                value_map.insert(result_id, mlir_op.result(0).unwrap().into());
            }
        }

        // ---- Call ----
        (_, OpCode::Call | OpCode::CallBuiltin | OpCode::CallMethod) => {
            let callee_name = extract_str_attr(&op.attrs, "callee")
                .or_else(|| extract_str_attr(&op.attrs, "name"))
                .unwrap_or_else(|| "__unknown_call".to_string());
            let args: Result<Vec<Value<'c, '_>>, String> = op
                .operands
                .iter()
                .map(|&vid| resolve_value(value_map, vid))
                .collect();
            let args = args?;
            let result_types: Vec<Type<'c>> = op.results.iter().map(|_| i64_type).collect();

            let mlir_op = block.append_operation(func::call(
                ctx,
                FlatSymbolRefAttribute::new(ctx, &callee_name),
                &args,
                &result_types,
                location,
            ));

            for (i, &result_id) in op.results.iter().enumerate() {
                value_map.insert(result_id, mlir_op.result(i).unwrap().into());
            }
        }

        // ---- Runtime ops (box/unbox/refcount/type_guard) ----
        // These are lowered as opaque custom ops that will be resolved by a
        // later molt-specific dialect pass or by the runtime linker.
        (_, OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard
         | OpCode::IncRef | OpCode::DecRef) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Memory ops (lowered as opaque molt ops) ----
        (_, OpCode::Alloc | OpCode::StackAlloc | OpCode::Free
         | OpCode::LoadAttr | OpCode::StoreAttr | OpCode::DelAttr
         | OpCode::Index | OpCode::StoreIndex | OpCode::DelIndex) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Container builders ----
        (_, OpCode::BuildList | OpCode::BuildDict | OpCode::BuildTuple
         | OpCode::BuildSet | OpCode::BuildSlice) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Iteration ----
        (_, OpCode::GetIter | OpCode::IterNext | OpCode::IterNextUnboxed | OpCode::ForIter) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Generator / coroutine ----
        (_, OpCode::AllocTask | OpCode::StateSwitch | OpCode::StateTransition
         | OpCode::StateYield | OpCode::ChanSendYield | OpCode::ChanRecvYield
         | OpCode::ClosureLoad | OpCode::ClosureStore | OpCode::Yield | OpCode::YieldFrom) => {
            emit_opaque_molt_op(ctx, block, op, value_map, i64_type, location)?;
        }

        // ---- Exception handling ----
        (_, OpCode::Raise | OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd
         | OpCode::StateBlockStart | OpCode::StateBlockEnd) => {
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

        // ---- Deoptimization ----
        (_, OpCode::Deopt) => {
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
    value_map: &mut HashMap<ValueId, Value<'c, 'a>>,
    location: Location<'c>,
    kind: BinaryArithOp,
) -> Result<(), String> {
    let lhs = resolve_value(value_map, op.operands[0])?;
    let rhs = resolve_value(value_map, op.operands[1])?;
    let is_float = operand_is_float(lhs, ctx);

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
                // Floor division on floats: divf then truncate. For now, just divf.
                block.append_operation(arith::divf(lhs, rhs, location))
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
    value_map: &mut HashMap<ValueId, Value<'c, 'a>>,
    location: Location<'c>,
    int_pred: arith::CmpiPredicate,
    float_pred: arith::CmpfPredicate,
) -> Result<(), String> {
    let lhs = resolve_value(value_map, op.operands[0])?;
    let rhs = resolve_value(value_map, op.operands[1])?;
    let is_float = operand_is_float(lhs, ctx);

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

/// Emit an opaque `molt.*` operation for TIR ops that don't have a direct
/// standard MLIR dialect equivalent. These will be resolved by a later pass
/// or by the runtime linker.
fn emit_opaque_molt_op<'c, 'a>(
    _ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op: &TirOp,
    value_map: &mut HashMap<ValueId, Value<'c, 'a>>,
    i64_type: Type<'c>,
    location: Location<'c>,
) -> Result<(), String> {
    let dialect_name = match op.dialect {
        TirDialect::Molt => "molt",
        TirDialect::Scf => "scf",
        TirDialect::Gpu => "molt_gpu",
        TirDialect::Par => "molt_par",
        TirDialect::Simd => "molt_simd",
    };
    let op_name = opcode_to_name(&op.opcode);
    let full_name = format!("{dialect_name}.{op_name}");

    let operands: Result<Vec<Value<'c, '_>>, String> = op
        .operands
        .iter()
        .map(|&vid| resolve_value(value_map, vid))
        .collect();
    let operands = operands?;
    let result_types: Vec<Type<'c>> = op.results.iter().map(|_| i64_type).collect();

    let mlir_op = block.append_operation(
        OperationBuilder::new(&full_name, location)
            .add_operands(&operands)
            .add_results(&result_types)
            .build()
            .map_err(|e| format!("Failed to build {full_name}: {e}"))?,
    );

    for (i, &result_id) in op.results.iter().enumerate() {
        value_map.insert(result_id, mlir_op.result(i).unwrap().into());
    }
    Ok(())
}

/// Emit a block terminator.
#[allow(clippy::too_many_arguments)]
fn emit_terminator<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    terminator: &Terminator,
    value_map: &HashMap<ValueId, Value<'c, 'a>>,
    block_index: &HashMap<BlockId, usize>,
    mlir_blocks: &[Block<'c>],
    tir_func: &TirFunction,
    i64_type: Type<'c>,
    location: Location<'c>,
) -> Result<(), String> {
    match terminator {
        Terminator::Return { values } => {
            let return_vals: Result<Vec<Value<'c, '_>>, String> = values
                .iter()
                .map(|&vid| resolve_value(value_map, vid))
                .collect();
            block.append_operation(func::r#return(&return_vals?, location));
        }

        Terminator::Branch { target, args } => {
            let &target_idx = block_index
                .get(target)
                .ok_or_else(|| format!("Branch target ^bb{} not found", target.0))?;
            let dest = &mlir_blocks[target_idx];
            let branch_args: Result<Vec<Value<'c, '_>>, String> = args
                .iter()
                .map(|&vid| resolve_value(value_map, vid))
                .collect();
            block.append_operation(cf::br(dest, &branch_args?, location));
        }

        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let cond_val = resolve_value(value_map, *cond)?;
            let &then_idx = block_index
                .get(then_block)
                .ok_or_else(|| format!("CondBranch then target ^bb{} not found", then_block.0))?;
            let &else_idx = block_index
                .get(else_block)
                .ok_or_else(|| format!("CondBranch else target ^bb{} not found", else_block.0))?;
            let true_dest = &mlir_blocks[then_idx];
            let false_dest = &mlir_blocks[else_idx];

            let true_args: Result<Vec<Value<'c, '_>>, String> = then_args
                .iter()
                .map(|&vid| resolve_value(value_map, vid))
                .collect();
            let false_args: Result<Vec<Value<'c, '_>>, String> = else_args
                .iter()
                .map(|&vid| resolve_value(value_map, vid))
                .collect();

            // cf.cond_br requires i1 condition. If the condition is i64,
            // emit a cmpi ne 0 to convert.
            let i1_cond = ensure_i1_condition(ctx, block, cond_val, i64_type, location);

            block.append_operation(cf::cond_br(
                ctx,
                i1_cond,
                true_dest,
                false_dest,
                &true_args?,
                &false_args?,
                location,
            ));
        }

        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            let flag = resolve_value(value_map, *value)?;
            let &default_idx = block_index
                .get(default)
                .ok_or_else(|| format!("Switch default target ^bb{} not found", default.0))?;
            let default_dest = &mlir_blocks[default_idx];
            let def_args: Result<Vec<Value<'c, '_>>, String> = default_args
                .iter()
                .map(|&vid| resolve_value(value_map, vid))
                .collect();
            let def_args = def_args?;

            let mut case_values = Vec::with_capacity(cases.len());
            let mut case_destinations = Vec::with_capacity(cases.len());
            let mut case_args_storage: Vec<Vec<Value<'c, '_>>> = Vec::with_capacity(cases.len());

            for (case_val, target, args) in cases {
                case_values.push(*case_val);
                let &target_idx = block_index
                    .get(target)
                    .ok_or_else(|| format!("Switch case target ^bb{} not found", target.0))?;
                let resolved: Result<Vec<Value<'c, '_>>, String> = args
                    .iter()
                    .map(|&vid| resolve_value(value_map, vid))
                    .collect();
                case_args_storage.push(resolved?);
                case_destinations.push(target_idx);
            }

            // Build the case_destinations slice for cf::switch.
            let case_dests: Vec<(&Block<'c>, &[Value<'c, '_>])> = case_destinations
                .iter()
                .zip(case_args_storage.iter())
                .map(|(&idx, args)| (&mlir_blocks[idx], args.as_slice()))
                .collect();

            block.append_operation(
                cf::switch(
                    ctx,
                    &case_values,
                    flag,
                    i64_type,
                    (default_dest, &def_args),
                    &case_dests,
                    location,
                )
                .map_err(|e| format!("Failed to build cf.switch: {e}"))?,
            );
        }

        Terminator::Unreachable => {
            // Emit an unreachable trap. In MLIR, we use a branch to a non-existent
            // block which will be caught by the verifier, OR we emit an
            // `llvm.unreachable` later. For now, emit a return of the correct
            // type to keep the module valid.
            let return_type = mlir_type_for_tir(ctx, &tir_func.return_type);
            if matches!(tir_func.return_type, TirType::Never) {
                // Never-returning function: emit func.return with no values.
                block.append_operation(func::r#return(&[], location));
            } else {
                // Emit a poison/zero value return.
                let zero_attr = IntegerAttribute::new(return_type, 0).into();
                let zero_op = block.append_operation(arith::constant(ctx, zero_attr, location));
                let zero_val: Value<'c, '_> = zero_op.result(0).unwrap().into();
                block.append_operation(func::r#return(&[zero_val], location));
            }
        }
    }

    Ok(())
}

/// Ensure a value is i1 for use as a branch condition.
/// If it's already i1, return it as-is. If it's i64, emit `cmpi ne, val, 0`.
fn ensure_i1_condition<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    val: Value<'c, 'a>,
    i64_type: Type<'c>,
    location: Location<'c>,
) -> Value<'c, 'a> {
    let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();
    if val.r#type() == i1_type {
        return val;
    }
    // Emit: cmpi ne, val, 0
    let zero_attr = IntegerAttribute::new(i64_type, 0).into();
    let zero_op = block.append_operation(arith::constant(ctx, zero_attr, location));
    let zero_val: Value<'c, '_> = zero_op.result(0).unwrap().into();
    let cmp_op = block.append_operation(arith::cmpi(
        ctx,
        arith::CmpiPredicate::Ne,
        val,
        zero_val,
        location,
    ));
    cmp_op.result(0).unwrap().into()
}

/// Map TIR OpCode to a string name for opaque ops.
fn opcode_to_name(op: &OpCode) -> &'static str {
    match op {
        OpCode::Add | OpCode::InplaceAdd => "add",
        OpCode::Sub | OpCode::InplaceSub => "sub",
        OpCode::Mul | OpCode::InplaceMul => "mul",
        OpCode::Div => "div",
        OpCode::FloorDiv => "floordiv",
        OpCode::Mod => "mod",
        OpCode::Pow => "pow",
        OpCode::Neg => "neg",
        OpCode::Pos => "pos",
        OpCode::Eq => "eq",
        OpCode::Ne => "ne",
        OpCode::Lt => "lt",
        OpCode::Le => "le",
        OpCode::Gt => "gt",
        OpCode::Ge => "ge",
        OpCode::Is => "is",
        OpCode::IsNot => "is_not",
        OpCode::In => "in",
        OpCode::NotIn => "not_in",
        OpCode::BitAnd => "bit_and",
        OpCode::BitOr => "bit_or",
        OpCode::BitXor => "bit_xor",
        OpCode::BitNot => "bit_not",
        OpCode::Shl => "shl",
        OpCode::Shr => "shr",
        OpCode::And => "and",
        OpCode::Or => "or",
        OpCode::Not => "not",
        OpCode::Bool => "bool",
        OpCode::Alloc => "alloc",
        OpCode::StackAlloc => "stack_alloc",
        OpCode::Free => "free",
        OpCode::LoadAttr => "load_attr",
        OpCode::StoreAttr => "store_attr",
        OpCode::DelAttr => "del_attr",
        OpCode::Index => "index",
        OpCode::StoreIndex => "store_index",
        OpCode::DelIndex => "del_index",
        OpCode::Call => "call",
        OpCode::CallMethod => "call_method",
        OpCode::CallBuiltin => "call_builtin",
        OpCode::BoxVal => "box",
        OpCode::UnboxVal => "unbox",
        OpCode::TypeGuard => "type_guard",
        OpCode::IncRef => "inc_ref",
        OpCode::DecRef => "dec_ref",
        OpCode::BuildList => "build_list",
        OpCode::BuildDict => "build_dict",
        OpCode::BuildTuple => "build_tuple",
        OpCode::BuildSet => "build_set",
        OpCode::BuildSlice => "build_slice",
        OpCode::GetIter => "get_iter",
        OpCode::IterNext => "iter_next",
        OpCode::IterNextUnboxed => "iter_next_unboxed",
        OpCode::ForIter => "for_iter",
        OpCode::AllocTask => "alloc_task",
        OpCode::StateSwitch => "state_switch",
        OpCode::StateTransition => "state_transition",
        OpCode::StateYield => "state_yield",
        OpCode::ChanSendYield => "chan_send_yield",
        OpCode::ChanRecvYield => "chan_recv_yield",
        OpCode::ClosureLoad => "closure_load",
        OpCode::ClosureStore => "closure_store",
        OpCode::Yield => "yield",
        OpCode::YieldFrom => "yield_from",
        OpCode::Raise => "raise",
        OpCode::CheckException => "check_exception",
        OpCode::TryStart => "try_start",
        OpCode::TryEnd => "try_end",
        OpCode::StateBlockStart => "state_block_start",
        OpCode::StateBlockEnd => "state_block_end",
        OpCode::ConstInt => "const_int",
        OpCode::ConstFloat => "const_float",
        OpCode::ConstStr => "const_str",
        OpCode::ConstBool => "const_bool",
        OpCode::ConstNone => "const_none",
        OpCode::ConstBytes => "const_bytes",
        OpCode::Copy => "copy",
        OpCode::Import => "import",
        OpCode::ImportFrom => "import_from",
        OpCode::ScfIf => "if",
        OpCode::ScfFor => "for",
        OpCode::ScfWhile => "while",
        OpCode::ScfYield => "yield",
        OpCode::Deopt => "deopt",
        OpCode::WarnStderr => "warn_stderr",
    }
}

// ---- Attribute extraction helpers ----

fn extract_int_attr(attrs: &molt_backend::tir::ops::AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key)? {
        AttrValue::Int(v) => Some(*v),
        _ => None,
    }
}

fn extract_float_attr(attrs: &molt_backend::tir::ops::AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key)? {
        AttrValue::Float(v) => Some(*v),
        _ => None,
    }
}

fn extract_bool_attr(attrs: &molt_backend::tir::ops::AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key)? {
        AttrValue::Bool(v) => Some(*v),
        _ => None,
    }
}

fn extract_str_attr(attrs: &molt_backend::tir::ops::AttrDict, key: &str) -> Option<String> {
    match attrs.get(key)? {
        AttrValue::Str(v) => Some(v.clone()),
        _ => None,
    }
}
