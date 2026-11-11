//! HF-773 D0: observe the existing solve, never promote a target or seed the
//! product solve. The separate debug artifact leaves pass-delta readers intact.

use std::collections::{BTreeSet, HashSet};

use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use serde::Serialize;

use super::{ConstVal, Lattice, Solver, Use, ValueFacts, decided_edge};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::call_targets::direct_call_symbol_for_op;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Index in the declared entry signature, INCLUDING a closure environment.
/// It is not a source-level positional index; the signature join owns that map.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct ParamIndex(usize);

/// Symbols are open frontend/linker identities. Text is confined to this
/// diagnostic boundary; it never grants the optimizer callable identity.
#[derive(Debug, Serialize)]
struct CandidateSymbol<'a>(&'a str);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum CallKind {
    StaticDirect,
    Guarded,
    Bind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum ArgumentFact {
    None,
    True,
    False,
    OtherConstant,
    NotNone,
    Unknown,
    Unreached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum AnnotationEffect {
    /// A read-only solve with this parameter's annotation nullness decides it.
    DecidesFromNullness,
    /// The nullness domain did not decide it; class/range facts are unmodelled.
    NotDecidedFromNullness,
    /// No usable annotation nullness (DynBox or a mixed/unknown union).
    Unavailable,
}

#[derive(Debug, Serialize)]
struct ParameterBranch {
    parameter: ParamIndex,
    block: BlockId,
    annotation: AnnotationEffect,
    /// Annotation metadata alone is not an entry admission proof. In
    /// particular, declared classes and mixed unions may have no tag guard.
    entry_tag_guard: bool,
}

#[derive(Debug, Serialize)]
struct CallSite<'a> {
    block: BlockId,
    op: usize,
    kind: CallKind,
    candidate: Option<CandidateSymbol<'a>>,
    /// None for CALL_BIND: its operand is a consumed builder, NOT an argument.
    /// The frontend signature/builder join must resolve binding separately.
    positional: Option<Vec<ArgumentFact>>,
}

#[derive(Debug, Serialize)]
struct CallCensus<'a> {
    schema_version: u32,
    function: CandidateSymbol<'a>,
    parameters: &'a [String],
    annotations: &'a [TirType],
    undecided_branches: usize,
    parameter_branches: Vec<ParameterBranch>,
    calls: Vec<CallSite<'a>>,
}

fn annotation_nullness(ty: &TirType) -> Option<Lattice> {
    match ty {
        TirType::None => Some(Lattice::Constant(ConstVal::None)),
        TirType::DynBox | TirType::Never => None,
        TirType::Box(inner) => annotation_nullness(inner),
        TirType::Union(members) => {
            let mut members = members.iter();
            let first = annotation_nullness(members.next()?)?;
            members.try_fold(first, |fact, ty| {
                let other = annotation_nullness(ty)?;
                (fact.nullness() == other.nullness()).then(|| fact.meet(other))
            })
        }
        _ => Some(Lattice::Refined(ValueFacts::NOT_NONE)),
    }
}

fn argument_fact(solver: &Solver<'_>, block: BlockId, value: ValueId) -> ArgumentFact {
    match solver.state(value) {
        Lattice::Constant(ConstVal::None) => ArgumentFact::None,
        Lattice::Constant(ConstVal::Bool(true)) => ArgumentFact::True,
        Lattice::Constant(ConstVal::Bool(false)) => ArgumentFact::False,
        Lattice::Constant(_) => ArgumentFact::OtherConstant,
        Lattice::Unknown => ArgumentFact::Unreached,
        _ if solver.none_tests.nullness_in(block, value) == Some(true) => ArgumentFact::None,
        state
            if state.nullness() == Some(false)
                || solver.none_tests.nullness_in(block, value) == Some(false) =>
        {
            ArgumentFact::NotNone
        }
        _ => ArgumentFact::Unknown,
    }
}

/// Conservative SSA dependence, using the SAME op/normal/exception-edge uses
/// as the solver. An opaque op may depend on any operand; this is a ceiling,
/// not a promise that substituting the parameter will decide the branch.
fn dependent_branches(solver: &Solver<'_>, parameter: ValueId) -> BTreeSet<BlockId> {
    let mut pending = vec![parameter];
    let mut seen = HashSet::new();
    let mut branches = BTreeSet::new();
    while let Some(value) = pending.pop() {
        if !seen.insert(value) {
            continue;
        }
        for usage in solver.uses.get(&value).into_iter().flatten() {
            match usage {
                Use::Op { block, op } => {
                    pending.extend(&solver.func.blocks[block].ops[op.0 as usize].results);
                }
                Use::EdgeArg { target, arg, .. } => {
                    if let Some(value) = solver.func.blocks[target].args.get(arg.0 as usize) {
                        pending.push(value.id);
                    }
                }
                Use::Predicate { block }
                    if solver.executable_blocks.contains(block)
                        && matches!(
                            solver.func.blocks[block].terminator,
                            Terminator::CondBranch { .. }
                        )
                        && decided_edge(&solver.func.blocks[block].terminator, &solver.lattice)
                            .is_none() =>
                {
                    branches.insert(*block);
                }
                Use::Predicate { .. } => {}
            }
        }
    }
    branches
}

