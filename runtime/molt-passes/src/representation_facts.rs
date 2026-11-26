use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::repr::Repr;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ReprRawI64FullDeoptSeedRule, opcode_repr_raw_i64_full_deopt_seed_rule_table,
    opcode_requires_i64_shift_count_guard_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::value_identity::ScalarFacts;
use crate::tir::passes::value_range::compute_with_scalar_facts;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn value_type_by_id_for(facts: &ScalarFacts<'_>) -> HashMap<ValueId, TirType> {
    let tir_func = facts.body();
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
    types.extend(facts.types().iter().map(|(&id, ty)| (id, ty.clone())));
    types
}

/// The backend-neutral construction of the value-keyed representation lattice
/// map: the **single source of truth** for TIR `ValueId` carrier
/// classification, consumed by every backend.
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
    repr_by_value_from(&ScalarFacts::new(tir_func), vr)
}

/// Compute the full carrier plan, sharing one SSA proof with range analysis.
pub fn analyze_representations(tir_func: &TirFunction) -> HashMap<ValueId, Repr> {
    representations_from(&ScalarFacts::new(tir_func))
}

/// Terminal codegen also needs the observations used to justify its carriers.
/// Reuse the exact same immutable body analysis for linkage admission.
pub fn analyze_representations_and_checked_calls(
    tir_func: &TirFunction,
) -> (HashMap<ValueId, Repr>, HashSet<ValueId>) {
    let facts = ScalarFacts::new(tir_func);
    let representations = representations_from(&facts);
    let checked = facts.checked_call_results();
    (representations, checked)
}

fn representations_from(facts: &ScalarFacts<'_>) -> HashMap<ValueId, Repr> {
    let ranges = compute_with_scalar_facts(facts);
    repr_by_value_from(facts, Some(&ranges))
}

pub(crate) fn repr_by_value_from(
    facts: &ScalarFacts<'_>,
    vr: Option<&crate::tir::ValueRangeResult>,
) -> HashMap<ValueId, Repr> {
    let value_types = value_type_by_id_for(facts);
    repr_by_value_with_types(facts.body(), vr, &value_types)
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
        Some(TirType::None) => Repr::BoxedImmortal,
        Some(ty) => Repr::default_for(ty),
        None => declared.map(Repr::default_for).unwrap_or(Repr::DynBox),
    };
    let mut repr_by_value = HashMap::default();
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
        floor_module_chunk_carriers(tir_func, &mut repr_by_value);
        propagate_boxed_immortal_carriers(tir_func, &mut repr_by_value);
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
        raw_i64_full_deopt_values_for_with_types(tir_func, vr, &overflow_safe_values, value_types);
    for &id in &full_deopt_values {
        repr_by_value.insert(id, Repr::RawI64FullDeopt);
    }
    constrain_boxed_binding_observations(tir_func, &mut repr_by_value, value_types);
    floor_module_chunk_carriers(tir_func, &mut repr_by_value);
    propagate_boxed_immortal_carriers(tir_func, &mut repr_by_value);
    repr_by_value
}

/// A module chunk keeps every value boxed on the native and WASM backends
/// (`molt_ir::ir::is_module_chunk_function`), and the RC drop plane reads this
/// map to decide which values need counts. A carrier whose boxed word can own
/// a heap object (a full-range int boxes to a heap int past the inline window,
/// a float boxes NaN to a heap float) floors to the boxed lattice here, so the
/// drop plane counts exactly the values those backends hold as heap words.
/// Without the floor, a chunk passed one hoisted heap int to several calls
/// that each adopt their argument, and freed it after the first.
fn floor_module_chunk_carriers(tir_func: &TirFunction, repr: &mut HashMap<ValueId, Repr>) {
    if !molt_ir::ir::is_module_chunk_function(&tir_func.name) {
        return;
    }
    for carrier in repr.values_mut() {
        if matches!(carrier, Repr::RawI64FullDeopt | Repr::FloatUnboxed) {
            *carrier = Repr::DynBox;
        }
    }
}

