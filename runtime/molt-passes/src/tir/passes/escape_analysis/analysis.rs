//! Escape-state analysis and the derived alloc-root fact sets
//! (`rewritable_alloc_roots`, `dict_requiring_alloc_roots`,
//! `finalizer_alloc_roots`). See the module-level docs on [`super`].

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_result_absorbs_operand_ownership_table;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::super::effects;
use super::classify::{
    EscapeState, UseInfo, attr_str, is_alloc_site, is_borrowing_builtin, is_borrowing_method_call,
    is_container_builder_passthrough, is_pure_move_copy, store_attr_value_operand_index,
};

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
                // Dynamic method dispatch: check if the method is known non-storing.
                // Pure methods on immutable types (str, tuple, int, float,
                // frozenset) never capture their receiver or arguments.
                OpCode::CallMethod | OpCode::CallMethodIc | OpCode::CallSuperMethodIc => {
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
                // CheckedAdd/CheckedMul operate on raw i64 scalars (never heap
                // refs).
                OpCode::Add
                | OpCode::CheckedAdd
                | OpCode::CheckedMul
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
                | OpCode::UnpackSequence
                | OpCode::ForIter
                | OpCode::StateSwitch
                | OpCode::ClosureLoad
                | OpCode::CheckException
                | OpCode::ExceptionPending
                // Reads a scalar version stamp out of the function object; the
                // function operand is only borrowed (read), never captured.
                | OpCode::FunctionDefaultsVersion
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
pub(super) fn rewritable_alloc_roots(func: &TirFunction) -> HashSet<ValueId> {
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
pub(super) fn dict_requiring_alloc_roots(func: &TirFunction) -> HashSet<ValueId> {
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
