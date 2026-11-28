//! Clone an SSA region while keeping all edges out of it on their original
//! continuations. The caller admits implicit exception edges and repairs uses
//! joining the two versions.

use molt_ir::deterministic_hash::HashMap;

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::clone_support::clone_attrs_without_simple_names;
use crate::tir::function::TirFunction;
use crate::tir::ops::TirOp;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

pub(super) struct RegionClone {
    pub(super) blocks: HashMap<BlockId, BlockId>,
    pub(super) values: HashMap<ValueId, ValueId>,
    pub(super) definitions: Vec<(ValueId, TirType)>,
    pub(super) ops_added: usize,
}

impl RegionClone {
    pub(super) fn install(
        func: &mut TirFunction,
        original: &TirFunction,
        blocks: &[BlockId],
    ) -> Self {
        let mut region = Self {
            blocks: HashMap::default(),
            values: HashMap::default(),
            definitions: Vec::new(),
            ops_added: 0,
        };
        for &id in blocks {
            region.blocks.insert(id, func.fresh_block());
            let body = &original.blocks[&id];
            for (value, ty) in body.args.iter().map(|arg| (arg.id, arg.ty.clone())).chain(
                body.ops
                    .iter()
                    .flat_map(|op| op.results.iter().copied())
                    .map(|id| {
                        let ty = original
                            .value_types
                            .get(&id)
                            .cloned()
                            .unwrap_or(TirType::DynBox);
                        (id, ty)
                    }),
            ) {
                let cloned = func.fresh_value();
                if let Some(known) = original.value_types.get(&value) {
                    func.value_types.insert(cloned, known.clone());
                }
                region.values.insert(value, cloned);
                region.definitions.push((value, ty));
            }
        }
        for &id in blocks {
            let source = &original.blocks[&id];
            let mut terminator = source.terminator.clone();
            terminator.for_each_value_mut(|value| *value = region.value(*value));
            terminator.for_each_edge_mut(|target, _| {
                if let Some(&cloned) = region.blocks.get(target) {
                    *target = cloned;
                }
            });
            let cloned = TirBlock {
                id: region.blocks[&id],
                args: source
                    .args
                    .iter()
                    .map(|arg| TirValue {
                        id: region.value(arg.id),
                        ty: arg.ty.clone(),
                    })
                    .collect(),
                ops: source
                    .ops
                    .iter()
                    .map(|op| TirOp {
                        operands: op.operands.iter().map(|&v| region.value(v)).collect(),
                        results: op.results.iter().map(|&v| region.value(v)).collect(),
                        attrs: clone_attrs_without_simple_names(&op.attrs),
                        ..op.clone()
                    })
                    .collect(),
                terminator,
            };
            region.ops_added += cloned.ops.len();
            func.blocks.insert(cloned.id, cloned);
        }
        region
    }

    pub(super) fn value(&self, id: ValueId) -> ValueId {
        self.values.get(&id).copied().unwrap_or(id)
    }
}