fn call_kind(op: &TirOp) -> Option<CallKind> {
    if !matches!(op.opcode, OpCode::Call | OpCode::Copy) || !op.has_valid_shape() {
        return None;
    }
    match op.origin {
        Some(molt_ir::OpKind::CallGuarded) => Some(CallKind::Guarded),
        Some(molt_ir::OpKind::CallBind) => Some(CallKind::Bind),
        _ => direct_call_symbol_for_op(op).map(|_| CallKind::StaticDirect),
    }
}

fn capture<'a>(solver: &Solver<'a>) -> CallCensus<'a> {
    let func = solver.func;
    let mut parameter_branches = Vec::new();
    for (index, parameter) in func.blocks[&func.entry_block].args.iter().enumerate() {
        let branches = dependent_branches(solver, parameter.id);
        if branches.is_empty() {
            continue;
        }
        let entry_tag_guard = func.blocks[&func.entry_block].ops.iter().any(|op| {
            op.origin == Some(molt_ir::OpKind::GuardTag)
                && op
                    .operands
                    .first()
                    .is_some_and(|&value| solver.none_tests.copy_root(value) == parameter.id)
        });
        let annotated = func
            .param_types
            .get(index)
            .and_then(annotation_nullness)
            .map(|fact| {
                let mut hypothetical = Solver::new(func);
                hypothetical.solve_with_parameter_fact(Some((parameter.id, fact)));
                hypothetical
            });
        for block in branches {
            let annotation = match &annotated {
                None => AnnotationEffect::Unavailable,
                Some(hypothetical)
                    if hypothetical.executable_blocks.contains(&block)
                        && decided_edge(&func.blocks[&block].terminator, &hypothetical.lattice)
                            .is_some() =>
                {
                    AnnotationEffect::DecidesFromNullness
                }
                Some(_) => AnnotationEffect::NotDecidedFromNullness,
            };
            parameter_branches.push(ParameterBranch {
                parameter: ParamIndex(index),
                block,
                annotation,
                entry_tag_guard,
            });
        }
    }
    let mut calls = Vec::new();
    let mut blocks: Vec<_> = solver.executable_blocks.iter().copied().collect();
    blocks.sort_unstable();
    for block in blocks {
        for (index, op) in func.blocks[&block].ops.iter().enumerate() {
            let Some(kind) = call_kind(op) else { continue };
            let candidate = match kind {
                CallKind::StaticDirect => direct_call_symbol_for_op(op),
                CallKind::Guarded => match op.attrs.get("s_value") {
                    Some(AttrValue::Str(name)) if !name.is_empty() => Some(name.as_str()),
                    _ => None,
                },
                CallKind::Bind => None,
            }
            .map(CandidateSymbol);
            let positional = match kind {
                CallKind::Bind => None,
                CallKind::StaticDirect => Some(&op.operands[..]),
                CallKind::Guarded => op.operands.get(1..),
            }
            .map(|args| {
                args.iter()
                    .map(|&value| argument_fact(solver, block, value))
                    .collect()
            });
            calls.push(CallSite {
                block,
                op: index,
                kind,
                candidate,
                positional,
            });
        }
    }
    CallCensus {
        schema_version: 1,
        function: CandidateSymbol(&func.name),
        parameters: &func.param_names,
        annotations: &func.param_types,
        undecided_branches: solver
            .executable_blocks
            .iter()
            .filter(|block| {
                let term = &func.blocks[block].terminator;
                matches!(term, Terminator::CondBranch { .. })
                    && decided_edge(term, &solver.lattice).is_none()
            })
            .count(),
        parameter_branches,
        calls,
    }
}

pub(super) fn emit(solver: &Solver<'_>) {
    if !crate::tir::pass_delta::emit_enabled()
        && backend_env::var(BackendEnv::TIR_OPT_STATS).as_deref() != Ok("1")
    {
        return;
    }
    let result = serde_json::to_vec(&capture(solver))
        .map_err(std::io::Error::other)
        .and_then(|mut line| {
            line.push(b'\n');
            crate::debug_artifacts::append_debug_artifact("tir/call_census.jsonl", line)
        });
    if let Err(error) = result {
        eprintln!("[TIR] call census failed: {error}");
    }
}

#[cfg(test)]
mod tests;
