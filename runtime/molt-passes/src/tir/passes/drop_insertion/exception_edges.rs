//! Exact exceptional release edges after ordinary lifetime placement.
//!
//! Availability proves each released root's definition and initialization at its
//! own observation. Repeated suffixes of those exact SSA roots share cleanup
//! tails; every edge entering a tail proves availability of every captured root
//! it will release. Unshared prefixes remain parameterized and alpha-equivalent
//! prefixes share code without capturing another edge's values. Handler arguments
//! are forwarded explicitly and retained, with their multiplicity, before any
//! release. No region registration enters an observation's cleanup tail.

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::LabelAllocator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::alias_analysis::AliasUnionFind;
use crate::tir::passes::liveness::visit_exception_liveness;
use crate::tir::passes::ownership_lattice_min::DropEligibility;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::availability::DefinitionAvailability;
use super::util::make_op;

#[derive(Clone, PartialEq, Eq, Hash)]
enum TailKey {
    Handler {
        target: BlockId,
        types: Vec<TirType>,
    },
    Release {
        value: ValueId,
        next: usize,
    },
}

struct CleanupTail {
    key: TailKey,
    observations: usize,
}

struct MaterializedTail {
    block: BlockId,
    label: Option<i64>,
    types: Vec<TirType>,
}

/// Unique prefixes are parameterized. Their continuation may already release a
/// shared exact-value suffix. Incoming payloads never capture another edge's SSA.
#[derive(PartialEq, Eq, Hash)]
struct LandingSignature {
    target: BlockId,
    forwarded: usize,
    parameter_types: Vec<TirType>,
    retained: Vec<usize>,
}

fn parameters(func: &mut TirFunction, types: &[TirType]) -> Vec<TirValue> {
    types
        .iter()
        .map(|ty| {
            let id = func.fresh_value();
            func.value_types.insert(id, ty.clone());
            TirValue { id, ty: ty.clone() }
        })
        .collect()
}

fn intern_tail(
    pool: &mut Vec<CleanupTail>,
    identities: &mut HashMap<TailKey, usize>,
    key: TailKey,
) -> usize {
    let index = *identities.entry(key.clone()).or_insert_with(|| {
        let index = pool.len();
        pool.push(CleanupTail {
            key,
            observations: 0,
        });
        index
    });
    pool[index].observations += 1;
    index
}

fn release_prefix<'a>(
    tails: &[CleanupTail],
    materialized: &'a [Option<MaterializedTail>],
    mut tail: usize,
) -> (Vec<ValueId>, &'a MaterializedTail) {
    let mut releases = Vec::new();
    loop {
        if let Some(continuation) = &materialized[tail] {
            return (releases, continuation);
        }
        let TailKey::Release { value, next } = tails[tail].key else {
            unreachable!("every handler continuation is materialized");
        };
        releases.push(value);
        tail = next;
    }
}

