use std::collections::HashMap;

use crate::repr::Repr;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::lower_to_simple::SimpleValueNames;
use crate::tir::op_kinds_generated::{
    ReprProjectableBoolResultRule, ReprProjectableFloatResultRule, ReprRawI64FullDeoptSeedRule,
    opcode_repr_projectable_bool_result_rule_table,
    opcode_repr_projectable_float_result_rule_table,
    opcode_repr_raw_i64_full_deopt_seed_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Enumerate every TIR `ValueId` with a carrier representation slot: every op
/// result and every block argument in the function being lowered.
fn value_ids_for(tir_func: &TirFunction) -> Vec<ValueId> {
    let mut ids = Vec::new();
    for block in tir_func.blocks.values() {
        ids.extend(block.args.iter().map(|arg| arg.id));
        for op in &block.ops {
            ids.extend(op.results.iter().copied());
        }
    }
    ids
}

fn value_type_by_id_for(tir_func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut types = tir_func.value_types.clone();
    for block in tir_func.blocks.values() {
        for arg in &block.args {
            types.entry(arg.id).or_insert_with(|| arg.ty.clone());
        }
    }
    types
}

/// The backend-neutral construction of the value-keyed representation lattice
/// map: the **single source of truth** for TIR `ValueId` carrier
/// classification, consumed identically by the LLVM backend and the WASM/LIR
/// backend.
///
/// Every value we know a `ValueId` for floors to [`Repr::default_for`] of its
/// refined `TirType`; bool and f64 therefore enter the value map as
/// [`Repr::Bool`] / [`Repr::FloatUnboxed`], while proven integer raw carriers
/// are raised into one of two explicit tiers. The floor makes this a complete
/// value->Repr map.
///
/// The `RawI64Safe` raise is sourced from the **value-range analysis** (S6)
/// when a [`ValueRangeResult`] is supplied (`vr` = `Some`): a `ValueId` is
/// `RawI64Safe` exactly when [`ValueRangeResult::fits_inline_int47`] proves its
/// entire range lies in `[-2^46, 2^46 - 1]`. The `RawI64FullDeopt` raise is
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
/// path — NO value is raised: every int floors to `MaybeBigInt` (conservative,
/// boxed, BigInt-correct; never a miscompile, at worst a perf bail).
///
/// `tir_func` is the post-pipeline TIR the backend is lowering, and `vr` (when
/// present) MUST have been computed on that same `tir_func` so its `ValueId`s
/// line up. This function is deliberately pure TIR and does not consult
/// SimpleIR names.
pub fn repr_by_value_for(
    tir_func: &TirFunction,
    vr: Option<&crate::tir::passes::value_range::ValueRangeResult>,
) -> HashMap<ValueId, Repr> {
    let value_types = value_type_by_id_for(tir_func);
    let mut repr_by_value: HashMap<ValueId, Repr> = value_ids_for(tir_func)
        .into_iter()
        .map(|id| {
            let repr = value_types
                .get(&id)
                .map(Repr::default_for)
                .unwrap_or(Repr::DynBox);
            (id, repr)
        })
        .collect();
    // No value-range supplied → the conservative floor stands. Every int is
    // `MaybeBigInt` (boxed, BigInt-safe); no raw-i64 carrier is minted.
    let Some(vr) = vr else {
        return repr_by_value;
    };
    // Seed the raw-i64-safe set from the value-range proof + the GPU-intrinsic
    // pre-seed, then propagate it across value-preserving SSA edges (`Copy`
    // chains and phis) so loop-carried induction variables — whose phi has no
    // direct `fits_inline_int47` fact but whose every incoming value is proven —
    // inherit the carrier. Shared with the RC drop-insertion raw-scalar filter
    // (`raw_i64_safe_values_for`) — single source of truth.
    let overflow_safe_values = raw_i64_safe_values_for_with_types(tir_func, vr, &value_types);
    for &id in &overflow_safe_values {
        repr_by_value.insert(id, Repr::RawI64Safe);
    }
    let full_deopt_values =
        raw_i64_full_deopt_values_for_with_types(tir_func, &overflow_safe_values, &value_types);
    for &id in &full_deopt_values {
        repr_by_value.insert(id, Repr::RawI64FullDeopt);
    }
    repr_by_value
}

/// Compute the value-range analysis for `tir_func` — the proof source for the
/// value-keyed (WASM/LLVM) `RawI64Safe` promotion. Computed on the SAME TIR the
/// backend lowers so its `ValueId`s line up with [`repr_by_value_for`].
pub(crate) fn projected_scalar_carrier_name_reprs_for(
    tir_func: &TirFunction,
    names: &SimpleValueNames,
) -> Vec<(String, Repr)> {
    let vr = value_range_for(tir_func);
    let repr_by_value = repr_by_value_for(tir_func, Some(&vr));
    let carrier_by_value = native_projectable_scalar_reprs_for(tir_func, &repr_by_value);
    let mut out = Vec::new();

    let mut push_value = |name: String, value: ValueId| {
        if let Some(&repr) = carrier_by_value.get(&value) {
            out.push((name, repr));
        }
    };

    let mut block_ids: Vec<BlockId> = tir_func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|block_id| block_id.0);
    for block_id in block_ids {
        let block = &tir_func.blocks[&block_id];
        for (index, arg) in block.args.iter().enumerate() {
            push_value(names.value_name(arg.id), arg.id);
            push_value(names.block_arg_slot(block.id, index), arg.id);
        }
        for op in &block.ops {
            let simple_out = op.attrs.get("_simple_out").and_then(|attr| match attr {
                AttrValue::Str(name) => Some(name.as_str()),
                _ => None,
            });
            for (result_index, &result) in op.results.iter().enumerate() {
                push_value(names.value_name(result), result);
                if result_index == 0
                    && let Some(simple_out) = simple_out
                {
                    push_value(simple_out.to_string(), result);
                }
            }
        }
    }

    out
}

