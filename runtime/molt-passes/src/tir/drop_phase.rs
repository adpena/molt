//! RC drop-insertion **terminal phase** (design 20, round-7).
//!
//! Drop insertion (`drop_insertion` + `refcount_elim_post`) is the compiler pass
//! family that closes molt's whole-program expression-temporary leak: the runtime
//! allocates every heap result at `ref_count = 1` and, before this family runs,
//! never decrements it for expression temporaries.
//!
//! ## Why this is a SEPARATE terminal phase (the round-7 structural arc)
//!
//! Drop insertion is deliberately NOT part of the per-function optimization
//! pipeline ([`crate::tir::passes::run_pipeline`] /
//! [`crate::tir::pass_manager::build_default_pipeline`]). It runs ONCE per
//! function, AFTER all per-function optimization AND all module-level transforms
//! (the E1 inliner + module-slot promotion, plus the per-caller / per-promoted
//! re-optimizations those run through the per-function pipeline).
//!
//! The reason is structural and load-bearing.
//! [`module_slot_promotion`](crate::tir::passes::module_slot_promotion) hoists a
//! module-global accumulator out of the module dict into a register-carried loop
//! phi (the `total = inc(total)` benchmark shape). Its soundness gate REFUSES to
//! promote a slot whose loop body carries a refcount barrier op
//! (`DecRef`/`IncRef`) — a finalizer running during the decrement could observe
//! the half-updated slot, so promoting across it is unsound. If drop insertion
//! ran inside the per-function pipeline (at step-1, or inside the inliner's
//! per-caller re-opt), it would seed those barrier ops into the loop BEFORE
//! promotion ran, and promotion would refuse every module-global accumulator —
//! leaving a per-iteration `module_get_attr`/`module_set_attr`/`dec_ref`
//! round-trip ~5× slower than the promoted register flow. Running drops as the
//! FINAL phase lets promotion see the clean loop and lets drops land on the final
//! (promoted) shape. This is design 20 §2.1's "runs LAST" made
//! whole-program-correct: after the module phase, not merely last within one
//! per-function pipeline invocation.
//!
//! ## The two entry shapes (one drop implementation)
//!
//! Both delegate to the single [`crate::tir::passes::run_drop_phase`]
//! implementation; they differ only in the IR carrier they iterate:
//!
//! * [`finalize_module_drops`] — runs over a [`TirModule`] in TIR form. Called by
//!   [`crate::tir::module_phase::run_module_pipeline`] as its terminal step (the
//!   whole-program build path: native non-batched, LLVM, WASM non-batched). The
//!   inliner already built the TIR module, so no SimpleIR round-trip is needed.
//! * [`finalize_simple_ir_drops`] — runs over a slice of `FunctionIR` (SimpleIR).
//!   Called by the `skip_ir_passes` build paths (the stdlib-cache object and the
//!   per-batch application codegen) where the whole-program module phase does NOT
//!   run, so the per-function pipeline is the last transform and drops must be
//!   applied after it, post-cache. It lifts each function to TIR, runs the drop
//!   phase, and back-converts every function whose executable ops or RC authority
//!   facts changed to SimpleIR.
//!
//! Both honor the same per-function invariants: full-function idempotency (the
//! drop pass bails on the `drop_inserted` marker, so a re-processed
//! fully-owned-RC function is a no-op), exception-only pre-bail idempotency (the
//! narrower `exception_region_drops_inserted` marker protects handler-safe
//! CreationRef/MatchRef releases without suppressing native legacy RC), and the
//! debug double-process guard (a function must not arrive already fully
//! drop-inserted).

use super::function::{TirFunction, TirModule};
use super::ops::AttrValue;
use super::passes::drop_insertion::DROP_INSERTED_ATTR;
use super::target_info::TargetInfo;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::time::Instant;

fn drop_stage_audit_enabled() -> bool {
    molt_ir::stage_audit::stage_audit_enabled(molt_ir::stage_audit::AuditStage::Drop)
}

fn emit_drop_function_audit(
    stage: &str,
    func: &TirFunction,
    index: Option<usize>,
    total: Option<usize>,
    changed: Option<bool>,
    elapsed_ms: Option<u128>,
) {
    if !drop_stage_audit_enabled() {
        return;
    }
    if let Ok(filter) = backend_env::var(BackendEnv::MOLT_DROP_STAGE_AUDIT_FUNC)
        && !filter.trim().is_empty()
        && !func.name.contains(filter.trim())
    {
        return;
    }
    let blocks = func.blocks.len();
    let ops = func
        .blocks
        .values()
        .fold(0usize, |count, block| count.saturating_add(block.ops.len()));
    eprintln!(
        "[molt-drop-stage-audit] stage={stage} function={} index={} total={} blocks={} ops={} value_types={} attrs={} changed={} elapsed_ms={} peak_rss_mib={}",
        func.name,
        index
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        total
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        blocks,
        ops,
        func.value_types.len(),
        func.attrs.len(),
        changed
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        elapsed_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        crate::process_diagnostics::process_peak_rss_mib_label(),
    );
}

/// Run the existing typed reachability pass only while ownership is mutable.
/// Full and exception-only release insertion are both phase cuts: deleting a
/// definition afterward cannot rely on an idempotent drop pass to repair it.
fn prepare_final_operation_reachability(func: &mut TirFunction, tti: &TargetInfo) -> bool {
    if tti.target != super::target_info::TargetKind::Wasm
        || backend_env::var(BackendEnv::MOLT_DISABLE_DEAD_OP_ELIM).is_ok()
        || [
            DROP_INSERTED_ATTR,
            super::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR,
        ]
        .iter()
        .any(|marker| matches!(func.attrs.get(*marker), Some(AttrValue::Bool(true))))
    {
        return false;
    }
    // Canonical SSA effects preserve import initialization and exceptions even
    // when the imported value is unused. No source projection is re-analyzed.
    super::passes::dce::run(func).total_changes() > 0
}

#[cfg(test)]
#[path = "drop_phase/wasm_reachability_tests.rs"]
mod wasm_reachability_tests;

/// Complete the native driver's lowering transformations while SSA and
/// ownership are still mutable. Cranelift and LLVM previously performed this
/// family on source projections; both now use the same typed owner before
/// terminal drops. Other target policies are unchanged.
pub fn prepare_final_backend_lowering(func: &mut TirFunction, tti: &TargetInfo) -> bool {
    if !matches!(
        tti.target,
        super::target_info::TargetKind::NativeCranelift | super::target_info::TargetKind::Llvm
    ) {
        return false;
    }
    let mut changed = super::passes::copy_prop::run(func).total_changes() > 0;
    changed |= super::passes::deforestation::run_split_field_reads(func).total_changes() > 0;
    changed |= canonicalize_direct_raise_edges(func);
    changed
}

/// A transient summary of effect-free unconditional forwarding. Values remain
/// in the source block's SSA namespace; composing an edge substitutes only the
/// destination's block parameters. Each block chain is summarized once, so
/// many checks sharing a long forwarding tail do not repeatedly walk that tail.
/// Changed cached payloads are bounded by that block's parameter/edge input;
/// unchanged payloads share storage. Queries likewise cap changed output at
/// their own incoming width. Expected hash work and retained storage are thus
/// linear in the traversed graph and its actual argument data, not chain length
/// times the width of a distant handler. Wider transport remains unoptimized.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectRaiseEdge {
    target: super::blocks::BlockId,
    args: std::rc::Rc<[super::values::ValueId]>,
}

