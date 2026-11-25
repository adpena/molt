//! A call returns a physical word before its exception observer can transfer
//! control. Only uses dominated by the observer's continuation see success.

use std::cell::OnceCell;

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::call_targets::{direct_boxed_runtime, guarded_dispatch, static_call};
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
            .is_some_and(|block| block.args.len() == op.operands.len())
    })
}

pub(super) struct CheckedResults {
    pub sites: HashMap<ValueId, Site>,
    pub unproven: HashSet<ValueId>,
    pub projections: HashMap<ValueId, ValueId>,
}

impl CheckedResults {
    pub fn collect(func: &TirFunction, order: &[BlockId]) -> Self {
        let labels = OnceCell::new();
        let mut sites = HashMap::default();
        let mut unproven = HashSet::default();
        for &block in order {
            let ops = &func.blocks[&block].ops;
            for (call, op) in ops.iter().enumerate() {
                let [word] = op.results[..] else { continue };
                let compiled = matches!(op.opcode, OpCode::CallLoadedMethod | OpCode::DirectCall)
                    || static_call(op).is_some()
                    || guarded_dispatch(op).is_some();
                if !compiled
                    && !direct_boxed_runtime(op)
                        .is_some_and(|entry| entry.returns_exact_int_on_success())
                {
                    continue;
                }
                let mut check = call + 1;
                if compiled
                    && ops.get(check).is_some_and(|op| {
                        op.opcode == OpCode::RecursionRelease && op.has_valid_shape()
                    })
                {
                    check += 1;
                }
                if op.has_valid_shape()
                    && ops
                        .get(check)
                        .and_then(|op| checked_result_observer(func, &labels, op))
                        .is_some()
                {
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
                } else if compiled && func.has_exception_handling && !op.has_nothrow_proof() {
                    unproven.insert(word);
                }
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
                    for &word in &op.operands {
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
}

/// Physical results whose own checked continuation dominates every use.
pub fn checked_successful_call_results(func: &TirFunction) -> HashSet<ValueId> {
    let order = executable_reverse_postorder(func);
    CheckedResults::collect(func, &order)
        .sites
        .into_iter()
        .filter_map(|(word, site)| {
            let op = &func.blocks[&site.block].ops[site.call];
            (site.compiled
                && !site.physical_use
                && op.runtime.is_none()
                && (static_call(op).is_some()
                    || (op.opcode == OpCode::DirectCall && op.callee.is_some())))
            .then_some(word)
        })
        .collect()
}

/// Results a handler reads directly, outside the check's explicit payload.
/// Recursion refusal must define their word before the common observation.
pub(crate) fn implicit_handler_call_results(func: &TirFunction) -> HashSet<ValueId> {
    let order = executable_reverse_postorder(func);
    CheckedResults::collect(func, &order)
        .sites
        .into_iter()
        .filter_map(|(word, site)| (site.compiled && site.implicit_handler_use).then_some(word))
        .collect()
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