pub(super) fn insert_exception_edge_releases(
    func: &mut TirFunction,
    eligibility: &DropEligibility<'_>,
    definitions: &DefinitionAvailability<'_>,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
    mut retains: HashMap<(BlockId, usize), Vec<usize>>,
) -> usize {
    let mut plans = Vec::new();
    let mut tails = Vec::new();
    let mut tail_identities = HashMap::default();
    visit_exception_liveness(func, aliases, raw, |block, index, target, demand| {
        let op = &func.blocks[&block].ops[index];
        let retained = retains.remove(&(block, index)).unwrap_or_default();
        let original_index =
            definitions.transfer_position_from_end(block, demand.transfer_from_end);
        let mut releases: Vec<_> = demand
            .abandoned()
            .filter(|&root| {
                eligibility.is_droppable(root)
                    && definitions.available_before(root, block, original_index)
            })
            .collect();
        releases.sort_unstable_by_key(|value| std::cmp::Reverse(value.0));
        if releases.is_empty() && retained.is_empty() {
            return;
        }
        let types = op
            .operands
            .iter()
            .map(|value| {
                func.value_types
                    .get(value)
                    .cloned()
                    .expect("exception handler payload must have a representation")
            })
            .collect();
        let mut tail = intern_tail(
            &mut tails,
            &mut tail_identities,
            TailKey::Handler { target, types },
        );
        // Intern oldest first: each node points to an already allocated suffix.
        // The identity includes the exact value and complete continuation,
        // so it cannot reorder releases or merge distinct handler routes.
        for &value in releases.iter().rev() {
            tail = intern_tail(
                &mut tails,
                &mut tail_identities,
                TailKey::Release { value, next: tail },
            );
        }
        plans.push((block, index, op.operands.clone(), retained, tail));
    });
    assert!(
        retains.is_empty(),
        "DropInsertion planned landing retains for an observation it did not visit"
    );
    if plans.is_empty() {
        return 0;
    }

    // An observation or a second distinct predecessor needs an entry. A
    // shared tail's other nodes form straight-line chains. Since each such
    // node has one incoming edge, materialization walks its releases once.
    let mut entries = vec![false; tails.len()];
    let mut has_predecessor = vec![false; tails.len()];
    for &(_, _, _, _, tail) in &plans {
        entries[tail] = true;
    }
    for tail in &tails {
        if let TailKey::Release { next, .. } = tail.key {
            entries[next] |= std::mem::replace(&mut has_predecessor[next], true);
        }
    }

    let mut labels = LabelAllocator::for_function(func);
    let mut inserted = 0;
    let mut materialized: Vec<Option<MaterializedTail>> = (0..tails.len()).map(|_| None).collect();
    // Topological allocation avoids recursion, even for very deep construction
    // prefixes. A shared suffix's continuation is at least as widely referenced.
    for (index, tail) in tails.iter().enumerate() {
        match &tail.key {
            TailKey::Handler { target, types } => {
                materialized[index] = Some(MaterializedTail {
                    block: *target,
                    label: None,
                    types: types.clone(),
                });
            }
            TailKey::Release { .. } if tail.observations > 1 && entries[index] => {
                let (releases, continuation) = release_prefix(&tails, &materialized, index);
                let target = continuation.block;
                let types = continuation.types.clone();
                let args = parameters(func, &types);
                let forwarded = args.iter().map(|value| value.id).collect();
                inserted += releases.len();
                let block = func.fresh_block();
                let label = labels.fresh();
                func.label_id_map.insert(block.0, label);
                func.blocks.insert(
                    block,
                    TirBlock {
                        id: block,
                        args,
                        ops: releases
                            .into_iter()
                            .map(|value| make_op(OpCode::DecRef, vec![value]))
                            .collect(),
                        terminator: Terminator::Branch {
                            target,
                            args: forwarded,
                        },
                    },
                );
                materialized[index] = Some(MaterializedTail {
                    block,
                    label: Some(label),
                    types,
                });
            }
            TailKey::Release { .. } => {}
        }
    }

    let mut landings: HashMap<LandingSignature, i64> = HashMap::default();
    for (source, index, args, retained, tail) in plans {
        let (releases, continuation) = release_prefix(&tails, &materialized, tail);
        if releases.is_empty() && retained.is_empty() {
            // Only handler arguments cross this edge. Availability established
            // each captured release at every observation reaching this suffix.
            let label = continuation.label.expect("a cleanup plan releases a value");
            func.blocks.get_mut(&source).unwrap().ops[index]
                .attrs
                .insert("value".into(), AttrValue::Int(label));
            continue;
        }
        let payload: Vec<_> = args.iter().chain(&releases).copied().collect();
        let key = LandingSignature {
            target: continuation.block,
            forwarded: args.len(),
            parameter_types: payload
                .iter()
                .map(|value| {
                    func.value_types
                        .get(value)
                        .cloned()
                        .expect("exception cleanup payload must have a representation")
                })
                .collect(),
            retained: retained.clone(),
        };
        let label = if let Some(&existing) = landings.get(&key) {
            existing
        } else {
            let block = func.fresh_block();
            let label = labels.fresh();
            func.label_id_map.insert(block.0, label);
            let parameters = parameters(func, &key.parameter_types);
            let forwarded: Vec<_> = parameters[..args.len()]
                .iter()
                .map(|value| value.id)
                .collect();
            let ops = retained
                .iter()
                .map(|&position| make_op(OpCode::IncRef, vec![forwarded[position]]))
                .chain(
                    parameters[args.len()..]
                        .iter()
                        .map(|value| make_op(OpCode::DecRef, vec![value.id])),
                )
                .collect();
            inserted += retained.len() + releases.len();
            func.blocks.insert(
                block,
                TirBlock {
                    id: block,
                    args: parameters,
                    ops,
                    terminator: Terminator::Branch {
                        target: key.target,
                        args: forwarded,
                    },
                },
            );
            landings.insert(key, label);
            label
        };
        let check = &mut func.blocks.get_mut(&source).unwrap().ops[index];
        check.attrs.insert("value".into(), AttrValue::Int(label));
        check.operands = payload;
    }
    inserted
}