fn substitute_edge_values(
    parameters: &[super::values::TirValue],
    incoming: &[super::values::ValueId],
    values: &[super::values::ValueId],
) -> Option<Vec<super::values::ValueId>> {
    if parameters.len() != incoming.len() {
        return None;
    }
    #[cfg(test)]
    tests::record_substitution_work(parameters.len().saturating_add(values.len()));
    let substitutions: std::collections::HashMap<_, _> = parameters
        .iter()
        .zip(incoming)
        .map(|(parameter, value)| (parameter.id, *value))
        .collect();
    Some(
        values
            .iter()
            .map(|value| substitutions.get(value).copied().unwrap_or(*value))
            .collect(),
    )
}

/// Keep unchanged payloads shared. A changed composition is admitted only when
/// its width is bounded by the input transport being processed. Expanding
/// permutations/duplication remain real CFG edges instead of making this
/// peephole materialize a wide payload at every narrow forwarding block.
fn substitute_shared_edge_values(
    parameters: &[super::values::TirValue],
    incoming: &[super::values::ValueId],
    values: &std::rc::Rc<[super::values::ValueId]>,
    changed_width_budget: usize,
) -> Option<std::rc::Rc<[super::values::ValueId]>> {
    if parameters.len() != incoming.len() {
        return None;
    }
    #[cfg(test)]
    tests::record_substitution_work(parameters.len());
    if parameters
        .iter()
        .zip(incoming)
        .all(|(parameter, value)| parameter.id == *value)
    {
        return Some(std::rc::Rc::clone(values));
    }
    if values.len() > changed_width_budget {
        return None;
    }
    substitute_edge_values(parameters, incoming, values).map(Into::into)
}

struct DirectRaiseForwarding<'a> {
    function: &'a TirFunction,
    summaries: std::collections::HashMap<super::blocks::BlockId, Option<DirectRaiseEdge>>,
    resume_targets: std::collections::HashSet<super::blocks::BlockId>,
    handlers: std::collections::HashSet<super::blocks::BlockId>,
}

impl<'a> DirectRaiseForwarding<'a> {
    fn new(
        function: &'a TirFunction,
        handlers: std::collections::HashSet<super::blocks::BlockId>,
    ) -> Self {
        let mut resume_targets = std::collections::HashSet::new();
        for block in function.blocks.values() {
            if matches!(
                block.terminator,
                super::blocks::Terminator::StateDispatch { .. }
            ) {
                block.terminator.for_each_edge(|target, _| {
                    resume_targets.insert(target);
                });
            }
        }
        Self {
            function,
            summaries: std::collections::HashMap::new(),
            resume_targets,
            handlers,
        }
    }

    fn edge(
        &mut self,
        target: super::blocks::BlockId,
        args: &[super::values::ValueId],
    ) -> Option<DirectRaiseEdge> {
        use super::blocks::Terminator;
        let mut current = target;
        let mut path = Vec::new();
        let mut visiting = std::collections::HashSet::new();
        while !self.summaries.contains_key(&current) {
            if !visiting.insert(current) {
                self.summaries.insert(current, None);
                break;
            }
            let Some(block) = self.function.blocks.get(&current) else {
                self.summaries.insert(current, None);
                break;
            };
            if block.ops.is_empty()
                && !self.handlers.contains(&current)
                && !self.resume_targets.contains(&current)
                && !self.function.loop_roles.contains_key(&current)
                && let Terminator::Branch { target, .. } = &block.terminator
            {
                path.push(current);
                current = *target;
            } else {
                self.summaries.insert(
                    current,
                    Some(DirectRaiseEdge {
                        target: current,
                        args: block.args.iter().map(|value| value.id).collect(),
                    }),
                );
            }
        }
        for block_id in path.into_iter().rev() {
            let block = &self.function.blocks[&block_id];
            let Terminator::Branch { target, args } = &block.terminator else {
                unreachable!("only unconditional empty blocks enter a forwarding chain");
            };
            let summary =
                self.summaries[target]
                    .as_ref()
                    .map(|summary| {
                        match substitute_shared_edge_values(
                            &self.function.blocks[target].args,
                            args,
                            &summary.args,
                            block.args.len().saturating_add(args.len()),
                        ) {
                            Some(args) => DirectRaiseEdge {
                                target: summary.target,
                                args,
                            },
                            None => DirectRaiseEdge {
                                target: block_id,
                                args: block.args.iter().map(|value| value.id).collect(),
                            },
                        }
                    });
            self.summaries.insert(block_id, summary);
        }
        let summary = self.summaries[&target].as_ref()?;
        Some(DirectRaiseEdge {
            target: summary.target,
            args: substitute_shared_edge_values(
                &self.function.blocks.get(&target)?.args,
                args,
                &summary.args,
                args.len(),
            )?,
        })
    }

    fn branch(&mut self, block: &super::blocks::TirBlock) -> Option<DirectRaiseEdge> {
        let super::blocks::Terminator::Branch { target, args } = &block.terminator else {
            return None;
        };
        self.edge(*target, args)
    }
}

