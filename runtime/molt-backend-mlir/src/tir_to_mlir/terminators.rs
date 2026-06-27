use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    dialect::{arith, cf, func},
    ir::{
        Block, Location, Type, Value,
        attribute::{FloatAttribute, IntegerAttribute},
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
