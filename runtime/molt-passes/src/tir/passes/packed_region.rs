//! Give the non-reentrant path of a loop one bounded view per packed owner.
//! A cold edge leaves the cloned region at its original continuation. It may
//! run Python, replace the owner's backing, or finish an already-committed
//! store. Only the entry can acquire a new view.

use std::collections::BTreeMap;

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::frame_entry::FramePrefix;
use molt_ir::repr::Repr;

use crate::representation_facts::repr_by_value_from;
use crate::tir::analysis::{Analysis, LoopForest};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::{
    DominatorTree, build_pred_map, compute_dominator_tree, exception_label_to_block,
    terminator_successors,
};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::ssa_update::{self, Variable};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::alias_analysis::AliasUnionFind;
use super::effects::op_effects_with_types;
use super::region_clone::RegionClone;
use super::value_identity::ScalarFacts;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Source {
    owner: ValueId,
    field: Option<i64>,
}

#[derive(Clone, Copy)]
struct Access {
    block: BlockId,
    source: Source,
    field_read: Option<(BlockId, ValueId)>,
    read: bool,
}

struct View {
    backing: ValueId,
    data: ValueId,
    stride: ValueId,
}

struct Plan {
    preheader: BlockId,
    header: BlockId,
    blocks: Vec<BlockId>,
    bounds: BTreeMap<Source, [i64; 2]>,
    accesses: Vec<Access>,
    callable_guards: Vec<BlockId>,
    checked_arithmetic: Vec<(BlockId, [bool; 2])>,
}

fn packed_access(op: &TirOp) -> bool {
    matches!(
        op.opcode,
        OpCode::TryBuffer2dLoadI64 | OpCode::TryBuffer2dStoreI64
    )
}

pub(crate) fn run(func: &mut TirFunction) -> bool {
    if func.execution_context != molt_ir::ExecutionContextPolicy::Local
        || func.has_state_machine()
        || func.frame_locals_exposed()
        || !func
            .blocks
            .values()
            .any(|b| b.ops.iter().any(packed_access))
    {
        return false;
    }
    let trace = crate::tir::pass_manager::trace_func_enabled(&func.name);
    let containing = LoopForest::compute(func);
    let poll_blocks: HashSet<_> = containing
        .bodies
        .values()
        .filter(|body| {
            body.iter()
                .any(|id| func.blocks[id].ops.iter().any(packed_access))
        })
        .flat_map(|body| body.iter().copied())
        .collect();
    if poll_blocks.is_empty() {
        return false;
    }
    let mut candidate = func.clone();
    // Separate the original service edge from the check of an already pending
    // exception. The service arm stays in the original CFG, even if it returns.
    for (&block, source) in &func.blocks {
        if !poll_blocks.contains(&block) {
            continue;
        }
        for (index, op) in source.ops.iter().enumerate().rev() {
            if op.opcode == OpCode::CheckException
                && op.is_async_work_poll()
                && !op.has_checked_invocation_binding()
            {
                let mut observe = op.clone();
                observe.clear_async_work_poll();
                super::async_work_poll::split_pending_work(
                    &mut candidate,
                    block,
                    index,
                    vec![op.clone()],
                    vec![observe],
                );
            }
        }
    }
    let Some(rewritten) = select(&candidate, trace, |plan| {
        if trace {
            eprintln!(
                "[packed-region] {} {}: {} blocks, {} owners, {} accesses",
                func.name,
                plan.header,
                plan.blocks.len(),
                plan.bounds.len(),
                plan.accesses.len()
            );
        }
        let mut rewritten = candidate.clone();
        if !apply(&mut rewritten, &candidate, plan) {
            return None;
        }
        if let Err(errors) = crate::tir::verify::verify_function(&rewritten) {
            if trace {
                eprintln!(
                    "[packed-region] {}: invalid SSA: {:?}",
                    func.name,
                    errors.first()
                );
            }
            return None;
        }
        if let Err(errors) = crate::tir::exception_regions::verify_exception_regions(&rewritten) {
            if trace {
                eprintln!(
                    "[packed-region] {}: invalid exception region: {:?}",
                    func.name,
                    errors.first()
                );
            }
            return None;
        }
        Some(rewritten)
    }) else {
        return false;
    };
    *func = rewritten;
    true
}

fn stable_op(
    op: &TirOp,
    scalars: &HashMap<ValueId, TirType>,
    carriers: &HashMap<ValueId, Repr>,
    safe_release: bool,
) -> bool {
    use OpCode::*;
    use molt_ir::OpKind;
    let repr = |id: &ValueId| carriers.get(id).copied().unwrap_or(Repr::DynBox);
    match op.opcode {
        AsyncWorkPending
        | FrameEntryAdmits
        | FrameSetLocation
        | ProbeInlineField
        | StaticCellLoad
        | StringLiteralCellReady
        | TryBuffer2dLoadI64
        | TryBuffer2dStoreI64 => true,
        CheckException => !op.is_async_work_poll() && !op.has_checked_invocation_binding(),
        Copy if op.origin == Some(OpKind::Line) => true,
        Copy if op.origin == Some(OpKind::BindingAlias) => {
            // Retaining a boxed word cannot finalize or allocate. A raw
            // integer may have to box if its alias leaves the raw carrier.
            let [input] = op.operands.as_slice() else {
                return false;
            };
            !repr(input).is_raw_i64_full_deopt()
                || op.results.iter().all(|id| repr(id).is_raw_i64_carrier())
        }
        // Entry excludes programs that can overwrite homes through a frame
        // proxy. A proven finalizer-free displaced binding cannot reenter.
        // Check the same allocation boundary that frame placement uses:
        // an existing boxed word is already materialized.
        Copy if op.origin == Some(OpKind::FrameHomeStore) && safe_release => {
            !super::frame_home_placement::boxes_heap_int(op, carriers)
        }
        Copy if safe_release => op.operands.iter().all(|v| repr(v).boxed_refcount_is_noop()),
        IncRef | DecRef => op.operands.iter().all(|v| scalars.contains_key(v)),
        _ => {
            let effects = op_effects_with_types(op, scalars);
            effects.effect_free && effects.nothrow && !effects.may_call_python
        }
    }
}

