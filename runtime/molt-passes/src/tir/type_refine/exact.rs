//! Exact provenance, distinct from annotation/guard type refinement.
//!
//! A Python annotation admits overriding subclasses. Intrinsic producers and
//! computations on exact operands do not. This monotone SSA transfer uses the
//! existing generated result/effect rules without importing return hints.
//!
//! The lattice holds the exact scalars and `Class(id)`: an instance whose
//! class is exactly one class statement, produced only by that statement's
//! allocation (I6, design 78 section 5.1). Scalar consumers read
//! [`extract_exact_scalar_map`], which keeps the scalars; the type flow keys
//! its contexts on both (design 78 section 3.2).

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeSet;

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::executable_reverse_postorder;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    comparison_scalar_domain, opcode_exact_scalar_result_tir_type,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::value_edges::ValueEdges;
use crate::tir::values::ValueId;

use super::cfg_edges::{collect_branch_edges, handler_entry_blocks};
use super::checked_results::CheckedResults;
use super::result_inference::infer_result_facts_with_attrs;

fn exact_or_bottom(ty: TirType) -> TirType {
    if ty == TirType::Never
        || comparison_scalar_domain(&ty).is_some()
        || matches!(ty, TirType::Class(_))
    {
        ty
    } else {
        TirType::DynBox
    }
}

/// Permission to interpret a producer's scalar payload. Only the exact SSA
/// solver can construct this witness; declarations and guards cannot mint it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExactScalarWitness {
    kind: crate::repr::ScalarKind,
}

impl ExactScalarWitness {
    pub fn kind(self) -> crate::repr::ScalarKind {
        self.kind
    }

    pub fn ty(self) -> TirType {
        use crate::repr::ScalarKind;
        match self.kind {
            ScalarKind::Int => TirType::I64,
            ScalarKind::Bool => TirType::Bool,
            ScalarKind::Float => TirType::F64,
            ScalarKind::Str => TirType::Str,
            ScalarKind::NoneValue => TirType::None,
        }
    }
}

/// Project the exact solver's facts into non-forgeable lowering permissions.
pub fn exact_scalar_witnesses(func: &TirFunction) -> HashMap<ValueId, ExactScalarWitness> {
    use crate::repr::ScalarKind;
    extract_exact_scalar_map(func)
        .into_iter()
        .filter_map(|(id, ty)| {
            let kind = match ty.semantic_type() {
                TirType::I64 | TirType::BigInt => ScalarKind::Int,
                TirType::Bool => ScalarKind::Bool,
                TirType::F64 => ScalarKind::Float,
                TirType::Str => ScalarKind::Str,
                TirType::None => ScalarKind::NoneValue,
                _ => return None,
            };
            Some((id, ExactScalarWitness { kind }))
        })
        .collect()
}

/// Facts one solve starts from beyond the body's own producers: the exact
/// type of each parameter on entry and of the results of calls whose callee
/// is resolved (design 78, section 3.3). A plain solve reads the body's own
/// seeds, its `exact_param_types` and each op's `exact_result_proof`; the
/// type flow passes the seeds of the context it evaluates.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExactSeeds<'a> {
    /// One type per parameter; a missing entry is `DynBox`.
    pub params: &'a [TirType],
    pub call_results: Option<ExactCallResults<'a>>,
}

/// A body's result-to-site index is shared by all its contexts. Only the
/// types change as the callees' return facts converge.
#[derive(Clone, Copy, Debug)]
pub struct ExactCallResults<'a> {
    pub sites: &'a HashMap<ValueId, usize>,
    /// `Never` while the callee's return type is still unknown.
    pub types: &'a [TirType],
}

pub fn extract_exact_scalar_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    exact_scalar_map_from_plan(ExactScalarPlan::new(func))
}

pub(crate) fn exact_scalar_map_from_plan(plan: ExactScalarPlan<'_>) -> HashMap<ValueId, TirType> {
    let seeds = ExactSeeds {
        params: &plan.func.exact_param_types,
        call_results: None,
    };
    let mut facts = plan.solve_once(seeds);
    facts.retain(|_, ty| comparison_scalar_domain(ty).is_some());
    facts
}

