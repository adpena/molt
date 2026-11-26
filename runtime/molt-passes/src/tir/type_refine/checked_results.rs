//! A call returns a physical word before its exception observer can transfer
//! control. Only uses dominated by the observer's continuation see success.

use std::cell::OnceCell;

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::call_targets::{
    direct_boxed_runtime, invocation_before_observer, is_compiled_invocation,
    preserves_invocation_observation, static_call,
};
use crate::tir::dominators::{
    ProgramPointDominance, checked_exception_observer, exception_label_to_block,
    executable_reverse_postorder,
};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;

#[derive(Clone, Copy)]
pub(super) struct Site {
    pub block: BlockId,
    pub call: usize,
    check: usize,
    pub compiled: bool,
    pub physical_use: bool,
    implicit_handler_use: bool,
    normal_use: bool,
    projected: bool,
}

impl Site {
    fn available(
        &self,
        func: &TirFunction,
        points: &OnceCell<ProgramPointDominance>,
        block: BlockId,
        index: usize,
    ) -> bool {
        if block == self.block {
            index > self.check
        } else {
            points
                .get_or_init(|| ProgramPointDominance::compute_executable(func))
                .definition_available(self.block, Some(self.check), block, index)
        }
    }
}

/// A fused poll on an authored exception check still observes this call.
/// An independent poll has no such association. The exceptional payload
/// must bind the actual, uniquely resolved handler's arguments.
pub(crate) fn checked_result_observer(
    func: &TirFunction,
    labels: &OnceCell<HashMap<i64, BlockId>>,
    op: &TirOp,
) -> Option<BlockId> {
    if !matches!(op.origin, None | Some(molt_ir::OpKind::CheckException)) || !op.results.is_empty()
    {
        return None;
    }
    checked_exception_observer(op, |label| {
        labels
            .get_or_init(|| exception_label_to_block(func))
            .get(&label)
            .copied()
    })
    .filter(|target| {
        func.blocks
            .get(target)
            .is_some_and(|block| block.args.len() == op.exception_payload().len())
    })
}

/// Materialize the verified association after ownership and final lowering.
/// Subsequent rewrites remap the bound word with the other SSA operands.
pub(crate) fn bind_checked_invocations(func: &mut TirFunction) -> bool {
    let labels = OnceCell::new();
    let mut bindings = Vec::new();
    for body in func.blocks.values() {
        for (index, check) in body.ops.iter().enumerate() {
            if check.opcode == OpCode::CheckException
                && !check.has_checked_invocation_binding()
                && check.exception_edge_kind().is_some()
                && let Some((_, call)) = invocation_before_observer(&body.ops[..index])
                && checked_result_observer(func, &labels, check).is_some()
            {
                bindings.push((body.id, index, call.results[0]));
            }
        }
    }
    let changed = !bindings.is_empty();
    for (block, index, word) in bindings {
        func.blocks.get_mut(&block).unwrap().ops[index].bind_checked_invocation(word);
    }
    changed
}

#[derive(Clone)]
pub(crate) struct CheckedResults {
    pub(super) sites: HashMap<ValueId, Site>,
    pub(super) unproven: HashSet<ValueId>,
    pub(super) projections: HashMap<ValueId, ValueId>,
}

impl CheckedResults {
    /// Results a handler reads directly, outside the check's explicit payload.
    /// Recursion refusal must define their word before the common observation.
    pub(crate) fn implicit_handler_call_results(&self) -> HashSet<ValueId> {
        self.sites
            .iter()
            .filter_map(|(&word, site)| {
                (site.compiled && site.implicit_handler_use).then_some(word)
            })
            .collect()
    }
    pub fn collect(func: &TirFunction, order: &[BlockId]) -> Self {
        let labels = OnceCell::new();
        let mut sites = HashMap::default();
        let mut unproven = HashSet::default();
        for &block in order {
            let ops = &func.blocks[&block].ops;
            let mut pending_call = None;
            for (check, op) in ops.iter().enumerate() {
                if op.opcode != OpCode::CheckException {
                    if let [word] = op.results[..]
                        && is_compiled_invocation(op)
                    {
                        pending_call = op.has_valid_shape().then_some((word, check));
                        if func.has_exception_handling && !op.has_nothrow_proof() {
                            unproven.insert(word);
                        }
                    } else if !preserves_invocation_observation(op) {
                        pending_call = None;
                    }
                    continue;
                }
                let pending = pending_call.take();
                if check == 0 {
                    continue;
                }
                let (call, word, compiled) = if op.has_checked_invocation_binding() {
                    let labels = labels.get_or_init(|| exception_label_to_block(func));
                    let Some(edge) = crate::tir::dominators::checked_invocation(
                        &func.blocks[&block],
                        check,
                        labels,
                    ) else {
                        continue;
                    };
                    let Some((physical, call)) = pending else {
                        continue;
                    };
                    if physical != edge.physical
                        || !func
                            .blocks
                            .get(&edge.exceptional)
                            .is_some_and(|handler| handler.args.len() == edge.payload.len())
                    {
                        continue;
                    }
                    (call, edge.physical, true)
                } else {
                    let (call, word, compiled) =
                        if let Some((call, producer)) = invocation_before_observer(&ops[..check]) {
                            (call, producer.results[0], true)
                        } else {
                            let producer = &ops[check - 1];
                            let [word] = producer.results[..] else {
                                continue;
                            };
                            if !producer.has_valid_shape()
                                || !direct_boxed_runtime(producer)
                                    .is_some_and(|entry| entry.returns_exact_int_on_success())
                            {
                                continue;
                            }
                            (check - 1, word, false)
                        };
                    if checked_result_observer(func, &labels, op).is_none() {
                        continue;
                    }
                    (call, word, compiled)
                };
                unproven.remove(&word);
                sites.insert(
                    word,
                    Site {
                        block,
                        call,
                        check,
                        compiled,
                        normal_use: false,
                        physical_use: false,
                        implicit_handler_use: false,
                        projected: false,
                    },
                );
            }
        }

        let mut projections = HashMap::default();
        if !sites.is_empty() {
            let points = OnceCell::new();
            for &block in order {
                let body = &func.blocks[&block];
                let mut observe = |word, index, projection| {
                    if let Some(site) = sites.get_mut(&word) {
                        if site.available(func, &points, block, index) {
                            site.normal_use = true;
                            if site.compiled
                                && let Some(success) = projection
                            {
                                projections.insert(success, word);
                                site.projected = true;
                            }
                        } else {
                            site.physical_use = true;
                            site.implicit_handler_use |= (block, index) != (site.block, site.check);
                        }
                    }
                };
                for (index, op) in body.ops.iter().enumerate() {
                    let projection = op.checked_call_result_source().map(|_| op.results[0]);
                    // The bound result identifies the observation. Only a
                    // handler argument is a physical use on the failure arm.
                    for &word in op.exception_payload() {
                        observe(word, index, projection);
                    }
                }
                body.terminator
                    .for_each_value(|word| observe(word, body.ops.len(), None));
            }
        }
        unproven.extend(
            sites
                .iter()
                .filter_map(|(&word, site)| (site.compiled && site.physical_use).then_some(word)),
        );
        Self {
            sites,
            unproven,
            projections,
        }
    }

