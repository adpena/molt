use std::cell::OnceCell;
use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::INLINE_INT47_HI;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

pub(super) struct BranchlessFacts {
    type_map: HashMap<ValueId, TirType>,
    const_map: HashMap<ValueId, i64>,
    ranges: OnceCell<crate::tir::ValueRangeResult>,
}

impl BranchlessFacts {
    pub(super) fn collect(func: &TirFunction) -> Self {
        let type_map = crate::tir::type_refine::extract_exact_scalar_map(func);
        let mut const_map = HashMap::new();

        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt
                    && op.has_valid_result_arity()
                    && op.operands.is_empty()
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    for &res in &op.results {
                        const_map.insert(res, *v);
                    }
                }
            }
        }

        Self {
            type_map,
            const_map,
            ranges: OnceCell::new(),
        }
    }

    pub(super) fn is_bool(&self, value: ValueId) -> bool {
        matches!(self.type_map.get(&value), Some(TirType::Bool))
    }

    pub(super) fn const_int(&self, value: ValueId) -> Option<i64> {
        self.const_map.get(&value).copied()
    }

    pub(super) fn can_increment_unconditionally(
        &self,
        func: &TirFunction,
        block: BlockId,
        value: ValueId,
    ) -> bool {
        if self.type_map.get(&value) != Some(&TirType::I64) {
            return false;
        }
        // The false path used to forward the same object without calling add.
        // Unconditional addition is legal only in the nonallocating inline lane,
        // including the incremented endpoint. An annotation is not this proof.
        let ranges = self
            .ranges
            .get_or_init(|| crate::representation_facts::value_range_for(func));
        let range = ranges.range_at(block, value);
        range.fits_inline_int47() && range.hi < INLINE_INT47_HI
    }
}
