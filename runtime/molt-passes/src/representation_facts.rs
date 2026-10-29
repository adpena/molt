use std::collections::{HashMap, HashSet};

use crate::repr::Repr;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ReprProjectableBoolResultRule, ReprProjectableFloatResultRule, ReprRawI64FullDeoptSeedRule,
    opcode_repr_projectable_bool_result_rule_table,
    opcode_repr_projectable_float_result_rule_table,
    opcode_repr_raw_i64_full_deopt_seed_rule_table, opcode_requires_i64_shift_count_guard_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn value_type_by_id_for(tir_func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut types = tir_func.value_types.clone();
    for block in tir_func.blocks.values() {
        for arg in &block.args {
            types.entry(arg.id).or_insert_with(|| arg.ty.clone());
        }
    }
    // An annotation or refinement describes Python semantics, not an exact
    // scalar producer. Preserve structural types, but rebuild every scalar
    // fact from the same provenance solver used by optimization effects. This
    // covers result slots, copies and executable phis without a predicate-only
    // exception or an annotation-based fallback into an unboxed lane.
    types.retain(|_, ty| crate::tir::op_kinds_generated::comparison_scalar_domain(ty).is_none());
    types.extend(crate::tir::type_refine::extract_exact_scalar_map(tir_func));
    types
}

/// The backend-neutral construction of the value-keyed representation lattice
/// map: the **single source of truth** for TIR `ValueId` carrier
/// classification, consumed identically by the LLVM backend and the WASM/LIR
/// backend.
///
/// Every defined `ValueId` has a conservative carrier. Only exact scalar
/// provenance raises bool/f64 into [`Repr::Bool`] / [`Repr::FloatUnboxed`];
/// annotations and guards cannot authorize this raise. Exact integers need
/// additional range or checked-overflow proof for either raw integer tier.
///
/// The `RawI64Safe` raise is sourced from the **value-range analysis** (S6)
/// when a [`ValueRangeResult`] is supplied (`vr` = `Some`): a `ValueId` is
/// eligible for `RawI64Safe` only when exact integer provenance and
/// [`ValueRangeResult::fits_inline_int47`] prove its entire range lies in
/// `[-2^46, 2^46 - 1]`. The `RawI64FullDeopt` raise is
/// sourced from checked-overflow arithmetic and propagates only across identity
/// edges whose carrier remains in the checked full-i64 lane. This split keeps
/// box-site discipline structural: inline-safe values may use inline int
/// boxing; full-deopt values must use overflow-safe boxing or a checked raw
/// runtime ABI.
///
/// The GPU thread/block-id intrinsics — which the value-range analysis has no
/// model for — are pre-seeded `RawI64Safe` (their results are hardware
/// lane/grid indices, structurally in `[0, 2^20)`), preserving GPU kernel
/// codegen.
///
/// When no value-range is supplied (`vr` = `None`) — a pre-TIR / unanalysed
/// path — NO integer value is raised: every int floors to `MaybeBigInt` (conservative,
/// boxed, BigInt-correct; never a miscompile, at worst a perf bail).
///
/// `tir_func` is the post-pipeline TIR the backend is lowering, and `vr` (when
/// present) MUST have been computed on that same `tir_func` so its `ValueId`s
/// line up. This function is deliberately pure TIR and does not consult
/// SimpleIR names.
pub fn repr_by_value_for(
    tir_func: &TirFunction,
    vr: Option<&crate::tir::ValueRangeResult>,
) -> HashMap<ValueId, Repr> {
    let value_types = value_type_by_id_for(tir_func);
    repr_by_value_with_types(tir_func, vr, &value_types)
}

fn repr_by_value_with_types(
    tir_func: &TirFunction,
    vr: Option<&crate::tir::ValueRangeResult>,
    value_types: &HashMap<ValueId, TirType>,
) -> HashMap<ValueId, Repr> {
    // Initialize each carrier once. Only the producer-proven map may mint
    // exact bool/float carriers; declarations retain their conservative floor.
    let representation = |value: ValueId, declared: Option<&TirType>| match value_types.get(&value)
    {
        Some(TirType::Bool) => Repr::Bool,
        Some(TirType::F64) => Repr::FloatUnboxed,
        Some(ty) => Repr::default_for(ty),
        None => declared.map(Repr::default_for).unwrap_or(Repr::DynBox),
    };
    let mut repr_by_value = HashMap::new();
    for block in tir_func.blocks.values() {
        for arg in &block.args {
            repr_by_value.insert(arg.id, representation(arg.id, Some(&arg.ty)));
        }
        for op in &block.ops {
            for &result in &op.results {
                repr_by_value.insert(
                    result,
                    representation(result, tir_func.value_types.get(&result)),
                );
            }
        }
    }
    // No value-range supplied → the conservative floor stands. Every int is
    // `MaybeBigInt` (boxed, BigInt-safe); no raw-i64 carrier is minted.
    let Some(vr) = vr else {
        return repr_by_value;
    };
    // Seed the raw-i64-safe set from the value-range proof + the GPU-intrinsic
    // pre-seed, then propagate it across value-preserving SSA edges (`Copy`
    // chains and phis) so loop-carried induction variables — whose phi has no
    // direct `fits_inline_int47` fact but whose every incoming value is proven —
    // inherit the carrier. RC drop-insertion's non-heap filter consumes this
    // same map — single source of truth.
    let overflow_safe_values = raw_i64_safe_values_for_with_types(tir_func, vr, value_types);
    for &id in &overflow_safe_values {
        repr_by_value.insert(id, Repr::RawI64Safe);
    }
    let full_deopt_values =
        raw_i64_full_deopt_values_for_with_types(tir_func, &overflow_safe_values, value_types);
    for &id in &full_deopt_values {
        repr_by_value.insert(id, Repr::RawI64FullDeopt);
    }
    constrain_boxed_binding_observations(tir_func, &mut repr_by_value);
    repr_by_value
}

