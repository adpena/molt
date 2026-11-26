//! Provenance of an exact native range iterator at a particular step. A
//! semantic Iterator(int) can still run arbitrary Python; follow the objects
//! created by the program and the successful iterator guard instead.

use molt_ir::OpKind;
use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::ProgramPointDominance;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{
    INLINE_INT47_HI, INLINE_INT47_LO, IntRange, TripCount, affine_recurrence_range,
    python_range_len,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::type_refine::ExactScalarPlan;
use crate::tir::values::ValueId;

use super::ownership_lattice_min::{CompletionPath, conditionally_valid_regions};
use super::value_identity::forwarded_python_value_source;

/// The iterator constructor survives as GetIter or its preserved frontend carrier.
pub(super) fn is_get_iter_op(op: &TirOp) -> bool {
    op.opcode == OpCode::GetIter || (op.opcode == OpCode::Copy && op.origin == Some(OpKind::Iter))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Origin {
    RangeIterator,
    TupleIterator,
    Range,
    TupleOfRanges,
    InlineInt,
}

struct Sources<'a> {
    ops: HashMap<ValueId, &'a TirOp>,
    joins: HashMap<ValueId, Vec<ValueId>>,
    non_none: HashMap<ValueId, Vec<BlockId>>,
    dominance: ProgramPointDominance,
}

impl<'a> Sources<'a> {
    fn new(func: &'a TirFunction, plan: &ExactScalarPlan<'_>) -> Self {
        let mut ops = HashMap::default();
        let mut joins = HashMap::default();
        for &id in plan.block_order() {
            let block = &func.blocks[&id];
            for op in &block.ops {
                for &result in &op.results {
                    ops.insert(result, op);
                }
            }
            if let Some(incoming) = plan.normal_incoming_edges(id) {
                for (index, arg) in block.args.iter().enumerate() {
                    if let Some(values) = incoming
                        .iter()
                        .map(|edge| edge.get(index).copied())
                        .collect::<Option<Vec<_>>>()
                    {
                        joins.insert(arg.id, values);
                    }
                }
            }
        }
        let mut non_none: HashMap<ValueId, Vec<BlockId>> = HashMap::default();
        for block in func.blocks.values() {
            let Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                ..
            } = block.terminator
            else {
                continue;
            };
            if then_block == else_block
                || !plan
                    .normal_incoming_edges(else_block)
                    .is_some_and(|edges| edges.len() == 1)
            {
                continue;
            }
            let Some(test) = ops.get(&cond) else { continue };
            let [left, right] = test.operands.as_slice() else {
                continue;
            };
            if test.opcode != OpCode::Is || test.results != [cond] {
                continue;
            }
            for (value, other) in [(left, right), (right, left)] {
                if ops
                    .get(other)
                    .is_some_and(|op| op.opcode == OpCode::ConstNone)
                {
                    non_none.entry(*value).or_default().push(else_block);
                }
            }
        }
        Self {
            ops,
            joins,
            non_none,
            dominance: ProgramPointDominance::compute_executable(func),
        }
    }

    fn literal_item_range(&self, arguments: &[ValueId]) -> Option<IntRange> {
        let literal = |v: &ValueId| {
            let op = self.ops.get(v)?;
            let Some(AttrValue::Int(x)) = op.attrs.get("value") else {
                return None;
            };
            (op.opcode == OpCode::ConstInt).then_some(*x)
        };
        let [a, b, k] = arguments else { return None };
        let (start, stop, step) = (literal(a)?, literal(b)?, literal(k)?);
        let count = python_range_len(start, stop, step)?;
        affine_recurrence_range(start, step, &TripCount::Constant(count))
    }

    fn inline_range_iterator(
        &self,
        iterator: ValueId,
        block: BlockId,
        index: usize,
    ) -> Option<IntRange> {
        let mut pending = vec![(iterator, Origin::RangeIterator)];
        let mut visited = HashSet::default();
        let mut item_range: Option<IntRange> = None;
        while let Some((value, required)) = pending.pop() {
            if !visited.insert((value, required)) {
                continue;
            }
            if let Some(incoming) = self.joins.get(&value) {
                if incoming.is_empty() {
                    return None;
                }
                pending.extend(incoming.iter().map(|&item| (item, required)));
                continue;
            }
            let Some(&op) = self.ops.get(&value) else {
                return None;
            };
            if let Some(source) = forwarded_python_value_source(op) {
                pending.push((source, required));
                continue;
            }
            match required {
                Origin::RangeIterator | Origin::TupleIterator => {
                    let [source] = op.operands.as_slice() else {
                        return None;
                    };
                    if op.results != [value]
                        || !is_get_iter_op(op)
                        || !self.non_none.get(&value).is_some_and(|edges| {
                            edges.iter().any(|&success| {
                                self.dominance
                                    .definition_available(success, None, block, index)
                            })
                        })
                    {
                        return None;
                    }
                    let target = if required == Origin::RangeIterator {
                        Origin::Range
                    } else {
                        Origin::TupleOfRanges
                    };
                    pending.push((*source, target));
                }
                Origin::Range => {
                    if let Some(call) = op.builtin_call()
                        && call.wire_kind == OpKind::RangeNew
                        && op.results == [value]
                        && call.arguments.len() == 3
                    {
                        // These bounds keep every yielded item immediate and retirement of
                        // the iterator's target free of user finalizers.
                        let range = self
                            .literal_item_range(call.arguments)
                            .unwrap_or(IntRange::new(INLINE_INT47_LO, INLINE_INT47_HI));
                        item_range = Some(item_range.map_or(range, |r| r.join(range)));
                        pending.extend(call.arguments.iter().map(|&v| (v, Origin::InlineInt)));
                    } else if op.opcode == OpCode::IterNextUnboxed
                        && op.results.first() == Some(&value)
                        && let [source] = op.operands.as_slice()
                    {
                        // Exhaustion produces None. iter(None) cannot pass the native
                        // iterator's non-None test at the consuming step.
                        pending.push((*source, Origin::TupleIterator));
                    } else {
                        return None;
                    }
                }
                Origin::TupleOfRanges => {
                    if !(op.opcode == OpCode::BuildTuple
                        || (op.opcode == OpCode::Copy && op.origin == Some(OpKind::TupleNew)))
                        || op.results != [value]
                        || op.operands.is_empty()
                    {
                        return None;
                    }
                    pending.extend(op.operands.iter().map(|&item| (item, Origin::Range)));
                }
                Origin::InlineInt => {
                    if !(op.opcode == OpCode::ConstInt
                        && matches!(op.attrs.get("value"), Some(AttrValue::Int(x)) if IntRange::point(*x).fits_inline_int47()))
                    {
                        return None;
                    }
                }
            }
        }
        item_range
    }
}