/// Remove duplicate exception transfers only when their unconditional path
/// reaches the same handler with the same SSA payload. The CFG splits at each
/// check, so the normal path may contain empty forwarding blocks. No executable
/// operation, observation, loop/resume boundary or conditional edge is skipped.
fn canonicalize_direct_raise_edges(func: &mut TirFunction) -> bool {
    use super::ops::{OpCode, TirOp};
    if !func
        .blocks
        .values()
        .any(|block| block.ops.iter().any(|op| op.opcode == OpCode::Raise))
    {
        return false;
    }
    let labels = super::dominators::exception_label_to_block(func);
    let mut forwarding = DirectRaiseForwarding::new(func, labels.values().copied().collect());
    let check_edge = |op: &TirOp| {
        if op.opcode != OpCode::CheckException || op.is_async_work_poll() || !op.results.is_empty()
        {
            return None;
        }
        let Some(AttrValue::Int(label)) = op.attrs.get("value") else {
            return None;
        };
        Some(DirectRaiseEdge {
            target: *labels.get(label)?,
            args: op.operands.clone().into(),
        })
    };
    let mut remove = std::collections::HashSet::new();
    for block in func.blocks.values() {
        let n = block.ops.len();
        if n < 2 || block.ops[n - 2].opcode != OpCode::Raise {
            continue;
        }
        if let Some(check) = check_edge(&block.ops[n - 1])
            && forwarding.branch(block).as_ref() == Some(&check)
        {
            remove.insert((block.id, n - 1));
        }
    }
    // A builtin exception constructor can fail by returning None with its
    // original error pending. molt_raise(None) preserves that pending error;
    // on success it raises the exact constructed exception. Removing the
    // intervening check is therefore legal only along this precise raise path.
    // Arbitrary constructors/calls and intervening operations are not admitted.
    let mut constructor_checks = Vec::new();
    for block in func.blocks.values() {
        for (index, pair) in block.ops.windows(2).enumerate() {
            let [producer, check] = pair else {
                unreachable!()
            };
            if producer.opcode != OpCode::Copy
                || !matches!(producer.origin, Some(kind)
                if matches!(kind, molt_ir::OpKind::ExceptionNewBuiltin | molt_ir::OpKind::ExceptionNewBuiltinEmpty | molt_ir::OpKind::ExceptionNewBuiltinOne))
                || producer.results.len() != 1
                || producer.is_async_work_poll()
                || producer.attrs.contains_key("runtime_symbol")
            {
                continue;
            }
            let Some(expected) = check_edge(check) else {
                continue;
            };
            let mut tail = block.ops[index + 2..]
                .iter()
                .enumerate()
                .filter(|(offset, _)| !remove.contains(&(block.id, index + 2 + *offset)))
                .map(|(_, op)| op);
            let next = tail.next();
            if tail.next().is_some() {
                continue;
            }
            let (raise, exit) = if let Some(raise) = next {
                if raise.opcode != OpCode::Raise
                    || raise.operands.len() != 1
                    || raise.is_async_work_poll()
                {
                    continue;
                }
                (raise.operands.clone(), forwarding.branch(block))
            } else {
                let super::blocks::Terminator::Branch { args: incoming, .. } = &block.terminator
                else {
                    continue;
                };
                let Some(entry) = forwarding.branch(block) else {
                    continue;
                };
                // Many constructor predecessors may share one Raise block.
                // A shared unchanged summary can be wider than this incoming
                // edge: refuse before building its parameter substitution map.
                let entry_budget = incoming
                    .len()
                    .saturating_add(expected.args.len())
                    .saturating_add(1);
                if entry.args.len() > entry_budget
                    || forwarding.resume_targets.contains(&entry.target)
                    || func.loop_roles.contains_key(&entry.target)
                {
                    continue;
                }
                let target = &func.blocks[&entry.target];
                let mut operations = target
                    .ops
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !remove.contains(&(target.id, *i)))
                    .map(|(_, op)| op);
                let Some(raise) = operations.next() else {
                    continue;
                };
                if operations.next().is_some()
                    || raise.opcode != OpCode::Raise
                    || raise.operands.len() != 1
                    || raise.is_async_work_poll()
                {
                    continue;
                }
                let Some(raised) =
                    substitute_edge_values(&target.args, &entry.args, &raise.operands)
                else {
                    continue;
                };
                let super::blocks::Terminator::Branch { args: outgoing, .. } = &target.terminator
                else {
                    continue;
                };
                // The effectful Raise block's exit is queried for each such
                // predecessor. Bound this query before composing a wide exit
                // that a narrow constructor check cannot consume.
                if outgoing.len() > expected.args.len() {
                    continue;
                }
                let exit = forwarding.branch(target).and_then(|edge| {
                    Some(DirectRaiseEdge {
                        target: edge.target,
                        args: substitute_shared_edge_values(
                            &target.args,
                            &entry.args,
                            &edge.args,
                            expected.args.len(),
                        )?,
                    })
                });
                (raised, exit)
            };
            if raise == producer.results && exit.as_ref() == Some(&expected) {
                constructor_checks.push((block.id, index + 1));
            }
        }
    }
    drop(forwarding);
    remove.extend(constructor_checks);
    for block in func.blocks.values_mut() {
        let mut index = 0;
        block.ops.retain(|_| {
            let keep = !remove.contains(&(block.id, index));
            index += 1;
            keep
        });
    }
    !remove.is_empty()
}

