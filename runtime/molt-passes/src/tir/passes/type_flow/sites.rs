//! The call sites the type flow resolves, and the callees it may specialize.

use crate::tir::blocks::BlockId;
use crate::tir::call_targets::static_call;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::FunctionId;
use molt_ir::deterministic_hash::HashMap;

use super::super::exact_receiver_fields::{
    ExactMembers, MethodCandidates, method_call, method_load,
};

/// How a resolved call enters its callee.
#[derive(Clone, Copy)]
pub(super) enum SiteKind<'a> {
    /// A `call_guarded` whose callee integration proved (HF-846 slice 2): the
    /// callee word is the target's function object and the call is exactly
    /// positional. Operand 0 is the callee word.
    ProvenGuarded(FunctionId),
    /// A `call_internal`: a direct call of a compiled function.
    Internal(FunctionId),
    /// This load/call pair selects a target only in a context that proves
    /// the original receiver's exact class. Argument zero is that receiver
    /// for analysis and remains the captured bound self in emitted code.
    Method(&'a MethodCandidates),
}

/// One call the program can bind directly in at least one context.
pub(super) struct Site<'a> {
    pub(super) block: BlockId,
    pub(super) index: usize,
    pub(super) kind: SiteKind<'a>,
    args: Vec<ValueId>,
    pub(super) result: ValueId,
    pub(super) result_demanded: bool,
}

pub(super) enum Resolution {
    Pending,
    Opaque,
    Body(FunctionId),
}

impl Site<'_> {
    pub(super) fn position(&self) -> (BlockId, usize) {
        (self.block, self.index)
    }

    pub(super) fn parameter_arguments(
        &self,
        first_parameter: usize,
    ) -> impl Iterator<Item = (usize, ValueId)> + '_ {
        (first_parameter..).zip(self.args.iter().copied())
    }

    pub(super) fn argument_for(&self, parameter: usize, first_parameter: usize) -> Option<ValueId> {
        self.args
            .get(parameter.checked_sub(first_parameter)?)
            .copied()
    }

    pub(super) fn fixed_callee(&self) -> Option<FunctionId> {
        match self.kind {
            SiteKind::ProvenGuarded(id) | SiteKind::Internal(id) => Some(id),
            SiteKind::Method(_) => None,
        }
    }

    pub(super) fn resolve(&self, facts: &HashMap<ValueId, TirType>) -> Resolution {
        let SiteKind::Method(candidates) = self.kind else {
            return Resolution::Body(self.fixed_callee().expect("static call has a callee"));
        };
        match facts.get(&self.args[0]) {
            Some(TirType::Never) => Resolution::Pending,
            Some(TirType::Class(class)) => candidates
                .resolve(*class, self.args.len())
                .map_or(Resolution::Opaque, Resolution::Body),
            _ => Resolution::Opaque,
        }
    }

    pub(super) fn for_each_candidate(&self, mut f: impl FnMut(FunctionId)) {
        match self.kind {
            SiteKind::ProvenGuarded(id) | SiteKind::Internal(id) => f(id),
            SiteKind::Method(candidates) => candidates.callees(self.args.len()).for_each(f),
        }
    }
}

/// Why a callee keeps one generic body (design 78, section 3.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unspecializable {
    /// Its canonical body links from the shared stdlib partition.
    LinkedElsewhere,
    /// A generator or coroutine body: its frame outlives the call.
    StateMachine,
    /// It runs in its caller's execution context (a module chunk).
    ExecutionContext,
    /// A physical partition of a larger function.
    CodegenPartition,
    /// The target's backend checks a guarded call's code against its
    /// target, so another entry of the same function would always miss.
    TargetChecksGuardedCode,
}

