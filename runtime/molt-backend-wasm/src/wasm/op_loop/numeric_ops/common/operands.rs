use crate::OpIR;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};

#[derive(Clone, Copy)]
pub(in crate::wasm::op_loop::numeric_ops) struct BinaryOperands {
    pub(in crate::wasm::op_loop::numeric_ops) lhs: u32,
    pub(in crate::wasm::op_loop::numeric_ops) rhs: u32,
}

impl BinaryOperands {
    pub(in crate::wasm::op_loop::numeric_ops) fn locals(self) -> [u32; 2] {
        [self.lhs, self.rhs]
    }
}

#[derive(Clone, Copy)]
pub(in crate::wasm::op_loop::numeric_ops) struct IntBinaryTemps {
    pub(in crate::wasm::op_loop::numeric_ops) lhs: u32,
    pub(in crate::wasm::op_loop::numeric_ops) rhs: u32,
    pub(in crate::wasm::op_loop::numeric_ops) result: u32,
}

pub(in crate::wasm::op_loop::numeric_ops) fn binary_operands(
    op: &OpIR,
    locals: &WasmFrameLocals,
) -> BinaryOperands {
    let args = op.args.as_ref().unwrap();
    BinaryOperands {
        lhs: locals[&args[0]],
        rhs: locals[&args[1]],
    }
}

pub(in crate::wasm::op_loop::numeric_ops) fn unary_operand(
    op: &OpIR,
    locals: &WasmFrameLocals,
) -> u32 {
    let args = op.args.as_ref().unwrap();
    locals[&args[0]]
}

pub(in crate::wasm::op_loop::numeric_ops) fn ternary_operands(
    op: &OpIR,
    locals: &WasmFrameLocals,
) -> [u32; 3] {
    let args = op.args.as_ref().unwrap();
    [locals[&args[0]], locals[&args[1]], locals[&args[2]]]
}

pub(in crate::wasm::op_loop::numeric_ops) fn int_binary_temps(
    locals: &WasmFrameLocals,
) -> IntBinaryTemps {
    IntBinaryTemps {
        lhs: locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0),
        rhs: locals.synthetic(WasmFrameSyntheticLocal::MoltTmp1),
        result: locals.synthetic(WasmFrameSyntheticLocal::MoltTmp2),
    }
}