fn preserves_carriers(
    func: &TirFunction,
    carriers: &HashMap<ValueId, Repr>,
    target: BlockId,
    args: &[ValueId],
) -> bool {
    let carrier = |id: ValueId| {
        carriers
            .get(&id)
            .copied()
            .unwrap_or(Repr::DynBox)
            .carrier_type(&TirType::DynBox)
    };
    func.blocks[&target]
        .args
        .iter()
        .zip(args)
        .all(|(dest, &value)| carrier(dest.id) == carrier(value))
}

fn admits_edges(block: &TirBlock, func: &TirFunction, carriers: &HashMap<ValueId, Repr>) -> bool {
    if let Terminator::CondBranch { cond, .. } = block.terminator
        && carriers.get(&cond) != Some(&Repr::Bool)
    {
        return false;
    }
    let mut compatible = true;
    block.terminator.for_each_edge(|target, args| {
        compatible &= preserves_carriers(func, carriers, target, args);
    });
    compatible
}

fn select<T>(
    func: &TirFunction,
    trace: bool,
    mut attempt: impl FnMut(Plan) -> Option<T>,
) -> Option<T> {
    let facts = ScalarFacts::new(func);
    let loops = facts.loops();
    if loops.headers.is_empty() {
        return None;
    }
    let ranges = super::value_range::compute_with_scalar_facts(&facts);
    let carriers = repr_by_value_from(&facts, Some(&ranges));
    let preds = build_pred_map(func);
    let dom = compute_dominator_tree(func, &preds);
    let labels = exception_label_to_block(func);
    let order = crate::tir::dominators::executable_reverse_postorder(func);
    let reachable = order.iter().copied().collect();
    let releases =
        super::frame_location_placement::binding_releases(func, facts.types(), &labels, &order);
    let aliases = super::alias_analysis::build_alias_union_find(func);
    let raw = carriers
        .iter()
        .filter_map(|(&id, &repr)| repr.carrier_refcount_is_noop().then_some(id))
        .collect();
    let live = super::liveness::compute_liveness_in_domain(func, &aliases, &raw);
    let mut defs = HashMap::default();
    let mut stable = HashSet::default();
    let mut refusals = HashMap::default();
    let exceptional: HashSet<_> = func
        .blocks
        .values()
        .flat_map(|b| &b.ops)
        .filter_map(|op| crate::tir::dominators::exception_transfer_target(op, &labels))
        .collect();
    for (&id, block) in &func.blocks {
        for arg in &block.args {
            defs.insert(arg.id, (id, None));
        }
        for op in &block.ops {
            for &result in &op.results {
                defs.insert(result, (id, Some(op)));
            }
        }
        let blocked = block.ops.iter().enumerate().find(|(index, op)| {
            !stable_op(
                op,
                facts.types(),
                &carriers,
                releases.get(&(id, *index)).copied().unwrap_or(false),
            )
        });
        if blocked.is_none() && !exceptional.contains(&id) && admits_edges(block, func, &carriers) {
            stable.insert(id);
        } else if trace {
            if let Some((at, op)) = blocked
                && op.origin == Some(molt_ir::OpKind::FrameHomeStore)
            {
                let reprs = |values: &[ValueId]| {
                    values
                        .iter()
                        .map(|id| (*id, carriers.get(id), facts.types().get(id)))
                        .collect::<Vec<_>>()
                };
                eprintln!(
                    "[packed-region] {id} home {:?}: release={:?} in={:?} out={:?}",
                    super::frame_location_placement::home_slot(op),
                    releases.get(&(id, at)),
                    reprs(&op.operands),
                    reprs(&op.results)
                );
            }
            refusals.insert(id, blocked.map(|(_, op)| (op.opcode, op.origin)));
        }
    }
    // A cloned fallback can contain more blocks than the specialization it
    // protects. Prefer regions behind the successful exact-class check, then
    // prefer coverage. This ordering never licenses the rewrite itself.
    let preferred = exact_class_successors(func, &defs, &aliases, &preds);
    let mut scores = HashMap::default();
    if !preferred.is_empty() {
        let children = crate::tir::dominators::build_dom_children(&dom);
        for id in crate::tir::dominators::dom_tree_preorder(func.entry_block, &children) {
            let inherited = dom
                .idom(id)
                .and_then(|parent| scores.get(&parent).copied())
                .unwrap_or(0);
            scores.insert(id, inherited + usize::from(preferred.contains(&id)));
        }
    }
    let mut headers = loops.headers.clone();
    headers.sort_by_key(|id| {
        (
            std::cmp::Reverse(scores.get(id).copied().unwrap_or(0)),
            std::cmp::Reverse(loops.bodies[id].len()),
            id.0,
        )
    });
    let mut clone_budget = 512;
    for header in headers {
        let natural = &loops.bodies[&header];
        if !natural
            .iter()
            .any(|id| func.blocks[id].ops.iter().any(packed_access))
        {
            continue;
        }
        let Some(preheader) = super::checked_int_loops::plain_preheader(
            func, header, natural, &preds, &reachable, &labels,
        ) else {
            if trace {
                eprintln!("[packed-region] {header}: no plain preheader");
            }
            continue;
        };
        let mut reached = HashSet::default();
        let mut pending = vec![header];
        let mut cuts = Vec::new();
        while let Some(id) = pending.pop() {
            if natural.contains(&id) && stable.contains(&id) && reached.insert(id) {
                pending.extend(terminator_successors(&func.blocks[&id].terminator));
            } else if trace && natural.contains(&id) && !stable.contains(&id) {
                cuts.push((id, refusals.get(&id).copied().flatten()));
            }
        }
        // Keep only the reachable blocks that can continue around this loop.
        // Blocks off that cyclic path stay as shared side exits.
        let mut cyclic = HashSet::default();
        pending.push(header);
        while let Some(id) = pending.pop() {
            if reached.contains(&id) && cyclic.insert(id) {
                pending.extend(preds[&id].iter().copied().filter(|pred| {
                    terminator_successors(&func.blocks[pred].terminator).contains(&id)
                }));
            }
        }
        if trace {
            cuts.sort_unstable_by_key(|(id, _)| *id);
            cuts.dedup();
            eprintln!(
                "[packed-region] {header}: {} stable blocks, {} cyclic, cuts {cuts:?}",
                reached.len(),
                cyclic.len()
            );
        }
        if !preds[&header].iter().any(|id| {
            cyclic.contains(id)
                && terminator_successors(&func.blocks[id].terminator).contains(&header)
        }) || cyclic.len() > clone_budget
        {
            continue;
        }
        let mut blocks: Vec<_> = cyclic.iter().copied().collect();
        blocks.sort_unstable();
        let mut bounds = BTreeMap::<Source, [i64; 2]>::new();
        let mut accesses = Vec::new();
        for &block in &blocks {
            let source = &func.blocks[&block];
            let Some(op) = source.ops.last().filter(|op| packed_access(op)) else {
                continue;
            };
            let Terminator::CondBranch {
                cond, then_block, ..
            } = source.terminator
            else {
                continue;
            };
            if op.results.last() != Some(&cond) || !cyclic.contains(&then_block) {
                if trace {
                    eprintln!("[packed-region] {block}: probe hit leaves the cycle");
                }
                continue;
            }
            let axis = [op.operands[1], op.operands[2]].map(|v| ranges.range_at(block, v));
            if axis.iter().any(|r| r.lo < 0 || r.hi == i64::MAX) {
                if trace {
                    eprintln!("[packed-region] {block}: unknown axis {axis:?}");
                }
                continue;
            }
            let Some((source, field_read)) =
                view_source(op, &aliases, &defs, &dom, preheader, natural)
            else {
                if trace {
                    eprintln!("[packed-region] {block}: variant owner");
                }
                continue;
            };
            if !live.is_live_out(preheader, source.owner) {
                if trace {
                    eprintln!(
                        "[packed-region] {block}: owner {:?} not held at {preheader}",
                        source.owner
                    );
                }
                continue;
            }
            let extent = bounds.entry(source).or_default();
            for (current, range) in extent.iter_mut().zip(axis) {
                *current = (*current).max(range.hi);
            }
            accesses.push(Access {
                block,
                source,
                field_read,
                read: op.opcode == OpCode::TryBuffer2dLoadI64,
            });
        }
        if !accesses.is_empty() {
            let callable_guards = blocks
                .iter()
                .copied()
                .filter(|block| {
                    let block = &func.blocks[block];
                    let [op] = block.ops.as_slice() else {
                        return false;
                    };
                    let Terminator::CondBranch {
                        cond, then_block, ..
                    } = &block.terminator
                    else {
                        return false;
                    };
                    let [callable] = op.operands.as_slice() else {
                        return false;
                    };
                    op.opcode == OpCode::FrameEntryAdmits
                        && op.results.as_slice() == [*cond]
                        && cyclic.contains(then_block)
                        && matches!(op.attrs.get("s_value"), Some(AttrValue::Str(_)))
                        && FramePrefix::reservation_from_attrs(&op.attrs).is_some()
                        && !FramePrefix::region_callable_admitted(&op.attrs)
                        && defs.get(callable).is_some_and(|(definition, _)| {
                            !natural.contains(definition) && dom.dominates(*definition, preheader)
                        })
                        && live.is_live_out(preheader, *callable)
                })
                .collect();
            let checked_arithmetic = blocks
                .iter()
                .filter_map(|&block| {
                    let [op] = func.blocks[&block].ops.as_slice() else {
                        return None;
                    };
                    if !matches!(op.opcode, OpCode::CheckedAdd | OpCode::CheckedMul) {
                        return None;
                    }
                    let ([lhs, rhs], [_, overflow]) =
                        (op.operands.as_slice(), op.results.as_slice())
                    else {
                        return None;
                    };
                    let Terminator::CondBranch { cond, .. } = func.blocks[&block].terminator else {
                        return None;
                    };
                    if cond != *overflow {
                        return None;
                    }
                    let needs_guard = [*lhs, *rhs].map(|value| {
                        !carriers
                            .get(&value)
                            .is_some_and(|repr| repr.is_raw_i64_carrier())
                    });
                    needs_guard.contains(&true).then_some((block, needs_guard))
                })
                .collect();
            clone_budget -= blocks.len();
            if let Some(out) = attempt(Plan {
                preheader,
                header,
                blocks,
                bounds,
                accesses,
                callable_guards,
                checked_arithmetic,
            }) {
                return Some(out);
            }
        }
    }
    None
}