impl Unspecializable {
    pub(super) fn describe(self) -> &'static str {
        match self {
            Self::LinkedElsewhere => "the callee links from the shared stdlib partition",
            Self::StateMachine => "the callee is a generator or coroutine body",
            Self::ExecutionContext => "the callee runs in its caller's frame (a module chunk)",
            Self::CodegenPartition => "the callee is a codegen partition",
            Self::TargetChecksGuardedCode => {
                "this target's backend compares a guarded call's code with its target"
            }
        }
    }
}

/// Whether the body at `index` may have specialized clones, or why not.
pub(super) fn specializable(
    module: &TirModule,
    index: usize,
    non_inlinable: &molt_ir::deterministic_hash::HashSet<String>,
    final_tir: bool,
) -> Result<(), Unspecializable> {
    let func = &module.functions[index];
    if non_inlinable.contains(&func.name) {
        return Err(Unspecializable::LinkedElsewhere);
    }
    if func.has_state_machine() {
        return Err(Unspecializable::StateMachine);
    }
    // A function that enters its own frame (`Local`) clones with its code slot:
    // the clone pushes a record of the same code. A module chunk runs in its
    // caller's frame.
    if func.execution_context == molt_ir::ExecutionContextPolicy::Inherited {
        return Err(Unspecializable::ExecutionContext);
    }
    if func.is_codegen_partition() {
        return Err(Unspecializable::CodegenPartition);
    }
    if !final_tir {
        return Err(Unspecializable::TargetChecksGuardedCode);
    }
    Ok(())
}

/// Whether the body's return value is a value the flow can type: a plain
/// function that returns one object.
pub(super) fn returns_value(func: &TirFunction) -> bool {
    func.return_abi == molt_ir::FunctionReturnAbi::Value
        && !func.has_state_machine()
        && func.execution_context != molt_ir::ExecutionContextPolicy::Inherited
}

/// The resolved call `op` makes, if any, against the module's bodies.
fn static_site(
    op: &TirOp,
    block: BlockId,
    index: usize,
    body_index: &[Option<usize>],
    module: &TirModule,
) -> Option<Site<'static>> {
    if op.opcode != OpCode::Call || op.results.len() != 1 {
        return None;
    }
    let call = static_call(op)?;
    let callee_index = body_index.get(call.callee.index()).copied().flatten()?;
    let body = &module.functions[callee_index];
    let kind = match op.origin {
        Some(molt_ir::OpKind::CallGuarded) if call.callable.is_some() => {
            SiteKind::ProvenGuarded(call.callee)
        }
        Some(molt_ir::OpKind::CallInternal) => SiteKind::Internal(call.callee),
        _ => return None,
    };
    let arguments = call.bind(body)?;
    Some(Site {
        block,
        index,
        kind,
        args: arguments.positional.to_vec(),
        result: op.results[0],
        result_demanded: false,
    })
}

/// Calls of `func` that can resolve, in block-id then op order.
pub(super) fn sites_of<'a>(
    func: &TirFunction,
    body_index: &[Option<usize>],
    module: &TirModule,
    members: &'a ExactMembers,
) -> Vec<Site<'a>> {
    let mut loads = HashMap::default();
    if members.has_methods() {
        for op in func.blocks.values().flat_map(|block| &block.ops) {
            if let Some((pair, receiver, name)) = method_load(op)
                && let Some(candidates) = members.candidates(name)
            {
                loads.insert(pair, (receiver, candidates));
            }
        }
    }
    let mut blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    let mut sites = Vec::new();
    for block in blocks {
        for (index, op) in func.blocks[&block].ops.iter().enumerate() {
            if let Some(site) = static_site(op, block, index, body_index, module) {
                sites.push(site);
            } else if let Some((pair, args)) = method_call(op)
                && let Some(&(receiver, candidates)) = loads.get(&pair)
                && candidates.callees(args.len() + 1).next().is_some()
            {
                // The physical closure offset is selected with the body.
                sites.push(Site {
                    block,
                    index,
                    kind: SiteKind::Method(candidates),
                    args: std::iter::once(receiver)
                        .chain(args.iter().copied())
                        .collect(),
                    result: op.results[0],
                    result_demanded: false,
                });
            }
        }
    }
    sites
}