/// A scalar fact describes a mathematical value, not permission to mint another
/// object for a binding view. A full-width view that reaches a boxed consumer
/// must borrow the persistent box published by its store. Keep raw-only views
/// raw; the scalar, range and recurrence solvers remain independent of this
/// physical-carrier constraint.
fn constrain_boxed_binding_observations(func: &TirFunction, repr: &mut HashMap<ValueId, Repr>) {
    use crate::tir::passes::alias_analysis::build_alias_union_find;
    use crate::tir::passes::ownership_lattice_min::binding_view_roots;
    use crate::tir::passes::value_identity::scalar_value_source;

    // Most functions have no full-width binding publication. They need no
    // alias/CFG allocation or additional closure.
    if !func.blocks.values().flat_map(|block| &block.ops).any(|op| {
        op.opcode == OpCode::Copy
            && op_original_kind(op)
                .is_some_and(crate::tir::op_kinds_generated::copy_kind_is_binding_view_table)
            && op
                .results
                .iter()
                .any(|result| repr.get(result) == Some(&Repr::RawI64FullDeopt))
    }) {
        return;
    }

    let aliases = build_alias_union_find(func);
    let views = binding_view_roots(func, &aliases);
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);
    let incomings = block_arg_incomings_for(func);
    // Resolve each value's canonical alias root once; worklist edges never
    // walk an alias chain again.
    let roots: HashMap<ValueId, ValueId> = repr
        .keys()
        .map(|&value| (value, aliases.root(value)))
        .collect();
    let mut raw_aliases: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    for (&value, &carrier) in repr.iter() {
        if carrier.is_raw_i64_carrier() {
            raw_aliases.entry(roots[&value]).or_default().push(value);
        }
    }

    let mut observed = Vec::new();
    let mut invalidated = Vec::new();
    // Losing any one raw input invalidates a forwarding/checked result or
    // phi. No consumer needs to recount the other inputs on each removal.
    let mut dependents: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    // A boxed observation of a phi observes whichever incoming binding it
    // carries. Mixed owner/view phis need this edge too; the phi itself need
    // not be a binding-view root in OwnershipRootFacts.
    let mut phi_sources: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    let mut phi_arguments = HashSet::new();
    for (&(block, index), sources) in &incomings {
        if let Some(argument) = func.blocks[&block].args.get(index) {
            phi_arguments.insert(argument.id);
            phi_sources
                .entry(roots[&argument.id])
                .or_default()
                .extend(sources.iter().map(|value| roots[value]));
            if !is_raw_integer(repr, argument.id) {
                observed.push(roots[&argument.id]);
            } else {
                for &source in sources {
                    if !is_raw_integer(repr, source) {
                        invalidated.push(argument.id);
                    } else {
                        dependents.entry(source).or_default().push(argument.id);
                    }
                }
            }
        }
    }

    // Only operations whose consumption changes after losing a raw operand or result
    // need a use edge. Opaque calls are observed once, irrespective of arity;
    // copies/refcounts never become object observations by themselves.
    let mut conditional_consumers = Vec::new();
    let mut consumer_uses: HashMap<ValueId, Vec<usize>> = HashMap::new();
    for block in func
        .blocks
        .values()
        .filter(|block| reachable.contains(&block.id))
    {
        for op in &block.ops {
            let use_plan = integer_operand_use(op);
            let observation = use_plan.observation;
            let result_requirement = match observation {
                IntegerOperandObservation::UnlessRawOperandsAndResult => op.results.first(),
                _ => None,
            };
            match observation {
                IntegerOperandObservation::Never => {}
                IntegerOperandObservation::UnlessAllRaw
                | IntegerOperandObservation::UnlessRawOperandsAndResult
                    if op.operands.iter().all(|&value| is_raw_integer(repr, value))
                        && result_requirement.is_none_or(|&value| is_raw_integer(repr, value)) =>
                {
                    let index = conditional_consumers.len();
                    conditional_consumers.push(Some(op.operands.as_slice()));
                    for &operand in op.operands.iter().chain(result_requirement) {
                        if is_raw_integer(repr, operand) {
                            consumer_uses.entry(operand).or_default().push(index);
                        }
                    }
                }
                IntegerOperandObservation::Always
                | IntegerOperandObservation::UnlessAllRaw
                | IntegerOperandObservation::UnlessRawOperandsAndResult => {
                    observed.extend(
                        op.operands
                            .iter()
                            .filter(|&&value| is_raw_integer(repr, value))
                            .map(|value| roots[value]),
                    );
                }
            }
            if let Some(&result) = op.results.first()
                && is_raw_integer(repr, result)
            {
                // One scalar-forwarding operand, or all numeric operands,
                // determine the emitted result carrier. Other producers keep
                // their independent representation proof.
                let source = scalar_value_source(op);
                let inputs = if let Some(source) = source.as_ref() {
                    std::slice::from_ref(source)
                } else if use_plan.raw_result_depends_on_operands {
                    op.operands.as_slice()
                } else {
                    &[]
                };
                for &source in inputs {
                    if !is_raw_integer(repr, source) {
                        invalidated.push(result);
                    } else {
                        dependents.entry(source).or_default().push(result);
                    }
                }
            }
        }
        // Returns cross an object boundary. Conditions and switch selectors
        // consume scalars; phi argument transport is indexed above.
        if let Terminator::Return { values } = &block.terminator {
            observed.extend(values.iter().map(|value| roots[value]));
        }
    }

    // Each alias root is demanded once; each full-width value is removed
    // once. A conditional consumer becomes boxed at most once, and a phi's
    // demand traverses its incoming list once. Closure is O(V + E), including
    // high-arity operations and phis, after the canonical alias/CFG queries.
    let mut demanded = HashSet::new();
    while !observed.is_empty() || !invalidated.is_empty() {
        if let Some(root) = observed.pop() {
            if demanded.insert(root) {
                if views.contains(&root) {
                    invalidated.extend(raw_aliases.remove(&root).into_iter().flatten());
                }
                observed.extend(phi_sources.remove(&root).into_iter().flatten());
            }
            continue;
        }
        let value = invalidated.pop().expect("a carrier removal remains");
        if !is_raw_integer(repr, value) {
            continue;
        }
        repr.insert(value, Repr::MaybeBigInt);
        invalidated.extend(dependents.remove(&value).into_iter().flatten());
        if phi_arguments.contains(&value) {
            observed.push(roots[&value]);
        }
        for consumer in consumer_uses.remove(&value).into_iter().flatten() {
            if let Some(operands) = conditional_consumers[consumer].take() {
                observed.extend(
                    operands
                        .iter()
                        .filter(|&&operand| is_raw_integer(repr, operand))
                        .map(|operand| roots[operand]),
                );
            }
        }
    }
}