/// Every value of `func` the exact solver proves an instance of exactly one
/// class statement (`TirType::Class`), from its own seeds and allocations.
pub fn extract_exact_class_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut facts = solve_exact_scalars(
        func,
        ExactSeeds {
            params: &func.exact_param_types,
            call_results: None,
        },
    );
    facts.retain(|_, ty| matches!(ty, TirType::Class(_)));
    facts
}

/// The exact scalar facts of `func` from `seeds` and its producers: every
/// reachable value, with `Never` for one no executable path has reached and
/// `DynBox` for one that is not an exact scalar.
pub fn solve_exact_scalars(func: &TirFunction, seeds: ExactSeeds<'_>) -> HashMap<ValueId, TirType> {
    ExactScalarPlan::new(func).solve_once(seeds)
}

/// Input-independent shape of a body, shared by every interprocedural context.
pub(crate) struct ExactScalarPlan<'a> {
    func: &'a TirFunction,
    order: Cow<'a, [BlockId]>,
    handlers: HashSet<BlockId>,
    incoming: HashMap<BlockId, Vec<Vec<ValueId>>>,
    initial: HashMap<ValueId, TirType>,
    has_feedback: bool,
    users: OnceCell<ValueEdges<ValueId, usize>>,
    successful_integers: HashSet<ValueId>,
    checked: Cow<'a, CheckedResults>,
    projected_calls: HashMap<ValueId, &'a crate::tir::ops::TirOp>,
    returns: Vec<ValueId>,
    has_non_value_return: bool,
}

/// A sparse backwards walk of the values returned by one body. It can pause
/// at a checked invocation while the module derives that callee's origin.
pub(crate) struct ReturnParameterSearch {
    origins: HashMap<ValueId, ReturnOrigin>,
    pending: Vec<ValueId>,
    visited: HashSet<ValueId>,
    parameter: Option<usize>,
}

enum ReturnOrigin {
    Parameter(usize),
    Copy(ValueId),
    Join(BlockId, usize),
}

impl ReturnParameterSearch {
    fn new(plan: &ExactScalarPlan<'_>) -> Self {
        use crate::tir::passes::value_identity::forwarded_python_value_source;
        let entry = &plan.func.blocks[&plan.func.entry_block];
        let mut origins: HashMap<_, _> = entry
            .args
            .iter()
            .take(plan.func.param_types.len())
            .enumerate()
            .map(|(position, arg)| (arg.id, ReturnOrigin::Parameter(position)))
            .collect();
        for &id in plan.order.iter() {
            let block = &plan.func.blocks[&id];
            if plan.normal_incoming_edges(id).is_some() {
                for (position, arg) in block.args.iter().enumerate() {
                    origins.insert(arg.id, ReturnOrigin::Join(id, position));
                }
            }
            for op in &block.ops {
                // A checked projection must pass the continuation proof.
                // Copy forwarding would instead ask about the mixed-use ABI word.
                if op.has_valid_shape()
                    && op.checked_call_result_source().is_none()
                    && let Some(source) = forwarded_python_value_source(op)
                {
                    origins.insert(op.results[0], ReturnOrigin::Copy(source));
                }
            }
        }
        Self {
            origins,
            pending: plan.returns.clone(),
            visited: HashSet::default(),
            parameter: None,
        }
    }

