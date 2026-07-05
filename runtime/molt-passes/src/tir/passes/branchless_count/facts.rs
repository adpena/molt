use std::collections::HashMap;

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_operand_independent_result_tir_type;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

pub(super) struct BranchlessFacts {
    type_map: HashMap<ValueId, TirType>,
    const_map: HashMap<ValueId, i64>,
}

impl BranchlessFacts {
    pub(super) fn collect(func: &TirFunction) -> Self {
        let mut type_map = HashMap::new();
        let mut const_map = HashMap::new();

        for block in func.blocks.values() {
            for arg in &block.args {
                type_map.insert(arg.id, arg.ty.clone());
            }
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    for &res in &op.results {
                        const_map.insert(res, *v);
                    }
                }
                if let Some(ty) = opcode_operand_independent_result_tir_type(op.opcode) {
                    for &res in &op.results {
                        type_map.insert(res, ty.clone());
                    }
                }
            }
        }

        Self {
            type_map,
            const_map,
        }
    }

    pub(super) fn is_bool(&self, value: ValueId) -> bool {
        matches!(self.type_map.get(&value), Some(TirType::Bool))
    }

    pub(super) fn const_int(&self, value: ValueId) -> Option<i64> {
        self.const_map.get(&value).copied()
    }
}