/// Calls to the original runtime iterator, with its progress, exhaustion and
/// exception edge intact, that cannot invoke Python in a spliced frame.
pub(super) fn callback_free_steps(
    func: &TirFunction,
    plan: &ExactScalarPlan<'_>,
) -> HashSet<ValueId> {
    let mut sources = None;
    let mut result = HashSet::default();
    for block in func.blocks.values() {
        if !block.ops.iter().any(|op| {
            op.runtime == Some(molt_ir::runtime_fn_generated::RuntimeFn::SplicedFrameLink)
        }) {
            continue;
        }
        for (index, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::IterNextUnboxed || op.is_async_work_poll() {
                continue;
            }
            let ([iterator], [item, _done]) = (op.operands.as_slice(), op.results.as_slice())
            else {
                continue;
            };
            if sources
                .get_or_insert_with(|| Sources::new(func, plan))
                .inline_range_iterator(*iterator, block.id, index)
                .is_some()
            {
                result.insert(*item);
            }
        }
    }
    result
}

/// Keep the runtime's conditional item and its ownership intact. Give an
/// independently defined scalar to a binding only where the step succeeded.
/// The exhaustion word and every original handler argument keep their types.
pub(super) fn project_successful_items(func: &mut TirFunction, items: &HashSet<ValueId>) {
    if items.is_empty() {
        return;
    }
    let aliases = super::alias_analysis::build_alias_union_find(func);
    let regions = conditionally_valid_regions(func, &aliases, CompletionPath::ExclusiveEmptyBlocks);
    let dominance = ProgramPointDominance::compute_executable(func);
    let mut sites = Vec::new();
    for block in func.blocks.values() {
        for (index, op) in block.ops.iter().enumerate() {
            let [item] = op.operands.as_slice() else {
                continue;
            };
            if op.opcode != OpCode::Copy
                || op.origin != Some(OpKind::BindingAlias)
                || op.results.len() != 1
            {
                continue;
            }
            if items.contains(item)
                && let Some(&success) = regions.get(&aliases.root(*item))
                && dominance.definition_available(success, None, block.id, index)
            {
                sites.push((block.id, index, *item));
            }
        }
    }
    // Reverse operation order keeps insertion points stable within each block.
    sites.sort_unstable_by_key(|&(block, index, _)| (block, index));
    for (block, index, item) in sites.into_iter().rev() {
        let raw = func.fresh_value();
        let ops = &mut func.blocks.get_mut(&block).expect("binding block").ops;
        let mut projection = TirOp::new(OpCode::UnboxInlineInt, vec![item], vec![raw]);
        projection.source_span = ops[index].source_span;
        func.value_types
            .insert(raw, crate::tir::types::TirType::I64);
        ops[index].operands[0] = raw;
        ops.insert(index, projection);
    }
}

/// A raw projection can carry a global interval of its own. Re-establish the
/// successful-edge and constructor facts for this body; range analysis never
/// gives the conditional runtime result a global range.
pub(super) fn seed_projection_ranges(
    func: &TirFunction,
    ranges: &mut super::value_range::ValueRangeResult,
) {
    let mut steps = HashMap::default();
    let mut projections = Vec::new();
    for block in func.blocks.values() {
        for (index, op) in block.ops.iter().enumerate() {
            match (op.opcode, op.operands.as_slice(), op.results.as_slice()) {
                (OpCode::IterNextUnboxed, [iter], [item, _]) => {
                    steps.insert(*item, (block.id, index, *iter));
                }
                (OpCode::UnboxInlineInt, [item], [raw]) => {
                    projections.push((block.id, index, *item, *raw));
                }
                _ => {}
            }
        }
    }
    projections.retain(|&(_, _, item, _)| steps.contains_key(&item));
    if projections.is_empty() {
        return;
    }
    let plan = ExactScalarPlan::new(func);
    let sources = Sources::new(func, &plan);
    let aliases = super::alias_analysis::build_alias_union_find(func);
    let regions = conditionally_valid_regions(func, &aliases, CompletionPath::ExclusiveEmptyBlocks);
    for (block, index, item, raw) in projections {
        let (step_block, step_index, iter) = steps[&item];
        if let Some(&success) = regions.get(&aliases.root(item))
            && sources
                .dominance
                .definition_available(success, None, block, index)
            && let Some(range) = sources.inline_range_iterator(iter, step_block, step_index)
        {
            ranges.record_global_range(raw, range);
        }
    }
}