    /// All returned values must trace to one formal. Copies may retain or
    /// release ownership and a call may still fail. This proves the successful
    /// result's Python value only.
    pub(crate) fn advance<E>(
        &mut self,
        plan: &ExactScalarPlan<'_>,
        mut call_actual: impl FnMut(&TirOp) -> Result<Option<ValueId>, E>,
    ) -> Result<Option<usize>, E> {
        while let Some(value) = self.pending.pop() {
            if !self.visited.insert(value) {
                continue;
            }
            match self.origins.get(&value) {
                Some(&ReturnOrigin::Parameter(origin)) => {
                    if self.parameter.is_some_and(|current| current != origin) {
                        return Ok(None);
                    }
                    self.parameter = Some(origin);
                }
                Some(&ReturnOrigin::Copy(source)) => self.pending.push(source),
                Some(&ReturnOrigin::Join(block, position)) => {
                    for edge in &plan.incoming[&block] {
                        let Some(&actual) = edge.get(position) else {
                            return Ok(None);
                        };
                        self.pending.push(actual);
                    }
                }
                None => {
                    let Some(call) = plan.checked.checked_compiled_call(plan.func, value) else {
                        return Ok(None);
                    };
                    match call_actual(call) {
                        Ok(Some(actual)) => self.pending.push(actual),
                        Ok(None) => return Ok(None),
                        Err(dependency) => {
                            // Resume at exactly this projection after the
                            // callee becomes known. A CFG backedge is visited
                            // once and cannot invent a formal seed.
                            self.visited.remove(&value);
                            self.pending.push(value);
                            return Err(dependency);
                        }
                    }
                }
            }
        }
        Ok(self.parameter)
    }
}

impl<'a> ExactScalarPlan<'a> {
    pub(crate) fn new(func: &'a TirFunction) -> Self {
        let order = executable_reverse_postorder(func);
        let checked = CheckedResults::collect(func, &order);
        Self::from_checked(func, Cow::Owned(order), Cow::Owned(checked))
    }

    pub(crate) fn from_checked(
        func: &'a TirFunction,
        order: Cow<'a, [BlockId]>,
        checked: Cow<'a, CheckedResults>,
    ) -> Self {
        let handlers = handler_entry_blocks(func);
        let mut initial = HashMap::default();
        let mut incoming: HashMap<BlockId, Vec<Vec<ValueId>>> = HashMap::default();
        let mut returns = Vec::new();
        let mut has_non_value_return = false;
        let projected_calls = checked
            .projections
            .iter()
            .map(|(&projection, word)| {
                let site = checked.sites[word];
                (projection, &func.blocks[&site.block].ops[site.call])
            })
            .collect();
        let mut seen = HashSet::default();
        let mut has_feedback = false;
        for id in order.iter() {
            seen.insert(*id);
            let block = &func.blocks[id];
            for arg in &block.args {
                initial.insert(arg.id, TirType::Never);
            }
            for op in &block.ops {
                for &result in &op.results {
                    initial.insert(result, TirType::Never);
                }
            }
            for (target, args) in collect_branch_edges(block) {
                // Ordinary SSA operands are dominated by their definitions.
                // Only a payload on a backward edge can make an earlier
                // block's input depend on a later definition.
                has_feedback |= !args.is_empty() && seen.contains(&target);
                incoming.entry(target).or_default().push(args);
            }
            if let Terminator::Return { values } = &block.terminator {
                if let [value] = values.as_slice() {
                    returns.push(*value);
                } else {
                    has_non_value_return = true;
                }
            }
        }
        let successful_integers = checked
            .sites
            .iter()
            .filter_map(|(&word, site)| (!site.compiled && !site.physical_use).then_some(word))
            .collect();
        Self {
            func,
            successful_integers,
            checked,
            projected_calls,
            order,
            handlers,
            incoming,
            initial,
            has_feedback,
            users: OnceCell::new(),
            returns,
            has_non_value_return,
        }
    }

    #[cfg(test)]
    pub(crate) fn forwarded_return_parameter(&self) -> Option<usize> {
        self.return_parameter_search()?
            .advance(self, |_| Ok::<_, ()>(None))
            .ok()
            .flatten()
    }

    /// Only executable, value-returning paths can establish a forwarded formal.
    pub(crate) fn return_parameter_search(&self) -> Option<ReturnParameterSearch> {
        let entry = self.func.entry_block;
        if self.incoming.contains_key(&entry)
            || self.handlers.contains(&entry)
            || self.returns.is_empty()
            || self.has_non_value_return
        {
            return None;
        }
        Some(ReturnParameterSearch::new(self))
    }

    pub(crate) fn return_values(&self) -> &[ValueId] {
        &self.returns
    }

    pub(crate) fn has_non_value_return(&self) -> bool {
        self.has_non_value_return
    }

    pub(crate) fn block_order(&self) -> &[BlockId] {
        &self.order
    }

