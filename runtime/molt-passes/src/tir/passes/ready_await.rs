//! Ready awaits have no resumption edge. Prove this from the coroutine's
//! authored body and the caller's first-poll value, before terminal ownership
//! splits genuine suspensions into pending and ready exits.

use molt_ir::FunctionId;
use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{
    CfgEdgePolicy, build_pred_map_with, exception_label_to_block, exception_successors,
    reachable_blocks_with,
};
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::op_kinds_generated::{
    copy_kind_mints_owned_alias_ref_table, opcode_is_state_machine_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

#[derive(Clone, Copy)]
enum Definition<'a> {
    Op(&'a TirOp),
    BlockArgument(BlockId, usize),
}

fn ready_operand(func: &TirFunction, value: ValueId, ready_bodies: &HashSet<FunctionId>) -> bool {
    let reachable = reachable_blocks_with(func, CfgEdgePolicy::Full);
    let labels = exception_label_to_block(func);
    let predecessors = build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly);
    let mut definitions = HashMap::default();
    let mut exception_entries = HashSet::default();
    for &block in &reachable {
        let block = &func.blocks[&block];
        exception_entries.extend(exception_successors(block, &labels));
        for (index, arg) in block.args.iter().enumerate() {
            definitions.insert(arg.id, Definition::BlockArgument(block.id, index));
        }
        for op in &block.ops {
            for &result in &op.results {
                definitions.insert(result, Definition::Op(op));
            }
        }
    }

    // Every leaf of the identity flow must be a constructor of a ready
    // coroutine. A loop-carried alias is harmless if it has such a seed:
    // SSA dominance makes the first entry come from outside the cycle.
    let mut work = vec![value];
    let mut visited = HashSet::default();
    let mut found_constructor = false;
    while let Some(value) = work.pop() {
        if !visited.insert(value) {
            continue;
        }
        match definitions.get(&value) {
            Some(Definition::Op(op)) => {
                if op.opcode == OpCode::AllocTask
                    && op.task_kind() == Some(molt_ir::task_kind::TaskKind::Coroutine)
                    && op
                        .callee
                        .is_some_and(|callee| ready_bodies.contains(&callee))
                {
                    found_constructor = true;
                } else if let Some(operand) = super::value_identity::copy_value_source(op) {
                    work.push(operand);
                } else if op.opcode == OpCode::Copy
                    && op.origin.is_some_and(copy_kind_mints_owned_alias_ref_table)
                    && let [operand] = op.operands.as_slice()
                    && op.results.len() == 1
                {
                    work.push(*operand);
                } else {
                    return false;
                }
            }
            Some(&Definition::BlockArgument(block, index))
                if block != func.entry_block && !exception_entries.contains(&block) =>
            {
                let before = work.len();
                let mut missing_argument = false;
                for predecessor in predecessors
                    .get(&block)
                    .into_iter()
                    .flatten()
                    .filter(|id| reachable.contains(id))
                {
                    func.blocks[predecessor]
                        .terminator
                        .for_each_edge(|target, args| {
                            if target == block {
                                match args.get(index) {
                                    Some(value) => work.push(*value),
                                    None => missing_argument = true,
                                }
                            }
                        });
                }
                if missing_argument || before == work.len() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    found_constructor
}

fn ready_body(func: &TirFunction) -> bool {
    func.blocks.values().any(|block| {
        matches!(&block.terminator, Terminator::StateDispatch { cases, .. } if cases.is_empty())
    }) && func.blocks.values().all(|block| {
        !matches!(&block.terminator, Terminator::StateDispatch { cases, .. } if !cases.is_empty())
            && block.ops.iter().all(|op| {
                // A constructor can create a child without suspending this
                // body. Terminal wait remains a distinct exit operation.
                (!opcode_is_state_machine_table(op.opcode) || op.opcode == OpCode::AllocTask)
                    && op.opcode != OpCode::TaskWait
            })
    })
}

#[derive(Clone, Copy)]
struct AwaitSite {
    block: BlockId,
    index: usize,
    operand: ValueId,
    pending: i64,
}

fn specialize(func: &mut TirFunction, ready_bodies: &HashSet<FunctionId>) -> usize {
    // A caller without a local constructor cannot establish this proof.
    if !func.blocks.values().any(|block| {
        block.ops.iter().any(|op| {
            op.opcode == OpCode::AllocTask
                && op
                    .callee
                    .is_some_and(|callee| ready_bodies.contains(&callee))
        })
    }) {
        return 0;
    }
    let mut dispatches = func
        .blocks
        .values()
        .filter(|block| matches!(block.terminator, Terminator::StateDispatch { .. }));
    let Some(dispatch) = dispatches.next().map(|block| block.id) else {
        return 0;
    };
    if dispatches.next().is_some() {
        return 0;
    }
    let constants: HashMap<_, _> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(
            |op| match (op.opcode, op.results.as_slice(), op.attrs.get("value")) {
                (OpCode::ConstInt, [value], Some(AttrValue::Int(state))) => Some((*value, *state)),
                _ => None,
            },
        )
        .collect();
    let mut sites = Vec::new();
    let mut owners = HashMap::<i64, usize>::default();
    for block in func.blocks.values() {
        for (index, op) in block.ops.iter().enumerate() {
            if op.opcode == OpCode::StateTransition {
                // Terminal lowering also requires a constant pending id.
                let [operand, pending] = op.operands.as_slice() else {
                    return 0;
                };
                let Some(&pending) = constants.get(pending) else {
                    return 0;
                };
                *owners.entry(pending).or_default() += 1;
                sites.push(AwaitSite {
                    block: block.id,
                    index,
                    operand: *operand,
                    pending,
                });
            } else if (op.opcode == OpCode::StateYield || op.opcode == OpCode::StateSet)
                && let Some(AttrValue::Int(state)) = op.attrs.get("value")
            {
                *owners.entry(*state).or_default() += 1;
            }
        }
    }
    sites.sort_unstable_by_key(|site| (site.block, std::cmp::Reverse(site.index)));

    let mut changed = 0;
    for site in sites {
        if owners[&site.pending] != 1 {
            continue;
        }
        let Terminator::StateDispatch { cases, .. } =
            &mut func.blocks.get_mut(&dispatch).unwrap().terminator
        else {
            unreachable!();
        };
        let mut matches = cases
            .iter()
            .enumerate()
            .filter(|(_, case)| case.0 == site.pending);
        let Some((index, _)) = matches.next() else {
            continue;
        };
        if matches.next().is_some() {
            continue;
        }
        let resume = cases.remove(index);
        // Ignore only this site's own resume entry. If any remaining path
        // reaches the reload it is still in the proof and causes refusal.
        if ready_operand(func, site.operand, ready_bodies) {
            super::drop_insertion::expose_ready_await(func, site.block, site.index, site.pending);
            changed += 1;
        } else {
            let Terminator::StateDispatch { cases, .. } =
                &mut func.blocks.get_mut(&dispatch).unwrap().terminator
            else {
                unreachable!();
            };
            cases.insert(index, resume);
        }
    }
    if changed != 0 {
        super::dce::run(func);
    }
    changed
}

pub(crate) fn run(module: &mut TirModule, external: &HashSet<String>) -> Vec<String> {
    let ready_bodies: HashSet<_> = module
        .ids
        .iter()
        .copied()
        .zip(&module.functions)
        .filter_map(|(id, func)| {
            (!external.contains(&func.name) && !func.is_codegen_partition() && ready_body(func))
                .then_some(id)
        })
        .collect();
    if ready_bodies.is_empty() {
        return Vec::new();
    }
    module
        .functions
        .iter_mut()
        .filter_map(|func| (specialize(func, &ready_bodies) != 0).then(|| func.name.clone()))
        .collect()
}
