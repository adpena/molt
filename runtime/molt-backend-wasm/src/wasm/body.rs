mod emit;
mod ops;
#[cfg(any(test, feature = "test-util"))]
mod test_view;

#[cfg(any(test, feature = "test-util"))]
use crate::wasm_abi_generated::WasmRuntimeImport;
use wasm_encoder::ValType;

pub use ops::WasmLirFallbackReason;
pub(crate) use ops::{WasmBodyOp, WasmBodyOps, WasmCallTarget};
#[cfg(feature = "test-util")]
pub use test_view::WasmBodyTestView;

#[derive(Debug, Clone)]
pub(crate) struct WasmBody {
    pub(crate) param_types: Vec<ValType>,
    pub(crate) result_types: Vec<ValType>,
    pub(crate) locals: Vec<ValType>,
    pub(crate) ops: Vec<WasmBodyOp>,
}

impl WasmBody {
    pub(crate) fn bail_to_generic_reason(&self) -> Option<WasmLirFallbackReason> {
        self.ops.iter().find_map(|op| match op {
            WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(reason)) => Some(*reason),
            WasmBodyOp::Instruction(_)
            | WasmBodyOp::Call(WasmCallTarget::RuntimeImport(_))
            | WasmBodyOp::ConstMaterialization(_)
            | WasmBodyOp::DataPtrI32(_) => None,
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn runtime_imports(&self) -> impl Iterator<Item = WasmRuntimeImport> + '_ {
        self.ops.iter().filter_map(|op| match op {
            WasmBodyOp::Call(WasmCallTarget::RuntimeImport(call)) => Some(call.import()),
            WasmBodyOp::ConstMaterialization(materialization) => {
                Some(materialization.runtime_import())
            }
            WasmBodyOp::Instruction(_) | WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(_)) => {
                None
            }
            WasmBodyOp::DataPtrI32(_) => None,
        })
    }
}