fn exact_class_successors(
    func: &TirFunction,
    defs: &HashMap<ValueId, (BlockId, Option<&TirOp>)>,
    aliases: &AliasUnionFind,
    preds: &HashMap<BlockId, Vec<BlockId>>,
) -> HashSet<BlockId> {
    let defining_op = |v| defs.get(&aliases.root(v)).and_then(|(_, op)| *op);
    func.blocks
        .iter()
        .filter_map(|(&id, block)| {
            let Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                ..
            } = block.terminator
            else {
                return None;
            };
            let predicate = defining_op(cond)?;
            let [lhs, rhs] = predicate.operands.as_slice() else {
                return None;
            };
            let producer = |&v| {
                defining_op(v)
                    .filter(|op| op.opcode == OpCode::Copy)
                    .and_then(|op| op.origin)
            };
            use molt_ir::OpKind::{BuiltinType, TypeOf};
            (predicate.opcode == OpCode::Is
                && matches!(
                    (producer(lhs), producer(rhs)),
                    (Some(TypeOf), Some(BuiltinType)) | (Some(BuiltinType), Some(TypeOf))
                )
                && then_block != else_block
                && preds.get(&then_block).is_some_and(|p| p.as_slice() == [id]))
            .then_some(then_block)
        })
        .collect()
}

