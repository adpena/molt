//! Compiled method sites (call_anatomy.md 4.5 "Methods", invariant J3).
//!
//! Every method load gets its site index, and the call that consumes the
//! load's whole pair gets the same index:
//!
//! ```text
//!   %f, %s = LoadMethod %obj              {name m}          -> {method_site k}
//!   %r     = CallLoadedMethod %f, %s, %a                    -> {method_site k}
//! ```
//!
//! The index names the site's record in the program's data; a backend lowers
//! both ops through the shared `molt_codegen_abi::method_site` generators
//! against that record. A call whose pair comes from elsewhere (a
//! zero-argument `super()` load) keeps the runtime call.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

/// Number `func`'s method sites. Requires: the module phase is done and
/// drops are not inserted. Idempotent: an already numbered function keeps
/// its numbers.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "method_sites",
        ..Default::default()
    };
    // `blocks` is a HashMap: visit in id order so the numbers, and the
    // emitted code, are the same in every process.
    let mut block_ids: Vec<_> = func.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    let numbered = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .any(|op| op.opcode == OpCode::LoadMethod && op.method_site().is_some());
    if numbered {
        return stats;
    }
    let mut pairs: HashMap<(ValueId, ValueId), u32> = HashMap::new();
    let mut next = 0u32;
    for &block in &block_ids {
        for op in &mut func.blocks.get_mut(&block).unwrap().ops {
            if op.opcode == OpCode::LoadMethod && op.results.len() == 2 {
                op.set_method_site(Some(next));
                pairs.insert((op.results[0], op.results[1]), next);
                next += 1;
                stats.values_changed += 1;
            }
        }
    }
    for &block in &block_ids {
        for op in &mut func.blocks.get_mut(&block).unwrap().ops {
            if op.opcode == OpCode::CallLoadedMethod
                && op.operands.len() >= 2
                && let Some(&site) = pairs.get(&(op.operands[0], op.operands[1]))
            {
                op.set_method_site(Some(site));
                stats.values_changed += 1;
            }
        }
    }
    stats
}

#[cfg(test)]
#[path = "method_sites_tests.rs"]
mod tests;