fn is_raw_integer(repr: &HashMap<ValueId, Repr>, value: ValueId) -> bool {
    repr.get(&value)
        .is_some_and(|repr| repr.is_raw_i64_carrier())
}

/// Only machine-scalar uses are transparent to binding identity. Opaque calls,
/// owned captures, identity comparisons and storage are object observations.
/// In particular, semantic integer types and borrowed call custody say nothing
/// about a callee's physical operand ABI.
#[derive(Clone, Copy)]
enum IntegerOperandObservation {
    Never,
    UnlessAllRaw,
    UnlessRawOperandsAndResult,
    Always,
}

/// One exhaustive physical-use policy owns both boxed observation demand and
/// operand-dependent result carriers. Adding an opcode requires an explicit
/// decision here; constants and independent producers never inherit a result
/// carrier from a boxed operand merely because its mathematical range fits.
struct IntegerOperandUse {
    observation: IntegerOperandObservation,
    raw_result_depends_on_operands: bool,
}

fn integer_operand_use(op: &TirOp) -> IntegerOperandUse {
    if crate::tir::passes::value_identity::copy_value_source(op).is_some()
        || crate::tir::dominators::exception_edge_binds_handler_arguments(op.opcode)
    {
        return IntegerOperandUse {
            observation: IntegerOperandObservation::Never,
            raw_result_depends_on_operands: false,
        };
    }
    let raw_operands = |arity| {
        if op.operands.len() == arity {
            IntegerOperandObservation::UnlessAllRaw
        } else {
            IntegerOperandObservation::Always
        }
    };
    let raw_operands_and_result = |arity| {
        if op.operands.len() == arity && op.results.len() == 1 {
            IntegerOperandObservation::UnlessRawOperandsAndResult
        } else {
            IntegerOperandObservation::Always
        }
    };
    let binary = raw_operands(2);
    let unary = raw_operands(1);
    let binary_result = raw_operands_and_result(2);
    let unary_result = raw_operands_and_result(1);
    let (observation, raw_result_depends_on_operands) = match op.opcode {
        OpCode::IncRef | OpCode::DecRef | OpCode::DelBoundary => {
            (IntegerOperandObservation::Never, false)
        }
        OpCode::CheckedAdd | OpCode::CheckedMul => (binary, true),
        OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => {
            (binary, false)
        }
        OpCode::Not | OpCode::Bool => (unary, false),
        OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor => (binary_result, true),
        OpCode::Pos | OpCode::BitNot => (unary_result, true),
        OpCode::Add
        | OpCode::InplaceAdd
        | OpCode::Sub
        | OpCode::InplaceSub
        | OpCode::Mul
        | OpCode::InplaceMul
        | OpCode::Div
        | OpCode::FloorDiv
        | OpCode::Mod
        | OpCode::Pow
        | OpCode::Neg
        | OpCode::Shl
        | OpCode::Shr => (IntegerOperandObservation::Always, true),
        OpCode::Is
        | OpCode::IsNot
        | OpCode::In
        | OpCode::NotIn
        | OpCode::And
        | OpCode::Or
        | OpCode::Alloc
        | OpCode::StackAlloc
        | OpCode::ObjectNewBound
        | OpCode::Free
        | OpCode::LoadAttr
        | OpCode::LoadMethod
        | OpCode::LoadSuperMethod
        | OpCode::StoreAttr
        | OpCode::DelAttr
        | OpCode::Index
        | OpCode::StoreIndex
        | OpCode::DelIndex
        | OpCode::DeleteVar
        | OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallLoadedMethod
        | OpCode::CallargsPushMethodReceiver
        | OpCode::CallBuiltin
        | OpCode::OrdAt
        | OpCode::BoxVal
        | OpCode::UnboxVal
        | OpCode::TypeGuard
        | OpCode::BuildList
        | OpCode::BuildDict
        | OpCode::BuildTuple
        | OpCode::BuildSet
        | OpCode::BuildSlice
        | OpCode::GetIter
        | OpCode::IterNext
        | OpCode::IterNextUnboxed
        | OpCode::UnpackSequence
        | OpCode::ForIter
        | OpCode::AllocTask
        | OpCode::StateSwitch
        | OpCode::StateTransition
        | OpCode::StateYield
        | OpCode::StateSet
        | OpCode::IsPending
        | OpCode::TaskWait
        | OpCode::ClosureLoad
        | OpCode::ClosureStore
        | OpCode::FrameContextSet
        | OpCode::Yield
        | OpCode::YieldFrom
        | OpCode::Raise
        | OpCode::CheckException
        | OpCode::ExceptionPending
        | OpCode::FunctionDefaultsVersion
        | OpCode::TryStart
        | OpCode::TryEnd
        | OpCode::StateBlockStart
        | OpCode::StateBlockEnd
        | OpCode::ModuleSlot
        | OpCode::ConstInt
        | OpCode::ConstBigInt
        | OpCode::ConstFloat
        | OpCode::ConstStr
        | OpCode::ConstBool
        | OpCode::ConstNone
        | OpCode::ConstBytes
        | OpCode::Copy
        | OpCode::Import
        | OpCode::ImportFrom
        | OpCode::ModuleCacheGet
        | OpCode::ModuleCacheSet
        | OpCode::ModuleCacheDel
        | OpCode::ModuleGetAttr
        | OpCode::ModuleImportFrom
        | OpCode::ModuleGetGlobal
        | OpCode::ModuleGetName
        | OpCode::ModuleSetAttr
        | OpCode::ModuleDelGlobal
        | OpCode::ModuleDelGlobalIfPresent
        | OpCode::WarnStderr
        | OpCode::ScfIf
        | OpCode::ScfFor
        | OpCode::ScfWhile
        | OpCode::ScfYield => (IntegerOperandObservation::Always, false),
    };
    IntegerOperandUse {
        observation,
        raw_result_depends_on_operands,
    }
}