/// Complete the physical boxed-word lattice in the same value-keyed authority.
/// Boxing a safe scalar cannot create a heap credit. Owned aliases preserve
/// that word without proving heap identity; phis require every incoming proof.
/// The worklist visits only dependents of newly proven values, avoiding a
/// repeated whole-function scan for long alias/CFG chains.
fn propagate_boxed_immortal_carriers(func: &TirFunction, repr: &mut HashMap<ValueId, Repr>) {
    if !repr
        .values()
        .any(|carrier| carrier.boxed_refcount_is_noop())
    {
        return;
    }
    enum Rule {
        Box(ValueId),
        Alias(ValueId),
        Phi(Vec<ValueId>),
    }
    let mut rules: Vec<(ValueId, Rule)> = Vec::new();
    let mut dependents: HashMap<ValueId, Vec<usize>> = HashMap::default();
    let mut add = |result, rule: Rule| {
        if !matches!(repr.get(&result), Some(Repr::DynBox | Repr::MaybeBigInt)) {
            return;
        }
        let index = rules.len();
        match &rule {
            Rule::Box(source) | Rule::Alias(source) => {
                dependents.entry(*source).or_default().push(index)
            }
            Rule::Phi(sources) => {
                for source in sources {
                    dependents.entry(*source).or_default().push(index);
                }
            }
        }
        rules.push((result, rule));
    };
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.results.len() != 1 {
                continue;
            }
            if matches!(op.opcode, OpCode::BoxVal | OpCode::BoxInlineInt) && op.operands.len() == 1
            {
                add(op.results[0], Rule::Box(op.operands[0]));
            } else if op.operands.len() == 1
                && (op.opcode == OpCode::IncRef
                    || (op.opcode == OpCode::Copy
                        && op.origin.is_some_and(|kind| {
                            crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(
                                kind,
                            ) || kind == molt_ir::OpKind::Borrow
                        })))
            {
                add(op.results[0], Rule::Alias(op.operands[0]));
            } else if let Some(source) =
                crate::tir::passes::value_identity::no_heap_alias_source(op)
            {
                add(op.results[0], Rule::Alias(source));
            }
        }
    }
    for ((block, index), sources) in block_arg_incomings_for(func) {
        if !sources.is_empty() {
            add(func.blocks[&block].args[index].id, Rule::Phi(sources));
        }
    }
    let mut remaining: Vec<usize> = rules
        .iter()
        .map(|(_, rule)| match rule {
            Rule::Box(source) | Rule::Alias(source) => usize::from(
                !repr
                    .get(source)
                    .is_some_and(|carrier| carrier.boxed_refcount_is_noop()),
            ),
            Rule::Phi(sources) => sources
                .iter()
                .filter(|source| {
                    !repr
                        .get(source)
                        .is_some_and(|carrier| carrier.boxed_refcount_is_noop())
                })
                .count(),
        })
        .collect();
    let mut pending: Vec<usize> = remaining
        .iter()
        .enumerate()
        .filter_map(|(index, &missing)| (missing == 0).then_some(index))
        .collect();
    while let Some(index) = pending.pop() {
        let result = rules[index].0;
        if repr
            .get(&result)
            .is_some_and(|carrier| carrier.boxed_refcount_is_noop())
        {
            continue;
        }
        let carrier = match &rules[index].1 {
            Rule::Box(_) => Repr::BoxedImmortal,
            Rule::Alias(source) => repr[source],
            Rule::Phi(sources) => {
                let first = repr[&sources[0]];
                if sources.iter().all(|source| repr[source] == first) {
                    first
                } else {
                    Repr::BoxedImmortal
                }
            }
        };
        repr.insert(result, carrier);
        for &dependent in dependents.get(&result).into_iter().flatten() {
            assert!(
                remaining[dependent] > 0,
                "boxed-word proof edge accounted twice"
            );
            remaining[dependent] -= 1;
            if remaining[dependent] == 0 {
                pending.push(dependent);
            }
        }
    }
}