fn view_source(
    op: &TirOp,
    aliases: &AliasUnionFind,
    defs: &HashMap<ValueId, (BlockId, Option<&TirOp>)>,
    dom: &DominatorTree,
    preheader: BlockId,
    natural: &HashSet<BlockId>,
) -> Option<(Source, Option<(BlockId, ValueId)>)> {
    // Follow custody, not Python value equality. A frame store transfers the
    // argument's credit into the frame and gives the result a new live view.
    let root = |v| aliases.root(v);
    let value = root(op.operands[0]);
    let &(block, def) = defs.get(&value)?;
    let mut field_read = None;
    let mut source = Source {
        owner: value,
        field: None,
    };
    if let Some(def) = def
        && def.opcode == OpCode::ProbeInlineField
    {
        let Some(AttrValue::Int(offset)) = def.attrs.get("value") else {
            return None;
        };
        field_read = Some((block, value));
        source = Source {
            owner: root(def.operands[0]),
            field: Some(*offset),
        };
    }
    let &(definition, _) = defs.get(&source.owner)?;
    (!natural.contains(&definition) && dom.dominates(definition, preheader))
        .then_some((source, field_read))
}

fn fresh(func: &mut TirFunction, ty: TirType) -> ValueId {
    let id = func.fresh_value();
    func.value_types.insert(id, ty);
    id
}

/// Make a checked machine operation consume raw operands on the leased path.
/// The original block remains the continuation for a heap integer. It has
/// executed none of this operation's effects, including any ownership work.
fn guard_checked_arithmetic(
    func: &mut TirFunction,
    original: &TirFunction,
    map: &RegionClone,
    source: BlockId,
    needs_guard: [bool; 2],
) -> [BlockId; 3] {
    let start = map.blocks[&source];
    let block = func.blocks.get_mut(&start).unwrap();
    let mut op = block.ops.pop().expect("isolated checked arithmetic");
    let hit = func.fresh_block();
    let (range_check, accepted) = (func.fresh_block(), func.fresh_block());
    let terminator = func.blocks[&start].terminator.clone();
    let miss_args: Vec<_> = original.blocks[&source]
        .args
        .iter()
        .map(|arg| map.value(arg.id))
        .collect();
    let mut projections = Vec::new();
    let mut checks = Vec::new();
    for (operand, guarded) in op.operands.iter_mut().zip(needs_guard) {
        if !guarded {
            continue;
        }
        let boxed = *operand;
        if let Some((_, raw)) = checks.iter().find(|(input, _)| *input == boxed) {
            *operand = *raw;
            continue;
        }
        let raw = fresh(func, TirType::I64);
        let mut decode = TirOp::new(OpCode::UnboxInlineInt, vec![boxed], vec![raw]);
        decode.mark_nothrow_proof();
        projections.push(decode);
        checks.push((boxed, raw));
        *operand = raw;
    }
    // A full-width checked result can still require a heap object when
    // committed to an observable local. Keep its inline subrange in the
    // clone and retire the borrowed backing before the original can box.
    let result = op.results[0];
    let overflow = op.results[1];
    let wide = fresh(func, TirType::I64);
    op.results[0] = wide;
    let Terminator::CondBranch {
        else_block,
        else_args,
        ..
    } = terminator
    else {
        unreachable!("checked arithmetic branch");
    };
    let fits = fresh(func, TirType::Bool);
    let mut narrow = TirOp::new(OpCode::UnboxInlineInt, vec![wide], vec![result]);
    narrow.mark_nothrow_proof();
    func.blocks.insert(
        accepted,
        TirBlock {
            id: accepted,
            args: vec![],
            ops: vec![narrow],
            terminator: Terminator::Branch {
                target: else_block,
                args: else_args,
            },
        },
    );
    func.blocks.insert(
        range_check,
        TirBlock {
            id: range_check,
            args: vec![],
            ops: vec![TirOp::new(OpCode::IsInlineInt, vec![wide], vec![fits])],
            terminator: Terminator::CondBranch {
                cond: fits,
                then_block: accepted,
                then_args: vec![],
                else_block: source,
                else_args: miss_args.clone(),
            },
        },
    );
    projections.push(op);
    func.blocks.insert(
        hit,
        TirBlock {
            id: hit,
            args: vec![],
            ops: projections,
            terminator: Terminator::CondBranch {
                cond: overflow,
                then_block: source,
                then_args: miss_args.clone(),
                else_block: range_check,
                else_args: vec![],
            },
        },
    );
    let mut next = hit;
    for (index, (boxed, _)) in checks.into_iter().enumerate().rev() {
        let inline = fresh(func, TirType::Bool);
        let id = if index == 0 {
            start
        } else {
            func.fresh_block()
        };
        let args = func
            .blocks
            .get_mut(&id)
            .map(|block| std::mem::take(&mut block.args))
            .unwrap_or_default();
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args,
                ops: vec![TirOp::new(OpCode::IsInlineInt, vec![boxed], vec![inline])],
                terminator: Terminator::CondBranch {
                    cond: inline,
                    then_block: next,
                    then_args: vec![],
                    else_block: source,
                    else_args: miss_args.clone(),
                },
            },
        );
        next = id;
    }
    [hit, range_check, accepted]
}

fn entry_guard(
    func: &mut TirFunction,
    mut op: TirOp,
    success: (BlockId, Vec<ValueId>),
    miss: BlockId,
    miss_args: &[ValueId],
) -> BlockId {
    let id = func.fresh_block();
    let ready = fresh(func, TirType::Bool);
    op.results[0] = ready;
    func.blocks.insert(
        id,
        TirBlock {
            id,
            args: vec![],
            ops: vec![op],
            terminator: Terminator::CondBranch {
                cond: ready,
                then_block: success.0,
                then_args: success.1,
                else_block: miss,
                else_args: miss_args.to_vec(),
            },
        },
    );
    id
}

