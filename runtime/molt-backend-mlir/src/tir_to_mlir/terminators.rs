use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    dialect::{arith, cf, func},
    ir::{
        Block, BlockLike, Location, Type, Value, ValueLike,
        attribute::{FlatSymbolRefAttribute, FloatAttribute, IntegerAttribute},
        operation::OperationBuilder,
        r#type::IntegerType,
    },
};
use molt_backend::tir::{
    blocks::{BlockId, Terminator},
    function::TirFunction,
    types::TirType,
};

use super::{
    types::mlir_type_for_tir,
    values::{ValueMap, resolve_value},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_terminator<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    terminator: &Terminator,
    value_map: &ValueMap<'c, 'a>,
    block_index: &HashMap<BlockId, usize>,
    mlir_blocks: &[Block<'c>],
    tir_func: &TirFunction,
    i64_type: Type<'c>,
    location: Location<'c>,
) -> Result<(), String> {
    match terminator {
        Terminator::Return { values } => {
            if values.len() > 1 {
                return Err(format!(
                    "MLIR backend supports one Python return value, found {} in '{}'",
                    values.len(),
                    tir_func.name
                ));
            }
            let return_values = if let Some(&value_id) = values.first() {
                let value = resolve_value(value_map, value_id)?;
                vec![coerce_value_to_tir_type(
                    ctx,
                    block,
                    value,
                    &tir_func.return_type,
                    location,
                )?]
            } else if matches!(tir_func.return_type, TirType::Never) {
                vec![]
            } else {
                vec![zero_value_for_return_type(
                    ctx,
                    block,
                    &tir_func.return_type,
                    location,
                )]
            };
            block.append_operation(func::r#return(&return_values, location));
        }

        Terminator::Branch { target, args } => {
            let &target_idx = block_index
                .get(target)
                .ok_or_else(|| format!("Branch target ^bb{} not found", target.0))?;
            let dest = &mlir_blocks[target_idx];
            let branch_args =
                resolve_edge_args(ctx, block, *target, args, value_map, tir_func, location)?;
            block.append_operation(cf::br(dest, &branch_args, location));
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

            let true_args = resolve_edge_args(
                ctx,
                block,
                *then_block,
                then_args,
                value_map,
                tir_func,
                location,
            )?;
            let false_args = resolve_edge_args(
                ctx,
                block,
                *else_block,
                else_args,
                value_map,
                tir_func,
                location,
            )?;

            // cf.cond_br requires i1 condition. If the condition is i64,
            // emit a cmpi ne 0 to convert.
            let i1_cond = ensure_i1_condition(ctx, block, cond_val, i64_type, location);

            block.append_operation(cf::cond_br(
                ctx,
                i1_cond,
                true_dest,
                false_dest,
                &true_args,
                &false_args,
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
            emit_switch(
                ctx,
                block,
                flag,
                cases,
                *default,
                default_args,
                value_map,
                block_index,
                mlir_blocks,
                tir_func,
                i64_type,
                location,
                "Switch",
            )?;
        }

        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => {
            let self_index = tir_func
                .param_names
                .iter()
                .position(|name| name == "self")
                .unwrap_or(0);
            let self_value = resolve_value(value_map, molt_backend::tir::values::ValueId(self_index as u32))
                .map_err(|_| {
                    format!(
                        "StateDispatch in '{}' requires the generator frame parameter ('self' or parameter 0)",
                        tir_func.name
                    )
                })?;
            let state_call = block.append_operation(func::call(
                ctx,
                FlatSymbolRefAttribute::new(ctx, "molt_obj_get_state"),
                &[self_value],
                &[i64_type],
                location,
            ));
            let state = state_call
                .result(0)
                .map_err(|error| format!("molt_obj_get_state returned no state value: {error}"))?
                .into();
            emit_switch(
                ctx,
                block,
                state,
                cases,
                *default,
                default_args,
                value_map,
                block_index,
                mlir_blocks,
                tir_func,
                i64_type,
                location,
                "StateDispatch",
            )?;
        }

        Terminator::Unreachable => {
            append_unreachable_assert(ctx, block, location);
            if matches!(tir_func.return_type, TirType::Never) {
                block.append_operation(func::r#return(&[], location));
            } else {
                let zero_val =
                    zero_value_for_return_type(ctx, block, &tir_func.return_type, location);
                block.append_operation(func::r#return(&[zero_val], location));
            }
        }
    }

    Ok(())
}

fn coerce_value_to_tir_type<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    value: Value<'c, 'a>,
    expected_type: &TirType,
    location: Location<'c>,
) -> Result<Value<'c, 'a>, String> {
    let expected = mlir_type_for_tir(ctx, expected_type);
    if value.r#type() == expected {
        return Ok(value);
    }

    let operation = match expected_type {
        TirType::Bool => {
            return Ok(ensure_i1_condition(
                ctx,
                block,
                value,
                IntegerType::new(ctx, 64).into(),
                location,
            ));
        }
        TirType::I64 => {
            let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();
            if value.r#type() == i1_type {
                arith::extui(value, expected, location)
            } else if value.r#type() == Type::float64(ctx) {
                arith::bitcast(value, expected, location)
            } else {
                return Err(format!(
                    "cannot coerce MLIR return type {} to i64",
                    value.r#type()
                ));
            }
        }
        TirType::F64 => arith::bitcast(value, expected, location),
        _ => OperationBuilder::new("molt.box", location)
            .add_operands(&[value])
            .add_results(&[expected])
            .build()
            .map_err(|error| format!("failed to box MLIR return value: {error}"))?,
    };
    Ok(block.append_operation(operation).result(0).unwrap().into())
}

fn resolve_edge_args<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    target: BlockId,
    args: &[molt_backend::tir::values::ValueId],
    value_map: &ValueMap<'c, 'a>,
    tir_func: &TirFunction,
    location: Location<'c>,
) -> Result<Vec<Value<'c, 'a>>, String> {
    let target_block = tir_func
        .blocks
        .get(&target)
        .ok_or_else(|| format!("edge target ^bb{} not found", target.0))?;
    if args.len() != target_block.args.len() {
        return Err(format!(
            "edge to ^bb{} passes {} values for {} block arguments",
            target.0,
            args.len(),
            target_block.args.len()
        ));
    }
    args.iter()
        .zip(&target_block.args)
        .map(|(&value_id, target_arg)| {
            coerce_value_to_tir_type(
                ctx,
                block,
                resolve_value(value_map, value_id)?,
                &target_arg.ty,
                location,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn emit_switch<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    flag: Value<'c, '_>,
    cases: &[(i64, BlockId, Vec<molt_backend::tir::values::ValueId>)],
    default: BlockId,
    default_args: &[molt_backend::tir::values::ValueId],
    value_map: &ValueMap<'c, 'a>,
    block_index: &HashMap<BlockId, usize>,
    mlir_blocks: &[Block<'c>],
    tir_func: &TirFunction,
    i64_type: Type<'c>,
    location: Location<'c>,
    kind: &str,
) -> Result<(), String> {
    let &default_idx = block_index
        .get(&default)
        .ok_or_else(|| format!("{kind} default target ^bb{} not found", default.0))?;
    let default_dest = &mlir_blocks[default_idx];
    let default_values = resolve_edge_args(
        ctx,
        block,
        default,
        default_args,
        value_map,
        tir_func,
        location,
    )?;

    let mut case_values = Vec::with_capacity(cases.len());
    let mut case_destinations = Vec::with_capacity(cases.len());
    let mut case_args_storage: Vec<Vec<Value<'c, '_>>> = Vec::with_capacity(cases.len());
    for (case_value, target, args) in cases {
        case_values.push(*case_value);
        let &target_idx = block_index
            .get(target)
            .ok_or_else(|| format!("{kind} case target ^bb{} not found", target.0))?;
        case_args_storage.push(resolve_edge_args(
            ctx, block, *target, args, value_map, tir_func, location,
        )?);
        case_destinations.push(target_idx);
    }
    let case_destinations: Vec<(&Block<'c>, &[Value<'c, '_>])> = case_destinations
        .iter()
        .zip(case_args_storage.iter())
        .map(|(&index, args)| (&mlir_blocks[index], args.as_slice()))
        .collect();

    block.append_operation(
        cf::switch(
            ctx,
            &case_values,
            flag,
            i64_type,
            (default_dest, &default_values),
            &case_destinations,
            location,
        )
        .map_err(|error| format!("Failed to build {kind} cf.switch: {error}"))?,
    );
    Ok(())
}

fn append_unreachable_assert<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    location: Location<'c>,
) {
    let i1_type: Type<'c> = IntegerType::new(ctx, 1).into();
    let false_attr = IntegerAttribute::new(i1_type, 0).into();
    let false_op = block.append_operation(arith::constant(ctx, false_attr, location));
    let false_val: Value<'c, '_> = false_op.result(0).unwrap().into();
    block.append_operation(cf::assert(
        ctx,
        false_val,
        "reached TIR unreachable terminator",
        location,
    ));
}

fn zero_value_for_return_type<'c, 'a>(
    ctx: &'c MlirContext,
    block: &'a Block<'c>,
    return_type: &TirType,
    location: Location<'c>,
) -> Value<'c, 'a> {
    let mlir_type = mlir_type_for_tir(ctx, return_type);
    let op = if matches!(return_type, TirType::F64) {
        arith::constant(
            ctx,
            FloatAttribute::new(ctx, mlir_type, 0.0).into(),
            location,
        )
    } else {
        arith::constant(ctx, IntegerAttribute::new(mlir_type, 0).into(), location)
    };
    block.append_operation(op).result(0).unwrap().into()
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
