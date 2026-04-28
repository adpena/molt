//! Escape analysis pass for TIR.
//!
//! Determines whether heap-allocated values escape the current function.
//! Values that don't escape (`NoEscape`) are rewritten from `Alloc` to
//! `StackAlloc`, and their `IncRef`/`DecRef` ops are elided.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::PassStats;
use super::effects;

/// Escape lattice for allocated values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscapeState {
    /// Value never leaves the function — safe to stack allocate.
    NoEscape = 0,
    /// Passed to a callee that doesn't store it (future refinement).
    ArgEscape = 1,
    /// Stored to heap/global or returned — must heap allocate.
    GlobalEscape = 2,
}

/// A recorded use of an alloc'd value.
#[derive(Debug)]
struct UseInfo {
    /// The opcode that uses the value.
    opcode: OpCode,
    /// All operands of the using op (for Store target analysis).
    operands: Vec<ValueId>,
    /// Index of our value within the operands list.
    operand_index: usize,
    /// Attribute dictionary from the using op (for callee name lookup).
    attrs: AttrDict,
}

/// Extract a string attribute value from an `AttrDict`.
fn attr_str<'a>(attrs: &'a AttrDict, key: &str) -> Option<&'a str> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Returns `true` if the named builtin only borrows (reads) its arguments and
/// never stores them into heap-reachable locations.
///
/// We use the effects system as the source of truth: any builtin that is
/// `effect_free` cannot store its arguments (storing is a side effect).
/// Additionally, builtins like `print`, `isinstance`, `type`, etc. that have
/// I/O or introspection effects but still never *capture* their arguments are
/// included explicitly.
fn is_borrowing_builtin(name: &str) -> bool {
    // If the effects system classifies it as effect_free, it borrows.
    if effects::builtin_effects(name).is_some_and(|fx| fx.effect_free) {
        return true;
    }
    // Builtins that have side effects (I/O) but never store their arguments.
    matches!(
        name,
        "print"
            | "type"
            | "isinstance"
            | "issubclass"
            | "hasattr"
            | "getattr"
            | "id"
            | "iter"
            | "next"
            | "any"
            | "all"
            | "vars"
            | "dir"
            | "format"
    )
}

/// Returns `true` if a `CallMethod` op only borrows its operands (receiver and
/// arguments) without storing them.
///
/// Uses the effects system: a method that is `effect_free` on an immutable
/// receiver type cannot capture its arguments. Falls back to `false` for
/// unknown receiver types or methods (conservative).
///
/// Supports two encodings of method identity on the SSA `attrs` dict:
///
/// 1. **Frontend canonical form (production)**: `method` is the
///    full `BoundMethod:<receiver_type>:<method_name>` string copied
///    from the SimpleIR `s_value` of `call_method` ops.  This is
///    what the frontend's `_emit_dynamic_call` produces for
///    monomorphic builtin-method dispatches and what the native
///    backend's `s_value` match arm expects to see at codegen
///    (`function_compiler.rs:16489+`).  We parse the receiver and
///    method out inline so the existing effects table
///    (`("list", "append")`, `("str", "upper")`, …) matches.
///
/// 2. **Test / future-refined form**: `method` is a bare method
///    name AND `receiver_type` is a separate attr.  This is what
///    the existing unit tests use, and what a future SSA-lift
///    refinement would emit if we ever derive receiver type from
///    the receiver value's `TirType` directly.
///
/// The two encodings are equivalent contracts; the parse logic
/// here lets a single effects-table lookup serve both.
fn is_borrowing_method_call(attrs: &AttrDict) -> bool {
    let method_attr = match attr_str(attrs, "method") {
        Some(m) => m,
        None => return false,
    };
    let (receiver_type, method) =
        if let Some(rest) = method_attr.strip_prefix("BoundMethod:") {
            // Frontend canonical form: split on the first ':' to
            // recover (receiver_type, method_name).  Both halves
            // must be non-empty for the lookup to succeed.
            let mut parts = rest.splitn(2, ':');
            match (parts.next(), parts.next()) {
                (Some(rcv), Some(mthd)) if !rcv.is_empty() && !mthd.is_empty() => {
                    (rcv, mthd)
                }
                _ => return false,
            }
        } else {
            // Test / future-refined form: bare method name plus
            // explicit receiver_type attr.
            let receiver_type = match attr_str(attrs, "receiver_type") {
                Some(rt) => rt,
                None => return false,
            };
            (receiver_type, method_attr)
        };
    effects::method_effects(receiver_type, method).is_some_and(|fx| fx.effect_free)
}

