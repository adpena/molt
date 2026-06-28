use wasm_encoder::{Function, Instruction, ValType};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WasmLirFallbackReason {
    BoxedI64AbiUnsupported,
    EscapedCallableTarget,
    BoxedCheckedArithmetic,
    BoxedUnaryArithmetic,
    BoxedBitwiseOrShift,
    BoxedTruthiness,
    BoxedControlCondition,
    RuntimeConstMaterialization,
    UnsupportedOperation,
}

impl WasmLirFallbackReason {
    pub(crate) fn diagnostic_name(self) -> &'static str {
        match self {
            Self::BoxedI64AbiUnsupported => "boxed-i64-abi-unsupported",
            Self::EscapedCallableTarget => "escaped-callable-target",
            Self::BoxedCheckedArithmetic => "boxed-checked-arithmetic",
            Self::BoxedUnaryArithmetic => "boxed-unary-arithmetic",
            Self::BoxedBitwiseOrShift => "boxed-bitwise-or-shift",
            Self::BoxedTruthiness => "boxed-truthiness",
            Self::BoxedControlCondition => "boxed-control-condition",
            Self::RuntimeConstMaterialization => "runtime-const-materialization",
            Self::UnsupportedOperation => "unsupported-operation",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum WasmCallTarget {
    RuntimeImport(&'static str),
    BailToGenericPath(WasmLirFallbackReason),
}

#[derive(Debug, Clone)]
pub(crate) enum WasmBodyOp {
    Instruction(Instruction<'static>),
    Call(WasmCallTarget),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WasmBodyOps {
    pub(crate) ops: Vec<WasmBodyOp>,
}

impl WasmBodyOps {
    pub(crate) fn push(&mut self, instruction: Instruction<'static>) {
        self.ops.push(WasmBodyOp::Instruction(instruction));
    }

    pub(crate) fn push_runtime_import_call(&mut self, name: &'static str) {
        self.ops
            .push(WasmBodyOp::Call(WasmCallTarget::RuntimeImport(name)));
    }

    pub(crate) fn push_bail_to_generic_path(&mut self, reason: WasmLirFallbackReason) {
        self.ops
            .push(WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(reason)));
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
                WasmBodyOp::Call(_) => {
                    panic!("peephole instruction test unexpectedly produced a typed call")
                }
            })
            .collect()
    }
}

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
            WasmBodyOp::Instruction(_) | WasmBodyOp::Call(WasmCallTarget::RuntimeImport(_)) => None,
        })
    }

    pub(crate) fn runtime_imports(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.ops.iter().filter_map(|op| match op {
            WasmBodyOp::Call(WasmCallTarget::RuntimeImport(name)) => Some(*name),
            WasmBodyOp::Instruction(_) | WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(_)) => {
                None
            }
        })
    }

    pub(crate) fn emit_into(
        &self,
        func_name: &str,
        mut import_index_for: impl FnMut(&str) -> u32,
        func: &mut Function,
    ) {
        for op in &self.ops {
            match op {
                WasmBodyOp::Instruction(instruction) => {
                    func.instruction(instruction);
                }
                WasmBodyOp::Call(WasmCallTarget::RuntimeImport(name)) => {
                    let import_index = import_index_for(name);
                    assert!(
                        import_index != u32::MAX,
                        "LIR fast body for '{func_name}' calls runtime import '{name}' which was skipped/pruned from the import set"
                    );
                    func.instruction(&Instruction::Call(import_index));
                }
                WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(reason)) => {
                    panic!(
                        "LIR fast body for '{func_name}' reached a generic-path bail marker during emission: {}",
                        reason.diagnostic_name()
                    );
                }
            }
        }
    }

    #[cfg(any(test, feature = "test-util"))]
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
                    WasmBodyOp::Call(_) => None,
                })
                .collect(),
            runtime_calls: self.runtime_imports().collect(),
            bails_to_generic_path: self.bail_to_generic_reason().is_some(),
            bail_to_generic_reason: self.bail_to_generic_reason(),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone)]
pub struct WasmBodyTestView {
    pub param_types: Vec<ValType>,
    pub result_types: Vec<ValType>,
    pub locals: Vec<ValType>,
    pub instructions: Vec<Instruction<'static>>,
    pub runtime_calls: Vec<&'static str>,
    pub bails_to_generic_path: bool,
    pub bail_to_generic_reason: Option<WasmLirFallbackReason>,
}
