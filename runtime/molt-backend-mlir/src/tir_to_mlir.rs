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

use melior::{
    Context as MlirContext,
    ir::{Location, Module as MlirModule, operation::OperationLike},
};
use molt_backend::tir::function::TirFunction;

use self::function_builder::build_func_op;

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
            "MLIR verification failed after TIR->MLIR lowering for function '{}'. IR:
{}",
            tir_func.name, text
        ));
    }

    Ok(module)
}