/// Returns `true` if this opcode is an allocation site whose result we
/// want to track for escape state.  Currently `Alloc` (generic heap
/// blocks) and `ObjectNewBound` (class-instance allocation from the
/// frontend's class-instantiation fold).
#[inline]
fn is_alloc_site(opcode: OpCode) -> bool {
    matches!(opcode, OpCode::Alloc | OpCode::ObjectNewBound)
}

/// Return the operand that carries the stored value for StoreAttr-family ops.
///
/// The TIR opcode intentionally groups several SimpleIR store variants behind
/// `StoreAttr`; the preserved `_original_kind` defines operand roles for the
/// variants whose attribute name/class guard is also an SSA operand.
fn store_attr_value_operand_index(attrs: &AttrDict, operand_count: usize) -> Option<usize> {
    let value_index = match attr_str(attrs, "_original_kind") {
        Some("module_set_attr") | Some("set_attr_name") => 2,
        Some("guarded_field_set") | Some("guarded_field_set_init") => 3,
        Some("set_attr") | Some("store_attr") if operand_count >= 3 => 2,
        _ => 1,
    };
    (value_index < operand_count).then_some(value_index)
}

/// Analyze escape state of all allocation sites in `func`.
///
/// Returns a map from each allocation result `ValueId` to its
/// `EscapeState`.  An allocation site is any op for which
/// `is_alloc_site` returns `true`.
pub fn analyze(func: &TirFunction) -> HashMap<ValueId, EscapeState> {
    // Step 1: Find all alloc-site ops and their result ValueIds.
    let mut escapes: HashMap<ValueId, EscapeState> = HashMap::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            if is_alloc_site(op.opcode) {
                for &result in &op.results {
                    escapes.insert(result, EscapeState::NoEscape);
                }
            }
        }
    }

    if escapes.is_empty() {
        return escapes;
    }

    let alloc_set: HashSet<ValueId> = escapes.keys().copied().collect();

    // Step 2: Build use-map — for each alloc'd ValueId, collect all uses.
    let mut use_map: HashMap<ValueId, Vec<UseInfo>> = HashMap::new();
    // Also track "stored-into" relationships: if value B is stored into A's
    // field, record (A -> B) so we can propagate escape from A to B.
    let mut stored_into: Vec<(ValueId, ValueId)> = Vec::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            for (idx, &operand) in op.operands.iter().enumerate() {
                if alloc_set.contains(&operand) {
                    use_map.entry(operand).or_default().push(UseInfo {
                        opcode: op.opcode,
                        operands: op.operands.clone(),
                        operand_index: idx,
                        attrs: op.attrs.clone(),
                    });
                }
            }
        }

        // Check terminator uses.
        let terminator_values: Vec<ValueId> = match &block.terminator {
            Terminator::Return { values } => values.clone(),
            Terminator::Branch { args, .. } => args.clone(),
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                let mut v = vec![*cond];
                v.extend(then_args);
                v.extend(else_args);
                v
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                let mut v = vec![*value];
                for (_, _, args) in cases {
                    v.extend(args);
                }
                v.extend(default_args);
                v
            }
            Terminator::Unreachable => vec![],
        };

        // Return terminators cause GlobalEscape.
        if let Terminator::Return { values } = &block.terminator {
            for &val in values {
                if alloc_set.contains(&val) {
                    escapes.insert(val, EscapeState::GlobalEscape);
                }
            }
        }

        // Branch args that pass alloc'd values to other blocks — for now
        // we don't escalate these (the value stays in-function), but we
        // need to track them in the use map is already done above via ops.
        // Actually branch args aren't ops, just mark them if they appear in
        // non-Return terminators. These are intra-function, so no escape.
        let _ = terminator_values; // used above for Return check
    }

    // Step 3: Classify each use.
    for (&val, uses) in &use_map {
        for use_info in uses {
            match use_info.opcode {
                // Generic Call: conservative — value escapes.
                OpCode::Call => {
                    escapes.insert(val, EscapeState::GlobalEscape);
                }
                // CallBuiltin: check if the builtin only borrows its arguments.
                // A builtin with known effect_free semantics never stores its
                // arguments, so the alloc'd value doesn't escape through the call.
                //
                // PLDI 2024 ArgEscape→NoEscape downgrade: when the callee is
                // known to be effect_free, it cannot store references (storing
                // is a side effect). An ArgEscape classification through such
                // a callee is safe to leave as NoEscape rather than escalating
                // to GlobalEscape. This is strictly more precise than the
                // original analysis which only checked `is_borrowing_builtin`.
                OpCode::CallBuiltin => {
                    let name = attr_str(&use_info.attrs, "name");
                    let borrows = name.is_some_and(is_borrowing_builtin);
                    if !borrows {
                        // Before escalating to GlobalEscape, check if the
                        // callee is effect_free. An effect_free function
                        // cannot store its arguments (storing is a side
                        // effect), so the value stays at its current escape
                        // level (NoEscape or ArgEscape) rather than jumping
                        // to GlobalEscape.
                        let callee_effect_free = name
                            .and_then(effects::builtin_effects)
                            .is_some_and(|fx| fx.effect_free);
                        if !callee_effect_free {
                            escapes.insert(val, EscapeState::GlobalEscape);
                        }
                        // else: effect_free callee doesn't store references.
                        // ArgEscape → NoEscape (or stays NoEscape). Don't escalate.
                    }
                }
                // CallMethod: check if the method is known non-storing.
                // Pure methods on immutable types (str, tuple, int, float,
                // frozenset) never capture their receiver or arguments.
                OpCode::CallMethod => {
                    let borrows = is_borrowing_method_call(&use_info.attrs);
                    if !borrows {
                        escapes.insert(val, EscapeState::GlobalEscape);
                    }
                }
                // Generator yields: value escapes.
                OpCode::Yield | OpCode::YieldFrom => {
                    escapes.insert(val, EscapeState::GlobalEscape);
                }
                // Raise: value escapes (exception propagation).
                OpCode::Raise => {
                    escapes.insert(val, EscapeState::GlobalEscape);
                }
                // StoreAttr / StoreIndex: check if target is also alloc'd.
                // StoreAttr groups SimpleIR variants whose value operand is
                // determined by `_original_kind`; see
                // `store_attr_value_operand_index`.
                // For StoreIndex: operands = [target, index, value].
                OpCode::StoreAttr => {
                    if use_info.operand_index
                        == store_attr_value_operand_index(&use_info.attrs, use_info.operands.len())
                            .unwrap_or(usize::MAX)
                    {
                        // This alloc'd value is being stored as a field value.
                        let target = use_info.operands[0];
                        if alloc_set.contains(&target) {
                            // Stored into another alloc — record for propagation.
                            stored_into.push((target, val));
                        } else {
                            // Stored into a non-alloc (heap object) → escapes.
                            escapes.insert(val, EscapeState::GlobalEscape);
                        }
                    }
                    // If operand_index == 0, this value is the target being written to.
                    // That's fine — it's a local mutation.
                }
                OpCode::StoreIndex => {
                    // operands[0] = target, operands[1] = index, operands[2] = value
                    if use_info.operand_index == 2 {
                        let target = use_info.operands[0];
                        if alloc_set.contains(&target) {
                            stored_into.push((target, val));
                        } else {
                            escapes.insert(val, EscapeState::GlobalEscape);
                        }
                    }
                    // target or index position: local use.
                }
                // Local ops that don't cause escape.
                OpCode::Add
                | OpCode::Sub
                | OpCode::Mul
                | OpCode::InplaceAdd
                | OpCode::InplaceSub
                | OpCode::InplaceMul
                | OpCode::Div
                | OpCode::FloorDiv
                | OpCode::Mod
                | OpCode::Pow
                | OpCode::Neg
                | OpCode::Pos
                | OpCode::Eq
                | OpCode::Ne
                | OpCode::Lt
                | OpCode::Le
                | OpCode::Gt
                | OpCode::Ge
                | OpCode::Is
                | OpCode::IsNot
                | OpCode::In
                | OpCode::NotIn
                | OpCode::BitAnd
                | OpCode::BitOr
                | OpCode::BitXor
                | OpCode::BitNot
                | OpCode::Shl
                | OpCode::Shr
                | OpCode::And
                | OpCode::Or
                | OpCode::Not
                | OpCode::Bool
                | OpCode::LoadAttr
                | OpCode::DelAttr
                | OpCode::Index
                | OpCode::DelIndex
                | OpCode::BoxVal
                | OpCode::UnboxVal
                | OpCode::TypeGuard
                | OpCode::IncRef
                | OpCode::DecRef
                | OpCode::Copy
                | OpCode::GetIter
                | OpCode::IterNext
                | OpCode::IterNextUnboxed
                | OpCode::ForIter
                | OpCode::StateSwitch
                | OpCode::ClosureLoad
                | OpCode::CheckException
                | OpCode::Deopt
                | OpCode::WarnStderr
                | OpCode::TryStart
                | OpCode::TryEnd
                | OpCode::StateBlockStart
                | OpCode::StateBlockEnd => {
                    // No escape.
                }
                // Build containers: if alloc'd value is an element, it escapes
                // into the new container (which may itself escape).
                OpCode::BuildList
                | OpCode::BuildDict
                | OpCode::BuildTuple
                | OpCode::BuildSet
                | OpCode::BuildSlice
                | OpCode::AllocTask => {
                    escapes.insert(val, EscapeState::GlobalEscape);
                }
                // Constants, imports, alloc, free, stack alloc — shouldn't
                // appear as uses of an alloc'd value, but be safe.
                OpCode::Alloc
                | OpCode::StackAlloc
                | OpCode::ObjectNewBound
                | OpCode::ObjectNewBoundStack
                | OpCode::Free
                | OpCode::ConstInt
                | OpCode::ConstFloat
                | OpCode::ConstStr
                | OpCode::ConstBool
                | OpCode::ConstNone
                | OpCode::ConstBytes
                | OpCode::Import
                | OpCode::ImportFrom
                | OpCode::StateTransition
                | OpCode::StateYield
                | OpCode::ChanSendYield
                | OpCode::ChanRecvYield
                | OpCode::ClosureStore
                | OpCode::ScfIf
                | OpCode::ScfFor
                | OpCode::ScfWhile
                | OpCode::ScfYield => {
                    // Conservative: treat as escape.
                    escapes.insert(val, EscapeState::GlobalEscape);
                }
            }
        }
    }

    // Step 4: Fixpoint propagation.
    // If target A escapes, then any value stored into A also escapes.
    let mut changed = true;
    while changed {
        changed = false;
        for &(target, stored_val) in &stored_into {
            let target_state = escapes
                .get(&target)
                .copied()
                .unwrap_or(EscapeState::NoEscape);
            let stored_state = escapes
                .get(&stored_val)
                .copied()
                .unwrap_or(EscapeState::NoEscape);
            if target_state > stored_state {
                escapes.insert(stored_val, target_state);
                changed = true;
            }
        }
    }

    escapes
}

