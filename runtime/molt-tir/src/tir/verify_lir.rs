//! Representation-aware LIR verifier.
//!
//! This checker is intentionally narrow in the first Task 1 slice. It proves
//! the core invariants required before any backend starts consuming LIR:
//! - entry block exists;
//! - every branch passes the right number of arguments;
//! - branch arguments match the target block parameters in semantic type and
//!   low-level representation;
//! - conditional branches consume `Bool1`;
//! - return values match the declared function return arity and a valid
//!   representation for the declared semantic type.
//!
//! The verifier is split move-only into cohesive submodules:
//! - [`signature`]: entry-block signature and type/repr acceptance;
//! - [`value_table`]: value-definition table and `Ref64` provenance;
//! - [`dom`]: dominator-tree construction over LIR control flow;
//! - [`ops`]: per-op surface and representation-rule checks;
//! - [`terminators`]: terminator, branch-argument, and use-dominance checks.

use std::collections::HashMap;

use super::blocks::BlockId;
use super::lir::{LirFunction, LirValue};

mod dom;
mod ops;
mod signature;
mod terminators;
mod value_table;

use dom::compute_dominator_tree;
use ops::verify_ops;
use signature::verify_entry_block_signature;
use terminators::verify_terminators;
use value_table::{build_value_table, verify_ref64_provenance};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LirVerifyError {
    pub block: Option<BlockId>,
    pub op_index: Option<usize>,
    pub message: String,
}

impl LirVerifyError {
    pub(super) fn func(message: impl Into<String>) -> Self {
        Self {
            block: None,
            op_index: None,
            message: message.into(),
        }
    }

    pub(super) fn block(block: BlockId, message: impl Into<String>) -> Self {
        Self {
            block: Some(block),
            op_index: None,
            message: message.into(),
        }
    }
}

pub fn verify_lir_function(func: &LirFunction) -> Result<(), Vec<LirVerifyError>> {
    let mut errors = Vec::new();
    if !func.blocks.contains_key(&func.entry_block) {
        errors.push(LirVerifyError::func(format!(
            "entry block ^{} does not exist in blocks map",
            func.entry_block
        )));
        return Err(errors);
    }

    verify_entry_block_signature(func, &mut errors);
    let values = build_value_table(func, &mut errors);
    verify_ref64_provenance(func, &mut errors);
    let dominators = compute_dominator_tree(func);
    verify_ops(func, &values, &dominators, &mut errors);
    verify_terminators(func, &values, &dominators, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[derive(Debug, Clone)]
pub(super) struct ValueDef {
    pub(super) value: LirValue,
    pub(super) block: BlockId,
    pub(super) op_index: Option<usize>,
}

#[derive(Debug, Default)]
pub(super) struct DominatorInfo {
    pub(super) preorder: HashMap<BlockId, usize>,
    pub(super) postorder: HashMap<BlockId, usize>,
}

impl DominatorInfo {
    pub(super) fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if a == b {
            return true;
        }
        match (
            self.preorder.get(&a),
            self.preorder.get(&b),
            self.postorder.get(&a),
            self.postorder.get(&b),
        ) {
            (Some(&a_pre), Some(&b_pre), Some(&a_post), Some(&b_post)) => {
                a_pre <= b_pre && b_post <= a_post
            }
            _ => false,
        }
    }
}
