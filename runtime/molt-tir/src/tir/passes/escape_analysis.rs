//! Escape analysis pass for TIR.
//!
//! Determines whether heap-allocated values escape the current function.
//! Values that don't escape (`NoEscape`) are rewritten from `Alloc` to
//! `StackAlloc`, and their `IncRef`/`DecRef` ops are elided.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    copy_kind_is_explicit_no_heap_move_table, kind_result_absorbs_operand_ownership_table,
    opcode_is_escape_alloc_site_table, opcode_result_absorbs_operand_ownership_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
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

/// Returns `true` when an `OpCode::Copy` op is a genuine SSA move (its result
/// aliases its operand — the same heap object), as opposed to the opaque
/// `_original_kind` passthrough that `kind_to_opcode` assigns to SimpleIR ops
/// without a dedicated TIR opcode.
///
/// A move has either no `_original_kind` (a true SSA-lift copy) or an
/// `_original_kind` the generated registry proves is a no-heap move of operand 0
/// (the named SSA/var moves plus the validate-and-pass-through guards). Anything
/// else under `Copy` is a passthrough whose result is a *distinct* value (e.g. a
/// freshly built container), so it must NOT be aliased to its operand.
///
/// The kind set is the single generated authority `op_kinds.toml`
/// `classifier_no_heap_move` (`copy_kind_is_explicit_no_heap_move_table`), shared
/// with `alias_analysis.rs` and the ownership lattice — escape analysis no longer
/// keeps a private hand-list that could diverge. Every alias-propagation site
/// below reads `op.operands.first()` / `op.results.first()`, which matches the
/// registry's operand-0 pure-move contract for all those kinds (including the
/// guard passthroughs), so consuming the broader authority is sound and only
/// tightens the alias relation toward the rest of the compiler.
fn is_pure_move_copy(attrs: &AttrDict) -> bool {
    match attr_str(attrs, "_original_kind") {
        None => true,
        Some(kind) => copy_kind_is_explicit_no_heap_move_table(kind),
    }
}

