//! Dead-store elimination for `StoreAttr` ops within a single basic block.
//!
//! Pattern 1: when two `StoreAttr` ops within the same block target the
//! same object value at the same offset and there is no intervening read
//! or escape of that object, the earlier store is dead and can be removed.
//!
//! Pattern 2: when the final stores to a typed-class instance target an
//! `ObjectNewBoundStack` value allocated in the same block, and that stack
//! object is not used by the terminator, those stores are also dead.  The
//! object cannot be observed outside the block, and any intervening observer
//! already invalidates the pending-store state below.
//!
//! The most common producer of this pattern is the frontend's class-
//! instantiation fold combined with the `__init__` inliner: the inlined
//! `__init__` body emits `store_init` for each declared field with the
//! constructor's default value, then user code immediately overwrites the
//! same fields with non-default values:
//!
//! ```text
//! object_new_bound_stack out=_v23 args=[cls] value=24
//! store_init args=[_v23, _v_zero] value=0   ; p.x = 0  (init)
//! store_init args=[_v23, _v_zero] value=8   ; p.y = 0  (init)
//! store args=[_v23, _v_i] value=0           ; p.x = i  (overwrite - kills the init)
//! store args=[_v23, _v_iplus1] value=8      ; p.y = i+1
//! ```
//!
//! The two `store_init` ops are dead in this loop body.  Eliminating them
//! drops 2 stores per typed-class instance in the hot loop.
//!
//! ## Soundness
//!
//! A store `S1[obj, *] offset=N` is dead iff, walking forward from S1
//! within the same basic block, we encounter another typed-slot store
//! `S2[obj_or_alias, *] offset=N` BEFORE any of:
//!   - a read of `obj` or one of its transparent aliases (`LoadAttr`,
//!     indexed access, or any op that could observe the slot's value),
//!   - an escape of `obj` (`Call`, `CallMethod`, `CallBuiltin`, `Raise`,
//!     yielding, storing it into another object/container, etc.),
//!   - a control-flow boundary (we restrict the analysis to a single
//!     block - cross-block dead-store would need full alias analysis).
//!
//! When all conditions hold, S1's writes are unobservable: the slot is
//! only read AFTER S2, which provides a fresh value.
//!
//! ### Key conservatism
//!
//! - Any op whose operand list contains `obj` or a tracked transparent
//!   alias and whose effects we don't recognize is treated as a possible
//!   read or escape => S1 stays live.
//! - We scope the search to a single block: dead stores across blocks are
//!   left live unless overwritten before the block ends. Cross-block
//!   elimination belongs in a full memory dataflow pass with alias facts.
//! - Stores with no resolvable offset attr stay live.
//! - Only `StoreAttr` ops with `_original_kind in {"store", "store_init"}`
//!   are considered - other StoreAttr variants (set_attr_name,
//!   guarded_field_set, module_set_attr, etc.) have different operand
//!   conventions and effects and are out of scope.
//!
//! ## Statistics
//!
//! Returns the number of dead stores removed via `PassStats.ops_removed`.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::Terminator;
use crate::tir::blocks::TirBlock;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::PassStats;

