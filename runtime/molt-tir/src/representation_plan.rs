use crate::repr::Repr;
use crate::tir::function::TirFunction;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::deterministic_hash::HashMap;

pub use molt_passes::representation_facts::{
    analyze_representations, repr_by_value_for, value_range_for,
};

/// Freeze the exact target-neutral parameter and return carriers used at independently
/// compiled native-object boundaries.
///
/// This is deliberately outside the LLVM feature: batch planning runs in every
/// native build, and all consumers must serialize the same carrier decision
/// before provider bodies are partitioned away.
pub fn native_linkage_types(tir_func: &TirFunction) -> (Vec<TirType>, TirType) {
    let plan = ProducerCarrierPlan::build(tir_func);
    (
        plan.effective_param_types(tir_func),
        plan.effective_return_type(tir_func),
    )
}

fn effective_value_type_from_repr(
    id: ValueId,
    declared: &TirType,
    repr_by_value: &HashMap<ValueId, Repr>,
) -> TirType {
    repr_by_value
        .get(&id)
        .copied()
        .unwrap_or_else(|| Repr::default_for(declared))
        .carrier_type(declared)
}

pub(crate) fn effective_param_types_from_repr(
    tir_func: &TirFunction,
    repr_by_value: &HashMap<ValueId, Repr>,
) -> Vec<TirType> {
    let entry_args = tir_func
        .blocks
        .get(&tir_func.entry_block)
        .map(|block| block.args.as_slice())
        .unwrap_or(&[]);
    tir_func
        .param_types
        .iter()
        .enumerate()
        .map(|(index, declared)| {
            entry_args.get(index).map_or_else(
                || Repr::default_for(declared).carrier_type(declared),
                |argument| effective_value_type_from_repr(argument.id, declared, repr_by_value),
            )
        })
        .collect()
}

/// A return slot has no ValueId. Join the carriers of its executable producers,
/// rather than using a semantic return annotation as a machine ABI. In
/// particular, a raw integer return and a boxed integer return must join boxed.
pub(crate) fn effective_return_type_from_repr(
    tir_func: &TirFunction,
    repr_by_value: &HashMap<ValueId, Repr>,
) -> TirType {
    let reachable = crate::tir::dominators::executable_reachable_blocks(tir_func);
    let mut result = TirType::Never;
    for block in tir_func
        .blocks
        .values()
        .filter(|block| reachable.contains(&block.id))
    {
        if let crate::tir::blocks::Terminator::Return { values } = &block.terminator {
            let carrier = values.first().map_or(TirType::None, |id| {
                effective_value_type_from_repr(*id, &tir_func.return_type, repr_by_value)
            });
            result = result.meet(&carrier);
        }
    }
    if result == TirType::Never {
        TirType::None
    } else {
        result
    }
}

/// Shared producer-proven physical carriers for a TIR function.
///
/// Backends lower `TirFunction` (SSA `ValueId`s) directly. This plan
/// carries the same value-keyed representation proof the WASM/LIR path consumes:
/// every backend decision here is derived from the post-pipeline TIR function
/// being lowered.
///
/// This makes the LLVM backend consume the identical typed facts the
/// native/WASM backends consume, rather than treating `TirType::I64` as an
/// exact-i64 carrier (which it is not — `type_refine` assigns `add(I64, I64) ->
/// I64` with no overflow proof, so unbounded integer arithmetic must stay
/// boxed/runtime-backed until a range proof exists).
#[derive(Clone, Debug, Default)]
pub struct ProducerCarrierPlan {
    /// The representation lattice element per TIR `ValueId`: the value-keyed
    /// source of truth. Exact scalar provenance authorizes Bool/Float carriers;
    /// annotations alone stay boxed. Inline-int47 carriers raise to
    /// [`Repr::RawI64Safe`], while checked overflow-peel carriers raise to
    /// [`Repr::RawI64FullDeopt`].
    ///
    /// The raw-i64 tiers are seeded from value-range and checked-op proofs, then
    /// propagated across TIR SSA identity edges: through `Copy` chains and block
    /// arguments (phis). Dataflow propagation is what lets the backend keep
    /// unproven accumulators boxed (`MaybeBigInt`/`DynBox`) while preserving
    /// proven raw carriers.
    pub repr_by_value: HashMap<ValueId, Repr>,
}

impl ProducerCarrierPlan {
    pub fn carrier_refcount_is_noop(&self, value: ValueId) -> bool {
        self.repr_by_value
            .get(&value)
            .is_some_and(|repr| repr.carrier_refcount_is_noop())
    }
    pub fn boxed_refcount_is_noop(&self, value: ValueId) -> bool {
        self.repr_by_value
            .get(&value)
            .is_some_and(|repr| repr.boxed_refcount_is_noop())
    }
    /// Build carrier facts from the exact TIR function being lowered.
    pub fn build(tir_func: &TirFunction) -> Self {
        let repr_by_value = analyze_representations(tir_func);
        Self { repr_by_value }
    }

    /// Whether the value `id` is an inline-int47-safe raw i64 carrier: the
    /// `{RawI64Safe}` view over `repr_by_value`.
    pub fn is_inline_safe_int(&self, id: ValueId) -> bool {
        self.repr_by_value
            .get(&id)
            .is_some_and(|repr| repr.is_raw_i64_safe())
    }

    /// Whether the value `id` is the full-range checked-overflow raw i64 tier.
    pub fn is_full_deopt_int(&self, id: ValueId) -> bool {
        self.repr_by_value
            .get(&id)
            .is_some_and(|repr| repr.is_raw_i64_full_deopt())
    }

    /// Whether the value `id` is any bare-i64 carrier. Box sites must still
    /// distinguish inline-safe and full-deopt tiers.
    pub fn is_raw_int_carrier(&self, id: ValueId) -> bool {
        self.repr_by_value
            .get(&id)
            .is_some_and(|repr| repr.is_raw_i64_carrier())
    }

    /// The same producer-fact projection owns parameters, phis and explicit
    /// materialization. No annotation alone selects any raw scalar kind.
    pub fn effective_value_type(&self, id: ValueId, declared: &TirType) -> TirType {
        effective_value_type_from_repr(id, declared, &self.repr_by_value)
    }

    /// Freeze a locally defined function's physical ABI from the exact SSA
    /// carriers used by either native instruction selector.
    pub fn linkage_abi(
        &self,
        body: &TirFunction,
        signature: molt_ir::ir::ExternFunctionSignature,
        parameter_custody: Vec<molt_ir::ir::ParameterCustody>,
    ) -> molt_ir::linkage_abi::NativeFunctionLinkageAbi {
        molt_ir::linkage_abi::NativeFunctionLinkageAbi {
            source_signature: signature,
            parameter_custody,
            param_types: self.effective_param_types(body),
            return_type: signature
                .returns_value
                .then(|| self.effective_return_type(body)),
        }
    }

    /// Caller and provider use the same frozen producer-fact projection.
    pub fn effective_param_types(&self, tir_func: &TirFunction) -> Vec<TirType> {
        effective_param_types_from_repr(tir_func, &self.repr_by_value)
    }

    pub fn effective_return_type(&self, tir_func: &TirFunction) -> TirType {
        effective_return_type_from_repr(tir_func, &self.repr_by_value)
    }
}

#[cfg(test)]
pub(crate) mod test_fixtures;

#[cfg(test)]
mod tests;