/// A scalar fact describes a mathematical value, not permission to mint another
/// object for a binding view. A full-width view that reaches a boxed consumer
/// must borrow the persistent box published by its store. Keep raw-only views
/// raw; the scalar, range and recurrence solvers remain independent of this
/// physical-carrier constraint.
fn constrain_boxed_binding_observations(
    func: &TirFunction,
    repr: &mut HashMap<ValueId, Repr>,
    value_types: &HashMap<ValueId, TirType>,
) {
    use crate::tir::passes::alias_analysis::build_alias_union_find;
    use crate::tir::passes::ownership_lattice_min::binding_view_roots;
    use crate::tir::passes::value_identity::forwarded_python_value_source;

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
    let mut raw_aliases: HashMap<ValueId, Vec<ValueId>> = HashMap::default();
    for (&value, &carrier) in repr.iter() {
        if carrier.is_raw_i64_carrier() {
            raw_aliases.entry(roots[&value]).or_default().push(value);
        }
    }

    let mut observed = Vec::new();
    let mut invalidated = Vec::new();
    // Losing any one raw input invalidates a forwarding/checked result or
    // phi. No consumer needs to recount the other inputs on each removal.
    let mut dependents: HashMap<ValueId, Vec<ValueId>> = HashMap::default();
    // A boxed observation of a phi observes whichever incoming binding it
    // carries. Mixed owner/view phis need this edge too; the phi itself need
    // not be a binding-view root in OwnershipRootFacts.
    let mut phi_sources: HashMap<ValueId, Vec<ValueId>> = HashMap::default();
    let mut phi_arguments = HashSet::default();
    let demanding = demanding_phi_incomings(func, &reachable, repr);
    for (&(block, index), sources) in &incomings {
        if let Some(argument) = func.blocks[&block].args.get(index) {
            phi_arguments.insert(argument.id);
            // A phi that is boxed before any demand merges a boxed incoming,
            // so it converts its raw incomings on their edges whatever is
            // observed. A raw phi that a demand later boxes keeps every
            // incoming in its demand.
            let converts = !is_raw_integer(repr, argument.id);
            phi_sources.entry(roots[&argument.id]).or_default().extend(
                sources
                    .iter()
                    .filter(|&&source| !converts || demanding.contains(&(argument.id, source)))
                    .map(|value| roots[value]),
            );
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
    let mut consumer_uses: HashMap<ValueId, Vec<usize>> = HashMap::default();
    for block in func
        .blocks
        .values()
        .filter(|block| reachable.contains(&block.id))
    {
        for op in &block.ops {
            let use_plan = integer_operand_use(op, value_types);
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
                let source = forwarded_python_value_source(op);
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
    let mut demanded = HashSet::default();
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

/// The (phi argument, incoming value) pairs through which a boxed
/// observation of an already-boxed phi also observes the incoming binding.
///
/// A phi that merges a boxed incoming (for example the exit of a versioned
/// loop, which merges the raw fast accumulator with the boxed slow one) boxes
/// each raw incoming on its edge. When the incoming is not live into the
/// join, no use of that raw instance follows the edge, so the edge is the
/// instance's one materialization on the path: an observation of the phi's
/// object (a return of the loop accumulator, the boxed continuation of a
/// versioned loop) sees one object, as CPython does, and makes no demand on
/// the incoming binding. The incoming keeps the demand when it is live into
/// the join (a later use could box the same instance again), when one edge
/// passes it to two arguments (two boxes of one instance), when the incoming
/// or the phi is not a conversion (raw into boxed), and on every exception
/// edge.
fn demanding_phi_incomings(
    func: &TirFunction,
    reachable: &HashSet<BlockId>,
    repr: &HashMap<ValueId, Repr>,
) -> HashSet<(ValueId, ValueId)> {
    let labels = crate::tir::dominators::exception_label_to_block(func);
    let mut liveness = SourceLiveness::new(func, reachable);
    let mut demanding = HashSet::default();
    for block in func
        .blocks
        .values()
        .filter(|block| reachable.contains(&block.id))
    {
        block.terminator.for_each_edge(|target, args| {
            let Some(target_block) = func.blocks.get(&target) else {
                return;
            };
            for (&source, argument) in args.iter().zip(&target_block.args) {
                let conversion = !is_raw_integer(repr, argument.id) && is_raw_integer(repr, source);
                let passed_twice = args.iter().filter(|&&value| value == source).count() > 1;
                if !conversion || passed_twice || liveness.live_in(source, target) {
                    demanding.insert((argument.id, source));
                }
            }
        });
        for op in &block.ops {
            if crate::tir::dominators::exception_edge_binds_handler_arguments(op.opcode)
                && let Some(handler) =
                    crate::tir::dominators::exception_transfer_target(op, &labels)
                && let Some(handler_block) = func.blocks.get(&handler)
            {
                for (&source, argument) in op.exception_payload().iter().zip(&handler_block.args) {
                    demanding.insert((argument.id, source));
                }
            }
        }
    }
    demanding
}

/// Block-entry liveness of single values, computed on demand: a value is live
/// into a block when some path from the block's entry reaches a use of it
/// before its definition runs again. Exception edges count as paths.
struct SourceLiveness {
    preds: HashMap<BlockId, Vec<BlockId>>,
    def_block: HashMap<ValueId, BlockId>,
    use_blocks: HashMap<ValueId, Vec<BlockId>>,
    live: HashMap<ValueId, HashSet<BlockId>>,
}

impl SourceLiveness {
    fn new(func: &TirFunction, reachable: &HashSet<BlockId>) -> Self {
        let mut def_block = HashMap::default();
        let mut use_blocks: HashMap<ValueId, Vec<BlockId>> = HashMap::default();
        for block in func.blocks.values() {
            for arg in &block.args {
                def_block.insert(arg.id, block.id);
            }
            for op in &block.ops {
                for &result in &op.results {
                    def_block.insert(result, block.id);
                }
            }
            if !reachable.contains(&block.id) {
                continue;
            }
            for op in &block.ops {
                for &operand in op.exception_payload() {
                    use_blocks.entry(operand).or_default().push(block.id);
                }
            }
            block.terminator.for_each_value(|value| {
                use_blocks.entry(value).or_default().push(block.id);
            });
        }
        Self {
            preds: crate::tir::dominators::build_pred_map(func),
            def_block,
            use_blocks,
            live: HashMap::default(),
        }
    }

    fn live_in(&mut self, value: ValueId, block: BlockId) -> bool {
        if !self.live.contains_key(&value) {
            let defined = self.def_block.get(&value).copied();
            let mut live: HashSet<BlockId> = HashSet::default();
            let mut work: Vec<BlockId> = Vec::new();
            for &user in self.use_blocks.get(&value).into_iter().flatten() {
                // A use in the defining block follows its definition.
                if Some(user) != defined && live.insert(user) {
                    work.push(user);
                }
            }
            while let Some(current) = work.pop() {
                for &pred in self.preds.get(&current).into_iter().flatten() {
                    if Some(pred) != defined && live.insert(pred) {
                        work.push(pred);
                    }
                }
            }
            // The definition block is never live in: its definition runs first.
            self.live.insert(value, live);
        }
        self.live[&value].contains(&block)
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

fn integer_operand_use(op: &TirOp, value_types: &HashMap<ValueId, TirType>) -> IntegerOperandUse {
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
    // A binding alias (each read of a local) retains its operand. A raw int
    // has nothing to retain and its raw result is the same machine value, so
    // the alias boxes nothing and creates no second object: it observes the
    // binding only when its result is boxed.
    if op.opcode == OpCode::Copy && op.origin == Some(molt_ir::OpKind::BindingAlias) {
        return IntegerOperandUse {
            observation: unary_result,
            raw_result_depends_on_operands: true,
        };
    }
    let (observation, raw_result_depends_on_operands) = match op.opcode {
        OpCode::IncRef
        | OpCode::DecRef
        | OpCode::DelBoundary
        | OpCode::FrameSetLocation
        | OpCode::SplicedFrameSlot
        | OpCode::SplicedFrameObserved
        | OpCode::LiteralCellReady
        | OpCode::FloatSqrt => (IntegerOperandObservation::Never, false),
        // The packed probe compares its owner's tag and reads exact integer
        // coordinates, without invoking Python or publishing any operand.
        OpCode::TryBuffer2dLoadI64 | OpCode::ProbeInlineField | OpCode::FrameEntryAdmits => {
            (IntegerOperandObservation::Never, false)
        }
        OpCode::CheckedAdd | OpCode::CheckedMul => (binary, true),
        OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge => {
            (binary, false)
        }
        OpCode::Not | OpCode::Bool => (unary, false),
        // A raw i64 payload in, a raw f64 out: no boxed int is observed.
        OpCode::IntToFloat => (unary, false),
        OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor => (binary_result, true),
        OpCode::Pos | OpCode::BitNot => (unary_result, true),
        // Late-TIR primitives read their operands as boxed words. UnboxInlineInt
        // produces a raw word by construction, independent of operand carriers.
        OpCode::IsInlineInt
        | OpCode::UnboxInlineInt
        | OpCode::IsBoolWord
        | OpCode::IsFloatWord
        | OpCode::UnboxFloatWord
        | OpCode::UnboxBoolWord
        | OpCode::BoxInlineInt
        | OpCode::CodeIs
        | OpCode::DirectCall
        | OpCode::FunctionClosure
        | OpCode::RecursionAdmits
        | OpCode::RecursionCount
        | OpCode::RecursionRelease
        | OpCode::CallRuntime
        | OpCode::NativeSymbolAddr
        | OpCode::CallNative => (IntegerOperandObservation::Always, false),
        // Arithmetic whose operands are all exact ints runs int's own
        // operation, which never exposes an operand object, so it observes no
        // binding identity. A non-int operand can receive the int through a
        // reflected dunder (`obj.__rmod__(i)`), which is an observation.
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
        | OpCode::Shr => {
            let all_exact_ints = op
                .operands
                .iter()
                .all(|operand| is_raw_i64_semantic_candidate(value_types, *operand));
            if all_exact_ints {
                (IntegerOperandObservation::Never, true)
            } else {
                (IntegerOperandObservation::Always, true)
            }
        }
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
        | OpCode::CallClassCandidate
        | OpCode::CallMethod
        | OpCode::CallLoadedMethod
        | OpCode::CallargsPushMethodReceiver
        | OpCode::CallBuiltin
        | OpCode::OrdAt
        | OpCode::BoxVal
        | OpCode::UnboxVal
        | OpCode::BuildList
        | OpCode::BuildDict
        | OpCode::BuildTuple
        | OpCode::BuildSet
        | OpCode::BuildSlice
        | OpCode::GetIter
        | OpCode::IterNext
        | OpCode::IterNextUnboxed
        | OpCode::IterNextUnpack2
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
        | OpCode::AsyncWorkPending
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
        | OpCode::ModuleImportClosed
        | OpCode::ImportFrom
        | OpCode::ModuleCacheGet
        | OpCode::ModuleCacheSet
        | OpCode::ModuleCacheDel
        | OpCode::ModuleGetAttr
        | OpCode::ModuleImportFrom
        | OpCode::ModuleGetGlobal
        | OpCode::GlobalSlotBase
        | OpCode::GlobalSlotGet
        | OpCode::StaticCellBase
        | OpCode::StaticCellLoad
        | OpCode::StaticCellSet
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
pub fn raw_i64_safe_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    raw_i64_safe_values_from(&ScalarFacts::new(tir_func), vr)
}

pub(crate) fn raw_i64_safe_values_from(
    facts: &ScalarFacts<'_>,
    vr: &crate::tir::ValueRangeResult,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    let value_types = value_type_by_id_for(facts);
    raw_i64_safe_values_for_with_types(facts.body(), vr, &value_types)
}

fn raw_i64_safe_values_for_with_types(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    let mut seed = raw_i64_safe_value_seed(tir_func, vr, value_types);
    seed.extend(gpu_intrinsic_raw_i64_values(tir_func, value_types));
    propagate_raw_i64_identity_values(tir_func, seed, value_types)
}

/// Full-range raw-i64 carriers. These values are not inline-box-safe; their
/// soundness comes from the overflow flag and the boxed slow path, or from a
/// result that provably fits i64 (`x // c`, `c` a constant other than 0 and
/// -1, of a raw dividend).
fn raw_i64_full_deopt_values_for_with_types(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
    inline_safe_values: &molt_ir::deterministic_hash::HashSet<ValueId>,
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    let mut seed = molt_ir::deterministic_hash::HashSet::default();
    let inline_payloads: molt_ir::deterministic_hash::HashSet<ValueId> = tir_func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::UnboxInlineInt)
        .flat_map(|op| op.results.iter().copied())
        .collect();
    for block in tir_func.blocks.values() {
        for op in &block.ops {
            // Checked integer loop versioning proved the exact result in
            // i64: the machine op computes it, and it boxes through the
            // full-range path.
            if op.has_i64_exact_proof() {
                seed.extend(op.results.iter().copied().filter(|&result| {
                    is_raw_i64_semantic_candidate(value_types, result)
                        && !inline_safe_values.contains(&result)
                }));
                continue;
            }
            match opcode_repr_raw_i64_full_deopt_seed_rule_table(op.opcode) {
                // Two values in the inline window sum exactly in i64; the result
                // may leave the window, so it boxes through the full-range path.
                // At least one operand is a late-stage payload, so only the
                // stage's fast arms take this rule (HF-821).
                ReprRawI64FullDeoptSeedRule::ExactSumOfInlinePayloads
                    if let [lhs, rhs] = op.operands[..]
                        && let [result] = op.results[..]
                        && inline_safe_values.contains(&lhs)
                        && inline_safe_values.contains(&rhs)
                        && (inline_payloads.contains(&lhs) || inline_payloads.contains(&rhs))
                        && !inline_safe_values.contains(&result) =>
                {
                    seed.insert(result);
                }
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
    let mut full = propagate_raw_i64_full_deopt_identity_values(
        tir_func,
        inline_safe_values,
        seed.clone(),
        value_types,
    );
    // A quotient joins once its dividend is a raw carrier; a chain of them
    // (`x // 3 // 5`) needs one round per link.
    loop {
        let before = seed.len();
        for op in tir_func.blocks.values().flat_map(|block| &block.ops) {
            if opcode_repr_raw_i64_full_deopt_seed_rule_table(op.opcode)
                == ReprRawI64FullDeoptSeedRule::QuotientByConstantDivisor
                && let [dividend, divisor] = op.operands[..]
                && let [result] = op.results[..]
                && is_raw_i64_semantic_candidate(value_types, result)
                && !inline_safe_values.contains(&result)
                && (inline_safe_values.contains(&dividend) || full.contains(&dividend))
                && vr
                    .const_int_of(divisor)
                    .is_some_and(|divisor| divisor != 0 && divisor != -1)
            {
                seed.insert(result);
            }
        }
        if seed.len() == before {
            return full;
        }
        full = propagate_raw_i64_full_deopt_identity_values(
            tir_func,
            inline_safe_values,
            seed.clone(),
            value_types,
        );
    }
}

/// Values without a refcounted heap obligation, derived from the same exact
/// producer and physical-carrier facts as lowering. A float annotation may
/// admit a heap-owning subclass and must never suppress its drop. `Never` is
/// the representation lattice's join bottom, not a defined-value non-heap
/// proof, so it is deliberately absent from this set.
pub(crate) fn non_heap_values_for(
    tir_func: &TirFunction,
    vr: &crate::tir::ValueRangeResult,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    non_heap_values_in_domain(
        repr_by_value_for(tir_func, Some(vr)),
        RefcountDomain::Carrier,
    )
}

/// Values that remain non-owning after boxing into an object field. A checked
/// full-i64 carrier has no local RC obligation but may box to a heap BigInt;
/// only the inline-safe integer tier discharges a boxed storage obligation.
/// Store elimination must use this projection, not [`non_heap_values_for`].
pub(crate) fn non_heap_boxed_values_from(
    facts: &ScalarFacts<'_>,
    vr: &crate::tir::ValueRangeResult,
) -> HashSet<ValueId> {
    non_heap_values_in_domain(repr_by_value_from(facts, Some(vr)), RefcountDomain::Boxed)
}

pub(crate) fn analyze_non_heap_carriers(func: &TirFunction) -> HashSet<ValueId> {
    non_heap_values_in_domain(analyze_representations(func), RefcountDomain::Carrier)
}

pub(crate) fn analyze_non_heap_boxed(func: &TirFunction) -> HashSet<ValueId> {
    non_heap_values_in_domain(analyze_representations(func), RefcountDomain::Boxed)
}

enum RefcountDomain {
    Carrier,
    Boxed,
}

fn non_heap_values_in_domain(
    representations: HashMap<ValueId, Repr>,
    domain: RefcountDomain,
) -> HashSet<ValueId> {
    representations
        .into_iter()
        .filter_map(|(value, repr)| {
            let non_heap = match domain {
                RefcountDomain::Carrier => repr.carrier_refcount_is_noop(),
                RefcountDomain::Boxed => repr.boxed_refcount_is_noop(),
            };
            non_heap.then_some(value)
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
/// all-incomings-raw rule. This is a hard soundness requirement of every
/// backend that holds a phi raw: a `RawI64Safe` (`I64`) phi slot must never be
/// fed a `MaybeBigInt`/`DynBox` incoming, or the backend would emit an unsound
/// unbox of a possible heap BigInt. Seeding a phi directly from its own proven range
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
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    let mut seed = molt_ir::deterministic_hash::HashSet::default();
    for block in tir_func.blocks.values() {
        for op in &block.ops {
            // The late lowering stage proved these results before it split
            // blocks. A fresh range analysis cannot see the loop through the
            // split, and a value's range does not change when its block does.
            if op.has_inline_int_proof() {
                seed.extend(
                    op.results
                        .iter()
                        .copied()
                        .filter(|&result| is_raw_i64_semantic_candidate(value_types, result)),
                );
                continue;
            }
            // CheckedAdd/CheckedMul are full-range overflow-peel carriers.
            // They are seeded as RawI64FullDeopt, never as RawI64Safe, even
            // where the range of their first result fits the inline window.
            if opcode_repr_raw_i64_full_deopt_seed_rule_table(op.opcode)
                == ReprRawI64FullDeoptSeedRule::CheckedResultZero
            {
                continue;
            }
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

fn op_original_kind(op: &TirOp) -> Option<molt_ir::OpKind> {
    op.origin
}

pub(crate) fn block_arg_incomings_for(
    tir_func: &TirFunction,
) -> HashMap<(BlockId, usize), Vec<ValueId>> {
    let reachable = crate::tir::dominators::executable_reachable_blocks(tir_func);
    let labels = crate::tir::dominators::exception_label_to_block(tir_func);
    let mut block_arg_incomings: HashMap<(BlockId, usize), Vec<ValueId>> = HashMap::default();
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
                add_edge(target, op.exception_payload());
            }
        }
        block.terminator.for_each_edge(&mut add_edge);
    }
    block_arg_incomings
}

/// The result `ValueId`s of GPU thread/block-id intrinsic calls — hardware lane,
/// block, and grid indices that are structurally bounded in `[0, 2^20)` and so
/// always fit a raw i64 carrier. The value-range analysis has no model for these
/// `Call` results, so the value-keyed representation authority seeds exactly
/// this bounded GPU-index population, never an arbitrary runtime call.
fn gpu_intrinsic_raw_i64_values(
    tir_func: &TirFunction,
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    let mut values = molt_ir::deterministic_hash::HashSet::default();
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
    seed: molt_ir::deterministic_hash::HashSet<ValueId>,
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    propagate_raw_i64_identity_values_with_phi_support(tir_func, seed, None, value_types)
}

fn propagate_raw_i64_full_deopt_identity_values(
    tir_func: &TirFunction,
    inline_safe_values: &molt_ir::deterministic_hash::HashSet<ValueId>,
    seed: molt_ir::deterministic_hash::HashSet<ValueId>,
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    propagate_raw_i64_identity_values_with_phi_support(
        tir_func,
        seed,
        Some(inline_safe_values),
        value_types,
    )
}

fn propagate_raw_i64_identity_values_with_phi_support(
    tir_func: &TirFunction,
    seed: molt_ir::deterministic_hash::HashSet<ValueId>,
    phi_support: Option<&molt_ir::deterministic_hash::HashSet<ValueId>>,
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    use molt_ir::deterministic_hash::HashSet;

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
    let mut copy_source: HashMap<ValueId, ValueId> = HashMap::default();
    let mut block_arg_ids: HashMap<ValueId, (BlockId, usize)> = HashMap::default();
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
            if let Some(source) =
                crate::tir::passes::value_identity::forwarded_python_value_source(op)
                && let Some(&result) = op.results.first()
            {
                copy_source.insert(result, source);
            }
            for &result in &op.results {
                all_value_ids.push(result);
            }
        }
    }

    // The inline-safe lane has no support: its cycles admit only seeds.
    let no_support = HashSet::default();
    propagate_checked_identity_values(
        &seed,
        phi_support.unwrap_or(&no_support),
        &copy_source,
        &block_arg_ids,
        &block_arg_incomings,
        &all_value_ids,
        value_types,
    )
}

/// Both raw-i64 lanes over identity edges, solved optimistically.
///
/// A loop accumulator is a phi cycle: the header phi receives the latch value,
/// and the latch value is a phi or copy of the header phi on some paths
/// (`if c: total += i`). A loop-invariant carried value is the shortest cycle:
/// the latch passes the header phi back unchanged. A least fixpoint never
/// admits a cycle, because each member waits for the other. The greatest
/// fixpoint admits it when every incoming from outside the cycle is a lane
/// producer (`seed`) or, for the checked lane, an inline-safe value
/// (`support`). That is sound by induction over executions: each value the
/// cycle can hold entered it from one of those producers. A value must also
/// depend on a seed; for the checked lane an all-inline-safe cycle belongs to
/// the inline-safe lane.
fn propagate_checked_identity_values(
    seed: &molt_ir::deterministic_hash::HashSet<ValueId>,
    support: &molt_ir::deterministic_hash::HashSet<ValueId>,
    copy_source: &HashMap<ValueId, ValueId>,
    block_arg_ids: &HashMap<ValueId, (BlockId, usize)>,
    block_arg_incomings: &HashMap<(BlockId, usize), Vec<ValueId>>,
    all_value_ids: &[ValueId],
    value_types: &HashMap<ValueId, TirType>,
) -> molt_ir::deterministic_hash::HashSet<ValueId> {
    use molt_ir::deterministic_hash::HashSet;

    let sources = |id: ValueId| -> Option<Vec<ValueId>> {
        if let Some(&src) = copy_source.get(&id) {
            return Some(vec![src]);
        }
        let &(block, index) = block_arg_ids.get(&id)?;
        block_arg_incomings
            .get(&(block, index))
            .filter(|incomings| !incomings.is_empty())
            .cloned()
    };
    // Optimistic start: every identity value of int type may be checked.
    let candidates: Vec<(ValueId, Vec<ValueId>)> = all_value_ids
        .iter()
        .copied()
        .filter(|id| !seed.contains(id) && is_raw_i64_semantic_candidate(value_types, *id))
        .filter_map(|id| sources(id).map(|srcs| (id, srcs)))
        .collect();
    let mut assumed: HashSet<ValueId> = seed.clone();
    assumed.extend(candidates.iter().map(|(id, _)| *id));
    let mut changed = true;
    while changed {
        changed = false;
        for (id, srcs) in &candidates {
            if assumed.contains(id)
                && !srcs
                    .iter()
                    .all(|src| assumed.contains(src) || support.contains(src))
            {
                assumed.remove(id);
                changed = true;
            }
        }
    }
    // Keep the values that some checked result reaches.
    let mut checked: HashSet<ValueId> = seed.clone();
    changed = true;
    while changed {
        changed = false;
        for (id, srcs) in &candidates {
            if assumed.contains(id)
                && !checked.contains(id)
                && srcs.iter().any(|src| checked.contains(src))
            {
                checked.insert(*id);
                changed = true;
            }
        }
    }
    checked
}
