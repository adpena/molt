use crate::tir::function::TirFunction;
use crate::tir::ops::AttrValue;

use super::super::PassStats;
use super::scan::ReuseCandidate;

/// Annotate `DecRef` and `Alloc` ops with reuse candidate metadata.
///
/// Paired `DecRef` ops carry a `reuse_token_id` attribute and paired `Alloc`
/// ops carry a matching `reuse_from_token` attribute. Downstream lowering uses
/// these annotations to emit conditional reuse tokens.
pub fn annotate(func: &mut TirFunction, candidates: &[ReuseCandidate]) -> PassStats {
    let mut stats = PassStats {
        name: "reuse_analysis",
        values_changed: 0,
        attrs_changed: 0,
        ops_removed: 0,
        ops_added: 0,
        facts_changed: 0,
    };

    for (token_id, candidate) in candidates.iter().enumerate() {
        let block = func.blocks.get_mut(&candidate.block_id).unwrap();

        block.ops[candidate.decref_op_idx]
            .attrs
            .insert("reuse_token_id".into(), AttrValue::Int(token_id as i64));

        block.ops[candidate.alloc_op_idx]
            .attrs
            .insert("reuse_from_token".into(), AttrValue::Int(token_id as i64));

        stats.values_changed += 1;
    }

    stats
}