/// Returns `true` when an `OpCode::Copy` op is the passthrough carrier for a
/// container constructor. Such ops absorb their operand lifetimes into a new
/// container that may outlive the frame, so every operand must be treated as
/// escaping — exactly like the first-class `BuildList`/`BuildDict`/… opcodes.
fn is_container_builder_passthrough(attrs: &AttrDict) -> bool {
    attr_str(attrs, "_original_kind").is_some_and(kind_result_absorbs_operand_ownership_table)
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
    let (receiver_type, method) = if let Some(rest) = method_attr.strip_prefix("BoundMethod:") {
        // Frontend canonical form: split on the first ':' to
        // recover (receiver_type, method_name).  Both halves
        // must be non-empty for the lookup to succeed.
        let mut parts = rest.splitn(2, ':');
        match (parts.next(), parts.next()) {
            (Some(rcv), Some(mthd)) if !rcv.is_empty() && !mthd.is_empty() => (rcv, mthd),
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
/// want to track for escape state.
///
/// * `Alloc` — generic heap blocks.
/// * `ObjectNewBound` — class-instance allocation from the frontend's
///   class-instantiation fold.
/// * `BuildList` / `BuildDict` / `BuildTuple` / `BuildSet` / `AllocTask` —
///   container / task allocation sites (S5 phase 1). Tracking these as escape
///   roots lets the alias analysis classify a freshly-built container's escape
///   state; it is sound because `apply` only ever *rewrites* `Alloc` /
///   `ObjectNewBound` opcodes (never the `Build*` family), so adding these
///   roots can only refine the escape map, never change which ops get
///   stack-promoted.
#[inline]
fn is_alloc_site(opcode: OpCode) -> bool {
    opcode_is_escape_alloc_site_table(opcode)
}

/// Return the operand that carries the stored value for StoreAttr-family ops.
///
/// The TIR opcode intentionally groups several SimpleIR store variants behind
/// `StoreAttr`; the preserved `_original_kind` defines operand roles for the
/// variants whose attribute name/class guard is also an SSA operand.
fn store_attr_value_operand_index(attrs: &AttrDict, operand_count: usize) -> Option<usize> {
    let value_index = match attr_str(attrs, "_original_kind") {
        Some("set_attr_name") => 2,
        Some("guarded_field_set") | Some("guarded_field_init") => 3,
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

    let mut alloc_set: HashSet<ValueId> = escapes.keys().copied().collect();

    // Step 2: Build use-map — for each alloc'd ValueId, collect all uses.
    let mut use_map: HashMap<ValueId, Vec<UseInfo>> = HashMap::new();
    // Also track "stored-into" relationships: if value B is stored into A's
    // field, record (A -> B) so we can propagate escape from A to B.
    let mut stored_into: Vec<(ValueId, ValueId)> = Vec::new();

    // Step 1b: Track *pure SSA move* `Copy` aliases of allocation results.
    //
    // `OpCode::Copy` is overloaded in this IR: it is BOTH a pure SSA move
    // (result and operand name the same object) AND the opaque carrier for any
    // SimpleIR op that has no dedicated TIR opcode (the `_original_kind`
    // passthrough — see `kind_to_opcode`'s `_ => OpCode::Copy` fallback and the
    // `lower_to_simple` Copy reconstruction). Container constructors
    // (`list_new`, `dict_new`, `tuple_new`, `set_new`) ride this passthrough, so
    // a freshly-constructed object flowing into a literal appears as the operand
    // of a `Copy`-carried `list_new` whose *result is a new container*, not an
    // alias. Only a genuine move aliases its source; treat those (and only
    // those) as alias edges. Passthrough constructors are handled as escapes in
    // Step 3 (`is_container_builder_passthrough`).
    //
    // For a real move `tmp = Copy obj`, record a `(tmp -> obj)` propagation edge
    // and track `tmp` so its own uses are scanned; the Step 4 fixpoint then
    // escalates `obj` whenever any alias escapes. Without this, `[Box()]`'s
    // `obj = ObjectNewBound; tmp = move obj; <consume tmp>` left `obj` wrongly
    // `NoEscape` and stack-promoted — a use-after-free that release-mode codegen
    // masked while dev-mode codegen surfaced as a dangling element. Iterate to a
    // fixpoint so moves-of-moves are covered.
    let mut copy_added = true;
    while copy_added {
        copy_added = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode != OpCode::Copy || !is_pure_move_copy(&op.attrs) {
                    continue;
                }
                let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) else {
                    continue;
                };
                if alloc_set.contains(&src) && !alloc_set.contains(&dst) {
                    alloc_set.insert(dst);
                    escapes.insert(dst, EscapeState::NoEscape);
                    stored_into.push((dst, src));
                    copy_added = true;
                }
            }
        }
    }

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
            // `StateDispatch` has no condition value; only its per-edge args.
            Terminator::StateDispatch {
                cases,
                default_args,
                ..
            } => {
                let mut v = Vec::new();
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

    // Monotone escalation to a lattice point: never lowers an existing state
    // (the lattice order is NoEscape < ArgEscape < GlobalEscape, encoded by the
    // derived `Ord`). Fail-closed: a value only ever moves UP the lattice.
    let escalate = |escapes: &mut HashMap<ValueId, EscapeState>, val: ValueId, to: EscapeState| {
        let cur = escapes.get(&val).copied().unwrap_or(EscapeState::NoEscape);
        if to > cur {
            escapes.insert(val, to);
        }
    };

    // Step 3: Classify each use.
    for (&val, uses) in &use_map {
        for use_info in uses {
            if opcode_result_absorbs_operand_ownership_table(use_info.opcode) {
                escapes.insert(val, EscapeState::GlobalEscape);
                continue;
            }
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
                    if borrows {
                        // The callee provably only borrows (reads) the value; it
                        // crossed a call boundary but was NOT captured. Record
                        // that boundary crossing as `ArgEscape` — the value does
                        // not escape the *function* (still stack-promotable), but
                        // it is no longer purely frame-local. This realizes the
                        // ArgEscape lattice point.
                        escalate(&mut escapes, val, EscapeState::ArgEscape);
                    } else {
                        // Before escalating to GlobalEscape, check if the
                        // callee is effect_free. An effect_free function
                        // cannot store its arguments (storing is a side
                        // effect), so the value stays at its current escape
                        // level (NoEscape or ArgEscape) rather than jumping
                        // to GlobalEscape.
                        let callee_effect_free = name
                            .and_then(effects::builtin_effects)
                            .is_some_and(|fx| fx.effect_free);
                        if callee_effect_free {
                            // Effect-free callee borrows without capture: record
                            // the call-boundary crossing as ArgEscape.
                            escalate(&mut escapes, val, EscapeState::ArgEscape);
                        } else {
                            escapes.insert(val, EscapeState::GlobalEscape);
                        }
                    }
                }
                // CallMethod: check if the method is known non-storing.
                // Pure methods on immutable types (str, tuple, int, float,
                // frozenset) never capture their receiver or arguments.
                OpCode::CallMethod => {
                    let borrows = is_borrowing_method_call(&use_info.attrs);
                    if borrows {
                        // Borrowing method: arg crossed a call boundary without
                        // capture → ArgEscape (still stack-promotable).
                        escalate(&mut escapes, val, EscapeState::ArgEscape);
                    } else {
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
                OpCode::ModuleCacheSet => {
                    if use_info.operand_index == 1 {
                        // The module value is retained in the runtime cache
                        // and mirrored into sys.modules.
                        escapes.insert(val, EscapeState::GlobalEscape);
                    }
                }
                OpCode::ModuleSetAttr => {
                    if use_info.operand_index == 2 {
                        // Module dictionaries outlive the module init frame.
                        escapes.insert(val, EscapeState::GlobalEscape);
                    }
                }
                OpCode::ModuleCacheDel
                | OpCode::ModuleDelGlobal
                | OpCode::ModuleDelGlobalIfPresent => {
                    // Deletes mutate global module state but do not store the
                    // operand value anywhere.
                }
                // Local ops that don't cause escape.
                // CheckedAdd operates on raw i64 scalars (never heap refs).
                OpCode::Add
                | OpCode::CheckedAdd
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
                | OpCode::OrdAt
                | OpCode::DelIndex
                | OpCode::BoxVal
                | OpCode::UnboxVal
                | OpCode::TypeGuard
                | OpCode::IncRef
                | OpCode::DecRef
                | OpCode::DeleteVar
                | OpCode::DelBoundary
                | OpCode::GetIter
                | OpCode::IterNext
                | OpCode::IterNextUnboxed
                | OpCode::ForIter
                | OpCode::StateSwitch
                | OpCode::ClosureLoad
                | OpCode::CheckException
                | OpCode::ExceptionPending
                // Reads a scalar version stamp out of the function object; the
                // function operand is only borrowed (read), never captured.
                | OpCode::FunctionDefaultsVersion
                | OpCode::Deopt
                | OpCode::WarnStderr
                | OpCode::TryStart
                | OpCode::TryEnd
                | OpCode::StateBlockStart
                | OpCode::StateBlockEnd => {
                    // No escape.
                }
                // `Copy` is overloaded: a pure SSA move (no escape — the move
                // alias is propagated separately in Step 1b/Step 4), the
                // passthrough carrier for container constructors (operands
                // escape into the new container), or the passthrough carrier for
                // some other SimpleIR op without a dedicated opcode. Only the
                // pure move is non-escaping; every passthrough is treated as an
                // escape because the carried op's storing semantics are not
                // modeled here (conservative-correct — it can only over-approximate).
                OpCode::Copy => {
                    if is_pure_move_copy(&use_info.attrs) {
                        // No escape — handled as an alias edge.
                    } else if is_container_builder_passthrough(&use_info.attrs) {
                        // Element flows into a (possibly escaping) container.
                        escapes.insert(val, EscapeState::GlobalEscape);
                    } else {
                        // Unknown passthrough op — assume it may capture the value.
                        escapes.insert(val, EscapeState::GlobalEscape);
                    }
                }
                // Other constructors/captures whose result may retain operands.
                // BuildList/Dict/Tuple/Set are normally consumed by the generated
                // absorption fact above; keeping them here is the fail-closed
                // exhaustive match behavior if that table ever changes.
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
                | OpCode::ConstBigInt
                | OpCode::ConstFloat
                | OpCode::ConstStr
                | OpCode::ConstBool
                | OpCode::ConstNone
                | OpCode::ConstBytes
                | OpCode::Import
                | OpCode::ImportFrom
                | OpCode::ModuleCacheGet
                | OpCode::ModuleGetAttr
                | OpCode::ModuleImportFrom
                | OpCode::ModuleGetGlobal
                | OpCode::ModuleGetName
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

/// The set of values that are results of, or transparent-move aliases of, a
/// *rewritable* allocation site (`Alloc` / `ObjectNewBound`). These are the only
/// values eligible for stack promotion + RC removal in [`apply`]. Container /
/// task allocation sites (`Build*` / `AllocTask`) are tracked by the escape
/// analysis for region classification but are deliberately excluded here:
/// rewriting or RC-stripping them is not this pass's responsibility.
///
/// Mirrors `analyze`'s Step 1b move-alias propagation so a `tmp = move alloc`
/// chain is promoted exactly when `alloc` is.
fn rewritable_alloc_roots(func: &TirFunction) -> HashSet<ValueId> {
    let mut roots: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if matches!(op.opcode, OpCode::Alloc | OpCode::ObjectNewBound) {
                for &result in &op.results {
                    roots.insert(result);
                }
            }
        }
    }
    if roots.is_empty() {
        return roots;
    }
    // Propagate through pure SSA-move copies to a fixpoint.
    let mut changed = true;
    while changed {
        changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode != OpCode::Copy || !is_pure_move_copy(&op.attrs) {
                    continue;
                }
                let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) else {
                    continue;
                };
                if roots.contains(&src) && roots.insert(dst) {
                    changed = true;
                }
            }
        }
    }
    roots
}

/// A `StoreAttr` is a TYPED-SLOT store (the only attribute write a fixed-layout
/// stack object can service) iff its `_original_kind` is `store` / `store_init`
/// — the frontend's offset-keyed forms for a proven-concrete-class declared
/// field. EVERY other `StoreAttr` spelling (`set_attr_generic_ptr`,
/// `set_attr_generic_obj`, `set_attr_name`, `guarded_field_set`, …) is a
/// GENERIC, name-keyed write that routes through the instance `__dict__`. A
/// dict-routed write must materialize a heap `__dict__` and stash its pointer in
/// the instance's trailing dict slot — a stack-promoted instance (immortal,
/// fixed payload, no heap identity to anchor a `__dict__` against) cannot do
/// this, so the store silently no-ops and the matching generic load raises
/// `AttributeError`. Returns `true` for the dict-routed shape, which forces the
/// target instance to stay heap-allocated.
fn store_attr_is_dict_routed(op: &TirOp) -> bool {
    if op.opcode != OpCode::StoreAttr {
        return false;
    }
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => !matches!(kind.as_str(), "store" | "store_init"),
        // A `StoreAttr` with NO `_original_kind` is a raw SSA-lift store with no
        // offset proof; conservatively dict-routed (treat as needing a heap
        // `__dict__`). Only the explicit offset-keyed forms prove a typed slot.
        _ => true,
    }
}

