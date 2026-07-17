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
use molt_backend::tir::{
    blocks::Terminator,
    function::TirFunction,
    ops::{AttrValue, OpCode},
};

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
    preflight_tir_function(tir_func)?;
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
    let func_op = build_func_op(tir_func, ctx, location)?;
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

fn preflight_tir_function(tir_func: &TirFunction) -> Result<(), String> {
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
            if op.is_async_work_poll() {
                return Err(format!(
                    "{} ^bb{} op #{op_index} async_work_poll cannot lower because the canonical pending-call/eval-breaker runtime boundary is unavailable for MLIR",
                    tir_func.name, block_id.0
                ));
            }
            if op.opcode == OpCode::Copy {
                let original_kind = match op.attrs.get("_original_kind") {
                    Some(AttrValue::Str(kind)) => Some(kind.as_str()),
                    _ => None,
                };
                if !matches!(original_kind, Some(kind) if kind != "binding_alias")
                    && (op.operands.len() != 1 || op.results.len() != 1)
                {
                    return Err(format!(
                        "{} ^bb{} op #{op_index} malformed {:?} copy requires one operand and one result",
                        tir_func.name,
                        block_id.0,
                        original_kind.unwrap_or("SSA")
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
    Ok(())
}
