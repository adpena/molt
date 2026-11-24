//! Whole-program type flow and specialization (design 78, slice 3; HF-847).
//!
//! The pass runs first in the TIR module phase, over the optimized bodies of
//! the whole document. It finds the calls whose callee and argument binding
//! the program proves (a proven `call_guarded`, a `call_internal`), solves the
//! exact scalar types that reach each callee per relevant parameter
//! ([`analysis`], [`relevance`]), and compiles one clone per context whose key
//! proves something:
//!
//! 1. Each clone is a copy of its origin's body whose `exact_param_types` hold
//!    the key; only the sites whose arguments have those types enter it. The
//!    origin stays generic for every other caller, a function object that
//!    escapes and a computed-name read alike, so no reader fact is needed.
//! 2. Dispatch retargets each such site to its clone and records the callee
//!    context's exact return type on the call (`exact_result_proof`). A
//!    proven guarded call keeps its callee word, which the late stage hands
//!    the entry as the invocation, so the clone's frame record holds the
//!    original function and code: `f.__code__`, `__qualname__`, line numbers
//!    and tracebacks are the original's (design 78, section 4).
//! 3. Clones and every caller whose calls gained a fact re-run the
//!    per-function pipeline. Clones of one function whose bodies then
//!    compile identically merge (by body digest), and a clone identical to
//!    its origin folds into it.
//!
//! A target whose backend compares a guarded call's code with its target
//! (`FinalLowering::BackendLanes`) gets no clones, since another entry of the
//! same function would always miss there; its proven calls still get their
//! callee's return type. Every resolved site that stays generic says why in
//! the report (design 78, section 3.11).
//!
//! A generic body that loses its last caller is left to dead-function
//! elimination: a module-level `def` keeps its function object, which any
//! computed-name read may return, so its body stays.

mod analysis;
mod relevance;
mod sites;

#[cfg(test)]
mod tests;

use molt_ir::deterministic_hash::{HashMap, HashSet};
use std::collections::BTreeSet;

use molt_ir::FunctionId;

use crate::tir::function::{TirFunction, TirModule};
use crate::tir::target_info::{FinalLowering, TargetInfo};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use analysis::{Analysis, ContextId, Inputs, Key, seeds_of};
use sites::{ArgKind, Site, SiteKind, Unspecializable};

/// A specialized clone the pass appended to the module's document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecializedClone {
    /// The clone's row.
    pub id: FunctionId,
    /// The row of the function it specializes: its Python identity, its
    /// linkage signature and its parameter custody.
    pub origin: FunctionId,
}

/// What the pass did.
#[derive(Debug, Default)]
pub struct TypeFlowOutcome {
    /// Rows appended to the document, in id order.
    pub clones: Vec<SpecializedClone>,
    /// Bodies the pass changed (clones included), by id.
    pub changed: BTreeSet<FunctionId>,
    /// Contexts the flow solved, generic ones included.
    pub contexts: usize,
    /// Resolved sites retargeted to a clone.
    pub sites_specialized: usize,
    /// Resolved sites whose result gained an exact type.
    pub results_typed: usize,
    /// One line per function and per resolved site that stays generic.
    pub report: Vec<String>,
}

