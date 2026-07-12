use std::collections::HashSet;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::type_refine;
use crate::tir::values::ValueId;

use super::compat::reuse_compatible;

/// A reuse candidate: a `DecRef` whose freed memory can potentially be reused
/// by a subsequent `Alloc` in the same basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseCandidate {
    /// The value being DecRef'd (the potential reuse source).
    pub decref_value: ValueId,
    /// The op index of the DecRef within its block.
    pub decref_op_idx: usize,
    /// The result value of the paired Alloc (the reuse sink).
    pub alloc_value: ValueId,
    /// The op index of the Alloc within its block.
    pub alloc_op_idx: usize,
    /// The block containing both ops.
    pub block_id: BlockId,
}

/// Analyze a TIR function for Perceus-style reuse candidates.
///
/// The reuse-window aliasing barrier is answered by
/// [`AliasAnalysisResult::is_barrier_for`], the single alias authority for this
/// pass family.
pub fn analyze(func: &TirFunction, alias: &AliasAnalysisResult) -> Vec<ReuseCandidate> {
    let type_map = type_refine::extract_type_map(func);

    let heap_allocs: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| {
            block.ops.iter().filter_map(|op| {
                if op.opcode == OpCode::Alloc {
                    op.results.first().copied()
                } else {
                    None
                }
            })
        })
        .collect();

    let mut candidates = Vec::new();
    let mut paired_allocs: HashSet<(BlockId, usize)> = HashSet::new();
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        let ops = &block.ops;

        for (decref_idx, op) in ops.iter().enumerate() {
            if op.opcode != OpCode::DecRef {
                continue;
            }
            let decref_val = match op.operands.first() {
                Some(&v) if heap_allocs.contains(&v) => v,
                _ => continue,
            };

            let decref_type = match type_map.get(&decref_val) {
                Some(ty) => ty,
                None => continue,
            };

            for alloc_idx in (decref_idx + 1)..ops.len() {
                let candidate_op = &ops[alloc_idx];

                if alias.is_barrier_for(candidate_op, decref_val) {
                    break;
                }

                if candidate_op.opcode != OpCode::Alloc {
                    continue;
                }

                if paired_allocs.contains(&(bid, alloc_idx)) {
                    continue;
                }

                let alloc_val = match candidate_op.results.first() {
                    Some(&v) => v,
                    None => continue,
                };

                let alloc_type = match type_map.get(&alloc_val) {
                    Some(ty) => ty,
                    None => continue,
                };

                if reuse_compatible(decref_type, alloc_type) {
                    candidates.push(ReuseCandidate {
                        decref_value: decref_val,
                        decref_op_idx: decref_idx,
                        alloc_value: alloc_val,
                        alloc_op_idx: alloc_idx,
                        block_id: bid,
                    });
                    paired_allocs.insert((bid, alloc_idx));
                    break;
                }
            }
        }
    }

    candidates
}
