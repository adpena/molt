//! Poll-body cloning and rewrite for generator fusion.
//!
//! Deep-clones a fusable generator `_poll` body into the caller's value/block
//! space (fresh SSA ids, remapped labels, frame slots promoted to SSA values by
//! the slot plan) so the splice in [`super::apply_fusion`] can weave it into
//! the consumer loop. The recognition and orchestration live in [`super`], the
//! CFG surgery in [`super::wire`].

use molt_ir::deterministic_hash::{HashMap, HashSet};

use molt_ir::runtime_fn_generated::RuntimeFn;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::{
    LabelAllocator, build_label_remap, clone_attrs_without_simple_names, clone_op_with,
    function_label_ids, remap_exception_label_attr, remap_terminator, transfer_label_id_map,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::ownership_lattice_min::Replacements;
use crate::tir::passes::spliced_frame::{
    KeySpace, Level, bracket_ops, linked_origin_block, may_observe_frames, origin_cold_block,
    raise_ops,
};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::slots::{SlotDef, SlotPlan};
use super::{attr_original_kind, attr_value_int};

pub(super) fn const_int_op(result: ValueId, value: i64) -> TirOp {
    let mut a = AttrDict::default();
    a.insert("value".into(), AttrValue::Int(value));
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: a,
        source_span: None,
    }
}

pub(super) fn const_none_op(result: ValueId) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::default(),
        source_span: None,
    }
}

// ---------------------------------------------------------------------------
// Clone + rewrite the poll body
// ---------------------------------------------------------------------------

/// The product of cloning + rewriting the poll body into the caller.
pub(super) struct ClonedPoll {
    /// Fresh entry block id of the cloned body (the preheader spine).
    pub(super) entry: BlockId,
    /// The cloned block + op index holding the (single) `state_yield`.
    pub(super) yield_block: BlockId,
    pub(super) yield_idx: usize,
    /// The yielded value (cloned).
    pub(super) yield_value: ValueId,
    /// Cloned blocks terminating in `Return`.
    pub(super) return_blocks: Vec<BlockId>,
    /// The cloned returns only an exception edge reaches: the body's exits
    /// with an exception pending.
    pub(super) exception_exits: HashSet<BlockId>,
}

/// The fused activation's frame context: what each frame state of the
/// clone names (spliced_frames.md 4.5).
pub(super) struct FusedFrame {
    /// The caller value holding the generator function.
    pub(super) function: ValueId,
    /// The caller's handler of the iteration, and what its check passes.
    pub(super) handler: BlockId,
    pub(super) handler_args: Vec<ValueId>,
}

/// True if `op` is a generator-frame bookkeeping op the splice drops: trace
/// slots, exception-stack save/restore, source-line markers, and source
/// location stores. These are frame activation/teardown overhead with no
/// fused-loop meaning. A location store's index addresses the generator's
/// own location table; copied into the consumer it would publish another
/// function's index into the consumer's frame (a wrong line, or an index past
/// the consumer's table). The consumer keeps its own last store, which is the
/// line CPython reports for the consumer's frame; a frame state carries the
/// generator's own location (spliced_frames.md 4.5). The PEP 667 locals
/// prologue (`molt_frame_locals_begin`) initializes the task payload the
/// fused activation does not have.
pub(super) fn is_bookkeeping_op(op: &TirOp) -> bool {
    op.opcode == OpCode::FrameSetLocation
        || op.runtime == Some(RuntimeFn::FrameLocalsBegin)
        || matches!(
            attr_original_kind(op),
            Some(
                molt_ir::OpKind::TraceEnterSlot
                    | molt_ir::OpKind::TraceExit
                    | molt_ir::OpKind::ExceptionStackEnter
                    | molt_ir::OpKind::ExceptionStackDepth
                    | molt_ir::OpKind::ExceptionStackExit
                    | molt_ir::OpKind::ExceptionStackSetDepth
                    | molt_ir::OpKind::Line
            )
        )
}