/// Run the type flow over `module`.
///
/// `non_inlinable` names bodies whose canonical definition links from
/// another object; they keep one body. `report_sites` asks for the per-site
/// report lines (design 78, 3.11).
pub fn run(
    module: &mut TirModule,
    tti: &TargetInfo,
    non_inlinable: &HashSet<String>,
    report_sites: bool,
) -> TypeFlowOutcome {
    let mut outcome = TypeFlowOutcome::default();
    let base_rows = module.table.len();
    let body_index = module.body_index();
    let sites: Vec<Vec<Site>> = module
        .functions
        .iter()
        .map(|func| sites::sites_of(func, &body_index, module))
        .collect();
    if sites.iter().all(Vec::is_empty) {
        return outcome;
    }
    let final_tir = tti.lowering.final_lowering == FinalLowering::FinalTir;
    let refusals: Vec<Result<(), Unspecializable>> = (0..module.functions.len())
        .map(|index| sites::specializable(module, index, non_inlinable, final_tir))
        .collect();
    let specializable: Vec<bool> = refusals.iter().map(Result::is_ok).collect();
    // An isolated body can still run through an unseen caller. Its generic
    // code remains available without computing facts it cannot communicate.
    let mut participates = vec![false; sites.len()];
    for (caller, calls) in sites.iter().enumerate() {
        participates[caller] |= !calls.is_empty();
        for site in calls {
            let callee = body_index[site.callee.index()].expect("a resolved site names a body");
            participates[callee] = true;
        }
    }
    let relevant = relevance::solve(module, &sites, &body_index, &participates);
    let inputs = Inputs {
        module,
        body_index: &body_index,
        sites: &sites,
        relevant: &relevant,
        participates: &participates,
        exact_plans: (0..sites.len())
            .map(|_| std::cell::OnceCell::new())
            .collect(),
        specializable: &specializable,
    };
    let analysis = Analysis::run(&inputs);
    outcome.contexts = analysis.contexts.len();

    // Mark the final call targets once. A site may have entered other
    // contexts while solving, so historical return readers are insufficient.
    let mut reached = vec![false; outcome.contexts];
    for context in &analysis.contexts {
        for target in context.targets.iter().flatten() {
            reached[target.0] = true;
        }
    }

    // One clone per reached specialized context, in context order.
    let mut clone_of_context = vec![None; outcome.contexts];
    let mut clones: Vec<(ContextId, TirFunction)> = Vec::new();
    for (index, context) in analysis.contexts.iter().enumerate() {
        if context.key.is_empty() || !reached[index] {
            continue;
        }
        let origin = &module.functions[context.body];
        let mut clone = origin.clone();
        clone.name = clone_symbol(&origin.name, &context.key);
        clone.exact_param_types = seeds_of(&context.key, origin.param_names.len());
        clone_of_context[index] = Some(FunctionId::from_index(base_rows + clones.len()));
        clones.push((ContextId(index), clone));
    }

    // Dispatch on the bodies the analysis evaluated: each body under its own
    // context, so every recorded fact is the one the analysis proved.
    let mut proofs: Vec<(usize, Vec<SiteDecision>)> = Vec::new();
    let mut report = Vec::new();
    for body in 0..module.functions.len() {
        let decisions = analysis
            .generic(body)
            .map(|id| decide(&inputs, &analysis, id, &clone_of_context))
            .unwrap_or_default();
        proofs.push((body, decisions));
    }
    let clone_decisions: Vec<Vec<SiteDecision>> = clones
        .iter()
        .map(|(context, _)| decide(&inputs, &analysis, *context, &clone_of_context))
        .collect();
    if report_sites {
        describe(
            &inputs,
            &analysis,
            &refusals,
            &proofs,
            &clone_of_context,
            &mut report,
        );
    }

    for (body, decisions) in &proofs {
        if apply(&mut module.functions[*body], decisions, &mut outcome) {
            outcome.changed.insert(module.ids[*body]);
        }
    }
    let first_clone = module.functions.len();
    let mut clone_origin = Vec::with_capacity(clones.len());
    for ((context, mut clone), decisions) in clones.into_iter().zip(&clone_decisions) {
        apply(&mut clone, decisions, &mut outcome);
        clone_origin.push(analysis.context(context).body);
        let id = module.table.push(&molt_ir::FunctionIR {
            name: clone.name.clone(),
            ..molt_ir::FunctionIR::default()
        });
        module.ids.push(id);
        module.functions.push(clone);
        outcome.changed.insert(id);
    }

    // Re-optimize every changed body under its new facts.
    let changed_bodies: Vec<usize> = module
        .ids
        .iter()
        .enumerate()
        .filter(|(_, id)| outcome.changed.contains(id))
        .map(|(index, _)| index)
        .collect();
    for &index in &changed_bodies {
        let _ = super::reoptimize(&mut module.functions[index], tti);
    }

    merge_identical(
        module,
        first_clone,
        &clone_origin,
        base_rows,
        &mut outcome,
        &mut report,
    );
    outcome.report = report;
    outcome
}

/// The linker symbol of a clone: its origin's symbol and its key, one letter
/// per parameter. Names serve linkage and messages; the clone's identity is
/// its row.
fn clone_symbol(origin: &str, key: &Key) -> String {
    let letters: String = key.iter().map(|kind| kind.tag()).collect();
    format!("{origin}__tf_{letters}")
}