pub fn native_projectable_scalar_reprs_for(
    tir_func: &TirFunction,
    repr_by_value: &HashMap<ValueId, Repr>,
) -> HashMap<ValueId, Repr> {
    let value_types = value_type_by_id_for(tir_func);
    let mut carrier_by_value: HashMap<ValueId, Repr> = repr_by_value
        .iter()
        .filter_map(|(&value, &repr)| match repr {
            Repr::RawI64Safe | Repr::RawI64FullDeopt
                if is_raw_i64_semantic_candidate(&value_types, value) =>
            {
                Some((value, repr))
            }
            _ => None,
        })
        .collect();

    let mut changed = true;
    while changed {
        changed = false;
        for block in tir_func.blocks.values() {
            for op in &block.ops {
                for result_index in 0..op.results.len() {
                    if let Some(repr) = native_projectable_op_result_repr(
                        op,
                        result_index,
                        &carrier_by_value,
                        repr_by_value,
                        &value_types,
                    ) {
                        let result = op.results[result_index];
                        if insert_projected_value_repr(
                            &mut carrier_by_value,
                            result,
                            repr,
                            &value_types,
                        ) {
                            changed = true;
                        }
                    }
                }
            }
        }
        if propagate_native_projectable_bool_float_values(
            tir_func,
            repr_by_value,
            &value_types,
            &mut carrier_by_value,
        ) {
            changed = true;
        }
    }

    carrier_by_value
}

pub fn value_range_for(tir_func: &TirFunction) -> crate::tir::ValueRangeResult {
    // The registered analysis shares one loop forest with SCEV and ranges.
    // Calling their standalone entries would discover the same loops twice.
    <crate::tir::passes::value_range::ValueRange as crate::tir::analysis::Analysis>::compute(
        tir_func,
    )
}

/// The set of `ValueId`s the backend may carry as a **bare i64** (`RawI64Safe`):
/// physically not NaN-boxed, no heap reference, raw machine arithmetic legal.
///
/// This is the SAME computation that mints `Repr::RawI64Safe` in
/// [`repr_by_value_for`] — the value-range proof seed + the overflow-peel
/// `CheckedAdd` full-range carriers + the GPU-index pre-seed, propagated across
/// value-identity SSA edges. Factored out here so it is available **on every
/// target** (the RC drop-insertion pass, design 20, runs in the shared TIR
/// pipeline on native/LLVM/WASM and must filter these raw scalars out of the
/// drop set — inserting a `DecRef` for a bare i64 register would pass a raw
/// integer to `molt_dec_ref_obj`, a type confusion). `repr_by_value_for` now
/// delegates its raw-i64 raise to this function (single source of truth).
///
/// `vr` MUST have been computed on this exact `tir_func` so its `ValueId`s line
/// up. The result is a strict subset of the values that genuinely fit i64, so a
/// missed promotion is at worst a perf bail (a spurious-but-harmless DecRef on a
/// value that turns out inline — the runtime `molt_dec_ref_obj` fast-paths
/// non-pointer tags), never an unsound carrier.
#[cfg(any(test, feature = "test-util"))]
pub fn raw_i64_safe_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
) -> std::collections::HashSet<ValueId> {
    let value_types = value_type_by_id_for(tir_func);
    raw_i64_safe_values_for_with_types(tir_func, vr, &value_types)
}

fn raw_i64_safe_values_for_with_types(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    let mut seed = raw_i64_safe_value_seed(tir_func, vr, value_types);
    seed.extend(gpu_intrinsic_raw_i64_values(tir_func, value_types));
    propagate_raw_i64_identity_values(tir_func, seed, value_types)
}

