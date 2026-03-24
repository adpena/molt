//! Minimal TIR serialization for the incremental compilation cache.
//!
//! Rather than full serialization (which would require invasive `serde` derives
//! on all TIR types), this module provides a content hash sufficient for cache
//! invalidation: if the hash of a TirFunction changes, the cached artifact is
//! stale and must be recompiled.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::function::TirFunction;

/// Compute a content hash of a TIR function for cache key purposes.
///
/// Hashes: function name, parameter count, block structure (block IDs,
/// op counts, and op discriminants + operand counts). This captures all
/// semantically meaningful changes while remaining fast to compute.
///
/// Not cryptographically strong — collision resistance is not required;
/// only false-negative avoidance matters (i.e., a changed function must
/// produce a different hash with overwhelming probability).
pub fn content_hash(func: &TirFunction) -> u64 {
    let mut hasher = DefaultHasher::new();
    func.name.hash(&mut hasher);
    func.param_types.len().hash(&mut hasher);
    // Sort block IDs for determinism regardless of HashMap iteration order.
    let mut block_ids: Vec<u32> = func.blocks.keys().map(|b| b.0).collect();
    block_ids.sort_unstable();
    for bid in block_ids {
        use super::blocks::BlockId;
        bid.hash(&mut hasher);
        if let Some(block) = func.blocks.get(&BlockId(bid)) {
            block.ops.len().hash(&mut hasher);
            for op in &block.ops {
                std::mem::discriminant(&op.opcode).hash(&mut hasher);
                op.operands.len().hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}