/// What dispatch does to one resolved site.
#[derive(Clone, Debug)]
struct SiteDecision {
    position: (crate::tir::blocks::BlockId, usize),
    /// The clone's provisional row, when the site enters one.
    retarget: Option<FunctionId>,
    /// The exact type of the call's result, when one is proven.
    result: Option<TirType>,
}

fn decide(
    inputs: &Inputs<'_>,
    analysis: &Analysis,
    context: ContextId,
    clone_of_context: &[Option<FunctionId>],
) -> Vec<SiteDecision> {
    let caller = analysis.context(context);
    inputs.sites[caller.body]
        .iter()
        .enumerate()
        .map(|(position, site)| {
            let target = caller.targets[position];
            let retarget = target.and_then(|target| clone_of_context[target.0]);
            // The site enters exactly its target: that context's return
            // type is its result's.
            let result = target
                .map(|target| analysis.context(target).ret.clone())
                .filter(|ty| {
                    matches!(
                        ty,
                        TirType::I64 | TirType::BigInt | TirType::Bool | TirType::F64
                    )
                });
            SiteDecision {
                position: site.position(),
                retarget,
                result,
            }
        })
        .collect()
}

/// Apply `decisions` to `func`; return whether it changed.
fn apply(
    func: &mut TirFunction,
    decisions: &[SiteDecision],
    outcome: &mut TypeFlowOutcome,
) -> bool {
    let mut changed = false;
    for decision in decisions {
        let (block, index) = decision.position;
        let op = &mut func
            .blocks
            .get_mut(&block)
            .expect("a decided site's block exists")
            .ops[index];
        if let Some(clone) = decision.retarget {
            op.callee = Some(clone);
            outcome.sites_specialized += 1;
            changed = true;
        }
        if let Some(ty) = &decision.result
            && op.exact_result_proof().as_ref() != Some(ty)
            && op.mark_exact_result_proof(ty)
        {
            outcome.results_typed += 1;
            changed = true;
        }
    }
    changed
}

