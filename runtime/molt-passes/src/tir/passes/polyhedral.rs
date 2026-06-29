//! Polyhedral optimization hooks for loop tiling, fusion, and interchange.
//! These require MLIR's affine dialect for full implementation.
//! This module provides the analysis + annotation infrastructure.

use std::collections::HashSet;

use super::PassStats;
use crate::tir::analysis::{AnalysisManager, LoopForest, LoopForestResult};
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_is_polyhedral_affine_body_table, opcode_is_polyhedral_loop_header_table,
};
use crate::tir::target_info::TargetInfo;

/// Representative element size (bytes) for the numeric loop nests the polyhedral
/// analyzer targets (i64 / f64 are both 8 bytes). Used to query the cost
/// model's cache-aware tile sizes.
const TILE_ELEM_SIZE_BYTES: usize = 8;

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

/// Analyze loop nests for polyhedral optimization potential. `tti` supplies the
/// cache-aware tile sizes (the baseline cost model yields a single L1-edge tile
/// of 32, reproducing the prior hardcoded `vec![32]`).
pub fn analyze_loop_nests(
    func: &TirFunction,
    loop_forest: &LoopForestResult,
    tti: &TargetInfo,
) -> Vec<LoopNest> {
    let mut nests = Vec::new();

    for &bid in &loop_forest.headers {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        if !block
            .ops
            .iter()
            .any(|op| opcode_is_polyhedral_loop_header_table(op.opcode))
        {
            continue;
        }
        let Some(body) = loop_forest.bodies.get(&bid) else {
            continue;
        };

        let is_affine = check_affine_body(func, body);
        nests.push(LoopNest {
            loop_blocks: vec![bid],
            trip_counts: vec![None], // Would need range analysis
            is_affine,
            tile_sizes: if is_affine {
                tti.tile_sizes(TILE_ELEM_SIZE_BYTES)
            } else {
                vec![]
            },
        });
    }
    nests
}

fn check_affine_body(func: &TirFunction, body: &HashSet<BlockId>) -> bool {
    // Simplified: check if all loop-body ops are arithmetic, memory,
    // loop control, or constants; no calls, builds, or side effects.
    body.iter().all(|bid| {
        func.blocks.get(bid).is_some_and(|block| {
            block.ops.iter().all(|op| {
                opcode_is_polyhedral_affine_body_table(op.opcode) || op.is_plain_value_copy()
            })
        })
    })
}

/// Annotate loops with polyhedral optimization hints.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, tti: &TargetInfo) -> PassStats {
    let loop_forest = am.get::<LoopForest>(func).clone();
    let nests = analyze_loop_nests(func, &loop_forest, tti);
    let mut annotated = 0;
    for nest in &nests {
        if nest.is_affine && !nest.tile_sizes.is_empty() {
            // Add polyhedral hints to the loop header block
            for &bid in &nest.loop_blocks {
                if let Some(block) = func.blocks.get_mut(&bid) {
                    for op in &mut block.ops {
                        if opcode_is_polyhedral_loop_header_table(op.opcode) {
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
        attrs_changed: 0,
        ops_removed: 0,
        ops_added: 0,
        facts_changed: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::{AnalysisManager, LoopForest};
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use std::collections::HashMap;

    fn add_self_backedge(func: &mut TirFunction) {
        let header = func.entry_block;
        let exit = func.fresh_block();
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.get_mut(&header).unwrap().terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: header,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        };
    }

    fn analyze_for_test(func: &TirFunction) -> Vec<LoopNest> {
        let mut am = AnalysisManager::new();
        let loop_forest = am.get::<LoopForest>(func).clone();
        analyze_loop_nests(func, &loop_forest, &TargetInfo::native_release_fast())
    }

    fn run_for_test(func: &mut TirFunction) -> PassStats {
        let mut am = AnalysisManager::new();
        run(func, &mut am, &TargetInfo::native_release_fast())
    }

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
        add_self_backedge(&mut func);
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
        add_self_backedge(&mut func);
        func
    }

    #[test]
    fn test_affine_loop_detected() {
        let func = make_affine_loop_func();
        let nests = analyze_for_test(&func);
        assert_eq!(nests.len(), 1);
        assert!(nests[0].is_affine);
        assert_eq!(nests[0].tile_sizes, vec![32]);
    }

    #[test]
    fn test_non_affine_loop_skipped() {
        let func = make_non_affine_loop_func();
        let nests = analyze_for_test(&func);
        assert_eq!(nests.len(), 1);
        assert!(!nests[0].is_affine);
        assert!(nests[0].tile_sizes.is_empty());
    }

    #[test]
    fn test_run_annotates_affine_loops() {
        let mut func = make_affine_loop_func();
        let stats = run_for_test(&mut func);
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
        assert_eq!(for_op.attrs.get("tile_size"), Some(&AttrValue::Int(32)));
    }

    #[test]
    fn test_run_skips_non_affine() {
        let mut func = make_non_affine_loop_func();
        let stats = run_for_test(&mut func);
        assert_eq!(stats.values_changed, 0);
    }
}
