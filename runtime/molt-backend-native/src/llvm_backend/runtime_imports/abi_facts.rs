#[cfg(feature = "llvm")]
use super::fixed::fixed_runtime_import_return_abi;
#[cfg(feature = "llvm")]
use crate::runtime_import_abi::RuntimeReturnAbi;
#[cfg(feature = "llvm")]
use molt_ir::runtime_fn_generated::RuntimeFn;

#[cfg(feature = "llvm")]
pub(crate) fn runtime_import_return_abi(
    name: &str,
    param_count: usize,
) -> Option<RuntimeReturnAbi> {
    fixed_runtime_import_return_abi(name, param_count)
        .or_else(|| {
            // Every all-word runtime entry (boxed call, raw call or machine
            // helper) has one generated identity with its word arity and
            // return contract. No native list mirrors it.
            RuntimeFn::from_symbol(name)
                .filter(|function| function.params() == param_count)
                .map(RuntimeReturnAbi::of)
        })
        .or_else(|| {
            // Runtime functions published as Python callables take and return
            // the boxed i64 carrier at their manifest arity. The generated
            // callable ABI is target-neutral, so native-only providers that the
            // WASM import table filters out still classify here.
            molt_ir::runtime_callable_abi_generated::runtime_callable_abi(name)
                .filter(|abi| abi.arity == param_count)
                .map(|_| RuntimeReturnAbi::I64)
        })
}

#[cfg(feature = "llvm")]
pub(crate) fn is_runtime_import_abi(
    name: &str,
    param_count: usize,
    return_abi: RuntimeReturnAbi,
) -> bool {
    runtime_import_return_abi(name, param_count) == Some(return_abi)
}
