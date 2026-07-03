//! Poll-body cloning and rewrite for generator fusion.
//!
//! Deep-clones a fusable generator `_poll` body into the caller's value/block
//! space (fresh SSA ids, remapped labels, frame slots promoted to phis) so the
//! splice in [`super::apply_fusion`] can weave it into the consumer loop. Split
//! out of `generator_fusion.rs` as a move-only decomposition; the recognition
//! and orchestration live in [`super`], the CFG surgery in [`super::wire`].

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_has_exception_label_attr_table;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::{GEN_CONTROL_BYTES, SlotInfo, attr_original_kind, attr_value_int};

/// A local slot's entry-init constant.
pub(super) enum LocalInit {
    Int(i64),
    None_,
}

/// Resolve a LOCAL slot's entry init: the value the poll's entry block stores
/// into `offset` before the loop. Phase 1 supports a `ConstInt` init or a
/// `None`/`missing` init (the unbound-local sentinel). Returns `None` for any
/// other (non-trivially-promotable) init.
pub(super) fn local_slot_init_const(poll: &TirFunction, offset: i64) -> Option<LocalInit> {
    let entry = poll.blocks.get(&poll.entry_block)?;
    // The LAST entry-block store to this slot is the effective init (a `missing`
    // sentinel store is typically followed by the real `= 0` store).
    let mut result: Option<LocalInit> = None;
    for op in &entry.ops {
        if op.opcode == OpCode::ClosureStore && attr_value_int(op) == Some(offset) {
            let &stored = op.operands.get(1)?;
            let loc = def_location(poll, stored)?;
            let def = &poll.blocks[&loc.0].ops[loc.1];
            result = if def.opcode == OpCode::ConstInt {
                Some(LocalInit::Int(attr_value_int(def)?))
            } else if def.opcode == OpCode::ConstNone || attr_original_kind(def) == Some("missing")
            {
                Some(LocalInit::None_)
            } else {
                return None;
            };
        }
    }
    result
}

/// Locate the (block, op_idx) defining `v` (single-result ops).
fn def_location(func: &TirFunction, v: ValueId) -> Option<(BlockId, usize)> {
    for (&bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            if op.results.first() == Some(&v) {
                return Some((bid, i));
            }
        }
    }
    None
}