fn apply(func: &mut TirFunction, original: &TirFunction, plan: Plan) -> bool {
    let map = RegionClone::install(func, original, &plan.blocks);
    let Terminator::Branch { args, .. } = &original.blocks[&plan.preheader].terminator else {
        return false;
    };
    let enter_args = args.clone();
    let mut success = (map.blocks[&plan.header], enter_args.clone());
    // Check invariant callable identities after acquiring the views. Their
    // successful edge excludes concurrent mutation in addition to the
    // region's existing restriction on callbacks and effectful operations.
    // A miss enters the original loop before any guest effects.
    for block in plan.callable_guards {
        let source = &original.blocks[&block].ops[0];
        let mut entry = TirOp::new(
            OpCode::FrameEntryAdmits,
            source.operands.clone(),
            vec![ValueId(0)],
        );
        entry
            .attrs
            .insert("s_value".into(), source.attrs["s_value"].clone());
        let cloned = func.blocks.get_mut(&map.blocks[&block]).unwrap();
        let op = &mut cloned.ops[0];
        FramePrefix::admit_region_callable(&mut op.attrs);
        success = (
            entry_guard(func, entry, success, plan.header, &enter_args),
            vec![],
        );
    }
    // The successful packed acquisitions below require the GIL build and
    // clear pending state. The selected cycle cannot release the GIL, enter a
    // physical frame, or run a writer of recursion or exception state. A
    // service, callback, or exceptional edge leaves the proof. On a failed
    // entry guard the original loop still reaches its own observations in
    // order, including after any observable writes.
    let mut reservation = FramePrefix::default();
    let mut needs_virtual_level = false;
    for &id in map.blocks.values() {
        let block = func.blocks.get_mut(&id).unwrap();
        for op in &mut block.ops {
            if op.opcode != OpCode::FrameEntryAdmits {
                continue;
            }
            let Some((prefix, virtual_level)) =
                FramePrefix::take_thread_requirements(&mut op.attrs)
            else {
                return false;
            };
            reservation.homes = reservation.homes.max(prefix.homes);
            reservation.depth = reservation.depth.max(prefix.depth);
            needs_virtual_level |= virtual_level;
        }
        if let [op] = block.ops.as_mut_slice()
            && op.opcode == OpCode::FrameEntryAdmits
            && FramePrefix::region_callable_admitted(&op.attrs)
            && let Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                ..
            } = &block.terminator
            && op.results.as_slice() == [*cond]
        {
            let mut ready = TirOp::new(OpCode::ConstBool, vec![], vec![*cond]);
            ready.attrs.insert("value".into(), AttrValue::Bool(true));
            *op = ready;
            block.terminator = Terminator::Branch {
                target: *then_block,
                args: then_args.clone(),
            };
        }
    }
    if reservation.depth != 0 || needs_virtual_level {
        let mut check = TirOp::new(OpCode::FrameRegionAdmits, vec![], vec![ValueId(0)]);
        check.attrs.extend(
            reservation
                .reservation_attrs()
                .map(|(key, value)| (key.into(), value)),
        );
        if needs_virtual_level {
            FramePrefix::require_virtual_level(&mut check.attrs);
        }
        success = (
            entry_guard(func, check, success, plan.header, &enter_args),
            vec![],
        );
    }
    let mut views = BTreeMap::new();
    for (source, limits) in plan.bounds.into_iter().rev() {
        let mut ops = Vec::new();
        let backing = if let Some(offset) = source.field {
            let field = fresh(func, TirType::DynBox);
            let mut probe = TirOp::new(OpCode::ProbeInlineField, vec![source.owner], vec![field]);
            probe.attrs.insert("value".into(), AttrValue::Int(offset));
            ops.push(probe);
            field
        } else {
            source.owner
        };
        let [row, col] = limits.map(|limit| super::spliced_frame::const_int(func, limit, &mut ops));
        let data = fresh(func, TirType::I64);
        let stride = fresh(func, TirType::I64);
        let hit = fresh(func, TirType::Bool);
        let mut acquire = TirOp::new(
            OpCode::TryAcquirePackedBuffer2d,
            vec![backing, row, col],
            vec![data, stride, hit],
        );
        acquire.mark_i64_exact_proof();
        ops.push(acquire);
        let id = func.fresh_block();
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops,
                terminator: Terminator::CondBranch {
                    cond: hit,
                    then_block: success.0,
                    then_args: success.1,
                    else_block: plan.header,
                    else_args: enter_args.clone(),
                },
            },
        );
        views.insert(
            source,
            View {
                backing,
                data,
                stride,
            },
        );
        success = (id, vec![]);
    }
    // Ready cells never revert to empty. Guard their read at region entry,
    // where a miss can use the original loop without replaying any effects.
    // The provider belongs in the key: different compilation objects have
    // distinct cells even when the literal bytes match.
    let mut literal_entries = BTreeMap::new();
    for &id in map.blocks.values() {
        let block = func.blocks.get_mut(&id).unwrap();
        let [op] = block.ops.as_mut_slice() else {
            continue;
        };
        let Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            ..
        } = &block.terminator
        else {
            continue;
        };
        if op.opcode != OpCode::StringLiteralCellReady || op.results.as_slice() != [*cond] {
            continue;
        }
        let (Some(provider), Some(AttrValue::Bytes(bytes))) = (op.callee, op.attrs.get("bytes"))
        else {
            continue;
        };
        literal_entries
            .entry((provider.0, bytes.clone()))
            .or_insert_with(|| op.clone());
        let mut ready = TirOp::new(OpCode::ConstBool, vec![], vec![*cond]);
        ready.attrs.insert("value".into(), AttrValue::Bool(true));
        *op = ready;
        block.terminator = Terminator::Branch {
            target: *then_block,
            args: then_args.clone(),
        };
    }
    for check in literal_entries.into_values() {
        success = (
            entry_guard(func, check, success, plan.header, &enter_args),
            vec![],
        );
    }
    func.blocks.get_mut(&plan.preheader).unwrap().terminator = Terminator::Branch {
        target: success.0,
        args: success.1,
    };
    let mut reused_fields = 0;
    for access in plan.accesses {
        let view = &views[&access.source];
        if let Some((block, result)) = access.field_read
            && let Some(&cloned) = map.blocks.get(&block)
            && let Some(probe) = func
                .blocks
                .get_mut(&cloned)
                .unwrap()
                .ops
                .iter_mut()
                .find(|op| {
                    op.opcode == OpCode::ProbeInlineField
                        && op.results.as_slice() == [map.value(result)]
                })
        {
            // The lease acquired this field on the same owner. A writer or
            // reentrant observer can run only after leaving the region.
            // Preserve the borrowed word on those continuations as well.
            *probe = TirOp::new(
                OpCode::Copy,
                vec![view.backing],
                std::mem::take(&mut probe.results),
            );
            reused_fields += 1;
        }
        let block = func.blocks.get_mut(&map.blocks[&access.block]).unwrap();
        let op = block.ops.pop().expect("selected access");
        let mut operands = vec![access.source.owner, view.data, view.stride];
        operands.extend_from_slice(&op.operands[1..]);
        let projection = if access.read {
            OpCode::LoadPackedBuffer2d
        } else {
            OpCode::StorePackedBuffer2d
        };
        block.ops.push(TirOp::new(projection, operands, op.results));
        let Terminator::CondBranch { else_block, .. } = &mut block.terminator else {
            return false;
        };
        let Terminator::CondBranch {
            else_block: miss, ..
        } = original.blocks[&access.block].terminator
        else {
            return false;
        };
        // A boxed operand can miss even with a verified numeric range.
        // The original slow continuation retires the lease.
        *else_block = miss;
    }
    let arithmetic_blocks: Vec<_> = plan
        .checked_arithmetic
        .into_iter()
        .flat_map(|(block, guard)| guard_checked_arithmetic(func, original, &map, block, guard))
        .collect();
    let variables: Vec<_> = map
        .definitions
        .iter()
        .map(|(id, ty)| Variable {
            ty: ty.clone(),
            names: vec![*id, map.values[id]],
        })
        .collect();
    if let Err(error) = ssa_update::repair_with_exception_payloads(func, &variables) {
        if crate::tir::pass_manager::trace_func_enabled(&func.name) {
            eprintln!(
                "[packed-region] {}: SSA repair failed: {error:?}",
                func.name
            );
        }
        return false;
    }
    let trace = crate::tir::pass_manager::trace_func_enabled(&func.name);
    if trace && reused_fields != 0 {
        eprintln!(
            "[packed-region] {}: reused {reused_fields} acquired field reads",
            func.name
        );
    }
    // An explicit packed load and its raw arithmetic may establish a clear
    // synchronous exception state where an original call could raise. Keep
    // authored asynchronous work service and remove checks only through the
    // common exception-flow proof.
    let checks = super::check_exception_elim::run_post_drop(func);
    if trace && checks.ops_removed != 0 {
        eprintln!(
            "[packed-region] {}: exception cleanup removed {} ops or blocks",
            func.name, checks.ops_removed
        );
    }
    // The regional entry and exit payloads can change binding demand.
    // Recheck the machine representation so an edge conversion or store
    // cannot allocate while the entry's borrowed backing is still in use.
    let carriers = crate::representation_facts::analyze_representations(func);
    let labels = exception_label_to_block(func);
    for &id in map.blocks.values().chain(&arithmetic_blocks) {
        let Some(block) = func.blocks.get(&id) else {
            continue;
        };
        if !admits_edges(block, func, &carriers) {
            if trace {
                eprintln!("[packed-region] {id}: cloned edge would convert a carrier");
            }
            return false;
        }
        for op in &block.ops {
            if let Some(target) = crate::tir::dominators::exception_transfer_target(op, &labels)
                && !preserves_carriers(func, &carriers, target, op.exception_payload())
            {
                if trace {
                    eprintln!("[packed-region] {id}: exceptional edge would convert a carrier");
                }
                return false;
            }
            // These are the extra representation obligations introduced by
            // the rewrite. Selection already checks the effects of the
            // original operations retained in the cloned blocks.
            let raw_operands = if matches!(
                op.opcode,
                OpCode::LoadPackedBuffer2d | OpCode::StorePackedBuffer2d
            ) {
                &op.operands[1..3]
            } else if matches!(op.opcode, OpCode::CheckedAdd | OpCode::CheckedMul) {
                // Checked arithmetic can fall back to a runtime call.
                op.operands.as_slice()
            } else {
                &[]
            };
            if raw_operands
                .iter()
                .any(|v| !carriers.get(v).is_some_and(|r| r.is_raw_i64_carrier()))
                || (super::frame_home_placement::is_home_store(op)
                    && super::frame_home_placement::boxes_heap_int(op, &carriers))
            {
                if trace {
                    eprintln!(
                        "[packed-region] {id}: cloned {:?} lost its carrier proof",
                        op.opcode
                    );
                }
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_store_requires_a_nonfinalizing_release_and_no_box_allocation() {
        let input = ValueId(0);
        let view = ValueId(1);
        let mut store = TirOp::new(OpCode::Copy, vec![input], vec![view]);
        store.origin = Some(molt_ir::OpKind::FrameHomeStore);
        let literal_types = HashMap::from_iter([(input, TirType::I64), (view, TirType::I64)]);
        let mut carriers =
            HashMap::from_iter([(input, Repr::MaybeBigInt), (view, Repr::MaybeBigInt)]);
        assert!(stable_op(&store, &literal_types, &carriers, true));
        assert!(!stable_op(&store, &literal_types, &carriers, false));
        carriers.insert(input, Repr::RawI64FullDeopt);
        assert!(!stable_op(&store, &literal_types, &carriers, true));
        carriers.insert(view, Repr::RawI64FullDeopt);
        assert!(stable_op(&store, &literal_types, &carriers, true));
    }

    fn guarded_loop(column: i64) -> (TirFunction, BlockId, BlockId) {
        let mut func = TirFunction::new(
            "packed_loop".into(),
            vec![TirType::DynBox],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        func.execution_context = molt_ir::ExecutionContextPolicy::Local;
        let owner = func.blocks[&func.entry_block].args[0].id;
        let mut constants = vec![];
        let row = super::super::spliced_frame::const_int(&mut func, 0, &mut constants);
        let col = super::super::spliced_frame::const_int(&mut func, column, &mut constants);
        let entry = func.entry_block;
        let header = func.fresh_block();
        let store = func.fresh_block();
        let latch = func.fresh_block();
        let service = func.fresh_block();
        let miss = func.fresh_block();
        let as_branch = |target| Terminator::Branch {
            target,
            args: vec![],
        };
        let guard = |cond, yes, no| Terminator::CondBranch {
            cond,
            then_block: yes,
            then_args: vec![],
            else_block: no,
            else_args: vec![],
        };
        func.blocks.get_mut(&entry).unwrap().ops = constants;
        func.blocks.get_mut(&entry).unwrap().terminator = as_branch(header);
        let read = fresh(&mut func, TirType::I64);
        let loaded = fresh(&mut func, TirType::Bool);
        let stored = fresh(&mut func, TirType::Bool);
        let pending = fresh(&mut func, TirType::Bool);
        for (id, ops, terminator) in [
            (
                header,
                vec![TirOp::new(
                    OpCode::TryBuffer2dLoadI64,
                    vec![owner, row, col],
                    vec![read, loaded],
                )],
                guard(loaded, store, miss),
            ),
            (
                store,
                vec![TirOp::new(
                    OpCode::TryBuffer2dStoreI64,
                    vec![owner, row, col, read],
                    vec![stored],
                )],
                guard(stored, latch, miss),
            ),
            (
                latch,
                vec![TirOp::new(OpCode::AsyncWorkPending, vec![], vec![pending])],
                guard(pending, service, header),
            ),
            (
                service,
                vec![TirOp::new(
                    OpCode::StoreIndex,
                    vec![owner, row, col],
                    vec![],
                )],
                as_branch(header),
            ),
            (miss, vec![], Terminator::Return { values: vec![] }),
        ] {
            func.blocks.insert(
                id,
                TirBlock {
                    id,
                    args: vec![],
                    ops,
                    terminator,
                },
            );
        }
        (func, latch, service)
    }

    #[test]
    fn one_view_covers_both_accesses_and_service_resumes_original_loop() {
        let (mut func, latch, service) = guarded_loop(1);
        assert!(run(&mut func));
        let count = |opcode| {
            func.blocks
                .values()
                .flat_map(|b| &b.ops)
                .filter(|op| op.opcode == opcode)
                .count()
        };
        assert_eq!(count(OpCode::TryAcquirePackedBuffer2d), 1);
        assert_eq!(count(OpCode::LoadPackedBuffer2d), 1);
        assert_eq!(count(OpCode::StorePackedBuffer2d), 1);
        assert_eq!(count(OpCode::StoreIndex), 1);
        let original_terminator = &func.blocks[&latch].terminator;
        assert!(func.blocks.iter().any(|(&id, block)| {
            id != latch
                && matches!(
                    block.terminator,
                    Terminator::CondBranch { then_block, else_block, .. }
                        if then_block == service
                          && !terminator_successors(original_terminator).contains(&else_block)
                )
        }));
    }

    #[test]
    fn cloned_accesses_share_the_field_captured_at_entry() {
        let (mut func, latch, _) = guarded_loop(1);
        let Terminator::CondBranch {
            else_block: header, ..
        } = func.blocks[&latch].terminator
        else {
            panic!("guarded latch")
        };
        let Terminator::CondBranch {
            then_block: store, ..
        } = func.blocks[&header].terminator
        else {
            panic!("guarded read")
        };
        let owner = func.blocks[&func.entry_block].args[0].id;
        let backing = fresh(&mut func, TirType::DynBox);
        let mut probe = TirOp::new(OpCode::ProbeInlineField, vec![owner], vec![backing]);
        probe.attrs.insert("value".into(), AttrValue::Int(0));
        for block in [header, store] {
            func.blocks.get_mut(&block).unwrap().ops[0].operands[0] = backing;
        }
        func.blocks.get_mut(&header).unwrap().ops.insert(0, probe);

        assert!(run(&mut func));
        let ops = || func.blocks.values().flat_map(|block| &block.ops);
        let acquire = ops()
            .find(|op| op.opcode == OpCode::TryAcquirePackedBuffer2d)
            .unwrap();
        let field_probes = ops()
            .filter(|op| op.opcode == OpCode::ProbeInlineField)
            .count();
        assert_eq!(
            field_probes, 2,
            "only the original and entry read the field"
        );
        assert!(ops().any(|op| op.opcode == OpCode::Copy && op.operands == [acquire.operands[0]]));
        crate::tir::verify::verify_function(&func).unwrap();
    }

    #[test]
    fn ready_literals_are_guarded_once_per_provider_with_original_cold_entry() {
        let (mut func, latch, _) = guarded_loop(1);
        let Terminator::CondBranch {
            else_block: header, ..
        } = func.blocks[&latch].terminator
        else {
            panic!("guarded latch");
        };
        let miss = func.fresh_block();
        func.blocks.insert(
            miss,
            TirBlock {
                id: miss,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let mut next = header;
        for provider in [0, 0, 1] {
            let id = func.fresh_block();
            let ready = fresh(&mut func, TirType::Bool);
            let mut probe = TirOp::new(OpCode::StringLiteralCellReady, vec![], vec![ready]);
            probe.callee = Some(molt_ir::FunctionId(provider));
            probe
                .attrs
                .insert("bytes".into(), AttrValue::Bytes(b"i64".to_vec()));
            func.blocks.insert(
                id,
                TirBlock {
                    id,
                    args: vec![],
                    ops: vec![probe],
                    terminator: Terminator::CondBranch {
                        cond: ready,
                        then_block: next,
                        then_args: vec![],
                        else_block: miss,
                        else_args: vec![],
                    },
                },
            );
            next = id;
        }
        let Terminator::CondBranch { else_block, .. } =
            &mut func.blocks.get_mut(&latch).unwrap().terminator
        else {
            unreachable!();
        };
        *else_block = next;
        assert!(run(&mut func));
        let mut guards: Vec<_> = func
            .blocks
            .values()
            .filter_map(|block| {
                let Terminator::CondBranch { else_block, .. } = block.terminator else {
                    return None;
                };
                let [op] = block.ops.as_slice() else {
                    return None;
                };
                (op.opcode == OpCode::StringLiteralCellReady)
                    .then(|| (else_block, op.callee.unwrap().0))
            })
            .collect();
        guards.sort_unstable();
        assert_eq!(guards.iter().filter(|g| g.0 == miss).count(), 3);
        assert_eq!(
            guards
                .iter()
                .filter(|g| g.0 == header)
                .map(|g| g.1)
                .collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn packed_region_checks_callable_and_thread_requirements_at_entry() {
        for depth in [0, 1] {
            let (mut func, latch, miss) = guarded_loop(1);
            let callable = func.blocks[&func.entry_block].args[0].id;
            let Terminator::CondBranch {
                else_block: header, ..
            } = func.blocks[&latch].terminator
            else {
                panic!("guarded latch");
            };
            let guarded = func.fresh_block();
            let admitted = fresh(&mut func, TirType::Bool);
            let mut guard = TirOp::new(OpCode::FrameEntryAdmits, vec![callable], vec![admitted]);
            guard
                .attrs
                .insert("s_value".into(), AttrValue::Str("expected_owner".into()));
            guard.attrs.extend(
                FramePrefix { homes: 0, depth }
                    .reservation_attrs()
                    .map(|(name, value)| (name.into(), value)),
            );
            func.blocks.insert(
                guarded,
                TirBlock {
                    id: guarded,
                    args: vec![],
                    ops: vec![guard],
                    terminator: Terminator::CondBranch {
                        cond: admitted,
                        then_block: header,
                        then_args: vec![],
                        else_block: miss,
                        else_args: vec![],
                    },
                },
            );
            let Terminator::CondBranch { else_block, .. } =
                &mut func.blocks.get_mut(&latch).unwrap().terminator
            else {
                unreachable!();
            };
            *else_block = guarded;
            assert!(run(&mut func));
            assert_eq!(
                func.blocks[&guarded].ops[0].opcode,
                OpCode::FrameEntryAdmits
            );
            let added: Vec<_> = func
                .blocks
                .iter()
                .filter(|(id, _)| **id != guarded)
                .flat_map(|(_, block)| block.ops.iter().map(move |op| (block, op)))
                .filter(|(_, op)| op.opcode == OpCode::FrameEntryAdmits)
                .collect();
            assert_eq!(added.len(), 1);
            let (entry, probe) = added
                .iter()
                .find(|(_, op)| !FramePrefix::region_callable_admitted(&op.attrs))
                .unwrap();
            assert!(
                matches!(entry.terminator, Terminator::CondBranch { else_block, .. } if else_block == header)
            );
            assert_eq!(
                FramePrefix::reservation_from_attrs(&probe.attrs),
                Some(FramePrefix::default())
            );
            let thread_guards: Vec<_> = func
                .blocks
                .values()
                .flat_map(|block| block.ops.iter().map(move |op| (block, op)))
                .filter(|(_, op)| op.opcode == OpCode::FrameRegionAdmits)
                .collect();
            assert_eq!(thread_guards.len(), depth as usize);
            if let Some((block, guard)) = thread_guards.first() {
                assert_eq!(
                    FramePrefix::reservation_from_attrs(&guard.attrs)
                        .unwrap()
                        .depth,
                    depth
                );
                assert!(
                    matches!(block.terminator, Terminator::CondBranch { else_block, .. } if else_block == header)
                );
            }
        }
    }

    #[test]
    fn negative_coordinate_or_reentrant_latch_keeps_checked_accesses() {
        assert!(!run(&mut guarded_loop(-1).0));
        let (mut func, latch, service) = guarded_loop(1);
        func.blocks.get_mut(&latch).unwrap().terminator = Terminator::Branch {
            target: service,
            args: vec![],
        };
        assert!(!run(&mut func));
    }

    #[test]
    fn packed_loop_keeps_the_view_after_custody_moves_into_the_frame() {
        let (mut func, _, _) = guarded_loop(1);
        let entry = func.entry_block;
        let owner = func.blocks[&entry].args[0].id;
        let view = fresh(&mut func, TirType::DynBox);
        for op in func.blocks.values_mut().flat_map(|block| &mut block.ops) {
            for value in &mut op.operands {
                if *value == owner {
                    *value = view;
                }
            }
        }
        let mut publish = TirOp::new(OpCode::Copy, vec![owner], vec![view]);
        publish.origin = Some(molt_ir::OpKind::FrameHomeStore);
        publish.attrs.insert("value".into(), AttrValue::Int(0));
        func.blocks.get_mut(&entry).unwrap().ops.push(publish);

        assert!(run(&mut func));
        let acquire = func
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .find(|op| op.opcode == OpCode::TryAcquirePackedBuffer2d)
            .expect("a single leased owner");
        assert_eq!(acquire.operands[0], view);
    }
}