/// Merge clones of one function whose bodies compile identically, and fold a
/// clone identical to its origin into the origin (design 78, 3.4). Rows of
/// merged clones are removed and every callee id is renumbered once.
fn merge_identical(
    module: &mut TirModule,
    first_clone: usize,
    clone_origin: &[usize],
    base_rows: usize,
    outcome: &mut TypeFlowOutcome,
    report: &mut Vec<String>,
) {
    let table = &module.table;
    let digest = |func: &TirFunction| body_digest(func, table);
    // Representative row of each clone: itself, an earlier identical clone of
    // the same origin, or the origin.
    let mut representative: Vec<FunctionId> = Vec::with_capacity(clone_origin.len());
    let mut seen: HashMap<(usize, Vec<u8>), usize> = HashMap::default();
    let mut origin_digests = HashMap::default();
    for (offset, &origin) in clone_origin.iter().enumerate() {
        let index = first_clone + offset;
        let own = digest(&module.functions[index]);
        let id = module.ids[index];
        let original = origin_digests
            .entry(origin)
            .or_insert_with(|| digest(&module.functions[origin]));
        if *original == own {
            representative.push(module.ids[origin]);
            continue;
        }
        match seen.entry((origin, own)) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let earlier = *entry.get();
                let exact = merged_seeds(
                    &module.functions[earlier].exact_param_types,
                    &module.functions[index].exact_param_types,
                );
                module.functions[earlier].exact_param_types = exact;
                representative.push(module.ids[earlier]);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(index);
                representative.push(id);
            }
        }
    }
    let kept: Vec<bool> = representative
        .iter()
        .enumerate()
        .map(|(offset, rep)| rep.index() == base_rows + offset)
        .collect();
    if kept.iter().all(|keep| *keep) {
        for (offset, &origin) in clone_origin.iter().enumerate() {
            outcome.clones.push(SpecializedClone {
                id: FunctionId::from_index(base_rows + offset),
                origin: module.ids[origin],
            });
        }
        return;
    }
    // Final row of every appended row: kept clones renumber densely after
    // the document's own rows; a merged clone takes its representative's.
    let mut new_index = vec![None; clone_origin.len()];
    let mut next = base_rows;
    for (offset, keep) in kept.iter().enumerate() {
        if *keep {
            new_index[offset] = Some(FunctionId::from_index(next));
            next += 1;
        }
    }
    let remap = |id: FunctionId| -> FunctionId {
        if id.index() < base_rows {
            return id;
        }
        let offset = id.index() - base_rows;
        let rep = representative[offset];
        if rep.index() < base_rows {
            rep
        } else {
            new_index[rep.index() - base_rows].expect("a representative is kept")
        }
    };
    for func in &mut module.functions {
        for block in func.blocks.values_mut() {
            for op in &mut block.ops {
                if let Some(callee) = op.callee {
                    op.callee = Some(remap(callee));
                }
            }
        }
    }
    for (offset, keep) in kept.iter().enumerate().rev() {
        if !*keep {
            let index = first_clone + offset;
            let removed = module.functions.remove(index);
            module.ids.remove(index);
            outcome
                .changed
                .remove(&FunctionId::from_index(base_rows + offset));
            report.push(format!(
                "[type-flow] merged {} into {}",
                removed.name,
                table.symbol(representative[offset])
            ));
        }
    }
    let rows = (0..base_rows)
        .map(FunctionId::from_index)
        .chain(
            kept.iter()
                .enumerate()
                .filter(|(_, keep)| **keep)
                .map(|(offset, _)| FunctionId::from_index(base_rows + offset)),
        )
        .map(|id| (table.symbol(id), table.kind(id)));
    module.table = molt_ir::FunctionTable::from_rows(rows);
    for (position, id) in module.ids.iter_mut().enumerate().skip(first_clone) {
        *id = FunctionId::from_index(base_rows + position - first_clone);
    }
    let changed: BTreeSet<FunctionId> = outcome
        .changed
        .iter()
        .filter(|id| id.index() < base_rows)
        .copied()
        .chain(module.ids[first_clone..].iter().copied())
        .collect();
    outcome.changed = changed;
    for (offset, &origin) in clone_origin.iter().enumerate() {
        if kept[offset] {
            outcome.clones.push(SpecializedClone {
                id: new_index[offset].expect("kept"),
                origin: module.ids[origin],
            });
        }
    }
}

/// The meet of two clones' seeds: a parameter they disagree on is proven
/// nothing in the merged body.
fn merged_seeds(a: &[TirType], b: &[TirType]) -> Vec<TirType> {
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            let a = ArgKind::of(a).unwrap_or(ArgKind::Dyn);
            let b = ArgKind::of(b).unwrap_or(ArgKind::Dyn);
            a.meet(b).seed()
        })
        .collect()
}

/// What decides a body's machine code: its canonical serialization under a
/// fixed name with no seeds, and the exact scalar family of every value its
/// parameters do not hold directly.
fn body_digest(func: &TirFunction, table: &molt_ir::FunctionTable) -> Vec<u8> {
    let mut normalized = func.clone();
    normalized.name.clear();
    normalized.exact_param_types.clear();
    // Allocation counters and the seeded parameter types are not code.
    normalized.next_value = 0;
    normalized.next_block = 0;
    let declared = normalized.param_types.clone();
    if let Some(entry) = normalized.blocks.get_mut(&normalized.entry_block) {
        for (arg, ty) in entry.args.iter_mut().zip(&declared) {
            arg.ty = ty.clone();
            normalized.value_types.insert(arg.id, ty.clone());
        }
    }
    let mut bytes = crate::tir::serialize::serialize_tir_function(&normalized, table)
        .expect("a module body serializes");
    let params: HashSet<ValueId> = func
        .blocks
        .get(&func.entry_block)
        .map(|entry| entry.args.iter().map(|arg| arg.id).collect())
        .unwrap_or_default();
    let mut facts: Vec<(ValueId, String)> = crate::tir::type_refine::extract_exact_scalar_map(func)
        .into_iter()
        .filter(|(value, _)| !params.contains(value))
        .map(|(value, ty)| (value, ArgKind::of(&ty).unwrap_or(ArgKind::Dyn).tag()))
        .collect();
    facts.sort_unstable();
    for (value, tag) in facts {
        bytes.extend_from_slice(&value.0.to_le_bytes());
        bytes.extend_from_slice(tag.as_bytes());
    }
    bytes
}