/// Full-range checked-overflow raw-i64 carriers. These values are not
/// inline-box-safe; their soundness comes from the overflow flag and the boxed
/// slow path.
fn raw_i64_full_deopt_values_for_with_types(
    tir_func: &TirFunction,
    inline_safe_values: &std::collections::HashSet<ValueId>,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    let mut seed = std::collections::HashSet::new();
    for block in tir_func.blocks.values() {
        for op in &block.ops {
            match opcode_repr_raw_i64_full_deopt_seed_rule_table(op.opcode) {
                ReprRawI64FullDeoptSeedRule::CheckedResultZero
                    if let Some(&result) = op.results.first()
                        && is_raw_i64_semantic_candidate(value_types, result) =>
                {
                    seed.insert(result);
                }
                ReprRawI64FullDeoptSeedRule::ConstIntNotInlineSafe => {
                    for &result in &op.results {
                        if is_raw_i64_semantic_candidate(value_types, result)
                            && !inline_safe_values.contains(&result)
                        {
                            seed.insert(result);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    propagate_raw_i64_full_deopt_identity_values(tir_func, inline_safe_values, seed, value_types)
}

/// Values without a refcounted heap obligation, derived from the same exact
/// producer and physical-carrier facts as lowering. A float annotation may
/// admit a heap-owning subclass and must never suppress its drop. `Never` is
/// the representation lattice's join bottom, not a defined-value non-heap
/// proof, so it is deliberately absent from this set.
pub(crate) fn non_heap_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
) -> std::collections::HashSet<ValueId> {
    non_heap_values_in_domain(tir_func, vr, RefcountDomain::Carrier)
}

/// Values that remain non-owning after boxing into an object field. A checked
/// full-i64 carrier has no local RC obligation but may box to a heap BigInt;
/// only the inline-safe integer tier discharges a boxed storage obligation.
/// Store elimination must use this projection, not [`non_heap_values_for`].
pub(crate) fn non_heap_boxed_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
) -> std::collections::HashSet<ValueId> {
    non_heap_values_in_domain(tir_func, vr, RefcountDomain::Boxed)
}

enum RefcountDomain {
    Carrier,
    Boxed,
}

fn non_heap_values_in_domain(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
    domain: RefcountDomain,
) -> std::collections::HashSet<ValueId> {
    let value_types = value_type_by_id_for(tir_func);
    repr_by_value_with_types(tir_func, Some(vr), &value_types)
        .into_iter()
        .filter_map(|(value, repr)| {
            (matches!(repr, Repr::RawI64Safe | Repr::Bool | Repr::FloatUnboxed)
                || (matches!(domain, RefcountDomain::Carrier) && repr == Repr::RawI64FullDeopt)
                || value_types.get(&value) == Some(&TirType::None))
            .then_some(value)
        })
        .collect()
}

/// Seed the raw-i64-safe set from the value-range proof: an **op-result**
/// `ValueId` is in the seed exactly when its entire proven range fits the signed
/// inline-int47 window `[-2^46, 2^46 - 1]`. This is a strict subset of the i64
/// domain, so the raw carrier is sound and overflow into a heap BigInt is
/// structurally impossible.
///
/// The seed is also semantically typed: only integer-family values are eligible.
/// Bool values have tiny ranges too, but their physical lane is
/// [`Repr::Bool`], not [`Repr::RawI64Safe`]. Keeping this gate here prevents
/// value-range proof from collapsing bool and int carriers back into one
/// accidental i64 lane.
///
/// **Block arguments (phis) are deliberately excluded from the direct seed.** A
/// phi is raised to `RawI64Safe` only by [`propagate_raw_i64_identity_values`]'s
/// all-incomings-raw rule. This is a hard soundness requirement of the LIR
/// lowering (`lower_to_lir`): a `RawI64Safe` (`I64`) phi slot must never be fed a
/// `MaybeBigInt`/`DynBox` incoming, or the lowering would emit an unsound unbox
/// of a possible heap BigInt. Seeding a phi directly from its own proven range
/// would bypass that check (the phi can be range-proven while a back-edge
/// incoming is not yet/never proven, e.g. an ambiguous multi-latch loop whose
/// update value is unrecognised). For the canonical induction variable the
/// range analysis ranges *both* the start constant and the back-edge update
/// value, so the IV phi is still raised — but only through the structurally safe
/// all-incomings path. Excluding phis here loses no real promotion: the only
/// phis the range analysis proves are AddRec IVs, whose incomings it also
/// proves.
fn raw_i64_safe_value_seed(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    let mut seed = std::collections::HashSet::new();
    for block in tir_func.blocks.values() {
        for op in &block.ops {
            // CheckedAdd/CheckedMul are full-range overflow-peel carriers.
            // They are seeded as RawI64FullDeopt, never as RawI64Safe.
            // A `Shl`/`Shr` result is a sound raw-i64 carrier only when its
            // machine shift-count operand is proven in `[0, 63]` — the valid
            // range for a raw i64 machine shift — *in addition to* the generic
            // inline-window proof below. The result-range proof alone is NOT
            // sufficient: `0 << 70` has result range `[0, 0]` (fits inline) yet
            // a machine shift count of 70, and `(x & 0xff) << 90` likewise. A
            // count outside `[0, 63]` is poison on LLVM (`shl`/`ashr` undefined
            // behaviour) and a silent wrong-value mask-mod-64 on WASM
            // (`i64.shl`/`i64.shr_s`), and a negative count is a Python
            // `ValueError` — all three MUST take the boxed runtime path
            // (`molt_lshift`/`molt_rshift`), which is BigInt- and
            // exception-correct (native already routes every shift there). So a
            // shift whose count is not range-proven in `[0, 63]` is excluded
            // from the raw seed here, at the single source of truth every
            // backend consults (`is_raw_i64_safe`), rather than re-guarded in
            // each backend's lowering. The proven-`[0, 63]` count case — a
            // literal `<< 1` (the peel / hot-loop shape) or a bounded variable
            // `<< (i & 63)` — keeps the raw lane and its perf.
            if opcode_requires_i64_shift_count_guard_table(op.opcode) {
                let count_in_range = op
                    .operands
                    .get(1)
                    .map(|&count| vr.range_at(block.id, count).proves_i64_shift_count())
                    .unwrap_or(false);
                if count_in_range {
                    for &result in &op.results {
                        if is_raw_i64_semantic_candidate(value_types, result)
                            && vr.fits_inline_int47(result)
                        {
                            seed.insert(result);
                        }
                    }
                }
                continue;
            }
            for &result in &op.results {
                if is_raw_i64_semantic_candidate(value_types, result)
                    && vr.fits_inline_int47(result)
                {
                    seed.insert(result);
                }
            }
        }
    }
    seed
}

fn is_raw_i64_semantic_candidate(value_types: &HashMap<ValueId, TirType>, id: ValueId) -> bool {
    matches!(value_types.get(&id), Some(TirType::I64 | TirType::BigInt))
}

fn is_bool_semantic_candidate(value_types: &HashMap<ValueId, TirType>, id: ValueId) -> bool {
    matches!(value_types.get(&id), Some(TirType::Bool))
}

fn is_float_semantic_candidate(value_types: &HashMap<ValueId, TirType>, id: ValueId) -> bool {
    matches!(value_types.get(&id), Some(TirType::F64))
}

fn value_allows_projected_repr(
    value_types: &HashMap<ValueId, TirType>,
    value: ValueId,
    repr: Repr,
) -> bool {
    match repr {
        Repr::RawI64Safe | Repr::RawI64FullDeopt => {
            is_raw_i64_semantic_candidate(value_types, value)
        }
        Repr::Bool => is_bool_semantic_candidate(value_types, value),
        Repr::FloatUnboxed => is_float_semantic_candidate(value_types, value),
        _ => false,
    }
}

fn insert_projected_value_repr(
    carrier_by_value: &mut HashMap<ValueId, Repr>,
    value: ValueId,
    repr: Repr,
    value_types: &HashMap<ValueId, TirType>,
) -> bool {
    if !value_allows_projected_repr(value_types, value, repr) {
        return false;
    }
    match carrier_by_value.get(&value).copied() {
        Some(existing) if existing == repr => false,
        Some(existing)
            if matches!(
                (existing, repr),
                (Repr::RawI64Safe, Repr::RawI64FullDeopt)
                    | (Repr::RawI64FullDeopt, Repr::RawI64Safe)
            ) =>
        {
            if existing == Repr::RawI64FullDeopt {
                false
            } else {
                carrier_by_value.insert(value, Repr::RawI64FullDeopt);
                true
            }
        }
        Some(_) => false,
        None => {
            carrier_by_value.insert(value, repr);
            true
        }
    }
}

fn native_projectable_op_result_repr(
    op: &TirOp,
    result_index: usize,
    carrier_by_value: &HashMap<ValueId, Repr>,
    repr_by_value: &HashMap<ValueId, Repr>,
    value_types: &HashMap<ValueId, TirType>,
) -> Option<Repr> {
    let result = *op.results.get(result_index)?;
    native_projectable_bool_result(op, result_index, result, carrier_by_value, value_types).or_else(
        || {
            native_projectable_float_result(
                op,
                result,
                carrier_by_value,
                repr_by_value,
                value_types,
            )
        },
    )
}

fn native_projectable_bool_result(
    op: &TirOp,
    result_index: usize,
    result: ValueId,
    carrier_by_value: &HashMap<ValueId, Repr>,
    value_types: &HashMap<ValueId, TirType>,
) -> Option<Repr> {
    if !is_bool_semantic_candidate(value_types, result) {
        return None;
    }
    let operands_all_bool = || {
        !op.operands.is_empty()
            && op
                .operands
                .iter()
                .all(|operand| carrier_by_value.get(operand) == Some(&Repr::Bool))
    };
    match opcode_repr_projectable_bool_result_rule_table(op.opcode) {
        ReprProjectableBoolResultRule::ComparisonOperands
            if crate::tir::op_semantics::op_instance_facts_for_op(op, value_types)
                .and_then(|facts| facts.result_type)
                .is_some_and(|ty| ty == TirType::Bool) =>
        {
            Some(Repr::Bool)
        }
        ReprProjectableBoolResultRule::Always => Some(Repr::Bool),
        ReprProjectableBoolResultRule::ResultOne if result_index == 1 => Some(Repr::Bool),
        ReprProjectableBoolResultRule::AllOperandsBool if operands_all_bool() => Some(Repr::Bool),
        ReprProjectableBoolResultRule::IndexRawI64Index
            if op.operands.get(1).is_some_and(|index| {
                carrier_by_value
                    .get(index)
                    .is_some_and(|repr| repr.is_raw_i64_carrier())
            }) =>
        {
            Some(Repr::Bool)
        }
        ReprProjectableBoolResultRule::CopySourceBool
            if crate::tir::passes::value_identity::scalar_value_source(op)
                .is_some_and(|source| carrier_by_value.get(&source) == Some(&Repr::Bool)) =>
        {
            Some(Repr::Bool)
        }
        _ => None,
    }
}

fn native_projectable_float_result(
    op: &TirOp,
    result: ValueId,
    carrier_by_value: &HashMap<ValueId, Repr>,
    repr_by_value: &HashMap<ValueId, Repr>,
    value_types: &HashMap<ValueId, TirType>,
) -> Option<Repr> {
    if !is_float_semantic_candidate(value_types, result) {
        return None;
    }
    let operand_is_float_input = |operand: &ValueId| {
        carrier_by_value
            .get(operand)
            .is_some_and(|repr| repr.is_float_unboxed() || repr.is_raw_i64_carrier())
            || matches!(
                repr_by_value.get(operand),
                Some(Repr::FloatUnboxed | Repr::RawI64Safe | Repr::RawI64FullDeopt)
            )
    };
    let operands_are_float_inputs =
        || !op.operands.is_empty() && op.operands.iter().all(operand_is_float_input);
    match opcode_repr_projectable_float_result_rule_table(op.opcode) {
        ReprProjectableFloatResultRule::Always => Some(Repr::FloatUnboxed),
        ReprProjectableFloatResultRule::AllOperandsProjectable if operands_are_float_inputs() => {
            Some(Repr::FloatUnboxed)
        }
        ReprProjectableFloatResultRule::FirstOperandProjectable
            if op.operands.first().is_some_and(operand_is_float_input) =>
        {
            Some(Repr::FloatUnboxed)
        }
        ReprProjectableFloatResultRule::CopySourceFloat
            if op_original_kind(op) == Some(molt_ir::OpKind::FloatFromObj)
                || crate::tir::passes::value_identity::scalar_value_source(op).is_some_and(
                    |source| {
                        carrier_by_value
                            .get(&source)
                            .is_some_and(|repr| repr.is_float_unboxed())
                            || repr_by_value.get(&source) == Some(&Repr::FloatUnboxed)
                    },
                ) =>
        {
            Some(Repr::FloatUnboxed)
        }
        _ => None,
    }
}

fn op_original_kind(op: &TirOp) -> Option<molt_ir::OpKind> {
    op.origin
}

fn block_arg_incomings_for(tir_func: &TirFunction) -> HashMap<(BlockId, usize), Vec<ValueId>> {
    let reachable = crate::tir::dominators::executable_reachable_blocks(tir_func);
    let labels = crate::tir::dominators::exception_label_to_block(tir_func);
    let mut block_arg_incomings: HashMap<(BlockId, usize), Vec<ValueId>> = HashMap::new();
    let mut add_edge = |target: BlockId, args: &[ValueId]| {
        for (index, &arg) in args.iter().enumerate() {
            block_arg_incomings
                .entry((target, index))
                .or_default()
                .push(arg);
        }
    };
    for block in tir_func.blocks.values() {
        if !reachable.contains(&block.id) {
            continue;
        }
        for op in &block.ops {
            if crate::tir::dominators::exception_edge_binds_handler_arguments(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = labels.get(label)
            {
                add_edge(target, &op.operands);
            }
        }
        block.terminator.for_each_edge(&mut add_edge);
    }
    block_arg_incomings
}

fn merge_projected_reprs<'a>(reprs: impl Iterator<Item = &'a Repr>) -> Option<Repr> {
    let mut merged: Option<Repr> = None;
    let mut saw_any = false;
    for &repr in reprs {
        saw_any = true;
        merged = match (merged, repr) {
            (None, repr) => Some(repr),
            (Some(existing), repr) if existing == repr => Some(existing),
            _ => return None,
        };
    }
    saw_any.then_some(merged).flatten()
}

fn propagate_native_projectable_bool_float_values(
    tir_func: &TirFunction,
    repr_by_value: &HashMap<ValueId, Repr>,
    value_types: &HashMap<ValueId, TirType>,
    carrier_by_value: &mut HashMap<ValueId, Repr>,
) -> bool {
    let block_arg_incomings = block_arg_incomings_for(tir_func);
    let mut changed = false;
    let mut copy_source: HashMap<ValueId, ValueId> = HashMap::new();
    let mut block_arg_ids: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    let mut all_value_ids: Vec<ValueId> = Vec::new();
    for block in tir_func.blocks.values() {
        for (index, arg) in block.args.iter().enumerate() {
            block_arg_ids.insert(arg.id, (block.id, index));
            all_value_ids.push(arg.id);
        }
        for op in &block.ops {
            if let Some(source) = crate::tir::passes::value_identity::scalar_value_source(op)
                && let Some(&result) = op.results.first()
            {
                copy_source.insert(result, source);
            }
            for &result in &op.results {
                all_value_ids.push(result);
            }
        }
    }

    for id in all_value_ids {
        if carrier_by_value.contains_key(&id) || is_raw_i64_semantic_candidate(value_types, id) {
            continue;
        }
        let projected = if let Some(source) = copy_source.get(&id) {
            carrier_by_value.get(source).copied().or_else(|| {
                (repr_by_value.get(source) == Some(&Repr::FloatUnboxed))
                    .then_some(Repr::FloatUnboxed)
            })
        } else if let Some(&(block, index)) = block_arg_ids.get(&id) {
            block_arg_incomings
                .get(&(block, index))
                .and_then(|incomings| {
                    if incomings.is_empty()
                        || incomings
                            .iter()
                            .any(|incoming| !carrier_by_value.contains_key(incoming))
                    {
                        return None;
                    }
                    merge_projected_reprs(
                        incomings
                            .iter()
                            .filter_map(|value| carrier_by_value.get(value)),
                    )
                })
        } else {
            None
        };
        // Integer carriers are already closed by repr_by_value_for, including
        // boxed binding observations and their phi dependencies. This native
        // projection must not recreate a constrained integer from a neighbor.
        // Only the native bool/float projection has a separate eligibility
        // filter and therefore needs this propagation.
        if let Some(repr @ (Repr::Bool | Repr::FloatUnboxed)) = projected
            && insert_projected_value_repr(carrier_by_value, id, repr, value_types)
        {
            changed = true;
        }
    }
    changed
}

/// The result `ValueId`s of GPU thread/block-id intrinsic calls — hardware lane,
/// block, and grid indices that are structurally bounded in `[0, 2^20)` and so
/// always fit a raw i64 carrier. The value-range analysis has no model for these
/// `Call` results, so the value-keyed representation authority seeds exactly
/// this bounded GPU-index population, never an arbitrary runtime call.
fn gpu_intrinsic_raw_i64_values(
    tir_func: &TirFunction,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    let mut values = std::collections::HashSet::new();
    for block in tir_func.blocks.values() {
        for op in &block.ops {
            if crate::tir::call_targets::gpu_runtime_result_type_for_op(op) == Some(TirType::I64) {
                for &result in &op.results {
                    if is_raw_i64_semantic_candidate(value_types, result) {
                        values.insert(result);
                    }
                }
            }
        }
    }
    values
}

/// Propagate raw-i64-safety across the TIR SSA graph to a fixpoint, starting
/// from `seed` (the value-range-proven inline-int47 carriers plus the
/// GPU-intrinsic pre-seed).
///
/// A value is raw-i64-safe when the backend may carry it as a bare i64 and emit
/// raw machine arithmetic for it. Beyond the seed, safety flows along
/// value-preserving edges:
///
/// - A `Copy` result is safe iff its source operand is.
/// - A block argument (phi) is safe iff *every* value passed to it on every
///   incoming branch edge **from a reachable block** is safe (dead-edge
///   insensitivity — see the edge-collection comment in the body).
///
/// Built upward from the seed (monotone — only ever adds safety), so the
/// worklist terminates; back-edges resolve because a phi becomes safe only once
/// all of its incomings are known safe. Because the seed is itself a strict
/// subset of the values that genuinely fit i64 (each proven by value-range or a
/// structurally-bounded GPU index), propagating across value-identity edges
/// cannot introduce an unsound carrier.
fn propagate_raw_i64_identity_values(
    tir_func: &TirFunction,
    seed: std::collections::HashSet<ValueId>,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    propagate_raw_i64_identity_values_with_phi_support(tir_func, seed, None, value_types)
}

fn propagate_raw_i64_full_deopt_identity_values(
    tir_func: &TirFunction,
    inline_safe_values: &std::collections::HashSet<ValueId>,
    seed: std::collections::HashSet<ValueId>,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    propagate_raw_i64_identity_values_with_phi_support(
        tir_func,
        seed,
        Some(inline_safe_values),
        value_types,
    )
}

fn propagate_raw_i64_identity_values_with_phi_support(
    tir_func: &TirFunction,
    seed: std::collections::HashSet<ValueId>,
    phi_support: Option<&std::collections::HashSet<ValueId>>,
    value_types: &HashMap<ValueId, TirType>,
) -> std::collections::HashSet<ValueId> {
    use std::collections::HashSet;

    // Collect block-argument incoming edges: (target block, arg index) -> list
    // of source values passed on each emitted branch edge.
    //
    // DEAD-EDGE-INSENSITIVE (the standard SCCP phi semantics): only edges
    // whose source block is reachable from the entry (under the Full policy —
    // terminator + exception edges) contribute incomings. A block no path can
    // execute passes args no execution can deliver; counting them is not
    // conservatism, it is analyzing a program that cannot run. Concretely:
    // the SSA lift keeps a vestigial `loop_end → header` edge whose args are
    // fabricated `ConstNone`s (the block is unreachable but preserved as loop
    // metadata). Counting that edge failed the all-incomings rule for EVERY
    // loop-header phi, silently demoting every frontend-peeled accumulator to
    // the boxed `molt_add` lane on the value-keyed (WASM/LLVM) backends.
    let block_arg_incomings = block_arg_incomings_for(tir_func);

    // Index Copy producers and block-arg membership for the worklist.
    let mut copy_source: HashMap<ValueId, ValueId> = HashMap::new();
    let mut block_arg_ids: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    let mut all_value_ids: Vec<ValueId> = Vec::new();
    for block in tir_func.blocks.values() {
        for (index, arg) in block.args.iter().enumerate() {
            block_arg_ids.insert(arg.id, (block.id, index));
            all_value_ids.push(arg.id);
        }
        for op in &block.ops {
            // Scalar value forwarding includes effectful guards and frame-home
            // publication. The shared fact validates the result/operand shape;
            // it never changes ownership, erases effects or refines from a tag.
            // Opaque Copy operators (arithmetic, conversions, constructors)
            // fail closed rather than borrowing their operand's representation.
            if let Some(source) = crate::tir::passes::value_identity::scalar_value_source(op)
                && let Some(&result) = op.results.first()
            {
                copy_source.insert(result, source);
            }
            for &result in &op.results {
                all_value_ids.push(result);
            }
        }
    }

    let mut safe: HashSet<ValueId> = seed;
    let mut changed = true;
    while changed {
        changed = false;
        for &id in &all_value_ids {
            if safe.contains(&id) {
                continue;
            }
            if !is_raw_i64_semantic_candidate(value_types, id) {
                continue;
            }
            let becomes_safe = if let Some(&src) = copy_source.get(&id) {
                safe.contains(&src)
            } else if let Some(&(block, index)) = block_arg_ids.get(&id) {
                block_arg_incomings
                    .get(&(block, index))
                    .is_some_and(|incomings| {
                        !incomings.is_empty()
                            && match phi_support {
                                Some(support) => {
                                    incomings
                                        .iter()
                                        .all(|src| safe.contains(src) || support.contains(src))
                                        && incomings.iter().any(|src| safe.contains(src))
                                }
                                None => incomings.iter().all(|src| safe.contains(src)),
                            }
                    })
            } else {
                false
            };
            if becomes_safe {
                safe.insert(id);
                changed = true;
            }
        }
    }
    safe
}