pub(super) fn const_int_op(result: ValueId, value: i64) -> TirOp {
    let mut a = AttrDict::new();
    a.insert("value".into(), AttrValue::Int(value));
    TirOp {
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
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::new(),
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
    /// Every fresh cloned block id (deterministic order).
    pub(super) cloned_blocks: Vec<BlockId>,
    /// The cloned block + op index holding the (single) `state_yield`.
    pub(super) yield_block: BlockId,
    pub(super) yield_idx: usize,
    /// The yielded pair value (cloned).
    pub(super) yield_pair: ValueId,
    /// Cloned blocks terminating in `Return` (the exhausted / normal exits).
    pub(super) return_blocks: Vec<BlockId>,
    /// Slot phi value per user slot (index-aligned with the `slot_infos` passed
    /// to [`clone_and_rewrite_poll`]).
    pub(super) slot_phis: Vec<ValueId>,
    /// Per slot, the value flowing on the loop back-edge (the cloned in-loop
    /// store value), or `None` for a loop-invariant slot (no in-loop store →
    /// thread the phi unchanged).
    pub(super) slot_backedge: Vec<Option<ValueId>>,
}

/// True if `op` is a generator-frame bookkeeping op the splice drops: trace
/// slots, exception-stack save/restore, source-line markers. These are frame
/// activation/teardown overhead with no fused-loop meaning.
fn is_bookkeeping_op(op: &TirOp) -> bool {
    matches!(
        attr_original_kind(op),
        Some(
            "trace_enter_slot"
                | "trace_exit"
                | "exception_stack_enter"
                | "exception_stack_depth"
                | "exception_stack_exit"
                | "exception_stack_set_depth"
                | "line"
        )
    )
}

/// Clone the poll body into the caller with fresh ids, applying the frame-slot
/// promotion + control-slot elimination rewrites. Returns `None` (bail) on any
/// unpromotable shape (a user slot stored in more than one in-loop site, an
/// `IncRef`/`DecRef` of the frame pointer, etc.).
pub(super) fn clone_and_rewrite_poll(
    poll: &TirFunction,
    caller: &mut TirFunction,
    slot_infos: &[SlotInfo],
) -> Option<ClonedPoll> {
    // Map each user slot offset -> a fresh slot phi value, and -> its index.
    let slot_phis: Vec<ValueId> = slot_infos
        .iter()
        .map(|_| {
            let v = caller.fresh_value();
            caller.value_types.entry(v).or_insert(TirType::DynBox);
            v
        })
        .collect();
    let slot_index: HashMap<i64, usize> = slot_infos
        .iter()
        .enumerate()
        .map(|(i, s)| (s.offset, i))
        .collect();

    // Fresh exception-label remap (mirrors the inliner): the poll body's
    // per-function SimpleIR labels must not collide with the caller's.
    let label_remap = build_label_remap(poll, caller);

    // Value remap: poll ValueId -> caller ValueId. Pre-seed user-slot loads to
    // the slot phi and control-slot loads to a shared `None`.
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();

    // A single cloned `None` (for send/throw slot reads) materialized in the
    // cloned entry block.
    let none_for_control = caller.fresh_value();
    caller
        .value_types
        .entry(none_for_control)
        .or_insert(TirType::None);

    // Pre-seed: every `closure_load(self, off)` result.
    for block in poll.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ClosureLoad
                && let Some(off) = attr_value_int(op)
                && let Some(&res) = op.results.first()
            {
                if off >= GEN_CONTROL_BYTES {
                    let Some(&idx) = slot_index.get(&off) else {
                        return None; // load of a slot we didn't plan — bail.
                    };
                    value_map.insert(res, slot_phis[idx]);
                } else {
                    // Control slot (send=0 / throw=8 / others): reads `None`.
                    value_map.insert(res, none_for_control);
                }
            }
        }
    }

    // The generator's exception-stack save/restore values: the results of the
    // prologue `exception_stack_enter` / `exception_stack_depth` ops. These ops
    // are bookkeeping (dropped), so their result values vanish. The body
    // restores them before every `check_exception` via a `Copy(exc_val, exc_val)`
    // (the SimpleIR `exception_stack_set_depth`/restore idiom captured as a Copy)
    // and passes the copies as `CheckException` operands. After fusion the
    // generator exception stack does not exist: we DROP those restore-copies and
    // CLEAR the `CheckException` operands (the consumer's own `CheckException`
    // carries no operands either — it reads the runtime pending flag directly).
    let exc_stack_vals: HashSet<ValueId> = poll
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| {
            matches!(
                attr_original_kind(op),
                Some("exception_stack_enter" | "exception_stack_depth")
            )
        })
        .filter_map(|op| op.results.first().copied())
        .collect();
    // Transitively include the restore-copies' results (a Copy of an exc value is
    // itself an exc-derived value that later copies/checks consume).
    let mut exc_derived = exc_stack_vals.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for block in poll.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::Copy
                    && !op.attrs.contains_key("_original_kind")
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
    // edge. Those args are exc-stack-derived too: fold them into `exc_derived` so
    // the clone strips them (the post-fusion exception edge carries no args).
    // The exit block is found via the inverse of `label_id_map`: the block whose
    // label is a `CheckException` `value` target.
    {
        let mut exc_target_labels: HashSet<i64> = HashSet::new();
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
    }
    // Block remap: poll BlockId -> fresh caller BlockId (deterministic order).
    let mut poll_block_ids: Vec<BlockId> = poll.blocks.keys().copied().collect();
    poll_block_ids.sort_by_key(|b| b.0);
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::new();
    for &bid in &poll_block_ids {
        block_map.insert(bid, caller.fresh_block());
    }

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

    // Per-slot back-edge value: the LAST user-slot store's (remapped) value.
    // A slot stored in >1 distinct block (conditional store) bails — the simple
    // single-reaching-def threading would be unsound.
    let mut slot_store_blocks: Vec<Option<BlockId>> = vec![None; slot_infos.len()];
    let mut slot_backedge: Vec<Option<ValueId>> = vec![None; slot_infos.len()];

    let mut cloned_blocks: Vec<BlockId> = Vec::with_capacity(poll_block_ids.len());
    let mut yield_block_idx: Option<(BlockId, usize, ValueId)> = None;
    let mut return_blocks: Vec<BlockId> = Vec::new();

    for &bid in &poll_block_ids {
        let src = &poll.blocks[&bid];
        let new_bid = remap_block(bid);
        cloned_blocks.push(new_bid);

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
        let new_args: Vec<TirValue> = if bid == poll.entry_block {
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

        let mut new_ops: Vec<TirOp> = Vec::with_capacity(src.ops.len());
        for op in src.ops.iter() {
            // Drop bookkeeping + the lone state_switch.
            if op.opcode == OpCode::StateSwitch || is_bookkeeping_op(op) {
                continue;
            }
            // Drop closure_load (its result was pre-seeded to a phi/None).
            if op.opcode == OpCode::ClosureLoad {
                continue;
            }
            // closure_store: control slot -> drop; user slot -> record back-edge.
            if op.opcode == OpCode::ClosureStore {
                let off = attr_value_int(op).unwrap_or(-1);
                if off >= GEN_CONTROL_BYTES {
                    let &idx = slot_index.get(&off)?;
                    let &stored = op.operands.get(1)?;
                    // Entry-block stores are the init (handled in the preheader),
                    // not the back-edge. Only record stores OUTSIDE the entry.
                    if bid != poll.entry_block {
                        if let Some(prev) = slot_store_blocks[idx]
                            && prev != bid
                        {
                            return None; // conditional/multi-block store — bail.
                        }
                        slot_store_blocks[idx] = Some(bid);
                        slot_backedge[idx] = Some(remap(stored, &value_map));
                    }
                }
                continue;
            }
            // state_yield: keep a marker copy (rewritten in wire_fused_loop). We
            // record its location and pair operand, and DROP it from the op
            // stream — the split happens at this index in the cloned block.
            if op.opcode == OpCode::StateYield {
                let &pair = op.operands.first()?;
                yield_block_idx = Some((new_bid, new_ops.len(), remap(pair, &value_map)));
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
            let mut attrs = clone_attrs_drop_simple_names(&op.attrs);
            remap_exception_label_attr_local(op.opcode, &mut attrs, &label_remap);
            let operands: Vec<ValueId> = if op.opcode == OpCode::CheckException {
                Vec::new()
            } else {
                op.operands.iter().map(|v| remap(*v, &value_map)).collect()
            };
            new_ops.push(TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands,
                results: op.results.iter().map(|v| remap(*v, &value_map)).collect(),
                attrs,
                source_span: op.source_span,
            });
        }

        let new_term = clone_terminator_local(&src.terminator, &value_map, &block_map);
        if matches!(new_term, Terminator::Return { .. }) {
            return_blocks.push(new_bid);
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

    // Materialize the shared `None` for control-slot reads at the top of the
    // cloned entry block (dominates every use).
    let entry_clone = remap_block(poll.entry_block);
    caller
        .blocks
        .get_mut(&entry_clone)
        .unwrap()
        .ops
        .insert(0, const_none_op(none_for_control));

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
    for (old_block_u32, label_val) in &poll.label_id_map {
        if let Some(new_bid) = block_map.get(&BlockId(*old_block_u32)) {
            let new_label = label_remap.get(label_val).copied().unwrap_or(*label_val);
            caller.label_id_map.entry(new_bid.0).or_insert(new_label);
        }
    }

    let (yield_block, yield_idx, yield_pair) = yield_block_idx?;

    Some(ClonedPoll {
        entry: entry_clone,
        cloned_blocks,
        yield_block,
        yield_idx,
        yield_pair,
        return_blocks,
        slot_phis,
        slot_backedge,
    })
}

/// Clone an op's attrs, dropping the SimpleIR value-name annotations (which are
/// function-local name strings with no id to remap — copying them verbatim would
/// alias the poll's names onto caller values).
fn clone_attrs_drop_simple_names(attrs: &AttrDict) -> AttrDict {
    attrs
        .iter()
        .filter(|(k, _)| k.as_str() != "_simple_out" && !k.starts_with("_simple_result_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Rewrite a cloned exception op's `value` label through `label_remap`.
fn remap_exception_label_attr_local(
    opcode: OpCode,
    attrs: &mut AttrDict,
    label_remap: &HashMap<i64, i64>,
) {
    if !opcode_has_exception_label_attr_table(opcode) {
        return;
    }
    if let Some(AttrValue::Int(old)) = attrs.get("value")
        && let Some(&new) = label_remap.get(old)
    {
        attrs.insert("value".into(), AttrValue::Int(new));
    }
}

/// Build the poll->fresh exception-label remap (mirrors the inliner's
/// `build_label_remap`): every label the poll uses is reassigned strictly above
/// the caller's current max so the cloned exception edges cannot collide.
fn build_label_remap(poll: &TirFunction, caller: &TirFunction) -> HashMap<i64, i64> {
    let poll_labels = function_label_ids(poll);
    if poll_labels.is_empty() {
        return HashMap::new();
    }
    let caller_max = function_label_ids(caller).iter().copied().max();
    let start = caller_max.map(|m| m + 1).unwrap_or(0);
    let mut remap = HashMap::with_capacity(poll_labels.len());
    for (label, next) in poll_labels.into_iter().zip(start..) {
        remap.insert(label, next);
    }
    remap
}

/// The set of SimpleIR label ids `func` uses (label_id_map values + exception-op
/// `value` labels).
fn function_label_ids(func: &TirFunction) -> BTreeSet<i64> {
    let mut labels: BTreeSet<i64> = func.label_id_map.values().copied().collect();
    for block in func.blocks.values() {
        for op in &block.ops {
            if opcode_has_exception_label_attr_table(op.opcode)
                && let Some(AttrValue::Int(l)) = op.attrs.get("value")
            {
                labels.insert(*l);
            }
        }
    }
    labels
}

/// Clone a terminator, remapping value operands + block targets.
fn clone_terminator_local(
    term: &Terminator,
    value_map: &HashMap<ValueId, ValueId>,
    block_map: &HashMap<BlockId, BlockId>,
) -> Terminator {
    let rv = |v: ValueId| *value_map.get(&v).unwrap_or(&v);
    let rb = |b: BlockId| *block_map.get(&b).unwrap_or(&b);
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: rb(*target),
            args: args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: rv(*cond),
            then_block: rb(*then_block),
            then_args: then_args.iter().map(|v| rv(*v)).collect(),
            else_block: rb(*else_block),
            else_args: else_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: rv(*value),
            cases: cases
                .iter()
                .map(|(c, blk, args)| (*c, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(c, blk, args)| (*c, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}
