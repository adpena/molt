use super::LirLowerCtx;
use molt_tir::tir::lir::{LirFunction, LirRepr};
use molt_tir::tir::ops::{AttrValue, TirOp};
use molt_tir::tir::types::TirType;
use molt_tir::tir::values::ValueId;
use std::collections::HashSet;

impl LirLowerCtx<'_> {
    pub(super) fn repr_of(&self, vid: ValueId) -> LirRepr {
        self.value_reprs
            .get(&vid)
            .copied()
            .unwrap_or(LirRepr::DynBox)
    }

    pub(super) fn type_of(&self, vid: ValueId) -> Option<&TirType> {
        self.value_types.get(&vid)
    }

    pub(super) fn has_flat_list_int_storage(&self, vid: ValueId) -> bool {
        self.flat_list_int_values.contains(&vid)
    }
}

pub(super) fn compute_lir_flat_list_int_values(func: &LirFunction) -> HashSet<ValueId> {
    let mut facts = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|block_id| block_id.0);
        for block_id in block_ids {
            let Some(block) = func.blocks.get(&block_id) else {
                continue;
            };
            for op in &block.ops {
                let tir_op = &op.tir_op;
                if tir_op_original_kind(tir_op) == Some("list_int_new") {
                    for &result in &tir_op.results {
                        changed |= facts.insert(result);
                    }
                    continue;
                }
                if tir_op.is_plain_value_copy()
                    && let (Some(&source), Some(&result)) =
                        (tir_op.operands.first(), tir_op.results.first())
                    && facts.contains(&source)
                {
                    changed |= facts.insert(result);
                }
            }
        }
    }
    facts
}

fn tir_op_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}