    pub(crate) fn successful_call_results(&self, func: &TirFunction) -> HashSet<ValueId> {
        self.sites
            .iter()
            .filter_map(|(&word, site)| {
                let op = &func.blocks[&site.block].ops[site.call];
                (site.compiled
                    && !site.physical_use
                    && (static_call(op).is_some()
                        || (op.opcode == OpCode::DirectCall && op.callee.is_some())))
                .then_some(word)
            })
            .collect()
    }

    pub(crate) fn checked_call_values<T>(
        &self,
        func: &TirFunction,
        fact_of: impl Fn(&TirOp) -> Option<T>,
    ) -> Vec<(ValueId, T)> {
        let fact = |site: &Site| fact_of(&func.blocks[&site.block].ops[site.call]);
        self.sites
            .iter()
            .filter(|(_, site)| site.compiled && !site.physical_use)
            .filter_map(|(&word, site)| Some((word, fact(site)?)))
            .chain(
                self.projections
                    .iter()
                    .filter_map(|(&success, word)| Some((success, fact(&self.sites[word])?))),
            )
            .collect()
    }
}

/// Physical results whose own checked continuation dominates every use.
pub fn checked_successful_call_results(func: &TirFunction) -> HashSet<ValueId> {
    let order = executable_reverse_postorder(func);
    CheckedResults::collect(func, &order).successful_call_results(func)
}

/// Results a handler reads directly, outside the check's explicit payload.
/// Recursion refusal must define their word before the common observation.
pub(crate) fn implicit_handler_call_results(func: &TirFunction) -> HashSet<ValueId> {
    CheckedResults::collect(func, &executable_reverse_postorder(func))
        .implicit_handler_call_results()
}

/// Split only mixed-use compiled results. Single-domain uses already have
/// one valid SSA-wide type. A mixed-use call gets one borrowed identity on
/// its normal continuation; its observer and cleanup still see the ABI word.
pub(crate) fn project_checked_call_results(
    func: &mut TirFunction,
    eligible: impl Fn(&TirOp) -> bool,
) -> bool {
    let order = executable_reverse_postorder(func);
    let mut sites: Vec<_> = CheckedResults::collect(func, &order)
        .sites
        .into_iter()
        .filter(|(_, site)| {
            site.compiled
                && site.normal_use
                && site.physical_use
                && !site.projected
                && eligible(&func.blocks[&site.block].ops[site.call])
        })
        .collect();
    if sites.is_empty() {
        return false;
    }
    sites.sort_unstable_by_key(|(_, site)| (site.block, site.check));
    let mut replacements = HashMap::default();
    for &(word, site) in &sites {
        replacements.insert(word, (site, func.fresh_value()));
    }
    let points = ProgramPointDominance::compute_executable(func);
    for block in order {
        let body = func.blocks.get_mut(&block).unwrap();
        let rewrite = |word: &mut ValueId, index| {
            if let Some(&(site, success)) = replacements.get(word)
                && points.definition_available(site.block, Some(site.check), block, index)
            {
                *word = success;
            }
        };
        for (index, op) in body.ops.iter_mut().enumerate() {
            for word in &mut op.operands {
                rewrite(word, index);
            }
        }
        body.terminator
            .for_each_value_mut(|word| rewrite(word, body.ops.len()));
    }
    for (word, site) in sites.into_iter().rev() {
        let success = replacements[&word].1;
        let mut projection = TirOp::checked_call_result(word, success);
        let body = func.blocks.get_mut(&site.block).unwrap();
        projection.source_span = body.ops[site.call].source_span;
        func.value_types.insert(
            success,
            body.ops[site.call]
                .exact_result_proof()
                .unwrap_or(crate::tir::types::TirType::DynBox),
        );
        body.ops.insert(site.check + 1, projection);
    }
    true
}