fn native_projectable_scalar_reprs_for(
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
        if propagate_native_projectable_identity_values(
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

pub fn value_range_for(
    tir_func: &TirFunction,
) -> crate::tir::passes::value_range::ValueRangeResult {
    let scev = crate::tir::passes::scev::compute_scev(tir_func);
    crate::tir::passes::value_range::compute_value_range(tir_func, &scev)
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
#[cfg(test)]
pub(crate) fn raw_i64_safe_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::passes::value_range::ValueRangeResult,
) -> std::collections::HashSet<ValueId> {
    let value_types = value_type_by_id_for(tir_func);
    raw_i64_safe_values_for_with_types(tir_func, vr, &value_types)
}

fn raw_i64_safe_values_for_with_types(
    tir_func: &TirFunction,
    vr: &crate::tir::passes::value_range::ValueRangeResult,
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

/// All bare-i64 carriers, independent of box-site tier.
pub(crate) fn raw_i64_carrier_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::passes::value_range::ValueRangeResult,
) -> std::collections::HashSet<ValueId> {
    let value_types = value_type_by_id_for(tir_func);
    let mut raw = raw_i64_safe_values_for_with_types(tir_func, vr, &value_types);
    raw.extend(raw_i64_full_deopt_values_for_with_types(
        tir_func,
        &raw,
        &value_types,
    ));
    raw
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
    vr: &crate::tir::passes::value_range::ValueRangeResult,
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
            if matches!(op.opcode, OpCode::Shl | OpCode::Shr) {
                let count_in_range = op
                    .operands
                    .get(1)
                    .map(|&count| vr.range_of(count).proves_i64_shift_count())
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
            if crate::tir::passes::value_identity::copy_value_source(op)
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
            if op_original_kind(op) == Some("float_from_obj")
                || crate::tir::passes::value_identity::copy_value_source(op).is_some_and(
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

fn op_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

fn block_arg_incomings_for(tir_func: &TirFunction) -> HashMap<(BlockId, usize), Vec<ValueId>> {
    let reachable = crate::tir::dominators::executable_reachable_blocks(tir_func);
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
        match &block.terminator {
            Terminator::Branch { target, args } => add_edge(*target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                add_edge(*then_block, then_args);
                add_edge(*else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, target, args) in cases {
                    add_edge(*target, args);
                }
                add_edge(*default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
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
            (Some(Repr::RawI64Safe), Repr::RawI64FullDeopt)
            | (Some(Repr::RawI64FullDeopt), Repr::RawI64Safe)
            | (Some(Repr::RawI64FullDeopt), Repr::RawI64FullDeopt) => Some(Repr::RawI64FullDeopt),
            _ => return None,
        };
    }
    saw_any.then_some(merged).flatten()
}

fn propagate_native_projectable_identity_values(
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
            if let Some(source) = crate::tir::passes::value_identity::copy_value_source(op)
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
        if carrier_by_value.contains_key(&id) {
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
        if let Some(repr) = projected
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
            if op.opcode != OpCode::Call {
                continue;
            }
            let is_gpu_index_intrinsic = matches!(
                op.attrs.get("s_value"),
                Some(AttrValue::Str(name))
                    if matches!(
                        name.as_str(),
                        "molt_gpu_thread_id"
                            | "molt_gpu_block_id"
                            | "molt_gpu_block_dim"
                            | "molt_gpu_grid_dim"
                    )
            );
            if is_gpu_index_intrinsic {
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
    let reachable = crate::tir::dominators::executable_reachable_blocks(tir_func);
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
        match &block.terminator {
            Terminator::Branch { target, args } => add_edge(*target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                add_edge(*then_block, then_args);
                add_edge(*else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, target, args) in cases {
                    add_edge(*target, args);
                }
                add_edge(*default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

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
            // Forward raw-i64 safety through a `Copy` ONLY when it is a genuine
            // value-identity move (`copy`/`copy_var`/`store_var`/`load_var`/
            // `identity_alias`, or a plain attribute-free SSA copy) — the
            // fail-closed `copy_value_source` predicate, shared with the
            // value-range pass so the two cannot drift. A Copy that CARRIES an
            // operator (`inplace_lshift`/`inplace_add`/`str_from_obj`/… — the
            // frontend lifts the in-place augmented ops and conversions to
            // `Copy{_original_kind}`) is NOT a value move: its result is the
            // operator's result, not operand 0. Forwarding safety through it
            // unconditionally marked an `a <<= 80` result (a `Copy{inplace_lshift}`
            // of a small lhs) RawI64Safe purely because the lhs `1` fit inline —
            // so the LLVM/WASM shift lane emitted a RAW machine shift by 80, which
            // LLVM constant-folds to `poison` (shift >= bit width is UB). The
            // value-range op-result range for the first-class `Shl`/`Shr` already
            // refuses the overflow case; this aligns the propagation's Copy
            // forwarding with that same value-identity contract.
            if let Some(source) = crate::tir::passes::value_identity::copy_value_source(op)
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
