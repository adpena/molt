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

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    comparison_scalar_domain, opcode_exact_scalar_result_tir_type,
};
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::cfg_edges::{collect_branch_edges, handler_entry_blocks};
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
    /// The result type of the op at `(block, index)`; `Never` while the
    /// callee's return type is still unknown (optimistic, as SCCP is).
    pub call_results: Option<&'a HashMap<(BlockId, usize), TirType>>,
}

pub fn extract_exact_scalar_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut facts = solve_exact_scalars(
        func,
        ExactSeeds {
            params: &func.exact_param_types,
            call_results: None,
        },
    );
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
    let mut plan = ExactScalarPlan::new(func);
    let initial = std::mem::take(&mut plan.initial);
    plan.solve_from(initial, seeds)
}

/// Input-independent shape of a body, shared by every interprocedural context.
pub(crate) struct ExactScalarPlan<'a> {
    func: &'a TirFunction,
    order: Vec<BlockId>,
    handlers: HashSet<BlockId>,
    incoming: HashMap<BlockId, Vec<Vec<ValueId>>>,
    initial: HashMap<ValueId, TirType>,
    returns: Vec<ValueId>,
    returned_parameter: std::cell::OnceCell<Option<usize>>,
}

impl<'a> ExactScalarPlan<'a> {
    pub(crate) fn new(func: &'a TirFunction) -> Self {
        let order = crate::tir::dominators::executable_reverse_postorder(func);
        let handlers = handler_entry_blocks(func);
        let mut initial = HashMap::default();
        let mut incoming: HashMap<BlockId, Vec<Vec<ValueId>>> = HashMap::default();
        let mut returns = Vec::new();
        for id in &order {
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
                incoming.entry(target).or_default().push(args);
            }
            if let Terminator::Return { values } = &block.terminator {
                returns.extend(values);
            }
        }
        Self {
            func,
            order,
            handlers,
            incoming,
            initial,
            returns,
            returned_parameter: std::cell::OnceCell::new(),
        }
    }

    /// A parameter whose type is forwarded by every successful value return.
    /// A frame publication or owned alias can retain and run cleanup; only
    /// its result type is projected, never its ownership or effects.
    pub(crate) fn forwarded_return_parameter(&self) -> Option<usize> {
        *self
            .returned_parameter
            .get_or_init(|| self.find_return_parameter())
    }

    fn find_return_parameter(&self) -> Option<usize> {
        use crate::tir::passes::value_identity::forwarded_python_value_source;
        let entry = self.func.entry_block;
        if self.incoming.contains_key(&entry)
            || self.handlers.contains(&entry)
            || self.returns.is_empty()
        {
            return None;
        }
        let mut parameters: HashMap<ValueId, usize> = self.func.blocks[&entry]
            .args
            .iter()
            .take(self.func.param_names.len())
            .enumerate()
            .map(|(i, arg)| (arg.id, i))
            .collect();
        let mut has_empty_return = false;
        for id in &self.order {
            let block = &self.func.blocks[id];
            if let Terminator::Return { values } = &block.terminator {
                match values.len() {
                    0 => has_empty_return = true,
                    1 => {}
                    _ => return None,
                }
            }
            for op in &block.ops {
                if op.has_valid_shape()
                    && let Some(source) = forwarded_python_value_source(op)
                    && let Some(&parameter) = parameters.get(&source)
                {
                    parameters.insert(op.results[0], parameter);
                }
            }
        }
        let parameter = *parameters.get(self.returns.first()?)?;
        if self
            .returns
            .iter()
            .any(|value| parameters.get(value) != Some(&parameter))
        {
            return None;
        }
        if has_empty_return && self.may_succeed_without_value() {
            return None;
        }
        Some(parameter)
    }

    /// An empty return is harmless only on unwind. A handler that just
    /// pops the frame and returns propagates the pending exception. Any
    /// other handler may consume it and resume ordinary execution.
    fn may_succeed_without_value(&self) -> bool {
        let labels = crate::tir::dominators::exception_label_to_block(self.func);
        let mut pending = vec![self.func.entry_block];
        for handler in &self.handlers {
            let block = &self.func.blocks[handler];
            if !matches!(&block.terminator, Terminator::Return { values } if values.is_empty())
                || !block.ops.iter().all(|op| {
                    op.opcode == OpCode::Copy && op.origin == Some(molt_ir::OpKind::TraceExit)
                })
            {
                pending.push(*handler);
            }
        }
        let mut seen = HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let block = &self.func.blocks[&id];
            // Raise sets pending state. Only an executable targeted check
            // transfers control: an untargeted observer permits a successor
            // to clear the exception and return normally.
            if block.ops.windows(2).any(|ops| {
                ops[0].opcode == OpCode::Raise
                    && ops[1].opcode == OpCode::CheckException
                    && crate::tir::dominators::exception_transfer_target(&ops[1], &labels).is_some()
            }) {
                continue;
            }
            if matches!(&block.terminator, Terminator::Return { values } if values.is_empty()) {
                if block
                    .ops
                    .last()
                    .is_some_and(|op| op.opcode == OpCode::Raise)
                {
                    continue;
                }
                return true;
            }
            block.terminator.for_each_edge(|id, _| pending.push(id));
        }
        false
    }

    pub(crate) fn return_values(&self) -> &[ValueId] {
        &self.returns
    }

    pub(crate) fn solve(&self, seeds: ExactSeeds<'_>) -> HashMap<ValueId, TirType> {
        self.solve_from(self.initial.clone(), seeds)
    }

    fn solve_from(
        &self,
        mut facts: HashMap<ValueId, TirType>,
        seeds: ExactSeeds<'_>,
    ) -> HashMap<ValueId, TirType> {
        let func = self.func;
        // The finite scalar lattice rises from bottom through exact types to top.
        // Cyclic SSA paths retain bottom until an executable incoming fact seeds
        // them; no annotation is used to bootstrap a loop or function parameter.
        // Reverse postorder follows executable predecessors on acyclic
        // paths regardless of block numbering. Backedges still converge
        // through the same joins.
        loop {
            let mut changed = false;
            for &block_id in &self.order {
                let block = &func.blocks[&block_id];
                for (index, arg) in block.args.iter().enumerate() {
                    // A parameter holds its seed on function entry; a branch back
                    // to the entry block joins its own arguments to that seed.
                    let entry_seed = (block_id == func.entry_block)
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
                        changed = true;
                    }
                }
                for (op_index, op) in block.ops.iter().enumerate() {
                    if op.results.is_empty() {
                        continue;
                    }
                    let seeded = seeds
                        .call_results
                        .and_then(|results| results.get(&(block_id, op_index)));
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
                    let scalar_source =
                        crate::tir::passes::value_identity::forwarded_python_value_source(op);
                    let transfers_exact =
                        op.opcode != OpCode::Copy && effects.consistent && effects.effect_free;
                    let valid_shape = op.has_valid_shape();
                    let types = if !valid_shape {
                        vec![TirType::DynBox; op.results.len()]
                    } else if let Some(seeded) = seeded {
                        // The callee context's return type (design 78, 3.3).
                        vec![seeded.clone(); op.results.len()]
                    } else if let Some(proven) = op.exact_result_proof() {
                        // The late stage replaced an op whose result was exact.
                        vec![proven; op.results.len()]
                    } else if let Some(source) = scalar_source {
                        vec![facts.get(&source).cloned().unwrap_or(TirType::DynBox)]
                    } else if let Some(class) = super::result_inference::exact_allocation_class(op)
                    {
                        // An allocation's result is exactly its class statement's
                        // instance (I6, C1).
                        vec![class; op.results.len()]
                    } else if let Some(result_type) = instance.and_then(|facts| facts.result_type) {
                        vec![result_type; op.results.len()]
                    } else if let Some(ty) =
                        crate::tir::call_targets::gpu_runtime_result_type_for_op(op)
                    {
                        vec![ty; op.results.len()]
                    } else if transfers_exact {
                        infer_result_facts_with_attrs(
                            op.opcode,
                            op.origin,
                            &operand_types,
                            None,
                            op.results.len(),
                        )
                    } else {
                        vec![TirType::DynBox; op.results.len()]
                    };
                    for (index, (&result, inferred)) in op.results.iter().zip(types).enumerate() {
                        // Output shape is independent of purity: a mutable runtime
                        // read or checked arithmetic may still produce an exact
                        // Boolean status. Never broadcast slot zero into siblings.
                        let ty = if valid_shape {
                            let origin = if op.operands.len() == 1 && op.results.len() == 1 {
                                op.origin
                            } else {
                                None
                            };
                            opcode_exact_scalar_result_tir_type(op.opcode, origin, index)
                                .unwrap_or(inferred)
                        } else {
                            inferred
                        };
                        let next = exact_or_bottom(facts[&result].meet(&ty));
                        if facts[&result] != next {
                            facts.insert(result, next);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        facts
    }
}