/// Run the RC drop phase on a single in-TIR function — the ONE per-function entry
/// every terminal-phase caller funnels through (the TIR-module finalizer, the
/// SimpleIR finalizer, and the LLVM `skip_ir_passes` branch). Returns `true` iff
/// the phase changed executable ops or semantic facts — i.e. the function now
/// carries `DecRef`/`IncRef` ops and/or one of the drop fact markers that must be
/// back-converted for SimpleIR consumers. `drop_inserted` is the full-function RC
/// authority marker that native codegen reads to suppress its competing
/// automatic temp-RC, so an attribute-only `drop_inserted` change counts even
/// when no physical `DecRef`/`IncRef` op was inserted.
/// `exception_region_drops_inserted` is only the handler-safe exception
/// transport slice and must not suppress native legacy RC. A function with no
/// droppable temporaries still reports `true` on activated targets when the
/// full-function marker is newly stamped; that marker is the backend authority
/// fact and must be preserved/back-converted.
///
/// `debug_assert`s that the function is not ALREADY drop-inserted on entry: the
/// only marker producers are this phase and the round-trip that preserves it, so
/// an already-marked function means drops were placed mid-transform (the bug
/// round-7 exists to prevent) or the finalizer ran twice. The drop pass is
/// idempotent (bails on the marker), so this is a debug-only invariant check, not
/// a correctness rail.
pub fn finalize_function_drops(func: &mut TirFunction, tti: &TargetInfo) -> bool {
    debug_assert!(
        !matches!(
            func.attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ),
        "function '{}' is already drop-inserted before the terminal drop phase \
         — drops were placed mid-transform or the finalizer ran twice",
        func.name,
    );
    let mut lowering_changed = prepare_final_backend_lowering(func, tti);
    let audit_start = Instant::now();
    let had_drop_inserted = matches!(
        func.attrs.get(DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    let had_exception_region_drops = matches!(
        func.attrs
            .get(super::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    // Drop placement keys on repr facts (`TirLivenessResult::is_raw_scalar`
    // distinguishes raw-i64/bool/float carriers — which hold no refcount and must
    // NOT be dropped — from heap-carrying values). Those facts are derived from
    // `value_types`, so the function MUST be type-refined before the drop pass
    // runs. When drops lived in the per-function pipeline this was guaranteed by
    // the `refine → run_pipeline → refine` bracket every backend wraps it in; now
    // that drops run as a separate terminal phase, functions the module phase did
    // NOT re-optimize (neither inlined nor promoted) arrive carrying only the
    // types their initial SimpleIR→TIR lift produced. Refining here makes the
    // invariant hold uniformly for every function (refinement is an idempotent
    // fixpoint, so re-refining the inlined/promoted bodies is a safe no-op).
    emit_drop_function_audit(
        "before-type-refine",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    super::type_refine::refine_types(func);
    emit_drop_function_audit(
        "after-type-refine",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    lowering_changed |= prepare_final_operation_reachability(func, tti);
    emit_drop_function_audit(
        "before-drop-pass",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    let stats = super::passes::run_drop_phase(func, tti);
    emit_drop_function_audit(
        "after-drop-pass",
        func,
        None,
        None,
        None,
        Some(audit_start.elapsed().as_millis()),
    );
    let changed: usize = stats
        .iter()
        .map(super::passes::PassStats::total_changes)
        .sum();
    let has_drop_inserted = matches!(
        func.attrs.get(DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    let has_exception_region_drops = matches!(
        func.attrs
            .get(super::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    );
    let changed = lowering_changed
        || changed > 0
        || had_drop_inserted != has_drop_inserted
        || had_exception_region_drops != has_exception_region_drops;
    emit_drop_function_audit(
        "after-change-count",
        func,
        None,
        None,
        Some(changed),
        Some(audit_start.elapsed().as_millis()),
    );
    changed
}

/// Terminal drop phase over a [`TirModule`] in TIR form. Runs the drop phase on
/// every function and returns the names of the functions it changed (those the
/// caller must back-convert to SimpleIR; the LLVM lane lowers the module directly
/// and ignores the return). The order of the returned names follows
/// `module.functions`.
pub fn finalize_module_drops(module: &mut TirModule, tti: &TargetInfo) -> Vec<String> {
    let mut changed = Vec::new();
    let total = module.functions.len();
    let audit_start = Instant::now();
    for (index, func) in module.functions.iter_mut().enumerate() {
        emit_drop_function_audit(
            "before-function",
            func,
            Some(index),
            Some(total),
            None,
            Some(audit_start.elapsed().as_millis()),
        );
        let did_change = finalize_function_drops(func, tti);
        emit_drop_function_audit(
            "after-function",
            func,
            Some(index),
            Some(total),
            Some(did_change),
            Some(audit_start.elapsed().as_millis()),
        );
        if did_change {
            changed.push(func.name.clone());
        }
    }
    changed
}

/// Terminal drop phase over SimpleIR `FunctionIR` bodies (the `skip_ir_passes`
/// build paths: stdlib-cache object + per-batch application codegen). For each
/// non-extern function it lifts to TIR (type-refined), runs the drop phase, and —
/// if the phase changed the body — back-converts the drop-inserted TIR to
/// SimpleIR in place (which re-emits the drop fact marker ops). Functions the
/// phase did not change keep their existing (post-per-function-pipeline)
/// SimpleIR untouched.
///
/// Extern functions (shared-stdlib-partition symbols with cleared bodies) are
/// skipped: they have no body to drop and lifting one would fail the TIR
/// verifier — the same contract the per-function pipeline honors.
///
/// This is the post-cache step: it runs AFTER the (cached) per-function pipeline,
/// so the cache never stores drop-inserted ops keyed by the drop-free input hash.
pub fn finalize_simple_ir_drops(functions: &mut [crate::ir::FunctionIR], tti: &TargetInfo) {
    finalize_simple_ir_drops_with_tir_custody(
        functions,
        tti,
        &mut std::collections::BTreeMap::new(),
        |_| {},
    );
}

/// Terminal drop phase over SimpleIR bodies, preferring already-optimized TIR
/// functions when the caller has them. This is the batch/native custody path:
/// per-function optimization produces typed TIR first, then SimpleIR only as a
/// legacy backend carrier. Running terminal drops from the TIR custody map avoids
/// re-lifting the expanded SimpleIR bridge form and keeps block-argument payloads
/// in TIR until the final backend lowering.
pub fn finalize_simple_ir_drops_with_tir_custody(
    functions: &mut [crate::ir::FunctionIR],
    tti: &TargetInfo,
    optimized_tir_by_name: &mut std::collections::BTreeMap<String, TirFunction>,
    mut consume_final_tir: impl FnMut(&TirFunction),
) {
    for func_ir in functions.iter_mut() {
        if func_ir.is_extern {
            continue;
        }
        let mut tir_func = optimized_tir_by_name
            .remove(&func_ir.name)
            .unwrap_or_else(|| super::lower_from_simple::lower_to_tir_for_target(func_ir, tti));
        // `finalize_function_drops` refines types before the drop pass (the drop
        // placement needs repr facts), so no separate refinement is needed here.
        // The function arriving here already went through the per-function pipeline
        // (its SimpleIR is the optimized output), so it is NOT yet drop-inserted —
        // its debug_assert verifies that.
        let changed = finalize_function_drops(&mut tir_func, tti);
        // Required consumer seam: final typed facts are projected while this
        // body's custody still exists, before its SimpleIR bridge consumes it.
        consume_final_tir(&tir_func);
        if changed {
            assert_eq!(
                func_ir.return_abi, tir_func.return_abi,
                "terminal drops must preserve the authored function return ABI"
            );
            let ops = super::lower_to_simple::lower_to_simple_ir(&tir_func);
            debug_assert!(
                super::lower_to_simple::validate_labels(&ops),
                "drop-phase back-conversion emitted invalid labels for '{}'",
                func_ir.name
            );
            func_ir.ops = ops;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::function::TirModule;

    std::thread_local! {
        static SUBSTITUTION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    pub(super) fn record_substitution_work(entries: usize) {
        SUBSTITUTION_WORK.with(|count| count.set(count.get().saturating_add(entries)));
    }

    #[test]
    fn direct_raise_split_cfg_preserves_payload_observations_and_projection() {
        use crate::tir::blocks::Terminator;
        use crate::tir::ops::{Dialect, OpCode, TirOp};
        use crate::tir::{TirBlock, TirType, TirValue};
        for (case, expected_checks) in [
            ("equivalent", 0),
            ("different_pre_payload", 1),
            ("different_post_payload", 2),
            ("different_target", 2),
            ("pre_poll", 1),
            ("post_poll", 2),
            ("release_before_check", 2),
            ("release_after_check", 2),
            ("callback_after_check", 2),
            ("constructor_override", 1),
            ("wrong_raised_value", 1),
            ("cycle", 2),
            ("conditional", 2),
            ("state_dispatch", 2),
        ] {
            let operation = |kind: molt_ir::OpKind, args: &[&str], out: Option<&str>| OpIR {
                kind,
                args: Some(args.iter().map(|name| (*name).into()).collect()),
                out: out.map(str::to_string),
                ..OpIR::new(kind)
            };
            let mut constructor =
                operation(molt_ir::OpKind::ExceptionNewBuiltinEmpty, &[], Some("exc"));
            constructor.value = Some(5);
            constructor.source_line = Some(31);
            constructor.source_op_idx = Some(700);
            let mut check = operation(molt_ir::OpKind::CheckException, &[], None);
            check.value = Some(9);
            let mut raise = operation(molt_ir::OpKind::Raise, &["exc"], None);
            raise.source_line = Some(32);
            raise.source_op_idx = Some(702);
            let mut jump = operation(molt_ir::OpKind::Jump, &[], None);
            jump.value = Some(9);
            let mut handler = operation(molt_ir::OpKind::Label, &[], None);
            handler.value = Some(9);
            let source = FunctionIR {
                name: format!("split_raise_{case}"),
                params: vec!["payload".into(), "other".into(), "condition".into()],
                param_types: Some(vec!["Any".into(), "Any".into(), "bool".into()]),
                return_abi: molt_ir::FunctionReturnAbi::Value,
                ops: vec![
                    constructor,
                    check.clone(),
                    raise,
                    check,
                    jump,
                    handler,
                    operation(molt_ir::OpKind::Ret, &["payload"], None),
                ],
                ..Default::default()
            };
            let mut typed = crate::tir::lower_from_simple::lower_to_tir(&source);
            let entry = typed.entry_block;
            let payload = typed.blocks[&entry].args[0].id;
            let other = typed.blocks[&entry].args[1].id;
            let condition = typed.blocks[&entry].args[2].id;
            // Strict dominance lets the lift use entry values directly in
            // these handlers. Give the payload comparison a real explicit SSA
            // transport contract instead of depending on redundant phi args
            // surviving that lift. The alternate target has the same payload
            // arity, so changing only a check's target remains valid TIR too.
            let alternate = typed.fresh_block();
            typed.label_id_map.insert(alternate.0, 10);
            typed.blocks.insert(
                alternate,
                TirBlock {
                    id: alternate,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            let labels = crate::tir::dominators::exception_label_to_block(&typed);
            for (label, incoming) in [(9, payload), (10, other)] {
                let handler = labels[&label];
                let parameter = typed.fresh_value();
                typed.value_types.insert(parameter, TirType::DynBox);
                let block = typed.blocks.get_mut(&handler).unwrap();
                assert!(block.args.is_empty());
                if label == 9 {
                    assert!(matches!(
                        &block.terminator,
                        Terminator::Return { values } if values == &[incoming]
                    ));
                }
                block.args.push(TirValue {
                    id: parameter,
                    ty: TirType::DynBox,
                });
                block.terminator = Terminator::Return {
                    values: vec![parameter],
                };
                for block in typed.blocks.values_mut() {
                    block.terminator.for_each_edge_mut(|target, args| {
                        if *target == handler {
                            assert!(args.is_empty());
                            args.push(incoming);
                        }
                    });
                    for operation in &mut block.ops {
                        if operation.opcode == OpCode::CheckException
                            && operation.attrs.get("value") == Some(&AttrValue::Int(label))
                        {
                            assert!(operation.operands.is_empty());
                            operation.operands.push(incoming);
                        }
                    }
                }
            }
            let raise_block = typed
                .blocks
                .values()
                .find(|block| block.ops.iter().any(|op| op.opcode == OpCode::Raise))
                .unwrap()
                .id;
            assert_ne!(
                entry, raise_block,
                "the real lift must split at the first check"
            );
            let Terminator::Branch {
                target: forwarding_block,
                ..
            } = typed.blocks[&raise_block].terminator
            else {
                panic!("the real post-check continuation must be an unconditional block");
            };
            assert!(typed.blocks[&forwarding_block].ops.is_empty());
            let effect = |opcode| TirOp {
                origin: None,
                dialect: Dialect::Molt,
                opcode,
                operands: vec![other],
                results: vec![],
                attrs: Default::default(),
                source_span: Some((17, 23)),
            };
            match case {
                "different_pre_payload" | "pre_poll" => {
                    let check = typed
                        .blocks
                        .get_mut(&entry)
                        .unwrap()
                        .ops
                        .last_mut()
                        .unwrap();
                    if case == "pre_poll" {
                        check.mark_async_work_poll();
                    } else {
                        assert_eq!(check.operands.len(), 1);
                        check.operands[0] = other;
                    }
                }
                "different_post_payload" | "different_target" | "post_poll" => {
                    let check = typed
                        .blocks
                        .get_mut(&raise_block)
                        .unwrap()
                        .ops
                        .last_mut()
                        .unwrap();
                    if case == "post_poll" {
                        check.mark_async_work_poll();
                    } else if case == "different_target" {
                        check.attrs.insert("value".into(), AttrValue::Int(10));
                    } else {
                        assert_eq!(check.operands.len(), 1);
                        check.operands[0] = other;
                    }
                }
                "release_before_check" => {
                    let block = typed.blocks.get_mut(&raise_block).unwrap();
                    block
                        .ops
                        .insert(block.ops.len() - 1, effect(OpCode::DecRef));
                }
                "release_after_check" | "callback_after_check" => {
                    let mut operation = effect(if case == "release_after_check" {
                        OpCode::DecRef
                    } else {
                        OpCode::CallBuiltin
                    });
                    if case == "callback_after_check" {
                        operation
                            .attrs
                            .insert("name".into(), AttrValue::Str("user_callback".into()));
                    }
                    typed
                        .blocks
                        .get_mut(&forwarding_block)
                        .unwrap()
                        .ops
                        .push(operation);
                }
                "constructor_override" => {
                    typed.blocks.get_mut(&entry).unwrap().ops[0].attrs.insert(
                        "runtime_symbol".into(),
                        AttrValue::Str("user_constructor".into()),
                    );
                }
                "wrong_raised_value" => {
                    typed.blocks.get_mut(&raise_block).unwrap().ops[0].operands = vec![other];
                }
                "cycle" | "conditional" | "state_dispatch" => {
                    let block = typed.blocks.get_mut(&forwarding_block).unwrap();
                    let Terminator::Branch { target, args } = block.terminator.clone() else {
                        unreachable!()
                    };
                    block.terminator = match case {
                        "cycle" => Terminator::Branch {
                            target: forwarding_block,
                            args: block.args.iter().map(|arg| arg.id).collect(),
                        },
                        "conditional" => Terminator::CondBranch {
                            cond: condition,
                            then_block: target,
                            then_args: args.clone(),
                            else_block: target,
                            else_args: args,
                        },
                        _ => Terminator::StateDispatch {
                            cases: vec![(0, target, args.clone())],
                            default: target,
                            default_args: args,
                        },
                    };
                }
                "equivalent" => {}
                _ => unreachable!(),
            }
            assert_eq!(
                crate::tir::verify::verify_function(&typed),
                Ok(()),
                "{case}: input"
            );
            let mut expected_survivors = typed.clone();
            for block in expected_survivors.blocks.values_mut() {
                block.ops.retain(|op| op.opcode != OpCode::CheckException);
            }
            canonicalize_direct_raise_edges(&mut typed);
            assert_eq!(
                crate::tir::verify::verify_function(&typed),
                Ok(()),
                "{case}: output"
            );
            let checks = typed
                .blocks
                .values()
                .flat_map(|block| &block.ops)
                .filter(|op| op.opcode == OpCode::CheckException)
                .count();
            assert_eq!(checks, expected_checks, "{case}");
            let projected = crate::tir::lower_to_simple::lower_to_simple_ir(&typed);
            assert_eq!(
                projected
                    .iter()
                    .filter(|op| matches!(
                        op.kind,
                        molt_ir::OpKind::CheckException | molt_ir::OpKind::AsyncWorkPoll
                    ))
                    .count(),
                expected_checks,
                "{case}: final projection must preserve the typed decision"
            );
            assert!(
                projected.iter().any(|op| op.kind == molt_ir::OpKind::Raise
                    && op.source_line == Some(32)
                    && op.source_op_idx == Some(702)),
                "{case}: raise provenance"
            );
            assert!(
                projected
                    .iter()
                    .any(|op| op.kind == molt_ir::OpKind::ExceptionNewBuiltinEmpty
                        && op.source_line == Some(31)
                        && op.source_op_idx == Some(700)),
                "{case}: constructor provenance"
            );
            for block in typed.blocks.values_mut() {
                block.ops.retain(|op| op.opcode != OpCode::CheckException);
            }
            assert_eq!(
                serde_json::to_value(&typed).unwrap(),
                serde_json::to_value(&expected_survivors).unwrap(),
                "{case}: only redundant checks may change; CFG payloads, types and operation metadata stay exact"
            );
        }
    }

    #[test]
    fn direct_raise_forwarding_preserves_valid_loop_and_resume_entries() {
        use crate::tir::blocks::LoopRole;
        use crate::tir::ops::{Dialect, OpCode, TirOp};
        use crate::tir::{Terminator, TirBlock, TirType};
        for boundary in ["ordinary", "loop", "resume"] {
            let mut function = TirFunction::new(
                format!("raise_boundary_{boundary}"),
                vec![TirType::DynBox, TirType::Bool],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let entry = function.entry_block;
            let exception = function.blocks[&entry].args[0].id;
            let condition = function.blocks[&entry].args[1].id;
            let forwarding = function.fresh_block();
            let handler = function.fresh_block();
            function.label_id_map.insert(handler.0, 9);
            function.has_exception_handling = true;
            function.blocks.get_mut(&entry).unwrap().ops = vec![
                TirOp {
                    origin: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::Raise,
                    operands: vec![exception],
                    results: vec![],
                    attrs: Default::default(),
                    source_span: None,
                },
                TirOp {
                    origin: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::CheckException,
                    operands: vec![],
                    results: vec![],
                    attrs: [("value".into(), AttrValue::Int(9))].into(),
                    source_span: None,
                },
            ];
            function.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
                target: forwarding,
                args: vec![],
            };
            function.blocks.insert(
                forwarding,
                TirBlock {
                    id: forwarding,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Branch {
                        target: handler,
                        args: vec![],
                    },
                },
            );
            function.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            if boundary == "loop" {
                // The protected empty block is an actual header with a backedge,
                // not just a role annotation on an acyclic malformed fixture.
                let exit = function.fresh_block();
                function.blocks.insert(
                    exit,
                    TirBlock {
                        id: exit,
                        args: vec![],
                        ops: vec![],
                        terminator: Terminator::Return { values: vec![] },
                    },
                );
                function.blocks.get_mut(&handler).unwrap().terminator = Terminator::CondBranch {
                    cond: condition,
                    then_block: forwarding,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                };
                function.loop_roles.insert(forwarding, LoopRole::LoopHeader);
            } else if boundary == "resume" {
                // Move the parameter definitions into the real dispatch entry;
                // they still dominate both initial execution and resumed entry.
                let dispatch = function.fresh_block();
                let args = std::mem::take(&mut function.blocks.get_mut(&entry).unwrap().args);
                function.blocks.insert(
                    dispatch,
                    TirBlock {
                        id: dispatch,
                        args,
                        ops: vec![],
                        terminator: Terminator::StateDispatch {
                            cases: vec![(1, forwarding, vec![])],
                            default: entry,
                            default_args: vec![],
                        },
                    },
                );
                function.entry_block = dispatch;
            }
            assert_eq!(
                crate::tir::verify::verify_function(&function),
                Ok(()),
                "{boundary}: input"
            );
            assert_eq!(
                canonicalize_direct_raise_edges(&mut function),
                boundary == "ordinary"
            );
            assert_eq!(
                crate::tir::verify::verify_function(&function),
                Ok(()),
                "{boundary}: output"
            );
            let projected = crate::tir::lower_to_simple::lower_to_simple_ir(&function);
            assert_eq!(
                projected
                    .iter()
                    .filter(|op| op.kind == molt_ir::OpKind::CheckException)
                    .count(),
                usize::from(boundary != "ordinary"),
                "{boundary}: the final projection must retain a protected entry check"
            );
        }
    }

    #[test]
    fn direct_raise_forwarding_does_not_expand_narrow_chains_into_wide_payloads() {
        use crate::tir::{Terminator, TirBlock, TirType, TirValue};
        use std::collections::{HashMap, HashSet};
        use std::rc::Rc;
        const CHAIN: usize = 128;
        const WIDTH: usize = 256;
        for transported in [false, true] {
            let mut function = TirFunction::new(
                "wide_forwarding".into(),
                vec![TirType::DynBox],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let entry = function.entry_block;
            let value = function.blocks[&entry].args[0].id;
            let chain: Vec<_> = (0..CHAIN).map(|_| function.fresh_block()).collect();
            let handler = function.fresh_block();
            let mut parameters = Vec::new();
            for _ in 0..WIDTH {
                let id = function.fresh_value();
                function.value_types.insert(id, TirType::DynBox);
                parameters.push(TirValue {
                    id,
                    ty: TirType::DynBox,
                });
            }
            function.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: parameters,
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            for (index, id) in chain.iter().copied().enumerate() {
                let local = if transported {
                    function.fresh_value()
                } else {
                    value
                };
                if transported {
                    function.value_types.insert(local, TirType::DynBox);
                }
                let target = chain.get(index + 1).copied().unwrap_or(handler);
                function.blocks.insert(
                    id,
                    TirBlock {
                        id,
                        args: if transported {
                            vec![TirValue {
                                id: local,
                                ty: TirType::DynBox,
                            }]
                        } else {
                            vec![]
                        },
                        ops: vec![],
                        terminator: Terminator::Branch {
                            target,
                            args: if target == handler {
                                vec![local; WIDTH]
                            } else if transported {
                                vec![local]
                            } else {
                                vec![]
                            },
                        },
                    },
                );
            }
            let incoming = if transported { vec![value] } else { vec![] };
            function.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
                target: chain[0],
                args: incoming.clone(),
            };
            assert_eq!(crate::tir::verify::verify_function(&function), Ok(()));
            // Input size is measured independently from the summaries. A
            // Cartesian chain-by-handler expansion violates this bound.
            let input_values: usize = function
                .blocks
                .values()
                .map(|block| {
                    let mut count = block.args.len();
                    block
                        .terminator
                        .for_each_edge(|_, args| count += args.len());
                    count
                })
                .sum();
            let mut forwarding = DirectRaiseForwarding::new(&function, HashSet::from([handler]));
            let edge = forwarding
                .edge(chain[0], &incoming)
                .expect("valid transport");
            if transported {
                assert_ne!(
                    edge.target, handler,
                    "narrow changed transport must stop before expansion"
                );
            } else {
                assert_eq!(edge.target, handler);
                assert_eq!(edge.args.as_ref(), vec![value; WIDTH]);
                for block in &chain {
                    assert!(Rc::ptr_eq(
                        &edge.args,
                        &forwarding.summaries[block].as_ref().unwrap().args
                    ));
                }
            }
            let mut distinct_payloads = HashMap::new();
            for summary in forwarding.summaries.values().flatten() {
                distinct_payloads.insert(summary.args.as_ptr(), summary.args.len());
            }
            assert!(
                distinct_payloads.values().sum::<usize>() <= input_values,
                "retained payload storage must fit the actual CFG transport input"
            );
        }
    }

    #[test]
    fn direct_raise_shared_block_bounds_work_for_many_narrow_constructor_checks() {
        use crate::tir::ops::{Dialect, OpCode, TirOp};
        use crate::tir::{Terminator, TirBlock, TirType, TirValue};
        const PREDECESSORS: usize = 128;
        const WIDTH: usize = 256;
        for wide_entry in [true, false] {
            let mut function = TirFunction::new(
                "shared_raise_work".into(),
                vec![TirType::I64, TirType::DynBox],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let entry = function.entry_block;
            let selector = function.blocks[&entry].args[0].id;
            let payload = function.blocks[&entry].args[1].id;
            let narrow_handler = function.fresh_block();
            let wide_handler = function.fresh_block();
            let raise_block = function.fresh_block();
            let forwarding_block = function.fresh_block();
            function.has_exception_handling = true;
            function.label_id_map.insert(narrow_handler.0, 9);
            function.label_id_map.insert(wide_handler.0, 10);
            for (block, width) in [
                (narrow_handler, 0),
                (wide_handler, WIDTH),
                (raise_block, if wide_entry { WIDTH } else { 1 }),
                (forwarding_block, 0),
            ] {
                let mut args = Vec::new();
                for _ in 0..width {
                    let id = function.fresh_value();
                    function.value_types.insert(id, TirType::DynBox);
                    args.push(TirValue {
                        id,
                        ty: TirType::DynBox,
                    });
                }
                function.blocks.insert(
                    block,
                    TirBlock {
                        id: block,
                        args,
                        ops: vec![],
                        terminator: Terminator::Return { values: vec![] },
                    },
                );
            }
            let operation = |opcode, operands, results, attrs| TirOp {
                origin: None,
                dialect: Dialect::Molt,
                opcode,
                operands,
                results,
                attrs,
                source_span: None,
            };
            let check = |label, args| {
                operation(
                    OpCode::CheckException,
                    args,
                    vec![],
                    [("value".into(), AttrValue::Int(label))].into(),
                )
            };
            let raised = function.blocks[&raise_block].args[0].id;
            let exit_args = if wide_entry {
                vec![]
            } else {
                vec![payload; WIDTH]
            };
            function.blocks.get_mut(&raise_block).unwrap().ops = vec![
                operation(OpCode::Raise, vec![raised], vec![], Default::default()),
                check(if wide_entry { 9 } else { 10 }, exit_args.clone()),
            ];
            function.blocks.get_mut(&raise_block).unwrap().terminator = Terminator::Branch {
                target: if wide_entry {
                    narrow_handler
                } else {
                    wide_handler
                },
                args: exit_args,
            };
            if wide_entry {
                // Every narrow predecessor sees the same unchanged wide entry
                // summary; the eventual raised value is not its constructor.
                function
                    .blocks
                    .get_mut(&forwarding_block)
                    .unwrap()
                    .terminator = Terminator::Branch {
                    target: raise_block,
                    args: vec![payload; WIDTH],
                };
            }
            let mut cases = Vec::new();
            for index in 0..PREDECESSORS {
                let block = function.fresh_block();
                let constructed = function.fresh_value();
                function.value_types.insert(constructed, TirType::DynBox);
                function.blocks.insert(
                    block,
                    TirBlock {
                        id: block,
                        args: vec![],
                        ops: vec![
                            operation(
                                OpCode::Copy,
                                vec![],
                                vec![constructed],
                                [("value".into(), AttrValue::Int(5))].into(),
                            )
                            .with_origin(molt_ir::OpKind::ExceptionNewBuiltinEmpty),
                            check(9, vec![]),
                        ],
                        terminator: Terminator::Branch {
                            target: if wide_entry {
                                forwarding_block
                            } else {
                                raise_block
                            },
                            args: if wide_entry {
                                vec![]
                            } else {
                                vec![constructed]
                            },
                        },
                    },
                );
                cases.push((index as i64, block, vec![]));
            }
            function.blocks.get_mut(&entry).unwrap().terminator = Terminator::Switch {
                value: selector,
                default: cases[0].1,
                default_args: vec![],
                cases,
            };
            assert_eq!(crate::tir::verify::verify_function(&function), Ok(()));
            let input_entries: usize = function
                .blocks
                .values()
                .map(|block| {
                    let mut count = 1 + block.args.len() + block.ops.len();
                    for op in &block.ops {
                        count += op.operands.len() + op.results.len();
                    }
                    block
                        .terminator
                        .for_each_edge(|_, args| count += args.len());
                    count
                })
                .sum();
            SUBSTITUTION_WORK.with(|count| count.set(0));
            assert!(canonicalize_direct_raise_edges(&mut function));
            let work = SUBSTITUTION_WORK.with(std::cell::Cell::get);
            assert!(
                work <= 4 * input_entries,
                "wide_entry={wide_entry}: {work} visited substitution entries exceed actual source input {input_entries}"
            );
            assert_eq!(crate::tir::verify::verify_function(&function), Ok(()));
            assert_eq!(
                function
                    .blocks
                    .values()
                    .flat_map(|block| &block.ops)
                    .filter(|op| op.opcode == OpCode::CheckException)
                    .count(),
                PREDECESSORS,
                "all narrow constructor checks remain; only the shared Raise's equal exit check is redundant"
            );
        }
    }

    /// A function with no droppable heap temporaries still reports changed on an
    /// activated target because the full-function `drop_inserted` authority fact
    /// must survive the pass-manager snapshot/restore boundary.
    #[test]
    fn module_finalizer_reports_marker_only_change_for_trivial_function() {
        // A trivial `return` function has no values whose ownership must move.
        let func_ir = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "trivial".into(),
            params: vec![],
            ops: vec![OpIR {
                kind: molt_ir::OpKind::RetVoid,
                ..OpIR::new(molt_ir::OpKind::RetVoid)
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let target = TargetInfo::native_release_fast();
        let mut tir = crate::tir::lower_from_simple::lower_to_tir_for_target(&func_ir, &target);
        crate::tir::type_refine::refine_types(&mut tir);
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![tir],
        };
        let changed = finalize_module_drops(&mut module, &target);
        assert_eq!(changed, vec!["trivial"]);
        assert!(matches!(
            module.functions[0].attrs.get(DROP_INSERTED_ATTR),
            Some(AttrValue::Bool(true))
        ));
        assert!(
            module.functions[0]
                .blocks
                .values()
                .flat_map(|block| block.ops.iter())
                .all(|op| op.opcode != super::super::ops::OpCode::DecRef
                    && op.opcode != super::super::ops::OpCode::IncRef),
            "ret_void needs no physical RC ops"
        );
    }

    #[test]
    fn luau_terminal_drop_phase_does_not_materialize_target_dead_rc_authority() {
        let func_ir = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "luau_gc".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: molt_ir::OpKind::ConstStr,
                    s_value: Some("owned".into()),
                    out: Some("temporary".into()),
                    ..OpIR::new(molt_ir::OpKind::ConstStr)
                },
                OpIR {
                    kind: molt_ir::OpKind::RetVoid,
                    ..OpIR::new(molt_ir::OpKind::RetVoid)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };
        let mut tir = crate::tir::lower_from_simple::lower_to_tir(&func_ir);
        crate::tir::type_refine::refine_types(&mut tir);
        let mut module = TirModule {
            name: "m".into(),
            functions: vec![tir],
        };

        let changed = finalize_module_drops(&mut module, &TargetInfo::luau_release_fast());
        assert!(changed.is_empty());
        assert!(!module.functions[0].attrs.contains_key(DROP_INSERTED_ATTR));
        assert!(
            module.functions[0]
                .blocks
                .values()
                .flat_map(|block| block.ops.iter())
                .all(|op| !matches!(
                    op.opcode,
                    super::super::ops::OpCode::DecRef | super::super::ops::OpCode::IncRef
                )),
            "Luau's GC target must not receive target-dead RC operations"
        );
    }

    /// Extern functions are skipped by the SimpleIR finalizer (no body to drop).
    #[test]
    fn simple_ir_finalizer_skips_extern() {
        let mut funcs = vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "ext".into(),
            params: vec![],
            ops: vec![],
            param_types: None,
            source_file: None,
            is_extern: true,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }];
        // Must not panic (no lift of the empty extern body).
        finalize_simple_ir_drops(&mut funcs, &TargetInfo::native_release_fast());
        assert!(funcs[0].ops.is_empty());
    }

    #[test]
    fn simple_ir_finalizer_prefers_tir_custody() {
        let mut funcs = vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "custody".into(),
            params: vec![],
            ops: vec![OpIR {
                kind: molt_ir::OpKind::RetVoid,
                ..OpIR::new(molt_ir::OpKind::RetVoid)
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }];

        let mut tir = TirFunction::new(
            "custody".into(),
            vec![],
            crate::tir::types::TirType::I64,
            molt_ir::FunctionReturnAbi::Value,
        );
        let value = tir.fresh_value();
        tir.value_types
            .insert(value, crate::tir::types::TirType::I64);
        tir.blocks.get_mut(&tir.entry_block).unwrap().ops = vec![crate::tir::ops::TirOp {
            origin: None,
            dialect: crate::tir::ops::Dialect::Molt,
            opcode: crate::tir::ops::OpCode::ConstInt,
            operands: vec![],
            results: vec![value],
            attrs: {
                let mut attrs = crate::tir::ops::AttrDict::new();
                attrs.insert("value".into(), crate::tir::ops::AttrValue::Int(7));
                attrs
            },
            source_span: None,
        }];
        tir.blocks.get_mut(&tir.entry_block).unwrap().terminator =
            crate::tir::blocks::Terminator::Return {
                values: vec![value],
            };

        let mut custody = std::collections::BTreeMap::new();
        custody.insert("custody".into(), tir);
        let mut consumed = Vec::new();
        finalize_simple_ir_drops_with_tir_custody(
            &mut funcs,
            &TargetInfo::native_release_fast(),
            &mut custody,
            |function| {
                assert_eq!(
                    function.attrs.get(DROP_INSERTED_ATTR),
                    Some(&AttrValue::Bool(true)),
                    "consumer must observe final lifetime authority"
                );
                consumed.push(function.name.clone());
            },
        );

        assert_eq!(consumed, ["custody"]);
        assert!(custody.is_empty());
        assert!(
            funcs[0]
                .ops
                .iter()
                .any(|op| op.kind == molt_ir::OpKind::Const && op.value == Some(7)),
            "finalizer must lower the TIR custody body, not the stale SimpleIR body"
        );
    }

    #[test]
    fn simple_ir_finalizer_back_converts_zero_drop_authority_marker() {
        let mut funcs = vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "borrowed_param_store".into(),
            params: vec!["self".into()],
            ops: vec![
                OpIR {
                    kind: molt_ir::OpKind::StoreVar,
                    var: Some("self".into()),
                    args: Some(vec!["self".into()]),
                    ..OpIR::new(molt_ir::OpKind::StoreVar)
                },
                OpIR {
                    kind: molt_ir::OpKind::RetVoid,
                    ..OpIR::new(molt_ir::OpKind::RetVoid)
                },
            ],
            param_types: Some(vec!["Any".into()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }];

        finalize_simple_ir_drops(&mut funcs, &TargetInfo::native_release_fast());

        assert_eq!(funcs[0].ops[0].kind, molt_ir::OpKind::DropInserted);
        assert!(
            funcs[0]
                .ops
                .iter()
                .all(|op| op.kind != molt_ir::OpKind::DecRef && op.kind != molt_ir::OpKind::IncRef),
            "borrowed-param-only SimpleIR must only gain the authority marker"
        );
    }

    #[test]
    fn native_roundtrip_preserves_call_bind_finalizer_fact_for_absorption_drops() {
        let func_ir = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "call_bind_finalizer_roundtrip".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: molt_ir::OpKind::ConstNone,
                    out: Some("cls".into()),
                    ..OpIR::new(molt_ir::OpKind::ConstNone)
                },
                OpIR {
                    kind: molt_ir::OpKind::CallargsNew,
                    out: Some("callargs".into()),
                    ..OpIR::new(molt_ir::OpKind::CallargsNew)
                },
                OpIR {
                    kind: molt_ir::OpKind::CallBind,
                    args: Some(vec!["cls".into(), "callargs".into()]),
                    out: Some("item".into()),
                    defines_del: Some(true),
                    ..OpIR::new(molt_ir::OpKind::CallBind)
                },
                OpIR {
                    kind: molt_ir::OpKind::ListNew,
                    args: Some(vec!["item".into()]),
                    out: Some("bag".into()),
                    bound_local: Some(true),
                    ..OpIR::new(molt_ir::OpKind::ListNew)
                },
                OpIR {
                    kind: molt_ir::OpKind::ConstStr,
                    s_value: Some("inside".into()),
                    out: Some("msg".into()),
                    ..OpIR::new(molt_ir::OpKind::ConstStr)
                },
                OpIR {
                    kind: molt_ir::OpKind::WarnStderr,
                    args: Some(vec!["msg".into()]),
                    ..OpIR::new(molt_ir::OpKind::WarnStderr)
                },
                OpIR {
                    kind: molt_ir::OpKind::RetVoid,
                    ..OpIR::new(molt_ir::OpKind::RetVoid)
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        };

        let tti = TargetInfo::native_release_fast();
        let mut optimized_tir =
            crate::tir::lower_from_simple::lower_to_tir_for_target(&func_ir, &tti);
        crate::tir::type_refine::refine_types(&mut optimized_tir);
        crate::tir::passes::run_pipeline(&mut optimized_tir, &tti);
        crate::tir::type_refine::refine_types(&mut optimized_tir);
        let optimized_ops = crate::tir::lower_to_simple::lower_to_simple_ir(&optimized_tir);

        let roundtripped_call = optimized_ops
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::CallBind)
            .expect("native per-function roundtrip must preserve call_bind");
        let item_name = roundtripped_call
            .out
            .clone()
            .expect("call_bind finalizer result must keep an output name");
        let bag_name = optimized_ops
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::ListNew)
            .and_then(|op| op.out.clone())
            .expect("absorbing list_new result must keep an output name");
        let roundtripped_list = optimized_ops
            .iter()
            .find(|op| op.kind == molt_ir::OpKind::ListNew)
            .expect("absorbing list_new must survive native per-function roundtrip");
        assert_eq!(
            roundtripped_call.defines_del,
            Some(true),
            "defines_del is a result-lifetime fact and must survive native's \
             optimize-roundtrip before terminal drop insertion"
        );
        assert_eq!(
            roundtripped_list.bound_local,
            Some(true),
            "bound_local is the Python lifetime-boundary fact and must survive \
             native's optimize-roundtrip before terminal drop insertion"
        );

        let mut funcs = vec![FunctionIR {
            return_abi: func_ir.return_abi,
            name: func_ir.name.clone(),
            params: func_ir.params.clone(),
            ops: optimized_ops,
            param_types: func_ir.param_types.clone(),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        }];
        finalize_simple_ir_drops(&mut funcs, &tti);
        let ops = &funcs[0].ops;
        // This oracle claims the no-error execution. Handler layout is not
        // return identity: follow the canonical CFG's ordinary successors,
        // leaving its separately represented exception edges untaken.
        let cfg = crate::tir::cfg::CFG::build(ops);
        let mut normal_ops = Vec::new();
        let mut visited = vec![false; cfg.blocks.len()];
        let mut block = cfg.entry;
        loop {
            assert!(!visited[block], "normal finalizer path must terminate");
            visited[block] = true;
            let range = &cfg.blocks[block];
            normal_ops.extend(ops[range.start_op..range.end_op].iter());
            match cfg.successors[block].as_slice() {
                [] => break,
                [next] => block = *next,
                successors => panic!("normal finalizer path is ambiguous: {successors:?}"),
            }
        }
        let ops = normal_ops;
        let warn_idx = ops
            .iter()
            .position(|op| op.kind == molt_ir::OpKind::WarnStderr)
            .expect("side effect must remain in the lowered body");
        let ret_idx = ops
            .len()
            .checked_sub(1)
            .expect("function must still return");
        assert_eq!(ops[ret_idx].kind, molt_ir::OpKind::RetVoid);
        let list_idx = ops
            .iter()
            .position(|op| op.kind == molt_ir::OpKind::ListNew)
            .expect("absorbing list constructor must remain in the lowered body");

        assert!(
            ops[list_idx + 1..warn_idx]
                .iter()
                .any(|op| op.kind == molt_ir::OpKind::DecRef
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == &item_name))),
            "drop insertion must release the absorbed call-owned item after \
             list_new takes ownership; ops={ops:?}"
        );
        assert!(
            ops[..warn_idx].iter().all(|op| {
                op.kind != molt_ir::OpKind::DecRef
                    || !op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| arg == &bag_name))
            }),
            "the finalizer-sensitive container root must survive until after \
             later side effects; ops={ops:?}"
        );
        let post_warn_dec_refs: Vec<_> = ops[warn_idx + 1..ret_idx]
            .iter()
            .filter(|op| {
                op.kind == molt_ir::OpKind::DecRef
                    && op.args.as_ref().is_some_and(|args| {
                        args.iter().any(|arg| arg == &item_name || arg == &bag_name)
                    })
            })
            .collect();
        assert!(
            post_warn_dec_refs.iter().all(|op| {
                !op.args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| arg == &item_name))
            }),
            "the absorbed item must not get a second terminal drop; ops={ops:?}"
        );
        assert!(
            post_warn_dec_refs.iter().any(|op| {
                op.args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(|arg| arg == &bag_name))
            }),
            "terminal drop insertion must still release the finalizer-sensitive \
             container root before returning; ops={ops:?}"
        );
    }
}
