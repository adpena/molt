//! Demand on a parameter's exact type (design 78, section 3.4).
//!
//! Ordinary uses seed demand. Value forwarding, block arguments and resolved
//! calls carry it backward. Refcount operations alone never seed demand.

use std::num::NonZeroUsize;

use molt_ir::FunctionId;
use molt_ir::deterministic_hash::HashMap;

use crate::tir::function::TirModule;
use crate::tir::ops::OpCode;
use crate::tir::passes::value_identity::forwarded_python_value_source;
use crate::tir::values::ValueId;

use super::sites::Site;

type ScopedValue = (FunctionId, ValueId);

/// An adjacency list stored in one arena, without a heap object per value.
#[derive(Default)]
struct DemandGraph {
    heads: HashMap<ScopedValue, Option<NonZeroUsize>>,
    edges: Vec<DemandEdge>,
}

struct DemandEdge {
    source: ScopedValue,
    next: Option<NonZeroUsize>,
}

impl DemandGraph {
    fn forward(&mut self, value: ScopedValue, source: ScopedValue) {
        let head = self.heads.entry(value).or_default();
        self.edges.push(DemandEdge {
            source,
            next: *head,
        });
        *head = NonZeroUsize::new(self.edges.len());
    }

    fn propagate(&mut self, pending: &mut Vec<ScopedValue>) {
        while let Some(value) = pending.pop() {
            let mut next = self.heads.remove(&value).flatten();
            while let Some(edge_id) = next {
                let edge = &self.edges[edge_id.get() - 1];
                pending.push(edge.source);
                next = edge.next;
            }
        }
    }
}

/// Per body (module index), per parameter: whether its type matters.
pub(super) fn solve(
    module: &TirModule,
    sites: &mut [Vec<Site>],
    body_index: &[Option<usize>],
    participates: &[bool],
) -> Vec<Vec<bool>> {
    let mut graph = DemandGraph::default();
    let mut pending = Vec::new();
    for (((&body, func), calls), &participates) in module
        .ids
        .iter()
        .zip(&module.functions)
        .zip(sites.iter())
        .zip(participates)
    {
        if !participates {
            continue;
        }
        // Keep even source-free parameters in the graph so removal below
        // records demand for them without a second value-sized table.
        if let Some(entry) = func.blocks.get(&func.entry_block) {
            for arg in entry.args.iter().take(func.param_names.len()) {
                graph.heads.entry((body, arg.id)).or_default();
            }
        }
        for (&block_id, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                if let Some(source) = forwarded_python_value_source(op) {
                    for &result in &op.results {
                        graph.forward((body, result), (body, source));
                    }
                } else if op.opcode == OpCode::Call
                    && let Ok(call) = calls.binary_search_by_key(&(block_id, index), Site::position)
                {
                    let site = &calls[call];
                    graph.heads.entry((body, site.result)).or_default();
                    let callee_index =
                        body_index[site.callee.index()].expect("a resolved site names a body");
                    let callee = &module.functions[callee_index];
                    if let Some(entry) = callee.blocks.get(&callee.entry_block) {
                        for (position, actual) in site.parameter_arguments() {
                            if let Some(formal) = entry.args.get(position) {
                                graph.forward((site.callee, formal.id), (body, actual));
                            }
                        }
                    }
                } else if !matches!(op.opcode, OpCode::IncRef | OpCode::DecRef) {
                    pending.extend(op.operands.iter().map(|&value| (body, value)));
                }
            }
            block
                .terminator
                .for_each_direct_value(|value| pending.push((body, value)));
            block.terminator.for_each_edge(|target, args| {
                if let Some(successor) = func.blocks.get(&target) {
                    for (actual, formal) in args.iter().zip(&successor.args) {
                        graph.forward((body, formal.id), (body, *actual));
                    }
                }
            });
        }
    }

    // Traverse each requested value once, even when recursion or a loop
    // forwards the value back to itself. A cycle alone creates no demand.
    graph.propagate(&mut pending);
    for (&body, calls) in module.ids.iter().zip(sites) {
        for site in calls {
            site.result_demanded = !graph.heads.contains_key(&(body, site.result));
        }
    }

    module
        .functions
        .iter()
        .zip(module.ids.iter().copied())
        .zip(participates)
        .map(|((func, body), &participates)| {
            let entry_args = func
                .blocks
                .get(&func.entry_block)
                .map(|entry| entry.args.as_slice());
            (0..func.param_names.len())
                .map(|position| {
                    participates
                        && entry_args
                            .and_then(|args| args.get(position))
                            .is_some_and(|arg| !graph.heads.contains_key(&(body, arg.id)))
                })
                .collect()
        })
        .collect()
}
