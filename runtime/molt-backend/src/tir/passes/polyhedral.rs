//! Polyhedral optimization hooks for loop tiling, fusion, and interchange.
//! These require MLIR's affine dialect for full implementation.
//! This module provides the analysis + annotation infrastructure.

use super::PassStats;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;

/// Loop nest analysis result.
#[derive(Debug, Clone)]
pub struct LoopNest {
    /// Block IDs forming this loop nest (outer to inner).
    pub loop_blocks: Vec<BlockId>,
    /// Estimated iteration counts per loop level.
    pub trip_counts: Vec<Option<u64>>,
    /// Whether the loop accesses arrays with affine indices.
    pub is_affine: bool,
    /// Suggested tile sizes (if tiling is beneficial).
    pub tile_sizes: Vec<u32>,
}

/// Analyze loop nests for polyhedral optimization potential.
pub fn analyze_loop_nests(func: &TirFunction) -> Vec<LoopNest> {
    let mut nests = Vec::new();
    let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            if matches!(op.opcode, OpCode::ForIter | OpCode::ScfFor) {
                // Check if loop body contains only affine operations
                let is_affine = check_affine_body(func, bid);
                nests.push(LoopNest {
                    loop_blocks: vec![bid],
                    trip_counts: vec![None], // Would need range analysis
                    is_affine,
                    tile_sizes: if is_affine { vec![32] } else { vec![] },
                });
            }
        }
    }
    nests
}

fn check_affine_body(func: &TirFunction, _bid: BlockId) -> bool {
    // Simplified: check if all ops in the function are arithmetic, memory,
    // loop control, or constants — no calls, builds, or side effects.
    func.blocks.values().all(|block| {
        block.ops.iter().all(|op| {
            matches!(
                op.opcode,
                OpCode::Add
                    | OpCode::Sub
                    | OpCode::Mul
                    | OpCode::Div
                    | OpCode::FloorDiv
                    | OpCode::Mod
                    | OpCode::Index
                    | OpCode::StoreIndex
                    | OpCode::ConstInt
                    | OpCode::ConstFloat
                    | OpCode::ConstBool
                    | OpCode::ConstNone
                    | OpCode::Copy
                    | OpCode::Lt
                    | OpCode::Le
                    | OpCode::Gt
                    | OpCode::Ge
                    | OpCode::Eq
                    | OpCode::Ne
                    | OpCode::ForIter
                    | OpCode::ScfFor
                    | OpCode::ScfYield
                    | OpCode::GetIter
                    | OpCode::IterNext
            )
        })
    })
}

/// Annotate loops with polyhedral optimization hints.
pub fn run(func: &mut TirFunction) -> PassStats {
    let nests = analyze_loop_nests(func);
    let mut annotated = 0;
    for nest in &nests {
        if nest.is_affine && !nest.tile_sizes.is_empty() {
            // Add polyhedral hints to the loop header block
            for &bid in &nest.loop_blocks {
                if let Some(block) = func.blocks.get_mut(&bid) {
                    for op in &mut block.ops {
                        if matches!(op.opcode, OpCode::ForIter | OpCode::ScfFor) {
                            op.attrs.insert(
                                "polyhedral_tileable".into(),
                                crate::tir::ops::AttrValue::Bool(true),
                            );
                            op.attrs.insert(
                                "tile_size".into(),
                                crate::tir::ops::AttrValue::Int(nest.tile_sizes[0] as i64),
                            );
                            annotated += 1;
                        }
                    }
                }
            }
        }
    }
    PassStats {
        name: "polyhedral",
        values_changed: annotated,
        ops_removed: 0,
        ops_added: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use std::collections::HashMap;

    fn make_affine_loop_func() -> TirFunction {
        let mut func = TirFunction::new("test_affine".into(), vec![], TirType::None);
        let bid = func.entry_block;
        let block = func.blocks.get_mut(&bid).unwrap();
        // Add a ForIter op with only affine body ops
        block.ops.push(TirOp {
            dialect: Dialect::Scf,
            opcode: OpCode::ForIter,
            operands: vec![],
            results: vec![],
            attrs: HashMap::new(),
            source_span: None,
        });
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![ValueId(2)],
            attrs: HashMap::new(),
            source_span: None,
        });
        func
    }

    fn make_non_affine_loop_func() -> TirFunction {
        let mut func = TirFunction::new("test_non_affine".into(), vec![], TirType::None);
        let bid = func.entry_block;
        let block = func.blocks.get_mut(&bid).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Scf,
            opcode: OpCode::ForIter,
            operands: vec![],
            results: vec![],
            attrs: HashMap::new(),
            source_span: None,
        });
        // A Call op makes it non-affine
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs: HashMap::new(),
            source_span: None,
        });
        func
    }

    #[test]
    fn test_affine_loop_detected() {
        let func = make_affine_loop_func();
        let nests = analyze_loop_nests(&func);
        assert_eq!(nests.len(), 1);
        assert!(nests[0].is_affine);
        assert_eq!(nests[0].tile_sizes, vec![32]);
    }

    #[test]
    fn test_non_affine_loop_skipped() {
        let func = make_non_affine_loop_func();
        let nests = analyze_loop_nests(&func);
        assert_eq!(nests.len(), 1);
        assert!(!nests[0].is_affine);
        assert!(nests[0].tile_sizes.is_empty());
    }

    #[test]
    fn test_run_annotates_affine_loops() {
        let mut func = make_affine_loop_func();
        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 1);
        // Check the annotation was added
        let block = &func.blocks[&func.entry_block];
        let for_op = block
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ForIter)
            .unwrap();
        assert_eq!(
            for_op.attrs.get("polyhedral_tileable"),
            Some(&AttrValue::Bool(true))
        );
        assert_eq!(
            for_op.attrs.get("tile_size"),
            Some(&AttrValue::Int(32))
        );
    }

    #[test]
    fn test_run_skips_non_affine() {
        let mut func = make_non_affine_loop_func();
        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
    }
}
