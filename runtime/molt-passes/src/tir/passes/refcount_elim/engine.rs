use crate::tir::analysis::{AnalysisManager, DomChildren, PredMap};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_proves_immediate_on_true;
use crate::tir::passes::alias_analysis::AliasAnalysis;

use super::super::PassStats;
use super::balance::refcount_balance_role;
use super::cross_block::eliminate_cross_block_pairs;
use super::facts::{collect_guarded_rc_sites, collect_rc_inert_values};
use super::local::eliminate_local_pairs;
use crate::tir::passes::value_identity::ScalarFacts;
use crate::tir::passes::value_range::{ValueRange, compute_in_with_scalar_facts};

/// All pipelines preserve heap destruction and use the same pairing contract.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "refcount_elim",
        ..Default::default()
    };
    // Most pre-drop bodies do not yet contain a retain or release. Their
    // ownership analyses have no consumer in this pass.
    let mut has_rc = false;
    let mut has_tag_test = false;
    for op in func.blocks.values().flat_map(|block| &block.ops) {
        has_rc |= refcount_balance_role(op.opcode).is_refcount_balance();
        has_tag_test |= opcode_proves_immediate_on_true(op.opcode);
        if has_rc && has_tag_test {
            break;
        }
    }
    if !has_rc {
        return stats;
    }

    am.get::<AliasAnalysis>(func);
    if am.cached::<ValueRange>().is_none() {
        let types = am
            .cached::<AliasAnalysis>()
            .expect("prepared alias facts")
            .share_scalar_types();
        let facts = ScalarFacts::with_types(func, &types);
        let ranges = compute_in_with_scalar_facts(&facts, am);
        am.prepopulate::<ValueRange>(ranges);
    }
    if has_tag_test {
        am.get::<DomChildren>(func);
    } else if func.blocks.len() > 1 {
        am.get::<PredMap>(func);
    }
    let alias = am.cached::<AliasAnalysis>().expect("prepared alias facts");
    let ranges = am.cached::<ValueRange>().expect("prepared value ranges");
    let inert_values = collect_rc_inert_values(func, alias, ranges);
    let guarded_sites = if has_tag_test {
        collect_guarded_rc_sites(func, am, alias)
    } else {
        Default::default()
    };
    eliminate_local_pairs(func, alias, &inert_values, &guarded_sites, &mut stats);
    eliminate_cross_block_pairs(func, am, alias, &mut stats);
    stats
}

/// Drop-inserted pipelines share the same release-preserving implementation.
pub fn run_post_drop(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run(func, am)
}