    pub(crate) fn normal_incoming_edges(&self, block: BlockId) -> Option<&[Vec<ValueId>]> {
        if block == self.func.entry_block || self.handlers.contains(&block) {
            return None;
        }
        self.incoming.get(&block).map(Vec::as_slice)
    }

    pub(crate) fn checked_call_values<T>(
        &self,
        fact_of: impl Fn(&TirOp) -> Option<T>,
    ) -> Vec<(ValueId, T)> {
        self.checked.checked_call_values(self.func, fact_of)
    }

    pub(crate) fn solve(&self, seeds: ExactSeeds<'_>) -> HashMap<ValueId, TirType> {
        self.solve_from(self.initial.clone(), seeds)
    }

    fn solve_once(mut self, seeds: ExactSeeds<'_>) -> HashMap<ValueId, TirType> {
        let initial = std::mem::take(&mut self.initial);
        self.solve_from(initial, seeds)
    }

    fn solve_from(
        &self,
        mut facts: HashMap<ValueId, TirType>,
        seeds: ExactSeeds<'_>,
    ) -> HashMap<ValueId, TirType> {
        if !self.has_feedback {
            for id in self.order.iter() {
                self.refine_block(&self.func.blocks[id], &mut facts, seeds, |_| {});
            }
            return facts;
        }

        // All contexts of this body share its input-dependent edges. Same-block
        // SSA uses have already seen the new value, except block arguments fed
        // by a self edge, which are recorded separately.
        let users = self.users.get_or_init(|| self.cross_block_users());
        let mut dirty = BTreeSet::new();
        let mut first_visit = 0;
        loop {
            let rank = if first_visit < self.order.len() {
                let rank = first_visit;
                first_visit += 1;
                rank
            } else if let Some(rank) = dirty.pop_first() {
                rank
            } else {
                break;
            };
            self.refine_block(
                &self.func.blocks[&self.order[rank]],
                &mut facts,
                seeds,
                |value| {
                    dirty.extend(
                        users
                            .from(&value)
                            .copied()
                            .filter(|&rank| rank < first_visit),
                    )
                },
            );
        }
        facts
    }

    fn cross_block_users(&self) -> ValueEdges<ValueId, usize> {
        let mut owners = HashMap::default();
        for (rank, id) in self.order.iter().enumerate() {
            let block = &self.func.blocks[id];
            for arg in &block.args {
                owners.insert(arg.id, rank);
            }
            for op in &block.ops {
                for &result in &op.results {
                    owners.insert(result, rank);
                }
            }
        }
        let mut users = ValueEdges::default();
        for (rank, id) in self.order.iter().enumerate() {
            let block = &self.func.blocks[id];
            if !self.handlers.contains(id)
                && let Some(edges) = self.incoming.get(id)
            {
                for edge in edges {
                    for &value in edge.iter().take(block.args.len()) {
                        users.add(value, rank);
                    }
                }
            }
            for op in &block.ops {
                if op.results.is_empty() {
                    continue;
                }
                for &value in &op.operands {
                    if owners.get(&value).is_some_and(|&owner| owner != rank) {
                        users.add(value, rank);
                    }
                }
            }
        }
        users
    }