/// The generator's exception-stack save/restore values: the results of the
/// prologue `exception_stack_enter` / `exception_stack_depth` ops. These ops are
/// bookkeeping (dropped), so their result values vanish. The body restores them
/// before every `check_exception` via a `Copy(exc_val, exc_val)` (the SimpleIR
/// `exception_stack_set_depth`/restore idiom captured as a Copy) and passes the
/// copies as `CheckException` operands. After fusion the generator exception
/// stack does not exist: the splice DROPS those restore-copies and CLEARS the
/// `CheckException` operands (the consumer's own `CheckException` carries no
/// operands either — it reads the runtime pending flag directly).
pub(super) fn exception_stack_values(poll: &TirFunction) -> HashSet<ValueId> {
    let exc_stack_vals: HashSet<ValueId> = poll
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| {
            matches!(
                attr_original_kind(op),
                Some(molt_ir::OpKind::ExceptionStackEnter | molt_ir::OpKind::ExceptionStackDepth)
            )
        })
        .filter_map(|op| op.results.first().copied())
        .collect();
    // Transitively include the restore-copies' results (a Copy of an exc value is
    // itself an exc-derived value that later copies/checks consume).
    let mut exc_derived = exc_stack_vals;
    let mut changed = true;
    while changed {
        changed = false;
        for block in poll.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::Copy
                    && !op.origin.is_some()
                    && op.operands.iter().any(|v| exc_derived.contains(v))
                    && let Some(&res) = op.results.first()
                    && exc_derived.insert(res)
                {
                    changed = true;
                }
            }
        }
    }
    // The poll's exception-EXIT block (the `CheckException` handler/exit target)
    // receives the saved exc-stack values as BLOCK ARGS on the implicit exception
    // edge. Those args are exc-stack-derived too: fold them in so the clone
    // strips them (the post-fusion exception edge carries no args). The exit
    // block is found via the inverse of `label_id_map`: the block whose label is
    // a `CheckException` `value` target.
    let mut exc_target_labels: HashSet<i64> = HashSet::default();
    for block in poll.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::CheckException
                && let Some(AttrValue::Int(l)) = op.attrs.get("value")
            {
                exc_target_labels.insert(*l);
            }
        }
    }
    for (&block_u32, &label) in &poll.label_id_map {
        if exc_target_labels.contains(&label)
            && let Some(b) = poll.blocks.get(&BlockId(block_u32))
        {
            for arg in &b.args {
                exc_derived.insert(arg.id);
            }
        }
    }
    exc_derived
}