/// The set of rewritable allocation ROOTS (`ObjectNewBound` / `Alloc` results)
/// whose instance is the target of at least one GENERIC (dict-routed) attribute
/// store — transitively through pure SSA-move copies. Such an instance needs a
/// heap `__dict__` and therefore MUST NOT be stack-promoted.
///
/// The dict requirement is seeded at every dict-routed `StoreAttr`'s target
/// operand, then propagated BACKWARD across pure-move copies (`dst = move src`
/// ⇒ `src` is dict-requiring whenever `dst` is) so the requirement reaches the
/// originating alloc result, which is the value `apply` actually rewrites. This
/// is the reverse of `rewritable_alloc_roots`'s forward alloc→copy propagation
/// and uses the same `is_pure_move_copy` alias relation, so the two analyses
/// agree on exactly which values name the same heap object.
fn dict_requiring_alloc_roots(func: &TirFunction) -> HashSet<ValueId> {
    let mut dict_required: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if store_attr_is_dict_routed(op)
                && let Some(&target) = op.operands.first()
            {
                dict_required.insert(target);
            }
        }
    }
    if dict_required.is_empty() {
        return dict_required;
    }
    // Backward propagation across pure-move copies to a fixpoint: if a copy's
    // RESULT requires a dict, its SOURCE (the same heap object) requires one too.
    let mut changed = true;
    while changed {
        changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode != OpCode::Copy || !is_pure_move_copy(&op.attrs) {
                    continue;
                }
                let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) else {
                    continue;
                };
                if dict_required.contains(&dst) && dict_required.insert(src) {
                    changed = true;
                }
            }
        }
    }
    dict_required
}