    fn refine_block(
        &self,
        block: &TirBlock,
        facts: &mut HashMap<ValueId, TirType>,
        seeds: ExactSeeds<'_>,
        mut changed: impl FnMut(ValueId),
    ) {
        let block_id = block.id;
        for (index, arg) in block.args.iter().enumerate() {
            // A parameter holds its seed on function entry; a branch back
            // to the entry block joins its own arguments to that seed.
            let entry_seed = (block_id == self.func.entry_block)
                .then(|| seeds.params.get(index).cloned().unwrap_or(TirType::DynBox));
            let ty = if self.handlers.contains(&block_id) {
                TirType::DynBox
            } else if let Some(edges) = self.incoming.get(&block_id) {
                edges
                    .iter()
                    .fold(entry_seed.unwrap_or(TirType::Never), |ty, edge| {
                        let incoming = edge
                            .get(index)
                            .and_then(|value| facts.get(value))
                            .unwrap_or(&TirType::DynBox);
                        ty.meet(incoming)
                    })
            } else {
                entry_seed.unwrap_or(TirType::DynBox)
            };
            let next = exact_or_bottom(facts[&arg.id].meet(&ty));
            if facts[&arg.id] != next {
                facts.insert(arg.id, next);
                changed(arg.id);
            }
        }
        for op in &block.ops {
            let Some(&first_result) = op.results.first() else {
                continue;
            };
            let valid_shape = op.has_valid_shape();
            let origin = if op.operands.len() == 1 && op.results.len() == 1 {
                op.origin
            } else {
                None
            };
            // A fixed result shape needs neither operand facts nor a
            // temporary vector to describe its one result.
            if valid_shape
                && op.results.len() == 1
                && let Some(ty) = opcode_exact_scalar_result_tir_type(op.opcode, origin, 0)
            {
                let previous = facts.get_mut(&first_result).expect("planned result");
                let next = exact_or_bottom(previous.meet(&ty));
                if *previous != next {
                    *previous = next;
                    changed(first_result);
                }
                continue;
            }
            let call = self
                .projected_calls
                .get(&first_result)
                .copied()
                .unwrap_or(op);
            let seeded = if matches!(call.opcode, OpCode::Call | OpCode::CallLoadedMethod) {
                seeds.call_results.and_then(|results| {
                    results
                        .sites
                        .get(&call.results[0])
                        .map(|&index| &results.types[index])
                })
            } else {
                None
            };
            let types = if !valid_shape || self.checked.unproven.contains(&first_result) {
                vec![TirType::DynBox; op.results.len()]
            } else if let Some(seeded) = seeded {
                // The callee context's return type (design 78, 3.3).
                vec![seeded.clone(); op.results.len()]
            } else if let Some(proven) = call.exact_result_proof()
                && call.checked_call_result_source().is_none()
            {
                // The late stage replaced an op whose result was exact.
                vec![proven; op.results.len()]
            } else if self.successful_integers.contains(&first_result) {
                // Semantic Python int, including a heap-backed BigInt.
                vec![TirType::I64]
            } else if let Some(source) =
                crate::tir::passes::value_identity::forwarded_python_value_source(op)
            {
                vec![facts.get(&source).cloned().unwrap_or(TirType::DynBox)]
            } else if let Some(class) = super::result_inference::exact_allocation_class(op) {
                // An allocation's result is exactly its class statement's
                // instance (I6, C1).
                vec![class; op.results.len()]
            } else {
                let operand_types: Vec<_> = op
                    .operands
                    .iter()
                    .map(|value| facts.get(value).cloned().unwrap_or(TirType::DynBox))
                    .collect();
                let instance =
                    crate::tir::op_semantics::op_instance_facts(op.opcode, &operand_types);
                let effects = instance.as_ref().map_or_else(
                    || crate::tir::op_kinds_generated::opcode_effects_table(op.opcode),
                    |facts| facts.effects,
                );
                if let Some(ty) = instance.and_then(|facts| facts.result_type) {
                    vec![ty; op.results.len()]
                } else if let Some(ty) =
                    crate::tir::call_targets::gpu_runtime_result_type_for_op(op)
                {
                    vec![ty; op.results.len()]
                } else if op.opcode != OpCode::Copy && effects.consistent && effects.effect_free {
                    infer_result_facts_with_attrs(
                        op.opcode,
                        op.origin,
                        &operand_types,
                        None,
                        op.results.len(),
                    )
                } else {
                    vec![TirType::DynBox; op.results.len()]
                }
            };
            for (index, (&result, inferred)) in op.results.iter().zip(types).enumerate() {
                // Output shape is independent of purity: a mutable runtime
                // read or checked arithmetic may still produce an exact
                // Boolean status. Never broadcast slot zero into siblings.
                let ty = if valid_shape {
                    opcode_exact_scalar_result_tir_type(op.opcode, origin, index)
                        .unwrap_or(inferred)
                } else {
                    inferred
                };
                let next = exact_or_bottom(facts[&result].meet(&ty));
                if facts[&result] != next {
                    facts.insert(result, next);
                    changed(result);
                }
            }
        }
    }
}