/// Clone the poll body into the caller with fresh ids, promoting the frame's
/// user slots by `plan` and eliminating its control slots. A promoted store
/// becomes a copy of its value that keeps the reference the frame took, and a
/// promoted read a copy of its reaching definition that keeps the reference the
/// `ClosureLoad` returned (`owners`, design 20 §1.2). `arguments` holds the
/// caller value for each argument position the plan reads. Returns `None`
/// (bail) on a malformed poll: no `state_yield`, or one without its value.
pub(super) fn clone_and_rewrite_poll(
    poll: &TirFunction,
    caller: &mut TirFunction,
    plan: &SlotPlan,
    arguments: &HashMap<usize, ValueId>,
    owners: &mut Replacements,
    frame: &FusedFrame,
) -> Option<ClonedPoll> {
    // Fresh exception-label remap (mirrors the inliner): the poll body's
    // per-function SimpleIR labels must not collide with the caller's.
    let label_remap = build_label_remap(poll, caller);

    // Value remap: poll ValueId -> caller ValueId. A frame read the plan does
    // not promote (a control slot, or a read in a block the poll never enters)
    // is pre-seeded to a shared `None`.
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::default();

    // A single cloned `None` (for send/throw slot reads) materialized in the
    // cloned entry block.
    let none_for_control = caller.fresh_value();
    caller
        .value_types
        .entry(none_for_control)
        .or_insert(TirType::None);

    for block in poll.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ClosureLoad
                && op.operands.first() == Some(&plan.frame)
                && let Some(&res) = op.results.first()
                && !plan.reads.contains_key(&res)
            {
                value_map.insert(res, none_for_control);
            }
        }
    }

    // The argument each join takes for each slot it merges.
    let mut join_args: HashMap<(BlockId, usize), ValueId> = HashMap::default();
    for (&block, slots) in &plan.joins {
        for &slot in slots {
            let arg = caller.fresh_value();
            caller.value_types.insert(arg, TirType::DynBox);
            join_args.insert((block, slot), arg);
        }
    }
    // The frame's reference each promoted store takes, per poll store.
    let mut held: HashMap<(BlockId, usize), ValueId> = HashMap::default();
    for &store in &plan.stores {
        let value = caller.fresh_value();
        caller.value_types.insert(value, TirType::DynBox);
        held.insert(store, value);
    }

    // The generator's exception-stack save/restore values, which the splice
    // drops along with the ops that define and restore them.
    let exc_derived = exception_stack_values(poll);
    // Block remap: poll BlockId -> fresh caller BlockId (deterministic order).
    let mut poll_block_ids: Vec<BlockId> = poll.blocks.keys().copied().collect();
    poll_block_ids.sort_by_key(|b| b.0);
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::default();
    for &bid in &poll_block_ids {
        block_map.insert(bid, caller.fresh_block());
    }

    // Frame states (spliced_frames.md 4.5): the poll's own operand facts, its
    // reaching locations, and labels for the cold Origin blocks after every
    // label the clone takes.
    let exact = crate::tir::type_refine::extract_exact_scalar_map(poll);
    let locations = super::super::frame_location_placement::reaching_locations(poll);
    let mut cold_labels = LabelAllocator::after_labels(
        function_label_ids(caller)
            .into_iter()
            .chain(label_remap.values().copied()),
    );
    let mut cold_blocks: Vec<(TirBlock, i64)> = Vec::new();

    // Mint fresh value ids for every non-pre-seeded result and every block arg.
    let fresh_for = |old: ValueId,
                     value_map: &mut HashMap<ValueId, ValueId>,
                     caller: &mut TirFunction|
     -> ValueId {
        if let Some(&existing) = value_map.get(&old) {
            return existing;
        }
        let v = caller.fresh_value();
        value_map.insert(old, v);
        v
    };
    for &bid in &poll_block_ids {
        let block = &poll.blocks[&bid];
        for arg in &block.args {
            fresh_for(arg.id, &mut value_map, caller);
        }
        for op in &block.ops {
            for r in &op.results {
                fresh_for(*r, &mut value_map, caller);
            }
        }
    }

    let remap = |v: ValueId, vm: &HashMap<ValueId, ValueId>| -> ValueId {
        *vm.get(&v)
            .unwrap_or_else(|| panic!("generator_fusion: poll value {v} has no remap"))
    };
    let remap_block = |b: BlockId| -> BlockId {
        *block_map
            .get(&b)
            .unwrap_or_else(|| panic!("generator_fusion: poll block {b} has no remap"))
    };

    // Each promoted definition's caller value.
    let resolve = |def: SlotDef| -> ValueId {
        match def {
            SlotDef::Argument(position) => arguments[&position],
            SlotDef::Stored { block, index } => held[&(block, index)],
            SlotDef::Join { block, slot } => join_args[&(block, slot)],
        }
    };
    // Each join's cloned block, back to its poll block.
    let join_of: HashMap<BlockId, BlockId> = plan
        .joins
        .keys()
        .map(|&block| (remap_block(block), block))
        .collect();

    let mut yield_block_idx: Option<(BlockId, usize, ValueId)> = None;
    let mut return_blocks: Vec<BlockId> = Vec::new();
    let mut exception_exits: HashSet<BlockId> = HashSet::default();

    for &bid in &poll_block_ids {
        let src = &poll.blocks[&bid];
        let new_bid = remap_block(bid);

        // Cloned block args (entry stays arg-less — the poll's `self` param is
        // eliminated; no other block in a well-formed poll carries args except
        // the exception-exit block, which becomes unreachable).
        // The cloned entry is arg-less (`self` is eliminated). Every other block:
        // keep its args EXCEPT the exception-stack values (`exc_derived`). The
        // poll's exception-exit block carries the saved exc-stack depth/value as
        // args, supplied on the implicit `CheckException` edge; after fusion that
        // edge passes no args (the consumer's own handler convention), so a
        // retained exc-stack arg would be an unsatisfied phi at the exception
        // edge ("predecessor … branches with 0 argument(s) but phi … required").
        // The ops that consumed those args were the dropped exc-stack-restore
        // copies, so the args are dead and safely removed.
        let mut new_args: Vec<TirValue> = if bid == poll.entry_block {
            Vec::new()
        } else {
            src.args
                .iter()
                .filter(|a| !exc_derived.contains(&a.id))
                .map(|a| TirValue {
                    id: remap(a.id, &value_map),
                    ty: a.ty.clone(),
                })
                .collect()
        };
        // A join takes each promoted slot it merges as a trailing argument.
        for &slot in plan.joins.get(&bid).into_iter().flatten() {
            new_args.push(TirValue {
                id: join_args[&(bid, slot)],
                ty: TirType::DynBox,
            });
        }

        let mut new_ops: Vec<TirOp> = Vec::with_capacity(src.ops.len() + 1);
        // Each slot's definition as the walk reaches each operation; `None` in
        // a block the poll never enters, which holds no frame state.
        let mut state: Option<Vec<Option<SlotDef>>> = plan.entry.get(&bid).cloned();
        // Materialize the control-slot value before recording any split point.
        // When entry itself yields, a later prepend would move the pair's
        // definition across the recorded yield boundary into the continuation.
        if bid == poll.entry_block {
            new_ops.push(const_none_op(none_for_control));
        }
        // The check that observes a dropped runtime call (the locals
        // prologue) observes nothing once the call is gone.
        let mut after_dropped_call = false;
        for (op_index, op) in src.ops.iter().enumerate() {
            // Drop bookkeeping, the lone state_switch, and the completion
            // reports (`state_set` before each return): a fused body has no
            // frame state, and its returns become the loop exit.
            if matches!(op.opcode, OpCode::StateSwitch | OpCode::StateSet) || is_bookkeeping_op(op)
            {
                after_dropped_call = op.runtime.is_some();
                continue;
            }
            if std::mem::take(&mut after_dropped_call) && op.opcode == OpCode::CheckException {
                continue;
            }
            // Frame accesses. A promoted store becomes a copy of its value that
            // keeps the reference the frame took, and a promoted read a copy of
            // its reaching definition that keeps the reference the load
            // returned. Any other frame read was pre-seeded to `None`, and any
            // other store goes.
            if op.operands.first() == Some(&plan.frame) {
                if op.opcode == OpCode::ClosureStore {
                    if let Some(&value) = held.get(&(bid, op_index)) {
                        owners.record_held(value);
                        if let Some(state) = state.as_mut()
                            && let Some(slot) =
                                attr_value_int(op).and_then(|offset| plan.slot_of(offset))
                        {
                            state[slot] = Some(SlotDef::Stored {
                                block: bid,
                                index: op_index,
                            });
                        }
                        new_ops.push(TirOp {
                            origin: None,
                            runtime: None,
                            callee: None,
                            dialect: op.dialect,
                            opcode: OpCode::Copy,
                            operands: vec![remap(op.operands[1], &value_map)],
                            results: vec![value],
                            attrs: AttrDict::default(),
                            source_span: op.source_span,
                        });
                    }
                    continue;
                }
                if op.opcode == OpCode::ClosureLoad {
                    if let Some(&def) = plan.reads.get(&op.results[0]) {
                        let load = TirOp {
                            origin: None,
                            runtime: None,
                            callee: None,
                            dialect: op.dialect,
                            opcode: op.opcode,
                            operands: Vec::new(),
                            results: vec![remap(op.results[0], &value_map)],
                            attrs: AttrDict::default(),
                            source_span: op.source_span,
                        };
                        owners.record(&load);
                        new_ops.push(TirOp {
                            opcode: OpCode::Copy,
                            operands: vec![resolve(def)],
                            ..load
                        });
                    }
                    continue;
                }
            }
            // state_yield: keep a marker copy (rewritten in wire_fused_loop). We
            // record its location and yielded value, and DROP it from the op
            // stream — the split happens at this index in the cloned block.
            if op.opcode == OpCode::StateYield {
                let &value = op.operands.first()?;
                yield_block_idx = Some((new_bid, new_ops.len(), remap(value, &value_map)));
                continue;
            }
            // Drop the exception-stack restore-copies (a `Copy(exc_val, ..)`
            // whose result is an exc-derived value). After fusion the generator
            // exception stack does not exist; these are pure bookkeeping.
            if op.opcode == OpCode::Copy
                && op.results.first().is_some_and(|r| exc_derived.contains(r))
            {
                continue;
            }
            // `CheckException` propagates a body exception to the function exit;
            // it is kept, but its operands (the cloned exception-stack restore
            // values) are CLEARED — the consumer's own `CheckException` reads the
            // runtime pending flag directly and carries no operands.
            let mut attrs = clone_attrs_without_simple_names(&op.attrs);
            remap_exception_label_attr(op.opcode, &mut attrs, &label_remap, &poll.name);
            let operands: Vec<ValueId> = if op.opcode == OpCode::CheckException {
                Vec::new()
            } else {
                op.operands.iter().map(|v| remap(*v, &value_map)).collect()
            };
            let exception = operands.first().copied();
            let mut cloned = clone_op_with(
                op,
                operands,
                op.results.iter().map(|v| remap(*v, &value_map)).collect(),
                attrs,
            );
            // The generator's frame stays observable where an observer runs:
            // an Origin edge, an explicit raise, an operation that may run
            // Python code (spliced_frames.md 4.2-4.4).
            let Some(state) = state.as_ref() else {
                new_ops.push(cloned);
                continue;
            };
            let level = Level {
                function: frame.function,
                location: locations.get(&(bid, op_index)).copied(),
                key_space: KeySpace::StatefulOffset,
                bindings: state
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, def)| Some((plan.offsets[slot], resolve((*def)?))))
                    .collect(),
            };
            let span = op.source_span;
            if op.opcode == OpCode::CheckException {
                // Every exception leaves the fused body at the caller's
                // handler of the iteration, with what the iteration's check
                // passed it: a handler-free poll's exception targets are its
                // exits, which hold no operation a fused body keeps. An
                // Origin edge first inserts the generator's entry (4.2). A poll
                // services asynchronous work inside the generator's linked
                // record, so a signal handler receives the generator's frame;
                // its edge leaves with that record still linked (5.2).
                let polled = op.is_async_work_poll();
                let cold = if polled {
                    linked_origin_block(
                        caller,
                        1,
                        op.exception_edge_kind() == Some(ExceptionEdgeKind::Origin),
                        frame.handler,
                        frame.handler_args.clone(),
                        span,
                    )
                } else if op.exception_edge_kind() == Some(ExceptionEdgeKind::Origin) {
                    origin_cold_block(
                        caller,
                        std::slice::from_ref(&level),
                        frame.handler,
                        frame.handler_args.clone(),
                        span,
                    )
                } else {
                    TirBlock {
                        id: caller.fresh_block(),
                        args: Vec::new(),
                        ops: Vec::new(),
                        terminator: Terminator::Branch {
                            target: frame.handler,
                            args: frame.handler_args.clone(),
                        },
                    }
                };
                let label = cold_labels.fresh();
                cloned.attrs.insert("value".into(), AttrValue::Int(label));
                cold_blocks.push((cold, label));
                if polled {
                    let (enter, exit) = bracket_ops(caller, &[level], span);
                    new_ops.extend(enter);
                    new_ops.push(cloned);
                    new_ops.extend(exit);
                } else {
                    new_ops.push(cloned);
                }
            } else if op.runtime == Some(RuntimeFn::ExceptionTracePrepend)
                && let Some(exception) = exception
            {
                new_ops.push(cloned);
                new_ops.extend(raise_ops(caller, Some(exception), &[level], span));
            } else if may_observe_frames(op, &exact) {
                let (enter, exit) = bracket_ops(caller, &[level], span);
                new_ops.extend(enter);
                new_ops.push(cloned);
                new_ops.extend(exit);
            } else {
                new_ops.push(cloned);
            }
        }

        let mut new_term = remap_terminator(&src.terminator, &value_map, &block_map, &poll.name);
        // Each edge into a join passes the definition of each slot it merges. A
        // block the poll never enters has none, and passes `None` until the
        // splice prunes it.
        let exit = plan.exits.get(&bid);
        new_term.for_each_edge_mut(|target, args| {
            let Some(join) = join_of.get(target) else {
                return;
            };
            for &slot in &plan.joins[join] {
                args.push(match exit {
                    Some(state) => {
                        resolve(state[slot].expect("a slot plan defines every slot a join merges"))
                    }
                    None => none_for_control,
                });
            }
        });
        if matches!(new_term, Terminator::Return { .. }) {
            return_blocks.push(new_bid);
            // A normal return reports the generator's completion (`state_set`
            // before it); every other return leaves with an exception pending,
            // including one a raise's transport branch reaches through
            // terminator edges.
            if !src.ops.iter().any(|op| op.opcode == OpCode::StateSet) {
                exception_exits.insert(new_bid);
            }
        }

        caller.blocks.insert(
            new_bid,
            TirBlock {
                id: new_bid,
                args: new_args,
                ops: new_ops,
                terminator: new_term,
            },
        );
    }

    let entry_clone = remap_block(poll.entry_block);

    // Transfer the poll's value_types for cloned values (remapped keys).
    let poll_param_ids: HashSet<ValueId> = poll.blocks[&poll.entry_block]
        .args
        .iter()
        .map(|a| a.id)
        .collect();
    for (old, ty) in &poll.value_types {
        if poll_param_ids.contains(old) {
            continue;
        }
        if let Some(&new) = value_map.get(old) {
            caller.value_types.entry(new).or_insert_with(|| ty.clone());
        }
    }

    // Transfer the poll's `label_id_map` (BlockId.0 → SimpleIR label) with the
    // block key remapped through `block_map` and the label VALUE remapped through
    // `label_remap` — the same table the cloned `CheckException`/`TryStart`/
    // `TryEnd` ops' `value` attrs were rewritten through. Without this, a cloned
    // `CheckException` whose handler/exit label was remapped to N has no block
    // carrying label N, and LLVM lowering fails ("check_exception target label N
    // is not present in label map"); the native back-conversion likewise cannot
    // resolve the exception edge.
    transfer_label_id_map(poll, caller, &block_map, &label_remap, &poll.name);
    for (cold, label) in cold_blocks {
        caller.label_id_map.insert(cold.id.0, label);
        caller.blocks.insert(cold.id, cold);
    }

    let (yield_block, yield_idx, yield_value) = yield_block_idx?;

    Some(ClonedPoll {
        entry: entry_clone,
        yield_block,
        yield_idx,
        yield_value,
        return_blocks,
        exception_exits,
    })
}
