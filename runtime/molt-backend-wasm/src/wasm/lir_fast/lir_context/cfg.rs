//! Topology-only adapter to the shared TIR graph analysis authority.
//! Never lowered as code: LIR values and payloads stay in the source function.

use molt_tir::tir::blocks::{Terminator, TirBlock};
use molt_tir::tir::dominators::{self, CfgEdgePolicy};
use molt_tir::tir::function::TirFunction;
use molt_tir::tir::lir::{LirFunction, LirRepr, LirTerminator};
use molt_tir::tir::types::TirType;
use molt_tir::tir::values::ValueId;
use std::collections::HashMap;

pub(super) fn validated_topology(func: &LirFunction) -> TirFunction {
    let fail = |detail: &str| -> ! { panic!("invalid LIR WASM CFG in '{}': {detail}", func.name) };
    if !func.blocks.contains_key(&func.entry_block) {
        fail("missing entry block");
    }
    let mut values = HashMap::new();
    for (&id, block) in &func.blocks {
        if id != block.id {
            fail("block key does not match block identity");
        }
        for value in block
            .args
            .iter()
            .chain(block.ops.iter().flat_map(|op| &op.result_values))
        {
            if values.insert(value.id, value.repr).is_some() {
                fail("duplicate SSA value definition");
            }
        }
    }
    let mut graph = TirFunction::new(
        func.name.clone(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    graph.blocks.clear();
    graph.entry_block = func.entry_block;
    for (&id, block) in &func.blocks {
        let mut targets = Vec::new();
        block.terminator.for_each_edge(|target, args| {
            let Some(destination) = func.blocks.get(&target) else {
                fail("branch target is missing");
            };
            if args.len() != destination.args.len() {
                fail("edge argument arity does not match target block");
            }
            for (&source, destination) in args.iter().zip(&destination.args) {
                let Some(&repr) = values.get(&source) else {
                    fail("edge source value is missing");
                };
                if repr != destination.repr {
                    fail("edge argument representation does not match target block");
                }
            }
            targets.push(target);
        });
        match &block.terminator {
            LirTerminator::CondBranch { cond, .. } => {
                if !values.contains_key(cond) {
                    fail("branch condition value is missing");
                }
            }
            LirTerminator::Switch { value, .. } => {
                if !values.get(value).is_some_and(|repr| {
                    matches!(repr, LirRepr::I64 | LirRepr::DynBox | LirRepr::Ref64)
                }) {
                    fail("switch selector requires an i64 carrier");
                }
            }
            _ => {}
        }
        // The canonical edge visitor determines topology. Synthetic values
        // are irrelevant to explicitly terminator-only graph analyses.
        let terminator = if let Some((&default, cases)) = targets.split_last() {
            Terminator::Switch {
                value: ValueId(0),
                cases: cases
                    .iter()
                    .enumerate()
                    .map(|(index, &target)| (index as i64, target, vec![]))
                    .collect(),
                default,
                default_args: vec![],
            }
        } else {
            Terminator::Return { values: vec![] }
        };
        graph.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator,
            },
        );
    }
    if graph.blocks.len() == 1 {
        return graph;
    }
    let reachable = dominators::reachable_blocks_with(&graph, CfgEdgePolicy::TerminatorOnly);
    graph.blocks.retain(|id, _| reachable.contains(id));
    graph
}