/// Returns `true` when an op produces a finalizer-bearing instance. The frontend
/// stamps `defines_del=true` after resolving `__del__` through the class MRO,
/// excluding `object`; devirtualized allocation and generic class instantiation
/// both transport that same fact.
///
/// Such an instance has a finalizer that CPython runs at the LAST reference
/// drop. Stack-promoting it (→ `ObjectNewBoundStack`, which the runtime stamps
/// `HEADER_FLAG_IMMORTAL`) or stripping its `IncRef`/`DecRef` would make the
/// refcount-zero transition never occur, so `dec_ref_ptr` would never reach
/// `maybe_run_object_finalizer` and `__del__` would silently never run. This is
/// the shared mechanism behind the standing LLVM/WASM `__del__` parity hole: on
/// every lane the escape pass classified a non-escaping finalizer-bearing
/// instance as promotable and stripped its release.
pub(crate) fn op_result_defines_del(op: &TirOp) -> bool {
    !op.results.is_empty() && matches!(op.attrs.get("defines_del"), Some(AttrValue::Bool(true)))
}

/// The set of allocation roots whose class defines a `__del__` finalizer,
/// transitively through pure SSA-move copies. This is the single
/// FinalizerSensitive fact (design 27): every fast-path / lifetime-shortening
/// optimization must query it before touching representation or refcount state.
///
/// Such an instance MUST stay heap-allocated with a live refcount so the
/// finalizer-aware `dec_ref_ptr` dispatches `__del__` at the last drop; it must
/// therefore be excluded from:
///   * the stack-promotion rewrite (`ObjectNewBound → ObjectNewBoundStack`, which
///     stamps `HEADER_FLAG_IMMORTAL` so the rc-zero transition never occurs) and
///     the `IncRef`/`DecRef` strip — both in [`apply`]; and
///   * the `DecRef → Free` unique-ownership promotion in `refcount_elim` Step 6
///     (`OpCode::Free` is a direct dealloc that does NOT route through
///     `maybe_run_object_finalizer`, so it would silently skip `__del__`).
///
/// Mirrors [`dict_requiring_alloc_roots`]: the requirement is seeded at the
/// finalizer-bearing alloc and propagated FORWARD across pure-move copies (the
/// same `is_pure_move_copy` alias relation `rewritable_alloc_roots` uses), so it
/// reaches every value that names the same heap object — and in particular the
/// alloc/call result that `apply` rewrites or whose RC ops it would otherwise
/// strip.
pub(crate) fn finalizer_alloc_roots(func: &TirFunction) -> HashSet<ValueId> {
    let mut del_required: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op_result_defines_del(op) {
                for &result in &op.results {
                    del_required.insert(result);
                }
            }
        }
    }
    if del_required.is_empty() {
        return del_required;
    }
    // Forward propagation across pure-move copies to a fixpoint: a move-alias of
    // a finalizer-bearing instance names the same heap object and inherits the
    // requirement.
    let mut changed = true;
    while changed {
        changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode != OpCode::Copy || !is_pure_move_copy(&op.attrs) {
                    continue;
                }
                let (Some(&src), Some(&dst)) = (op.operands.first(), op.results.first()) else {
                    continue;
                };
                if del_required.contains(&src) && del_required.insert(dst) {
                    changed = true;
                }
            }
        }
    }
    del_required
}