/// One report line per body with resolved sites, and one per site that stays
/// generic, naming why (design 78, section 3.11).
fn describe(
    inputs: &Inputs<'_>,
    analysis: &Analysis,
    refusals: &[Result<(), Unspecializable>],
    proofs: &[(usize, Vec<SiteDecision>)],
    clone_of_context: &[Option<FunctionId>],
    report: &mut Vec<String>,
) {
    let module = inputs.module;
    let mut body_counts = vec![(0usize, 0usize); module.functions.len()];
    for (context, clone) in analysis.contexts.iter().zip(clone_of_context) {
        let (contexts, clones) = &mut body_counts[context.body];
        *contexts += 1;
        *clones += usize::from(clone.is_some());
    }
    for (body, decisions) in proofs {
        let func = &module.functions[*body];
        let sites = &inputs.sites[*body];
        let unresolved = sites::unknown_callee_count(func);
        if unresolved > 0 {
            let kinds = sites::unknown_callee_kinds(func)
                .into_iter()
                .map(|(kind, count)| format!("{kind} {count}"))
                .collect::<Vec<_>>()
                .join(", ");
            report.push(format!("[type-flow-unknown] {}: {kinds}", func.name));
        }
        let (contexts, clones) = body_counts[*body];
        if sites.is_empty() && contexts <= 1 {
            if unresolved > 0 {
                report.push(format!(
                    "[type-flow] {}: {unresolved} call(s) with an unknown callee",
                    func.name
                ));
            }
            continue;
        }
        report.push(format!(
            "[type-flow] {}: {contexts} context(s), {clones} clone(s), {} resolved call(s), {unresolved} with an unknown callee",
            func.name,
            sites.len(),
        ));
        if sites.is_empty() {
            continue;
        }
        let context = analysis
            .generic(*body)
            .expect("a body with resolved call sites has a generic context");
        let facts = analysis.facts(inputs, context);
        for (site, decision) in sites.iter().zip(decisions) {
            if decision.retarget.is_some() {
                continue;
            }
            let callee = inputs.body_index[site.callee.index()].expect("a site names a body");
            let callee_name = &module.functions[callee].name;
            let why = match refusals[callee] {
                Err(refusal) => refusal.describe().to_string(),
                Ok(()) => generic_argument(func, site, &facts, &inputs.relevant[callee]),
            };
            let kind = match site.kind {
                SiteKind::ProvenGuarded => "proven call",
                SiteKind::Internal => "internal call",
            };
            report.push(format!(
                "[type-flow]   {kind} of {callee_name} at block {} op {}: generic: {why}",
                site.block.0, site.index
            ));
        }
    }
}

/// Why a specializable callee's generic body serves `site`.
fn generic_argument(
    func: &TirFunction,
    site: &Site,
    facts: &HashMap<ValueId, TirType>,
    relevant: &[bool],
) -> String {
    if !relevant.iter().any(|flag| *flag) {
        return "no parameter's type changes the callee's code".to_string();
    }
    for (position, arg) in site.args.iter().enumerate() {
        if !relevant.get(position).copied().unwrap_or(true) {
            continue;
        }
        let fact = facts.get(arg).cloned().unwrap_or(TirType::DynBox);
        if ArgKind::of(&fact) == Some(ArgKind::Dyn) {
            return format!(
                "argument {position} is not an exact scalar; it comes from {}",
                producer(func, *arg)
            );
        }
    }
    "every relevant argument is unknown on every path".to_string()
}

/// The operation or block argument that defines `value`.
fn producer(func: &TirFunction, value: ValueId) -> String {
    for block in func.blocks.values() {
        if let Some(position) = block.args.iter().position(|arg| arg.id == value) {
            return if block.id == func.entry_block {
                format!("parameter {position} of {}", func.name)
            } else {
                format!("a join at block {}", block.id.0)
            };
        }
        for op in &block.ops {
            if op.results.contains(&value) {
                return match op.origin {
                    Some(kind) => format!("{:?} ({kind:?})", op.opcode),
                    None => format!("{:?}", op.opcode),
                };
            }
        }
    }
    "an unknown producer".to_string()
}
