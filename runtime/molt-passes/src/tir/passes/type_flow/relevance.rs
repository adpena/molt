//! Relevance (design 78, section 3.4): a parameter splits a context only when
//! its type reaches an operation whose lowering depends on it.
//!
//! A parameter's value travels through forwarding copies and block arguments.
//! A use by any other operation, a branch condition, a switch value or a
//! return makes it relevant; a reference-count operation does not, and an
//! argument of a resolved call is relevant exactly when the callee's
//! parameter is. The module solves the callee dependence to a fixpoint from
//! "nothing relevant", so a parameter that is only passed along a recursive
//! cycle stays irrelevant.

use molt_ir::deterministic_hash::HashMap;

use crate::tir::function::TirModule;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::sites::Site;

/// Per body (module index), per parameter: whether its type matters.
pub(super) fn solve(
    module: &TirModule,
    sites: &[Vec<Site>],
    body_index: &[Option<usize>],
) -> Vec<Vec<bool>> {
    let origins: Vec<HashMap<ValueId, u64>> =
        module.functions.iter().map(parameter_origins).collect();
    let mut relevant: Vec<Vec<bool>> = module
        .functions
        .iter()
        .map(|func| vec![false; func.param_names.len()])
        .collect();
    loop {
        let mut changed = false;
        for (index, func) in module.functions.iter().enumerate() {
            let mask = direct_uses(func, &origins[index], &sites[index], |site, position| {
                body_index[site.callee.index()]
                    .is_some_and(|callee| relevant[callee].get(position).copied().unwrap_or(true))
            });
            for (param, flag) in relevant[index].iter_mut().enumerate() {
                // A parameter beyond the mask's width is conservatively relevant.
                let used = param >= 64 || mask & (1 << param) != 0;
                if used && !*flag {
                    *flag = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return relevant;
        }
    }
}

/// The parameters (as a bit mask) each value of `func` forwards unchanged.
fn parameter_origins(func: &crate::tir::function::TirFunction) -> HashMap<ValueId, u64> {
    let mut origins: HashMap<ValueId, u64> = HashMap::default();
    if let Some(entry) = func.blocks.get(&func.entry_block) {
        for (param, arg) in entry.args.iter().enumerate().take(64) {
            origins.insert(arg.id, 1 << param);
        }
    }
    loop {
        let mut changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                let Some(source) = crate::tir::passes::value_identity::scalar_value_source(op)
                else {
                    continue;
                };
                let Some(&mask) = origins.get(&source) else {
                    continue;
                };
                for &result in &op.results {
                    let slot = origins.entry(result).or_insert(0);
                    if *slot | mask != *slot {
                        *slot |= mask;
                        changed = true;
                    }
                }
            }
            block.terminator.for_each_edge(|target, args| {
                let Some(target_block) = func.blocks.get(&target) else {
                    return;
                };
                for (value, param) in args.iter().zip(&target_block.args) {
                    let Some(&mask) = origins.get(value) else {
                        continue;
                    };
                    let slot = origins.entry(param.id).or_insert(0);
                    if *slot | mask != *slot {
                        *slot |= mask;
                        changed = true;
                    }
                }
            });
        }
        if !changed {
            return origins;
        }
    }
}

/// The parameters of `func` some use makes relevant, given which callee
/// parameters are (`callee_relevant(site, position)`).
fn direct_uses(
    func: &crate::tir::function::TirFunction,
    origins: &HashMap<ValueId, u64>,
    sites: &[Site],
    callee_relevant: impl Fn(&Site, usize) -> bool,
) -> u64 {
    let site_at: HashMap<_, &Site> = sites.iter().map(|site| (site.position(), site)).collect();
    let mut mask = 0u64;
    let origin = |value: &ValueId| origins.get(value).copied().unwrap_or(0);
    for (&block_id, block) in &func.blocks {
        for (index, op) in block.ops.iter().enumerate() {
            if crate::tir::passes::value_identity::scalar_value_source(op).is_some()
                || matches!(op.opcode, OpCode::IncRef | OpCode::DecRef)
            {
                continue;
            }
            if let Some(site) = site_at.get(&(block_id, index)) {
                for (position, arg) in site.args.iter().enumerate() {
                    if callee_relevant(site, position) {
                        mask |= origin(arg);
                    }
                }
                continue;
            }
            for operand in &op.operands {
                mask |= origin(operand);
            }
        }
        block
            .terminator
            .for_each_direct_value(|value| mask |= origin(&value));
    }
    mask
}