/// Apply escape analysis results: rewrite `NoEscape` `Alloc` ops to `StackAlloc`,
/// and remove `IncRef`/`DecRef` on `NoEscape` values.
pub fn apply(func: &mut TirFunction, escapes: &HashMap<ValueId, EscapeState>) -> PassStats {
    let mut stats = PassStats {
        name: "escape_analysis",
        values_changed: 0,
        ops_removed: 0,
        ops_added: 0,
    };

    // Collect NoEscape values.
    let no_escape: HashSet<ValueId> = escapes
        .iter()
        .filter(|&(_, state)| *state == EscapeState::NoEscape)
        .map(|(&vid, _)| vid)
        .collect();

    if no_escape.is_empty() {
        return stats;
    }

    for block in func.blocks.values_mut() {
        // Rewrite alloc-site opcodes for NoEscape values:
        //   Alloc           → StackAlloc
        //   ObjectNewBound  → ObjectNewBoundStack  (Phase 5 step 3)
        //
        // The ObjectNewBound rewrite requires the op to carry the
        // payload size (in bytes) on its `value` attr — the frontend
        // sets this from `class_info["size"]` for typed classes.
        // Without the size, the backend's StackSlot lowering cannot
        // determine the slot size, so we must NOT rewrite or the
        // backend would either fall back to heap (wasting an op
        // kind) or — worse, if the heap fallback were missing —
        // SIGSEGV.  When the size is missing, the heap path stands.
        for op in &mut block.ops {
            if op.opcode == OpCode::Alloc && op.results.iter().any(|r| no_escape.contains(r)) {
                op.opcode = OpCode::StackAlloc;
                stats.values_changed += 1;
            } else if op.opcode == OpCode::ObjectNewBound
                && op.results.iter().any(|r| no_escape.contains(r))
            {
                // Only rewrite when we have a payload size to size
                // the StackSlot with.  The frontend always emits the
                // size for the class-instantiation fold, but defend
                // against synthetic ops that lack it.
                let has_size = matches!(
                    op.attrs.get("value"),
                    Some(crate::tir::ops::AttrValue::Int(v)) if *v > 0
                );
                if has_size {
                    op.opcode = OpCode::ObjectNewBoundStack;
                    stats.values_changed += 1;
                }
            }
        }

        // Remove IncRef/DecRef on NoEscape values.
        let before_len = block.ops.len();
        block.ops.retain(|op| {
            !((op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
                && op.operands.iter().any(|o| no_escape.contains(o)))
        });
        stats.ops_removed += before_len - block.ops.len();
    }

    stats
}

/// Convenience: analyze + apply in one step.
pub fn run(func: &mut TirFunction) -> PassStats {
    let escapes = analyze(func);
    apply(func, &escapes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    /// Helper to make a simple TirOp.
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

    /// Phase 5 step 2: ObjectNewBound result that is only used by
    /// LoadAttr (i.e. observed locally, never returned, never stored
    /// into a non-alloc heap location, never passed to an escaping
    /// op) is classified as NoEscape — the same lattice value as a
    /// local-only `Alloc`.  This is the prerequisite for the
    /// ObjectNewBound → ObjectNewBoundStack rewrite that lands in
    /// step 3.
    #[test]
    fn local_only_object_new_bound_is_no_escape() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0); // function parameter standing in for the class ref
        let inst_val = func.fresh_value();
        let load_result = func.fresh_value();
        let const_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(
            OpCode::ObjectNewBound,
            vec![class_ref],
            vec![inst_val],
        ));
        entry
            .ops
            .push(make_op(OpCode::LoadAttr, vec![inst_val], vec![load_result]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(escapes[&inst_val], EscapeState::NoEscape);
    }

    #[test]
    fn local_only_object_new_bound_with_layout_rewrites_to_stack() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let load_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(8));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs,
            source_span: None,
        });
        entry
            .ops
            .push(make_op(OpCode::LoadAttr, vec![inst_val], vec![load_result]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let stats = run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();

        assert_eq!(stats.values_changed, 1);
        assert_eq!(entry.ops[0].opcode, OpCode::ObjectNewBoundStack);
    }

    #[test]
    fn local_only_object_new_bound_without_layout_stays_heap() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let load_result = func.fresh_value();
        let const_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(
            OpCode::ObjectNewBound,
            vec![class_ref],
            vec![inst_val],
        ));
        entry
            .ops
            .push(make_op(OpCode::LoadAttr, vec![inst_val], vec![load_result]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let stats = run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();

        assert_eq!(stats.values_changed, 0);
        assert_eq!(entry.ops[0].opcode, OpCode::ObjectNewBound);
    }

    /// Phase 5 step 2: ObjectNewBound result that is returned escapes
    /// — same lattice handling as `Alloc`.
    #[test]
    fn returned_object_new_bound_is_global_escape() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(
            OpCode::ObjectNewBound,
            vec![class_ref],
            vec![inst_val],
        ));
        entry.terminator = Terminator::Return {
            values: vec![inst_val],
        };

        let escapes = analyze(&func);
        assert_eq!(escapes[&inst_val], EscapeState::GlobalEscape);
    }

    /// Test 1: Local-only alloc (created, field read, no escape) → NoEscape.
    #[test]
    fn local_only_alloc_is_no_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let load_result = func.fresh_value();
        let const_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op(
            OpCode::LoadAttr,
            vec![alloc_val],
            vec![load_result],
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(escapes[&alloc_val], EscapeState::NoEscape);
    }

    /// Test 2: Returned alloc → GlobalEscape.
    #[test]
    fn returned_alloc_is_global_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::DynBox);
        let alloc_val = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.terminator = Terminator::Return {
            values: vec![alloc_val],
        };

        let escapes = analyze(&func);
        assert_eq!(escapes[&alloc_val], EscapeState::GlobalEscape);
    }

    /// Test 3: Alloc stored into another (non-alloc) object's field → GlobalEscape.
    #[test]
    fn alloc_stored_into_non_alloc_field_is_global_escape() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let param = ValueId(0); // function parameter, not an alloc
        let alloc_val = func.fresh_value();
        let const_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        // StoreAttr: target=param (non-alloc), value=alloc_val
        entry
            .ops
            .push(make_op(OpCode::StoreAttr, vec![param, alloc_val], vec![]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(escapes[&alloc_val], EscapeState::GlobalEscape);
    }

    /// Helper to make a TirOp with attributes.
    fn make_op_with_attrs(
        opcode: OpCode,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
        attrs: AttrDict,
    ) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    /// Test: len() (borrowing builtin) does not cause GlobalEscape.
    #[test]
    fn borrowing_builtin_len_does_not_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str("len".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallBuiltin,
            vec![alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::NoEscape,
            "len() only borrows — alloc should not escape"
        );
    }

    /// Test: list.append() (mutating method) DOES cause GlobalEscape.
    #[test]
    fn mutating_method_append_causes_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let list_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("method".into(), AttrValue::Str("append".into()));
        attrs.insert("receiver_type".into(), AttrValue::Str("list".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![list_val]));
        // list_val.append(alloc_val) — alloc_val is stored into the list
        entry.ops.push(make_op_with_attrs(
            OpCode::CallMethod,
            vec![list_val, alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::GlobalEscape,
            "list.append() stores its argument — alloc must escape"
        );
    }

    /// Production frontend canonical form: the SSA lift copies the
    /// SimpleIR `call_method` op's `s_value` (e.g.
    /// `"BoundMethod:list:append"`) into `attrs["method"]` and does
    /// NOT set `receiver_type`.  Pre-fix, `is_borrowing_method_call`
    /// returned false on the missing receiver_type and the CallMethod
    /// arm still defaulted to GlobalEscape — *correct but for the
    /// wrong reason*: the borrow check never fired even for genuinely
    /// borrowing methods.  This test pins that, post-fix, the
    /// frontend canonical form correctly classifies a mutating
    /// method (list.append) as escaping.
    #[test]
    fn frontend_canonical_form_list_append_still_escapes() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let list_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert(
            "method".into(),
            AttrValue::Str("BoundMethod:list:append".into()),
        );

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![list_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallMethod,
            vec![list_val, alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::GlobalEscape,
            "list.append in canonical BoundMethod: form must escape \
             — list.append is not in the effect_free table"
        );
    }

    /// Production frontend canonical form: a *pure* monomorphic
    /// builtin method (e.g. `str.upper`) on an alloc'd receiver
    /// should classify as NoEscape — `str.upper` returns a new
    /// string and doesn't capture self.  Before the borrow-check
    /// fix that parses the `BoundMethod:` prefix, this test would
    /// FAIL: with no `receiver_type` attr set, the old code fell
    /// through to GlobalEscape.  Post-fix, the parse extracts
    /// `(str, upper)` from the method attr, looks up
    /// `effects::method_effects("str", "upper")`, sees `effect_free`,
    /// and stays at NoEscape.
    #[test]
    fn frontend_canonical_form_str_upper_does_not_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert(
            "method".into(),
            AttrValue::Str("BoundMethod:str:upper".into()),
        );

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallMethod,
            vec![alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::NoEscape,
            "str.upper in canonical BoundMethod: form should be \
             classified as borrowing (effect_free) — alloc stays \
             at NoEscape"
        );
    }

    /// Malformed `BoundMethod:` strings (empty receiver, empty
    /// method, missing colon) fall through to false.  Soundness
    /// failure mode: false-positive borrow ⇒ stack-allocated value
    /// dangling past frame ⇒ UAF.  Better to default to escape.
    #[test]
    fn malformed_bound_method_string_defaults_to_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        // No method portion (string ends at the receiver colon).
        attrs.insert(
            "method".into(),
            AttrValue::Str("BoundMethod:str:".into()),
        );

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallMethod,
            vec![alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::GlobalEscape,
            "malformed BoundMethod: string must NOT be parsed into \
             a successful borrow check — soundness over precision"
        );
    }

    /// Test: print() (I/O but borrowing) does not cause GlobalEscape.
    #[test]
    fn borrowing_builtin_print_does_not_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str("print".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallBuiltin,
            vec![alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::NoEscape,
            "print() borrows its argument for I/O — alloc should not escape"
        );
    }

    /// Test 4: Alloc passed to Call → GlobalEscape (conservative).
    #[test]
    fn alloc_passed_to_call_is_global_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry
            .ops
            .push(make_op(OpCode::Call, vec![alloc_val], vec![call_result]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(escapes[&alloc_val], EscapeState::GlobalEscape);
    }

    /// Test 5: Alloc with only local reads → NoEscape, IncRef/DecRef removed.
    #[test]
    fn no_escape_removes_incref_decref() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let load_result = func.fresh_value();
        let const_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry
            .ops
            .push(make_op(OpCode::IncRef, vec![alloc_val], vec![]));
        entry.ops.push(make_op(
            OpCode::LoadAttr,
            vec![alloc_val],
            vec![load_result],
        ));
        entry
            .ops
            .push(make_op(OpCode::DecRef, vec![alloc_val], vec![]));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let stats = run(&mut func);

        // Alloc should be rewritten to StackAlloc.
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.ops[0].opcode, OpCode::StackAlloc);

        // IncRef and DecRef should be removed.
        assert!(
            !entry
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::IncRef || op.opcode == OpCode::DecRef)
        );

        assert_eq!(stats.values_changed, 1);
        assert_eq!(stats.ops_removed, 2);
    }

    /// Phase 5 step 3 soundness regression: an `ObjectNewBound`
    /// whose result flows into a `CallMethod` (i.e. `pts.append(p)`
    /// in user code, lowered as `CALL_METHOD(append, [pts, p])`)
    /// MUST be classified as `GlobalEscape`.  The production frontend
    /// does not set `receiver_type` on `CallMethod` ops — only the
    /// `method` attribute is set from the SimpleIR `s_value` field —
    /// so `is_borrowing_method_call` falls through to `false` and
    /// the CallMethod arm correctly defaults to `GlobalEscape`.
    ///
    /// If anyone ever propagates `receiver_type=list` to production
    /// CallMethod ops AND adds `list.append` to the borrowing list
    /// without considering element-escape, this test will catch the
    /// regression: stack-allocating `p` would leave `pts` holding
    /// dangling pointers into a popped frame.
    #[test]
    fn object_new_bound_into_list_append_escapes_via_call_method() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let list_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        // ObjectNewBound carrying a positive payload size (24 bytes)
        // — the same shape the frontend emits for a typed class with
        // 2 int fields + the trailing __dict__ slot.
        let mut alloc_attrs = AttrDict::new();
        alloc_attrs.insert("value".into(), AttrValue::Int(24));

        // CallMethod with only `method=append` — production does NOT
        // emit `receiver_type`, so the borrow check falls through to
        // false and the value escapes.
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("method".into(), AttrValue::Str("append".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs: alloc_attrs,
            source_span: None,
        });
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![list_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallMethod,
            vec![list_val, inst_val],
            vec![call_result],
            call_attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let stats = run(&mut func);

        // The Point alloc must NOT have been rewritten — it escapes
        // via list.append, and stack-allocating it would leave the
        // list with a dangling pointer when the frame pops.
        let entry = &func.blocks[&func.entry_block];
        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBound,
            "ObjectNewBound stored into a list must NOT be rewritten to ObjectNewBoundStack — \
             would dangle when the frame pops"
        );
        assert_eq!(
            stats.values_changed, 0,
            "no values should be rewritten when the alloc escapes"
        );
    }

    #[test]
    fn object_new_bound_stored_to_module_attr_escapes() {
        let mut func = TirFunction::new(
            "molt_init_typing".into(),
            vec![TirType::DynBox, TirType::Str, TirType::DynBox],
            TirType::None,
        );
        let module_obj = ValueId(0);
        let attr_name = ValueId(1);
        let class_ref = ValueId(2);
        let inst_val = func.fresh_value();
        let const_result = func.fresh_value();

        let mut alloc_attrs = AttrDict::new();
        alloc_attrs.insert("value".into(), AttrValue::Int(24));
        let mut store_attrs = AttrDict::new();
        store_attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("module_set_attr".into()),
        );

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs: alloc_attrs,
            source_span: None,
        });
        entry.ops.push(make_op_with_attrs(
            OpCode::StoreAttr,
            vec![module_obj, attr_name, inst_val],
            vec![],
            store_attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let stats = run(&mut func);
        let entry = &func.blocks[&func.entry_block];

        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBound,
            "module globals outlive the module init frame, so stored objects must stay heap allocated"
        );
        assert_eq!(stats.values_changed, 0);
    }

    /// Test 6: Empty function → empty results.
    #[test]
    fn empty_function_produces_empty_results() {
        let func = TirFunction::new("empty".into(), vec![], TirType::None);
        let escapes = analyze(&func);
        assert!(escapes.is_empty());
    }

    /// Test 7: effect_free builtin (e.g. `sorted`) does not cause GlobalEscape.
    /// This tests the PLDI 2024 ArgEscape→NoEscape downgrade: an effect_free
    /// callee cannot store its arguments, so passing an alloc'd value to it
    /// should NOT escalate the escape state to GlobalEscape.
    #[test]
    fn effect_free_builtin_does_not_escalate_escape() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_result = func.fresh_value();

        let mut attrs = AttrDict::new();
        // `sorted` is effect_free (per effects.rs) but NOT in the
        // explicit is_borrowing_builtin list. The ArgEscape→NoEscape
        // downgrade should catch this.
        attrs.insert("name".into(), AttrValue::Str("sorted".into()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op_with_attrs(
            OpCode::CallBuiltin,
            vec![alloc_val],
            vec![call_result],
            attrs,
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::NoEscape,
            "effect_free builtin (sorted) should not escalate escape state"
        );
    }
}
