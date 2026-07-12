use super::super::const_materialization::WasmConstMaterialization;
use super::super::lir_fast::LirRuntimeCall;
use std::sync::Arc;
use wasm_encoder::Instruction;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WasmLirFallbackReason {
    BoxedI64AbiUnsupported,
    EscapedCallableTarget,
    BoxedCheckedArithmetic,
    UnsupportedOperation,
}

impl WasmLirFallbackReason {
    pub(crate) fn diagnostic_name(self) -> &'static str {
        match self {
            Self::BoxedI64AbiUnsupported => "boxed-i64-abi-unsupported",
            Self::EscapedCallableTarget => "escaped-callable-target",
            Self::BoxedCheckedArithmetic => "boxed-checked-arithmetic",
            Self::UnsupportedOperation => "unsupported-operation",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum WasmCallTarget {
    RuntimeImport(LirRuntimeCall),
    BailToGenericPath(WasmLirFallbackReason),
}

#[derive(Debug, Clone)]
pub(crate) enum WasmBodyOp {
    Instruction(Instruction<'static>),
    Call(WasmCallTarget),
    ConstMaterialization(WasmConstMaterialization),
    DataPtrI32(Arc<[u8]>),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WasmBodyOps {
    pub(crate) ops: Vec<WasmBodyOp>,
}

impl WasmBodyOps {
    pub(crate) fn push(&mut self, instruction: Instruction<'static>) {
        self.ops.push(WasmBodyOp::Instruction(instruction));
    }

    pub(crate) fn push_runtime_import_call(&mut self, call: LirRuntimeCall) {
        self.ops
            .push(WasmBodyOp::Call(WasmCallTarget::RuntimeImport(call)));
    }

    pub(crate) fn push_bail_to_generic_path(&mut self, reason: WasmLirFallbackReason) {
        self.ops
            .push(WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(reason)));
    }

    pub(crate) fn push_const_materialization(&mut self, materialization: WasmConstMaterialization) {
        self.ops
            .push(WasmBodyOp::ConstMaterialization(materialization));
    }

    pub(crate) fn push_data_ptr_i32(&mut self, bytes: Arc<[u8]>) {
        self.ops.push(WasmBodyOp::DataPtrI32(bytes));
    }

    pub(crate) fn into_vec(self) -> Vec<WasmBodyOp> {
        self.ops
    }

    #[cfg(test)]
    pub(crate) fn from_instructions(instructions: Vec<Instruction<'static>>) -> Self {
        Self {
            ops: instructions
                .into_iter()
                .map(WasmBodyOp::Instruction)
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn into_instructions_for_tests(self) -> Vec<Instruction<'static>> {
        self.ops
            .into_iter()
            .map(|op| match op {
                WasmBodyOp::Instruction(instruction) => instruction,
                WasmBodyOp::Call(_)
                | WasmBodyOp::ConstMaterialization(_)
                | WasmBodyOp::DataPtrI32(_) => {
                    panic!("peephole instruction test unexpectedly produced a typed body op")
                }
            })
            .collect()
    }
}
