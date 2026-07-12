use super::{WasmBody, WasmBodyOp, WasmLirFallbackReason};
use crate::wasm_abi_generated::WasmRuntimeImport;
use wasm_encoder::{Instruction, ValType};

#[derive(Debug, Clone)]
pub struct WasmBodyTestView {
    pub param_types: Vec<ValType>,
    pub result_types: Vec<ValType>,
    pub locals: Vec<ValType>,
    pub instructions: Vec<Instruction<'static>>,
    pub data_ptr_i32_literals: Vec<Vec<u8>>,
    pub runtime_calls: Vec<&'static str>,
    pub bails_to_generic_path: bool,
    pub bail_to_generic_reason: Option<WasmLirFallbackReason>,
}

impl WasmBody {
    pub(crate) fn test_view(&self) -> WasmBodyTestView {
        WasmBodyTestView {
            param_types: self.param_types.clone(),
            result_types: self.result_types.clone(),
            locals: self.locals.clone(),
            instructions: self
                .ops
                .iter()
                .filter_map(|op| match op {
                    WasmBodyOp::Instruction(instruction) => Some(instruction.clone()),
                    WasmBodyOp::Call(_)
                    | WasmBodyOp::ConstMaterialization(_)
                    | WasmBodyOp::DataPtrI32(_) => None,
                })
                .collect(),
            data_ptr_i32_literals: self
                .ops
                .iter()
                .filter_map(|op| match op {
                    WasmBodyOp::DataPtrI32(bytes) => Some(bytes.to_vec()),
                    WasmBodyOp::Instruction(_)
                    | WasmBodyOp::Call(_)
                    | WasmBodyOp::ConstMaterialization(_) => None,
                })
                .collect(),
            runtime_calls: self
                .runtime_imports()
                .map(WasmRuntimeImport::name)
                .collect(),
            bails_to_generic_path: self.bail_to_generic_reason().is_some(),
            bail_to_generic_reason: self.bail_to_generic_reason(),
        }
    }
}
