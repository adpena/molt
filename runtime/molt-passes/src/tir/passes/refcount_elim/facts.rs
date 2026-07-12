use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::balance::is_heap_exposing;

/// Build a map: ValueId -> BlockId that defines it.
pub(super) fn build_def_map(func: &TirFunction) -> HashMap<ValueId, BlockId> {
    let mut def_map = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_map.insert(arg.id, bid);
        }
        for op in &block.ops {
            for &result in &op.results {
                def_map.insert(result, bid);
            }
        }
    }
    def_map
}

/// Build the set of ValueIds that have heap exposure.
pub(super) fn build_heap_exposed_set(func: &TirFunction) -> HashSet<ValueId> {
    let mut heap_exposed = HashSet::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            if is_heap_exposing(op.opcode) {
                for &operand in &op.operands {
                    heap_exposed.insert(operand);
                }
            }
        }

        if let Terminator::Return { values } = &block.terminator {
            for &val in values {
                heap_exposed.insert(val);
            }
        }
    }

    heap_exposed
}

pub(super) fn collect_stack_alloc_values(func: &TirFunction) -> HashSet<ValueId> {
    let mut stack_alloc_vals = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::StackAlloc {
                for &result in &op.results {
                    stack_alloc_vals.insert(result);
                }
            }
        }
    }
    stack_alloc_vals
}

pub(super) fn collect_alloc_values(func: &TirFunction) -> HashSet<ValueId> {
    let mut alloc_vals = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Alloc || op.opcode == OpCode::StackAlloc {
                for &result in &op.results {
                    alloc_vals.insert(result);
                }
            }
        }
    }
    alloc_vals
}
