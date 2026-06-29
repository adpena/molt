#[cfg(feature = "llvm")]
use super::attributes::add_nounwind;
#[cfg(feature = "llvm")]
use super::fixed;
#[cfg(feature = "llvm")]
use crate::runtime_import_abi::{RuntimeImportSignature, RuntimeReturnAbi};
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::types::FunctionType;
#[cfg(feature = "llvm")]
use inkwell::values::FunctionValue;

#[cfg(feature = "llvm")]
pub(super) fn runtime_function_type<'ctx>(
    ctx: &'ctx Context,
    signature: RuntimeImportSignature,
) -> FunctionType<'ctx> {
    let i64_ty = ctx.i64_type();
    let params = vec![i64_ty.into(); signature.param_count];
    match signature.return_abi {
        RuntimeReturnAbi::I64 => i64_ty.fn_type(&params, false),
        RuntimeReturnAbi::Void => ctx.void_type().fn_type(&params, false),
    }
}

/// Declare a runtime symbol that is not in the fixed import table yet.
///
/// This is the only on-demand conservative declaration path lowering may use.
/// It deliberately applies the weakest globally valid runtime attribute set:
/// `nounwind` only. Stronger facts such as `willreturn` or `memory(read)` must
/// be encoded by adding the symbol to `fixed::FIXED_RUNTIME_IMPORTS`.
#[cfg(feature = "llvm")]
pub(crate) fn declare_conservative_runtime_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    name: &str,
    fn_ty: FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(func) = module.get_function(name) {
        return func;
    }
    let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    add_nounwind(ctx, func);
    func
}

/// Declare one fixed runtime import by name, if the fixed table owns it.
///
/// Callers that need a runtime import before the module-wide declaration pass
/// should use this instead of the conservative path so typed pointer/i32
/// signatures and stronger LLVM attributes still come from the fixed authority.
#[cfg(feature = "llvm")]
pub(crate) fn declare_fixed_runtime_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    name: &str,
) -> Option<FunctionValue<'ctx>> {
    fixed::declare_fixed_runtime_function(ctx, module, name)
}

/// Declare all fixed runtime functions that lowered code may call.
///
/// Fixed imports are external linkage symbols resolved by the Molt runtime
/// shared library or static archive. Their signature and optimization
/// attributes live in `fixed::FIXED_RUNTIME_IMPORTS`; the conservative
/// classified-import table is only for residual boxed fallback imports.
#[cfg(feature = "llvm")]
pub(crate) fn declare_runtime_functions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) {
    fixed::declare_fixed_runtime_functions(ctx, module);
}