/// Apply escape analysis results: rewrite non-escaping `Alloc` ops to
/// `StackAlloc`, and remove `IncRef`/`DecRef` on non-escaping values.
///
/// A value is "non-escaping the function" — and therefore stack-promotable —
/// iff its state is `NoEscape` or `ArgEscape`. `ArgEscape` means the value was
/// passed to a callee that provably only *borrows* it (an effect-free / pure
/// builtin or method) and never captures it: it crossed a call boundary but does
/// not outlive the frame, so it is exactly as promotable as a purely frame-local
/// `NoEscape` value. Only `GlobalEscape` (stored to heap/global or returned)
/// forces heap allocation. This preserves the pre-S5 behavior, under which
/// borrowing-call arguments were left at `NoEscape` and promoted.
pub fn apply(func: &mut TirFunction, escapes: &HashMap<ValueId, EscapeState>) -> PassStats {
    let mut stats = PassStats {
        name: "escape_analysis",
        values_changed: 0,
        attrs_changed: 0,
        ops_removed: 0,
        ops_added: 0,
        facts_changed: 0,
    };

    // The escape map now tracks container / task allocation sites too (so the
    // alias analysis can classify their escape state). But stack-promotion and
    // RC removal here apply ONLY to the originally-rewritable allocation roots
    // (`Alloc` / `ObjectNewBound`) and their transparent-move aliases. Touching a
    // `BuildList` / `AllocTask` result's refcount would be unsound (its RC
    // balance is the runtime's, not this pass's, to manage — dropping it risks a
    // leak or use-after-free). Restrict the promotable set accordingly; this
    // exactly preserves the pre-S5 contract.
    let rewritable_roots = rewritable_alloc_roots(func);

    // Instances that receive a generic (dict-routed) attribute store need a heap
    // `__dict__` and must NOT be stack-promoted: a fixed-layout immortal stack
    // object cannot anchor a `__dict__`, so `g.method = fn` (an out-of-layout
    // store) silently no-ops and `g.method()` then raises AttributeError. Exclude
    // these roots from promotion exactly as escape would (heap allocation), the
    // structurally-correct precondition for the dict-materialization path.
    let dict_required = dict_requiring_alloc_roots(func);

    // Instances whose class defines a `__del__` finalizer must stay heap-allocated
    // with a live refcount: stack-promoting them (→ IMMORTAL) or stripping their
    // RC would make the refcount-zero transition never happen, so `dec_ref_ptr`
    // would never dispatch `__del__`. Exclude them from BOTH the stack rewrite and
    // the RC strip below — the single shared fix for the LLVM/WASM/native `__del__`
    // parity hole. The non-finalizer common case is untouched (perf preserved).
    let del_required = finalizer_alloc_roots(func);

    // Collect non-escaping (NoEscape ∪ ArgEscape) values that are rewritable
    // allocation roots — those that do not escape the function and are therefore
    // safe to stack-promote / drop RC on. `ArgEscape` (borrowed-but-not-captured)
    // is as promotable as `NoEscape`.
    let no_escape: HashSet<ValueId> = escapes
        .iter()
        .filter(|&(vid, state)| {
            *state != EscapeState::GlobalEscape
                && rewritable_roots.contains(vid)
                && !dict_required.contains(vid)
                && !del_required.contains(vid)
        })
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

    /// Regression: a freshly-constructed object that flows into a container
    /// literal *through an SSA `Copy`* must be classified `GlobalEscape`, not
    /// `NoEscape`. The frontend lowers `[Box()]` to
    /// `obj = ObjectNewBound; tmp = Copy obj; BuildList tmp`. Without Copy-alias
    /// tracking, `obj` was wrongly left `NoEscape` and stack-promoted while the
    /// escaping list outlived the frame — a use-after-free (objects read back as
    /// type `object`) that only manifested under dev-mode codegen.
    #[test]
    fn object_new_bound_copied_into_container_escapes() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let copy_val = func.fresh_value();
        let list_val = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(
            OpCode::ObjectNewBound,
            vec![class_ref],
            vec![inst_val],
        ));
        entry
            .ops
            .push(make_op(OpCode::Copy, vec![inst_val], vec![copy_val]));
        entry
            .ops
            .push(make_op(OpCode::BuildList, vec![copy_val], vec![list_val]));
        entry.terminator = Terminator::Return {
            values: vec![list_val],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&inst_val],
            EscapeState::GlobalEscape,
            "object copied into an escaping container must escape"
        );
        assert_eq!(
            escapes[&copy_val],
            EscapeState::GlobalEscape,
            "the copy alias must escape too"
        );
    }

    /// Regression (apply-level): the same `ObjectNewBound -> Copy -> BuildList`
    /// shape, even with a payload size present, must NOT be rewritten to
    /// `ObjectNewBoundStack`, because the object escapes into the container.
    #[test]
    fn object_new_bound_copied_into_container_is_not_stack_promoted() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let copy_val = func.fresh_value();
        let list_val = func.fresh_value();

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
            .push(make_op(OpCode::Copy, vec![inst_val], vec![copy_val]));
        entry
            .ops
            .push(make_op(OpCode::BuildList, vec![copy_val], vec![list_val]));
        entry.terminator = Terminator::Return {
            values: vec![list_val],
        };

        run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();
        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBound,
            "an escaping object must stay heap-allocated"
        );
    }

    /// Build a `StoreAttr` op with the given `_original_kind` spelling targeting
    /// `target` (operand 0). The frontend's offset-keyed `store`/`store_init`
    /// forms are typed-slot writes; everything else (`set_attr_generic_ptr`, …)
    /// is dict-routed.
    fn make_store_attr(original_kind: &str, target: ValueId, value: ValueId) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert(
            "_original_kind".into(),
            AttrValue::Str(original_kind.into()),
        );
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::StoreAttr,
            operands: vec![target, value],
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    /// Regression: a non-escaping `ObjectNewBound` whose instance receives a
    /// GENERIC (dict-routed) attribute store — `g.method = fn`, lowered as
    /// `set_attr_generic_ptr` — must NOT be stack-promoted. A fixed-layout
    /// immortal stack object cannot anchor a `__dict__`, so the store would
    /// silently no-op and the later generic load raise AttributeError.
    #[test]
    fn object_new_bound_with_generic_attr_store_is_not_stack_promoted() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let fn_val = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(16));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs,
            source_span: None,
        });
        // g.method = fn  -> dict-routed generic store.
        entry
            .ops
            .push(make_store_attr("set_attr_generic_ptr", inst_val, fn_val));
        entry.terminator = Terminator::Return { values: vec![] };

        run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();
        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBound,
            "an object receiving a generic (dict-routed) attribute store must \
             stay heap-allocated so the runtime can materialize its __dict__"
        );
    }

    /// Counterpart: a non-escaping `ObjectNewBound` that receives ONLY typed-slot
    /// stores (`store_init` at a declared field offset) IS still stack-promoted —
    /// the dict-routed guard must not over-pessimize the common declared-field case.
    #[test]
    fn object_new_bound_with_only_typed_slot_store_still_stack_promotes() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let field_val = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(16));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs,
            source_span: None,
        });
        let mut store = make_store_attr("store_init", inst_val, field_val);
        store.attrs.insert("value".into(), AttrValue::Int(0)); // field offset 0
        entry.ops.push(store);
        entry.terminator = Terminator::Return { values: vec![] };

        run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();
        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBoundStack,
            "an object with only typed-slot field stores is layout-fixed and \
             must still be stack-promoted (no dict needed)"
        );
    }

    /// The dict requirement propagates BACKWARD across a pure-move copy: an
    /// `ObjectNewBound` whose move-alias receives the generic store must also be
    /// kept on the heap (the same heap object is named by both ids).
    #[test]
    fn generic_store_through_move_alias_keeps_object_heap() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let alias_val = func.fresh_value();
        let fn_val = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(16));

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
            .push(make_op(OpCode::Copy, vec![inst_val], vec![alias_val]));
        entry
            .ops
            .push(make_store_attr("set_attr_generic_ptr", alias_val, fn_val));
        entry.terminator = Terminator::Return { values: vec![] };

        run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();
        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBound,
            "a generic store through a move-alias must keep the originating \
             allocation heap-allocated"
        );
    }

    /// Build a `Copy` op carrying an `_original_kind` passthrough (the form the
    /// SSA lift assigns to SimpleIR ops without a dedicated TIR opcode, e.g.
    /// container constructors like `list_new`/`dict_new`).
    fn make_passthrough(
        original_kind: &str,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
    ) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert(
            "_original_kind".into(),
            AttrValue::Str(original_kind.into()),
        );
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    /// Regression for the real production lowering: container constructors
    /// (`list_new`/`dict_new`/…) have no dedicated TIR opcode and ride the
    /// `OpCode::Copy` `_original_kind` passthrough. A freshly-constructed object
    /// passed *directly* as such a constructor's operand — `obj = ObjectNewBound;
    /// lst = Copy[list_new] obj` — must be classified `GlobalEscape`. Before the
    /// fix the escape pass treated every `Copy` as a pure no-escape move, so the
    /// object was stack-promoted and freed while the escaping container lived on
    /// (the `[Box()]` / `{'k': Box()}` use-after-free that surfaced only under
    /// dev-mode codegen).
    #[test]
    fn object_new_bound_into_list_new_passthrough_escapes() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let list_val = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(
            OpCode::ObjectNewBound,
            vec![class_ref],
            vec![inst_val],
        ));
        entry
            .ops
            .push(make_passthrough("list_new", vec![inst_val], vec![list_val]));
        entry.terminator = Terminator::Return {
            values: vec![list_val],
        };

        let escapes = analyze(&func);
        assert_eq!(
            escapes[&inst_val],
            EscapeState::GlobalEscape,
            "object built directly into a list_new passthrough must escape"
        );
    }

    /// Same for the dict-value position (`{'k': Box()}` → `dict_new`), and
    /// asserted end-to-end through `run`: the object must stay heap-allocated.
    #[test]
    fn object_new_bound_into_dict_new_passthrough_is_not_stack_promoted() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let key_val = func.fresh_value();
        let inst_val = func.fresh_value();
        let dict_val = func.fresh_value();

        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(8));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::ConstStr, vec![], vec![key_val]));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs,
            source_span: None,
        });
        entry.ops.push(make_passthrough(
            "dict_new",
            vec![key_val, inst_val],
            vec![dict_val],
        ));
        entry.terminator = Terminator::Return {
            values: vec![dict_val],
        };

        run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();
        let obj_op = entry
            .ops
            .iter()
            .find(|op| op.results.first() == Some(&inst_val))
            .expect("object alloc op present");
        assert_eq!(
            obj_op.opcode,
            OpCode::ObjectNewBound,
            "object built into a dict_new passthrough value must stay heap-allocated"
        );
    }

    /// A genuine SSA move that does NOT escape must still allow stack promotion —
    /// the pure-move classification must not over-conservatively escape locals.
    #[test]
    fn local_object_new_bound_through_pure_move_still_stack_promotes() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let class_ref = ValueId(0);
        let inst_val = func.fresh_value();
        let moved_val = func.fresh_value();
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
        // Pure SSA move (no `_original_kind`): aliases inst_val.
        entry
            .ops
            .push(make_op(OpCode::Copy, vec![inst_val], vec![moved_val]));
        entry.ops.push(make_op(
            OpCode::LoadAttr,
            vec![moved_val],
            vec![load_result],
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_result]));
        entry.terminator = Terminator::Return {
            values: vec![const_result],
        };

        run(&mut func);
        let entry = func.blocks.get(&func.entry_block).unwrap();
        assert_eq!(
            entry.ops[0].opcode,
            OpCode::ObjectNewBoundStack,
            "a local object used only through a pure move must still stack-promote"
        );
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
        // `len()` borrows its argument across a call boundary without capturing
        // it: the value does not escape the function (still stack-promotable) but
        // is now precisely classified `ArgEscape` rather than `NoEscape` (the S5
        // realization of the ArgEscape lattice point).
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::ArgEscape,
            "len() only borrows — arg crosses a call boundary uncaptured (ArgEscape)"
        );
        assert_ne!(
            escapes[&alloc_val],
            EscapeState::GlobalEscape,
            "borrowed arg must remain non-escaping (stack-promotable)"
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
        // `str.upper` (effect-free immutable-receiver method) borrows without
        // capture: non-escaping, now precisely `ArgEscape`.
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::ArgEscape,
            "str.upper in canonical BoundMethod: form is borrowing (effect_free) — \
             alloc crosses the call boundary uncaptured (ArgEscape, non-escaping)"
        );
        assert_ne!(escapes[&alloc_val], EscapeState::GlobalEscape);
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
        attrs.insert("method".into(), AttrValue::Str("BoundMethod:str:".into()));

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
        // `print` borrows its argument for I/O without capturing it: non-escaping,
        // now precisely `ArgEscape`.
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::ArgEscape,
            "print() borrows its argument for I/O — non-escaping (ArgEscape)"
        );
        assert_ne!(escapes[&alloc_val], EscapeState::GlobalEscape);
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
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ObjectNewBound,
            operands: vec![class_ref],
            results: vec![inst_val],
            attrs: alloc_attrs,
            source_span: None,
        });
        entry.ops.push(make_op(
            OpCode::ModuleSetAttr,
            vec![module_obj, attr_name, inst_val],
            vec![],
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
        // An effect-free callee cannot store its argument, so the value does not
        // escape the function — but it did cross a call boundary, so it is
        // precisely `ArgEscape` (non-escaping, the key invariant: never
        // GlobalEscape).
        assert_eq!(
            escapes[&alloc_val],
            EscapeState::ArgEscape,
            "effect_free builtin (sorted) borrows without capture → ArgEscape"
        );
        assert_ne!(
            escapes[&alloc_val],
            EscapeState::GlobalEscape,
            "effect_free builtin must not escalate to GlobalEscape"
        );
    }
}
