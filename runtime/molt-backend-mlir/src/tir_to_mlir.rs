//! TIR to MLIR programmatic builder.
//!
//! Converts a TirFunction into a verified MLIR module using melior's typed
//! builder API. The lowering authority is split by structural role:
//! type mapping, value lookup, op emission, terminator emission, opaque Molt op
//! names, and top-level function assembly each live in their own module.

mod attrs;
mod function_builder;
mod opaque_ops;
mod ops;
mod terminators;
mod types;
mod values;

use std::collections::HashSet;

use melior::{
    Context as MlirContext,
    ir::{BlockLike, Location, Module as MlirModule, operation::OperationLike},
};
use molt_backend::tir::{blocks::Terminator, function::TirFunction, ops::OpCode};
use molt_backend::{repr::Repr, representation_plan::ProducerCarrierPlan};

use self::function_builder::{build_func_op, build_state_dispatch_runtime_declaration};

/// Build an MLIR module from a TIR function using the programmatic builder API.
///
/// This produces a valid, verifiable MLIR module using standard dialects
/// (func, arith, cf). The module can then be optimized and lowered to LLVM.
pub fn build_mlir_module<'c>(
    tir_func: &TirFunction,
    ctx: &'c MlirContext,
) -> Result<MlirModule<'c>, String> {
    // Progressive lowering deliberately preserves checked Molt runtime-boundary
    // operations until their ABI lowering exists.  Own that context policy at
    // the module builder so every public entry point (including direct users)
    // observes the same dialect contract.
    ctx.set_allow_unregistered_dialects(true);
    let carriers = preflight_tir_function(tir_func)?;
    let location = Location::unknown(ctx);
    let module = MlirModule::new(location);

    if tir_func
        .blocks
        .values()
        .any(|block| matches!(block.terminator, Terminator::StateDispatch { .. }))
    {
        module
            .body()
            .append_operation(build_state_dispatch_runtime_declaration(ctx, location));
    }
    let func_op = build_func_op(tir_func, &carriers, ctx, location)?;
    module.body().append_operation(func_op);

    if let Some(dump_dir) = std::env::var_os("MOLT_MLIR_DUMP_DIR") {
        let safe_name: String = tir_func
            .name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let path = std::path::Path::new(&dump_dir).join(format!("{safe_name}.mlir"));
        std::fs::create_dir_all(std::path::Path::new(&dump_dir))
            .and_then(|()| std::fs::write(&path, module.as_operation().to_string()))
            .map_err(|error| {
                format!("failed to write pre-verification MLIR dump {path:?}: {error}")
            })?;
    }

    if !module.as_operation().verify() {
        let text = module.as_operation().to_string();
        return Err(format!(
            "MLIR verification failed after TIR->MLIR lowering for function '{}'. IR:
{}",
            tir_func.name, text
        ));
    }

    Ok(module)
}

fn preflight_tir_function(tir_func: &TirFunction) -> Result<ProducerCarrierPlan, String> {
    let target = molt_backend::tir::TargetInfo::mlir_release_fast();
    let entry = tir_func
        .blocks
        .get(&tir_func.entry_block)
        .ok_or_else(|| format!("{} has no entry block", tir_func.name))?;
    if entry.args.len() != tir_func.param_types.len() {
        return Err(format!(
            "{} entry has {} arguments for {} parameters",
            tir_func.name,
            entry.args.len(),
            tir_func.param_types.len()
        ));
    }
    let mut block_ids: Vec<_> = tir_func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|block| block.0);
    if let Some(entry) = block_ids
        .iter()
        .position(|block| *block == tir_func.entry_block)
    {
        block_ids.swap(0, entry);
    }

    let mut available = HashSet::new();
    for block in tir_func.blocks.values() {
        available.extend(block.args.iter().map(|arg| arg.id));
    }
    for block_id in block_ids {
        let block = &tir_func.blocks[&block_id];
        for (op_index, op) in block.ops.iter().enumerate() {
            for operand in &op.operands {
                if !available.contains(operand) {
                    return Err(format!(
                        "{} ^bb{} op #{op_index} {:?} reads undefined or not-yet-defined value %{}",
                        tir_func.name, block_id.0, op.opcode, operand.0
                    ));
                }
            }
            if op.opcode == OpCode::Pow && op.results.len() != 1 {
                return Err(format!(
                    "{} ^bb{} op #{op_index} malformed OpCode::Pow requires one result slot",
                    tir_func.name, block_id.0
                ));
            }
            if op.is_async_work_poll() && !target.supports_pending_call_eval_breaker_poll() {
                return Err(format!(
                    "{} ^bb{} op #{op_index} async_work_poll cannot lower for {}: {}",
                    tir_func.name,
                    block_id.0,
                    target.target.as_str(),
                    molt_backend::tir::op_kinds_generated::PENDING_CALL_EVAL_BREAKER_REQUIREMENT_REASON,
                ));
            }
            if op.opcode == OpCode::Copy {
                let original_kind = op.origin;
                if !matches!(original_kind, Some(kind) if kind != molt_backend::OpKind::BindingAlias)
                    && (op.operands.len() != 1 || op.results.len() != 1)
                {
                    return Err(format!(
                        "{} ^bb{} op #{op_index} malformed {:?} copy requires one operand and one result",
                        tir_func.name,
                        block_id.0,
                        original_kind.map_or("SSA", molt_backend::OpKind::as_str)
                    ));
                }
            }
            available.extend(op.results.iter().copied());
        }
        let mut terminator_error = None;
        block.terminator.for_each_value(|value| {
            if terminator_error.is_none() && !available.contains(&value) {
                terminator_error = Some(format!(
                    "{} ^bb{} terminator reads undefined or not-yet-defined value %{}",
                    tir_func.name, block_id.0, value.0
                ));
            }
        });
        if let Some(error) = terminator_error {
            return Err(error);
        }
    }
    let carriers = ProducerCarrierPlan::build(tir_func);
    // MLIR has no Molt runtime provider: a boxed boundary is unsupported,
    // rather than permission to reinterpret annotation bits as a scalar.
    let require_raw = |id: molt_backend::tir::values::ValueId, role: &str| {
        if matches!(
            carriers.repr_by_value.get(&id),
            Some(Repr::RawI64Safe | Repr::RawI64FullDeopt | Repr::Bool | Repr::FloatUnboxed)
        ) {
            Ok(())
        } else {
            Err(format!(
                "{} {role} %{} has no producer-proven machine carrier (annotation-only or boxed value); MLIR has no Molt runtime provider",
                tir_func.name, id.0
            ))
        }
    };
    let mut blocks: Vec<_> = tir_func.blocks.values().collect();
    blocks.sort_by_key(|block| (block.id != tir_func.entry_block, block.id.0));
    for block in blocks {
        for arg in &block.args {
            require_raw(
                arg.id,
                if block.id == tir_func.entry_block {
                    "parameter"
                } else {
                    "block argument"
                },
            )?;
        }
        if let Terminator::Return { values } = &block.terminator {
            if values.is_empty() && tir_func.return_abi.returns_value() {
                return Err(format!(
                    "{} return value <implicit None> has no producer-proven machine carrier; MLIR has no Molt runtime provider",
                    tir_func.name
                ));
            }
            for &value in values {
                require_raw(value, "return value")?;
            }
        }
    }
    Ok(carriers)
}
