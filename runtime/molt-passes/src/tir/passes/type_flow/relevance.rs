//! Demand on a parameter's exact type (design 78, section 3.4).
//!
//! Ordinary uses seed demand. Value forwarding, block arguments and resolved
//! calls carry it backward. Refcount operations alone never seed demand.

use molt_ir::FunctionId;

use crate::tir::function::TirModule;
use crate::tir::ops::OpCode;
use crate::tir::passes::value_identity::forwarded_python_value_source;
use crate::tir::value_edges::ValueEdges;
use crate::tir::values::ValueId;

use super::sites::Site;

type ScopedValue = (FunctionId, ValueId);

/// Per body (module index), per parameter: whether its type matters.
pub(super) fn solve(
    module: &TirModule,
    sites: &mut [Vec<Site<'_>>],
    body_index: &[Option<usize>],
    participates: &[bool],
) -> Vec<Vec<bool>> {
    let mut graph = ValueEdges::<ScopedValue, ScopedValue>::default();
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
                graph.add_node((body, arg.id));
            }
        }
        for (&block_id, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                if let Some(source) = forwarded_python_value_source(op) {
                    for &result in &op.results {
                        graph.add((body, result), (body, source));
                    }
                } else if matches!(op.opcode, OpCode::Call | OpCode::CallLoadedMethod)
                    && let Ok(call) = calls.binary_search_by_key(&(block_id, index), Site::position)
                {
                    let site = &calls[call];
                    graph.add_node((body, site.result));
                    let Some(callee_id) = site.fixed_callee() else {
                        // This call remains opaque in a generic context. Keep
                        // its input demand without expanding every possible
                        // callee into the backward graph.
                        pending.extend(op.operands.iter().map(|&value| (body, value)));
                        continue;
                    };
                    let callee_index =
                        body_index[callee_id.index()].expect("a resolved site names a body");
                    let callee = &module.functions[callee_index];
                    if let Some(entry) = callee.blocks.get(&callee.entry_block) {
                        let first = usize::from(callee.has_closure_parameter());
                        for (position, actual) in site.parameter_arguments(first) {
                            if let Some(formal) = entry.args.get(position) {
                                graph.add((callee_id, formal.id), (body, actual));
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
                        graph.add((body, formal.id), (body, *actual));
                    }
                }
            });
        }
    }

    // Traverse each requested value once, even when recursion or a loop
    // forwards the value back to itself. A cycle alone creates no demand.
    while let Some(value) = pending.pop() {
        pending.extend(graph.take_from(&value).copied());
    }
    for (&body, calls) in module.ids.iter().zip(sites) {
        for site in calls {
            site.result_demanded = !graph.contains(&(body, site.result));
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
                            .is_some_and(|arg| !graph.contains(&(body, arg.id)))
                })
                .collect()
        })
        .collect()
}