/// Returns `Some(offset)` when this op is a `store` or `store_init`
/// against a typed-class instance slot at a known integer offset.
///
/// Conservatism: any other StoreAttr variant (set_attr_name,
/// guarded_field_set, etc.) returns `None`, leaving the op untouched.
fn store_offset(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::StoreAttr {
        return None;
    }
    let original = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(original, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Returns `Some((target, offset))` for the narrow typed-class slot
/// store contract this pass understands.
fn typed_slot_store(op: &TirOp) -> Option<(ValueId, i64)> {
    if op.operands.len() != 2 {
        return None;
    }
    Some((op.operands[0], store_offset(op)?))
}

fn copy_is_known_local_alias(op: &TirOp) -> bool {
    match op.attrs.get("_original_kind") {
        None => true,
        Some(AttrValue::Str(kind)) => matches!(
            kind.as_str(),
            "copy" | "copy_var" | "store_var" | "load_var" | "identity_alias"
        ),
        Some(_) => false,
    }
}

fn transparent_alias_root(op: &TirOp, aliases: &AliasState) -> Option<ValueId> {
    if op.results.is_empty() {
        return None;
    }

    match op.opcode {
        OpCode::TypeGuard => {
            if op.attrs.contains_key("_original_kind") || op.operands.len() != 1 {
                return None;
            }
            Some(aliases.root(op.operands[0]))
        }
        OpCode::Copy => {
            if !copy_is_known_local_alias(op) || op.operands.is_empty() {
                return None;
            }
            let root = aliases.root(op.operands[0]);
            if op
                .operands
                .iter()
                .all(|operand| aliases.root(*operand) == root)
            {
                Some(root)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn stack_object_alloc_result(op: &TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::ObjectNewBoundStack {
        return None;
    }
    if !matches!(op.attrs.get("value"), Some(AttrValue::Int(_))) {
        return None;
    }
    if op.results.len() != 1 {
        return None;
    }
    Some(op.results[0])
}

#[derive(Default)]
struct AliasState {
    parent: HashMap<ValueId, ValueId>,
}

impl AliasState {
    fn root(&self, value: ValueId) -> ValueId {
        let mut root = value;
        while let Some(next) = self.parent.get(&root).copied() {
            if next == root {
                break;
            }
            root = next;
        }
        root
    }

    fn operand_aliases_root(&self, op: &TirOp, root: ValueId) -> bool {
        op.operands
            .iter()
            .any(|operand| self.root(*operand) == root)
    }

    fn record_transparent_aliases(&mut self, op: &TirOp) {
        let Some(root) = transparent_alias_root(op, self) else {
            return;
        };
        for result in &op.results {
            self.parent.insert(*result, root);
        }
    }
}

/// Returns `true` when the op may observe the slot value of `obj`
/// (i.e. read it, escape it, or trigger a side effect that could).
///
/// This is the conservative side: any op that takes `obj` as an
/// operand AND is not in the "pure local consumer" allow-list is
/// treated as a possible observer, blocking dead-store elim. `root`
/// is an alias root, not necessarily the original SSA value used by
/// the pending store.
fn may_observe_slot(op: &TirOp, root: ValueId, aliases: &AliasState) -> bool {
    if !aliases.operand_aliases_root(op, root) {
        return false;
    }
    match op.opcode {
        // Reads of the slot - direct observation.
        OpCode::LoadAttr | OpCode::Index => true,
        // Recognized typed-slot stores to the same object root are not
        // observers; they are handled as possible overwrites below.
        // Unknown StoreAttr variants and stores where `root` appears as
        // the stored value are observers.
        OpCode::StoreAttr => match typed_slot_store(op) {
            Some((target, _)) => aliases.root(target) != root,
            None => true,
        },
        OpCode::StoreIndex => true,
        // Calls / raises / returns let the slot be observed externally.
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallBuiltin
        | OpCode::Raise
        | OpCode::Yield
        | OpCode::YieldFrom => true,
        // Building a container with `obj` as an element captures it.
        OpCode::BuildList
        | OpCode::BuildDict
        | OpCode::BuildSet
        | OpCode::BuildTuple
        | OpCode::BuildSlice
        | OpCode::AllocTask => true,
        // Transparent aliases and ref ops do not read slot values.
        OpCode::Copy | OpCode::TypeGuard if transparent_alias_root(op, aliases).is_some() => false,
        OpCode::IncRef | OpCode::DecRef | OpCode::CheckException => false,
        // Default: conservative - treat any other use as observation.
        _ => true,
    }
}

fn terminator_uses_root(terminator: &Terminator, root: ValueId, aliases: &AliasState) -> bool {
    let mut uses_root = |value: &ValueId| aliases.root(*value) == root;
    match terminator {
        Terminator::Branch { args, .. } => args.iter().any(&mut uses_root),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            uses_root(cond)
                || then_args.iter().any(&mut uses_root)
                || else_args.iter().any(&mut uses_root)
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            uses_root(value)
                || cases
                    .iter()
                    .any(|(_, _, args)| args.iter().any(&mut uses_root))
                || default_args.iter().any(&mut uses_root)
        }
        Terminator::Return { values } => values.iter().any(&mut uses_root),
        Terminator::Unreachable => false,
    }
}

/// Run dead-store elimination on a single block.  Returns the number
/// of ops removed.
fn run_block(block: &mut TirBlock) -> usize {
    // Walk forward.  For each store at (obj, offset), record (idx, obj,
    // offset).  When we see a later store at the same (obj, offset)
    // with no intervening observer, mark the earlier one for removal.
    //
    // `pending`: most recent live store keyed by (obj, offset).
    //   When a new store at the same key arrives, the old store is
    //   killed (added to dead_indices).
    let mut pending: HashMap<(ValueId, i64), usize> = HashMap::new();
    let mut dead_indices: Vec<usize> = Vec::new();
    let mut aliases = AliasState::default();
    let mut stack_object_roots: HashSet<ValueId> = HashSet::new();

    for (idx, op) in block.ops.iter().enumerate() {
        // First: any op that observes `obj` invalidates pending stores
        // for that obj.  We must do this BEFORE handling stores so that
        // a load-then-store sequence doesn't kill the load's witness.
        let mut invalidated_keys: Vec<(ValueId, i64)> = Vec::new();
        for &(obj, offset) in pending.keys() {
            if may_observe_slot(op, obj, &aliases) {
                invalidated_keys.push((obj, offset));
            }
        }
        for key in &invalidated_keys {
            pending.remove(key);
        }

        aliases.record_transparent_aliases(op);
        if let Some(result) = stack_object_alloc_result(op) {
            stack_object_roots.insert(aliases.root(result));
        }

        // Now handle the store, if this is one.
        if let Some((target, offset)) = typed_slot_store(op) {
            let key = (aliases.root(target), offset);
            if let Some(prev_idx) = pending.insert(key, idx) {
                // The previous store at this (obj, offset) is dead.
                dead_indices.push(prev_idx);
            }
        }
    }

    for (&(root, _offset), &idx) in &pending {
        if stack_object_roots.contains(&root)
            && !terminator_uses_root(&block.terminator, root, &aliases)
        {
            dead_indices.push(idx);
        }
    }

    if dead_indices.is_empty() {
        return 0;
    }

    // Remove ops in reverse-index order to preserve the indices of
    // earlier removals.
    dead_indices.sort_unstable();
    dead_indices.dedup();
    let removed = dead_indices.len();
    for &idx in dead_indices.iter().rev() {
        block.ops.remove(idx);
    }
    removed
}

/// Public entry point - run dead-store elimination on every block.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut total_removed = 0usize;
    for block in func.blocks.values_mut() {
        total_removed += run_block(block);
    }
    PassStats {
        name: "dead_store_elim",
        values_changed: 0,
        ops_removed: total_removed,
        ops_added: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_store(operands: Vec<ValueId>, offset: i64, original_kind: &str) -> TirOp {
        let mut op = make_op(OpCode::StoreAttr, operands, vec![]);
        op.attrs.insert("value".into(), AttrValue::Int(offset));
        op.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str(original_kind.into()),
        );
        op
    }

    fn make_object_alloc(opcode: OpCode, cls: ValueId, inst: ValueId) -> TirOp {
        let mut op = make_op(opcode, vec![cls], vec![inst]);
        op.attrs.insert("value".into(), AttrValue::Int(24));
        op
    }

    fn entry_only_func() -> TirFunction {
        TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None)
    }

    /// store_init then store at the same (obj, offset) with no
    /// intervening observer => store_init is dead.
    #[test]
    fn store_init_followed_by_store_same_offset_is_dead() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let val0 = ValueId(1);
        let val1 = ValueId(2);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, val0], 0, "store_init"));
        entry.ops.push(make_store(vec![obj, val1], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 1);
        // The surviving op is the LATER store (the live one).
        assert_eq!(entry.ops[0].operands, vec![obj, val1]);
    }

    /// Stores at *different* offsets are independent - neither dies.
    #[test]
    fn stores_at_different_offsets_are_independent() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let v0 = ValueId(1);
        let v1 = ValueId(2);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, v0], 0, "store_init"));
        entry.ops.push(make_store(vec![obj, v1], 8, "store_init"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 2);
    }

    /// A LoadAttr between two stores at the same offset blocks the
    /// elimination - the load observes the first store's value.
    #[test]
    fn load_between_stores_blocks_elim() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let v0 = ValueId(1);
        let v1 = ValueId(2);
        let load_result = ValueId(3);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, v0], 0, "store_init"));
        entry
            .ops
            .push(make_op(OpCode::LoadAttr, vec![obj], vec![load_result]));
        entry.ops.push(make_store(vec![obj, v1], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "load between stores must keep the first store live"
        );
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
    }

    /// A Call between two stores at the same offset blocks elim
    /// (the call could escape the object).
    #[test]
    fn call_between_stores_blocks_elim() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let v0 = ValueId(1);
        let v1 = ValueId(2);
        let call_result = ValueId(3);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, v0], 0, "store_init"));
        entry
            .ops
            .push(make_op(OpCode::Call, vec![obj], vec![call_result]));
        entry.ops.push(make_store(vec![obj, v1], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "call could escape obj - must keep store live"
        );
    }

    /// Stores against different objects are independent.
    #[test]
    fn stores_to_different_objects_independent() {
        let mut func = entry_only_func();
        let obj_a = ValueId(0);
        let obj_b = ValueId(1);
        let v = ValueId(2);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj_a, v], 0, "store_init"));
        entry.ops.push(make_store(vec![obj_b, v], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
    }

    /// Unknown StoreAttr variants have different operand/effect
    /// contracts from typed-slot store/store_init and must conservatively
    /// block elimination for that object.
    #[test]
    fn unknown_storeattr_variant_blocks_elim() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let init = ValueId(1);
        let unknown = ValueId(2);
        let replacement = ValueId(3);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, init], 0, "store_init"));
        entry
            .ops
            .push(make_store(vec![obj, unknown], 0, "set_attr_name"));
        entry
            .ops
            .push(make_store(vec![obj, replacement], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "unrecognized StoreAttr variants may observe or mutate obj"
        );
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
    }

    /// A direct typed-slot store into some other object with `obj` as
    /// the value operand escapes `obj`; after that escape, the first
    /// store may be externally observed.
    #[test]
    fn storeattr_value_operand_escape_blocks_elim() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let other_obj = ValueId(1);
        let init = ValueId(2);
        let replacement = ValueId(3);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, init], 0, "store_init"));
        entry
            .ops
            .push(make_store(vec![other_obj, obj], 16, "store"));
        entry
            .ops
            .push(make_store(vec![obj, replacement], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "using obj as the stored value escapes it before replacement"
        );
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
    }

    /// StoreIndex may dispatch through container/index semantics and is
    /// not a typed-class slot overwrite.  Treat it as an observer when
    /// it uses the object.
    #[test]
    fn store_index_between_stores_blocks_elim() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let init = ValueId(1);
        let index = ValueId(2);
        let value = ValueId(3);
        let replacement = ValueId(4);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, init], 0, "store_init"));
        entry
            .ops
            .push(make_op(OpCode::StoreIndex, vec![obj, index, value], vec![]));
        entry
            .ops
            .push(make_store(vec![obj, replacement], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "StoreIndex is not a proven typed-slot overwrite"
        );
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
    }

    /// Copy aliases created before the store must still be recognized:
    /// a call through the alias can observe the first store.
    #[test]
    fn transparent_copy_alias_call_blocks_elim() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let alias = ValueId(1);
        let init = ValueId(2);
        let call_result = ValueId(3);
        let replacement = ValueId(4);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Copy, vec![obj], vec![alias]));
        entry.ops.push(make_store(vec![obj, init], 0, "store_init"));
        entry
            .ops
            .push(make_op(OpCode::Call, vec![alias], vec![call_result]));
        entry
            .ops
            .push(make_store(vec![obj, replacement], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "uses through a transparent alias must observe the object root"
        );
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 4);
    }

    /// A transparent alias store to the same field still overwrites the
    /// original object slot, so the prior store is dead.
    #[test]
    fn transparent_copy_alias_store_kills_prior_store() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let init = ValueId(1);
        let alias = ValueId(2);
        let replacement = ValueId(3);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, init], 0, "store_init"));
        entry
            .ops
            .push(make_op(OpCode::Copy, vec![obj], vec![alias]));
        entry
            .ops
            .push(make_store(vec![alias, replacement], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 1);
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops.len(), 2);
        assert_eq!(entry.ops[1].operands, vec![alias, replacement]);
    }

    /// Three consecutive stores at the same (obj, offset) => first two
    /// are dead, last one survives.  This mirrors:
    ///   __init__: self.x = 0
    ///   user code: p.x = i
    ///   user code: p.x = j  (in a single block, i.e. no control flow
    ///                        between the second and third writes)
    #[test]
    fn triple_store_same_offset_kills_first_two() {
        let mut func = entry_only_func();
        let obj = ValueId(0);
        let v0 = ValueId(1);
        let v1 = ValueId(2);
        let v2 = ValueId(3);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_store(vec![obj, v0], 0, "store_init"));
        entry.ops.push(make_store(vec![obj, v1], 0, "store"));
        entry.ops.push(make_store(vec![obj, v2], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 2);
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 1);
        assert_eq!(
            func.blocks[&func.entry_block].ops[0].operands,
            vec![obj, v2]
        );
    }

    /// Real bench_struct pattern: alloc + 2 store_init + 2 store at the
    /// same offsets => both store_init ops are dead.
    #[test]
    fn bench_struct_pattern_eliminates_two_init_stores() {
        let mut func = entry_only_func();
        let cls = ValueId(0);
        let zero = ValueId(1);
        let i = ValueId(2);
        let i_plus_1 = ValueId(3);
        let inst = ValueId(4);

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_object_alloc(OpCode::ObjectNewBound, cls, inst));
        // store_init p.x = 0  (offset 0)
        entry
            .ops
            .push(make_store(vec![inst, zero], 0, "store_init"));
        // store_init p.y = 0  (offset 8)
        entry
            .ops
            .push(make_store(vec![inst, zero], 8, "store_init"));
        // store p.x = i       (offset 0, kills the first store_init)
        entry.ops.push(make_store(vec![inst, i], 0, "store"));
        // store p.y = i + 1   (offset 8, kills the second store_init)
        entry.ops.push(make_store(vec![inst, i_plus_1], 8, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 2,
            "both store_init ops should be eliminated - they are \
             overwritten by the user-code stores at the same offsets \
             with no intervening observer"
        );
        // Surviving ops: alloc + 2 user stores = 3.
        assert_eq!(func.blocks[&func.entry_block].ops.len(), 3);
    }

    /// A stack-allocated object that never leaves the block and whose
    /// fields are never read does not need final slot stores. DCE can
    /// then erase the now-unused allocation and value computations.
    #[test]
    fn stack_object_final_stores_with_no_live_out_are_dead() {
        let mut func = entry_only_func();
        let cls = ValueId(0);
        let x = ValueId(1);
        let y = ValueId(2);
        let inst = ValueId(3);

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_object_alloc(OpCode::ObjectNewBoundStack, cls, inst));
        entry.ops.push(make_store(vec![inst, x], 0, "store"));
        entry.ops.push(make_store(vec![inst, y], 8, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 2,
            "final stores to a noescape stack object with no block live-out are dead"
        );
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::StoreAttr),
            "all stack-object final stores should be removed"
        );
    }

    /// Full bench_struct stack form: the constructor-default stores are
    /// overwritten, and the final stores are also dead because the
    /// object remains local and unread.
    #[test]
    fn bench_struct_stack_pattern_eliminates_all_dead_stores() {
        let mut func = entry_only_func();
        let cls = ValueId(0);
        let zero = ValueId(1);
        let i = ValueId(2);
        let i_plus_1 = ValueId(3);
        let inst = ValueId(4);

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_object_alloc(OpCode::ObjectNewBoundStack, cls, inst));
        entry
            .ops
            .push(make_store(vec![inst, zero], 0, "store_init"));
        entry
            .ops
            .push(make_store(vec![inst, zero], 8, "store_init"));
        entry.ops.push(make_store(vec![inst, i], 0, "store"));
        entry.ops.push(make_store(vec![inst, i_plus_1], 8, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 4,
            "both overwritten init stores and final local stores are dead"
        );
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::StoreAttr),
            "all typed-slot stores should be removed"
        );
    }

    /// Real lowered bench_struct shape after copy propagation: local
    /// store/load transport can arrive as a Copy with duplicate operands
    /// before the aliased object result. That copy is still transparent;
    /// treating it as an observer keeps every slot store live and prevents
    /// cleanup DCE from removing the now-unused stack allocation.
    #[test]
    fn duplicate_operand_copy_alias_does_not_block_stack_store_elim() {
        let mut func = entry_only_func();
        let cls = ValueId(0);
        let zero = ValueId(1);
        let i = ValueId(2);
        let i_plus_1 = ValueId(3);
        let inst = ValueId(4);
        let alias = ValueId(5);

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_object_alloc(OpCode::ObjectNewBoundStack, cls, inst));
        entry
            .ops
            .push(make_store(vec![inst, zero], 0, "store_init"));
        entry
            .ops
            .push(make_store(vec![inst, zero], 8, "store_init"));
        entry
            .ops
            .push(make_op(OpCode::Copy, vec![inst, inst], vec![alias]));
        entry.ops.push(make_store(vec![alias, i], 0, "store"));
        entry
            .ops
            .push(make_store(vec![alias, i_plus_1], 8, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 4,
            "duplicate-operand local copy must remain a transparent alias so all stack-local stores die"
        );
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::StoreAttr),
            "all typed-slot stores should be removed through the copy alias"
        );
    }

    /// Heap allocations may be externally visible through runtime
    /// object identity/finalization rules, so final stores remain live
    /// unless another store overwrites them in the same block.
    #[test]
    fn heap_object_final_store_is_not_eliminated() {
        let mut func = entry_only_func();
        let cls = ValueId(0);
        let x = ValueId(1);
        let inst = ValueId(2);

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_object_alloc(OpCode::ObjectNewBound, cls, inst));
        entry.ops.push(make_store(vec![inst, x], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::StoreAttr),
            "heap-object final stores must stay live"
        );
    }

    /// A stack allocation passed through the terminator is live beyond
    /// the current block, so its final store must be preserved.
    #[test]
    fn stack_object_store_returned_from_block_is_not_eliminated() {
        let mut func = entry_only_func();
        let cls = ValueId(0);
        let x = ValueId(1);
        let inst = ValueId(2);

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_object_alloc(OpCode::ObjectNewBoundStack, cls, inst));
        entry.ops.push(make_store(vec![inst, x], 0, "store"));
        entry.terminator = Terminator::Return { values: vec![inst] };

        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0);
        assert!(
            func.blocks[&func.entry_block]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::StoreAttr),
            "terminator live-out must keep the final store"
        );
    }
}
