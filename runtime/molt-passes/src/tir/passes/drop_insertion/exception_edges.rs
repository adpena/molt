//! Exact exceptional release edges after ordinary lifetime placement.
//!
//! Availability proves each released root's definition and initialization at its
//! own observation. Repeated suffixes of those exact SSA roots share cleanup
//! tails; every edge entering a tail proves availability of every captured root
//! it will release. Unshared prefixes remain parameterized and alpha-equivalent
//! prefixes share code without capturing another edge's values. Handler arguments
//! are forwarded explicitly and retained, with their multiplicity, before any
//! release. No region registration enters an observation's cleanup tail.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::LabelAllocator;
use crate::tir::dominators::exception_edge_binds_handler_arguments;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::alias_analysis::AliasUnionFind;
use crate::tir::passes::liveness::visit_exception_liveness;
use crate::tir::passes::ownership_lattice_min::{DropEligibility, OwnershipRootFacts};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::availability::PointAvailability;
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

pub(super) fn insert_exception_edge_releases(
    func: &mut TirFunction,
    eligibility: &DropEligibility<'_>,
    root_facts: &OwnershipRootFacts,
    aliases: &AliasUnionFind,
    raw: &HashSet<ValueId>,
    retains: &HashMap<(BlockId, usize), Vec<usize>>,
) -> usize {
    let points = PointAvailability::compute(func, root_facts);
    let mut plans = Vec::new();
    let mut tails = Vec::new();
    let mut tail_identities = HashMap::new();
    let mut retaining_observations = 0;
    visit_exception_liveness(
        func,
        aliases,
        raw,
        |block, index, target, normal, exceptional| {
            let op = &func.blocks[&block].ops[index];
            if !exception_edge_binds_handler_arguments(op.opcode) {
                return;
            }
            let retained = retains.get(&(block, index)).cloned().unwrap_or_default();
            if !retained.is_empty() {
                retaining_observations += 1;
            }
            let mut releases: Vec<_> = normal
                .difference(exceptional)
                .copied()
                .filter(|&root| {
                    eligibility.is_droppable(root) && points.available_before(root, block, index)
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
        },
    );
    assert_eq!(
        retaining_observations,
        retains.len(),
        "DropInsertion planned landing retains for an observation it did not visit"
    );

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
            TailKey::Release { value, next } if tail.observations > 1 => {
                let continuation = materialized[*next]
                    .as_ref()
                    .expect("shared release suffix must have a materialized continuation");
                let target = continuation.block;
                let types = continuation.types.clone();
                let args = parameters(func, &types);
                let forwarded = args.iter().map(|value| value.id).collect();
                let block = func.fresh_block();
                let label = labels.fresh();
                func.label_id_map.insert(block.0, label);
                func.blocks.insert(
                    block,
                    TirBlock {
                        id: block,
                        args,
                        ops: vec![make_op(OpCode::DecRef, vec![*value])],
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
                inserted += 1;
            }
            TailKey::Release { .. } => {}
        }
    }

    let mut landings: HashMap<LandingSignature, i64> = HashMap::new();
    for (source, index, args, retained, mut tail) in plans {
        let mut releases = Vec::new();
        while materialized[tail].is_none() {
            let TailKey::Release { value, next } = tails[tail].key else {
                unreachable!("every handler continuation is materialized");
            };
            releases.push(value);
            tail = next;
        }
        let continuation = materialized[tail].as_ref().unwrap();
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
