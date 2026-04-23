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

use super::effects;
use super::PassStats;

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
    if effects::builtin_effects(name).map_or(false, |fx| fx.effect_free) {
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
fn is_borrowing_method_call(attrs: &AttrDict) -> bool {
    let method = match attr_str(attrs, "method") {
        Some(m) => m,
        None => return false,
    };
    let receiver_type = match attr_str(attrs, "receiver_type") {
        Some(rt) => rt,
        None => return false,
    };
    effects::method_effects(receiver_type, method).map_or(false, |fx| fx.effect_free)
}

/// Analyze escape state of all `Alloc` operations in `func`.
///
/// Returns a map from each `Alloc` result `ValueId` to its `EscapeState`.
pub fn analyze(func: &TirFunction) -> HashMap<ValueId, EscapeState> {
    // Step 1: Find all Alloc ops and their result ValueIds.
    let mut escapes: HashMap<ValueId, EscapeState> = HashMap::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Alloc {
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
                OpCode::CallBuiltin => {
                    let borrows = attr_str(&use_info.attrs, "name")
                        .map_or(false, |name| is_borrowing_builtin(name));
                    if !borrows {
                        escapes.insert(val, EscapeState::GlobalEscape);
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
                // Convention: operands[0] = target, operands[1] = value (or attr name operand).
                // For StoreAttr: operands = [target, value], attr name in attrs.
                // For StoreIndex: operands = [target, index, value].
                OpCode::StoreAttr => {
                    // operands[0] = target, operands[1] = value
                    if use_info.operand_index == 1 {
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
        // Rewrite Alloc → StackAlloc for NoEscape values.
        for op in &mut block.ops {
            if op.opcode == OpCode::Alloc && op.results.iter().any(|r| no_escape.contains(r)) {
                op.opcode = OpCode::StackAlloc;
                stats.values_changed += 1;
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

    /// Test 6: Empty function → empty results.
    #[test]
    fn empty_function_produces_empty_results() {
        let func = TirFunction::new("empty".into(), vec![], TirType::None);
        let escapes = analyze(&func);
        assert!(escapes.is_empty());
    }
}