/// The calls of `func` whose callee the program does not prove (design 78,
/// 3.11 "unknown callee"): the opaque sites of the shared operation-aware
/// call-site projection.
pub(super) fn unknown_callee_count(func: &TirFunction) -> usize {
    unknown_callee_kinds(func).values().sum()
}

/// The unknown-callee sites of `func` by operation kind (its original kind,
/// else its opcode): which call forms the type flow cannot follow, so the
/// report ranks the forms to resolve next (design 78, 3.11).
pub(super) fn unknown_callee_kinds(
    func: &TirFunction,
) -> std::collections::BTreeMap<String, usize> {
    use crate::tir::call_sites::{CallEvent, CallSiteTarget, visit_sites};

    let mut kinds = std::collections::BTreeMap::new();
    visit_sites(func, |event| {
        if let CallEvent::Operation { op, site, .. } = event
            && site.target == CallSiteTarget::Opaque
        {
            let kind = op.origin.map_or_else(
                || format!("{:?}", op.opcode),
                |origin| origin.as_str().to_owned(),
            );
            *kinds.entry(kind).or_insert(0) += 1;
        }
        false
    });
    kinds
}

/// One parameter's part of a context key: the exact scalar family it holds,
/// the exact class statement its instance came from, or nothing proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ArgKind {
    Dyn,
    Int,
    Bool,
    Float,
    Str,
    NoneValue,
    /// An instance of exactly this class statement (I6): only that
    /// statement's allocation produces it.
    Class(molt_ir::ClassId),
}

impl ArgKind {
    /// The family of an exact solver fact; `None` while it is bottom.
    pub(super) fn of(ty: &TirType) -> Option<Self> {
        Some(match ty.semantic_type() {
            TirType::Never => return None,
            TirType::I64 | TirType::BigInt => Self::Int,
            TirType::Bool => Self::Bool,
            TirType::F64 => Self::Float,
            TirType::Str => Self::Str,
            TirType::None => Self::NoneValue,
            TirType::Class(class) => Self::Class(*class),
            _ => Self::Dyn,
        })
    }

    /// The exact type a parameter of this family is seeded with. `I64` is
    /// the semantic Python `int` of any magnitude (op_kinds.toml).
    pub(super) fn seed(self) -> TirType {
        match self {
            Self::Dyn => TirType::DynBox,
            Self::Int => TirType::I64,
            Self::Bool => TirType::Bool,
            Self::Float => TirType::F64,
            Self::Str => TirType::Str,
            Self::NoneValue => TirType::None,
            Self::Class(class) => TirType::Class(class),
        }
    }

    /// This kind's part of a clone's name and of a body digest: one letter
    /// for a scalar family, `c` and the class id for a class.
    pub(super) fn tag(self) -> String {
        match self {
            Self::Dyn => "d".into(),
            Self::Int => "i".into(),
            Self::Bool => "b".into(),
            Self::Float => "f".into(),
            Self::Str => "s".into(),
            Self::NoneValue => "n".into(),
            Self::Class(class) => format!("c{:016x}", class.0),
        }
    }

    pub(super) fn meet(self, other: Self) -> Self {
        if self == other { self } else { Self::Dyn }
    }
}

/// The exact join of two return facts: the int family joins to the semantic
/// `int`, any other disagreement to `DynBox`. `Never` is bottom.
pub(super) fn join_return(a: &TirType, b: &TirType) -> TirType {
    let int = |ty: &TirType| matches!(ty, TirType::I64 | TirType::BigInt);
    match (a, b) {
        (TirType::Never, other) | (other, TirType::Never) => other.clone(),
        (a, b) if a == b => a.clone(),
        (a, b) if int(a) && int(b) => TirType::I64,
        _ => TirType::DynBox,
    }
}
