//! The inlined activation of a call site (design 20 §1.6).
//!
//! A call binds its arguments to the callee's parameters, runs the body,
//! clears the callee frame at each exit and hands the caller one owned result.
//! A splice keeps that contract with the drop pass's own plan rather than a
//! model of its own:
//!
//! * **Bindings.** A parameter the activation owns binds to an owned
//!   `binding_alias` of its argument: every `Transferred` parameter, and a
//!   `Borrowed` one whose argument the caller could otherwise release inside
//!   the body (an owned value the caller does not read after the call). Any
//!   other argument binds directly: a raw carrier holds no reference, and a
//!   caller parameter or a value the caller reads later outlives the
//!   activation by itself.
//! * **Frame clear.** Before each exit the activation releases what the
//!   callee's own `Return` releases there, in the same order
//!   ([`frame_clear`]), then its `Borrowed` bindings, as the caller releases a
//!   borrowed argument after the call. Each release is a `DelBoundary`: a
//!   Python boundary that the caller's DropInsertion places once and never
//!   moves.
//! * **Result.** A return that names a frame binding returns an owned
//!   `binding_alias` taken before that frame clear, as CPython's `LOAD_FAST`
//!   precedes its frame clear, so the call's result never carries a binding of
//!   the activation on to the caller's boundary.
//!
//! A `del` of a parameter the activation owns no reference for releases
//! nothing. Any other explicit release of one would end the caller's reference
//! and has no activation equivalent: eligibility refuses a callee that releases
//! a `Borrowed` parameter so (`InlineWhyNot::UnownedParameterRelease`), and a
//! site whose raw argument binds a `Transferred` parameter that the callee
//! releases so is not spliced.

use std::collections::{HashMap, HashSet};

use crate::ir::ParameterCustody;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExplicitReleaseOperands, opcode_explicit_release_operands_table,
};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::alias_analysis::{AliasUnionFind, build_alias_union_find};
use crate::tir::passes::drop_insertion::{DROP_INSERTED_ATTR, frame_clear};
use crate::tir::passes::liveness::{
    TirLivenessResult, compute_liveness_in_domain, compute_raw_scalars,
};
use crate::tir::passes::ownership_lattice_min::owned_alias;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::call_sites::CallSite;

/// How a call argument binds its callee parameter in the activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Binding {
    /// The parameter names the argument; the activation holds no reference.
    Direct,
    /// The parameter names an owned `binding_alias` of the argument, which the
    /// activation releases at its frame clear.
    Owned,
}

/// The activations one inliner run prepares, and what the caller being
/// spliced guarantees about its arguments. A callee is final before any
/// caller splices it, so its prepared bodies serve every later caller.
#[derive(Default)]
pub(super) struct Activations {
    caller: Option<CallerFacts>,
    prepared: HashMap<(String, Vec<Binding>), Option<TirFunction>>,
}

/// What a caller, read before its first splice, guarantees about the values
/// it passes. Every site's arguments are values of the unspliced caller.
struct CallerFacts {
    aliases: AliasUnionFind,
    raw: HashSet<ValueId>,
    liveness: TirLivenessResult,
    parameters: HashSet<ValueId>,
}

impl Activations {
    /// Begins a caller's sites: its facts are read before its first splice.
    pub(super) fn enter_caller(&mut self) {
        self.caller = None;
    }

    /// How each argument of the call at `site` binds its parameter of `callee`.
    pub(super) fn bindings(
        &mut self,
        caller: &TirFunction,
        callee: &TirFunction,
        site: &CallSite,
        arguments: &[ValueId],
    ) -> Vec<Binding> {
        let facts = self
            .caller
            .get_or_insert_with(|| CallerFacts::compute(caller));
        arguments
            .iter()
            .enumerate()
            .map(|(position, &argument)| {
                if facts.raw.contains(&argument) {
                    return Binding::Direct;
                }
                let root = facts.aliases.root(argument);
                let outlives_the_call =
                    facts.parameters.contains(&root) || facts.read_after(caller, site, root);
                if callee.parameter_custody(position) == ParameterCustody::Transferred
                    || !outlives_the_call
                {
                    Binding::Owned
                } else {
                    Binding::Direct
                }
            })
            .collect()
    }

    /// The body to splice for `callee` under `bindings`, or `None` when no
    /// activation keeps the call's reference contract.
    pub(super) fn prepare(
        &mut self,
        callee: &TirFunction,
        bindings: &[Binding],
    ) -> Option<&TirFunction> {
        self.prepared
            .entry((callee.name.clone(), bindings.to_vec()))
            .or_insert_with(|| prepare_activation(callee, bindings))
            .as_ref()
    }
}

impl CallerFacts {
    fn compute(caller: &TirFunction) -> Self {
        let aliases = build_alias_union_find(caller);
        let raw = compute_raw_scalars(caller);
        let liveness = compute_liveness_in_domain(caller, &aliases, &raw);
        let parameters = caller.blocks[&caller.entry_block]
            .args
            .iter()
            .map(|parameter| aliases.root(parameter.id))
            .collect();
        Self {
            aliases,
            raw,
            liveness,
            parameters,
        }
    }

    /// Whether the caller reads `root` after the call at `site`.
    fn read_after(&self, caller: &TirFunction, site: &CallSite, root: ValueId) -> bool {
        let block = &caller.blocks[&site.block];
        let names = |value: ValueId| self.aliases.root(value) == root;
        let mut read = block.ops[site.op_index + 1..]
            .iter()
            .any(|op| op.operands.iter().any(|&operand| names(operand)));
        block.terminator.for_each_value(|value| read |= names(value));
        read || self.liveness.is_live_out(site.block, root)
    }
}

/// The callee body the splice clones for `bindings`: the parameter custody
/// the activation holds, each exit's frame clear, and owned captures of the
/// frame bindings a return names.
fn prepare_activation(callee: &TirFunction, bindings: &[Binding]) -> Option<TirFunction> {
    // The module phase inlines before terminal DropInsertion. A body whose RC
    // is already placed for its own frame has no plan left to reproduce.
    if matches!(
        callee.attrs.get(DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    ) {
        return None;
    }
    let owns = |position: usize| {
        bindings[position] == Binding::Owned
            && callee.parameter_custody(position) == ParameterCustody::Transferred
    };
    if releases_unowned_parameter(callee, |position| !owns(position)) {
        return None;
    }
    let parameters: Vec<ValueId> = callee.blocks[&callee.entry_block]
        .args
        .iter()
        .map(|parameter| parameter.id)
        .collect();
    let aliases = build_alias_union_find(callee);
    let unowned: HashSet<ValueId> = (0..parameters.len())
        .filter(|&position| !owns(position))
        .map(|position| aliases.root(parameters[position]))
        .collect();
    let mut body = callee.clone();
    for block in body.blocks.values_mut() {
        block.ops.retain_mut(|op| {
            if op.opcode != OpCode::DelBoundary {
                return true;
            }
            op.operands
                .retain(|&value| !unowned.contains(&aliases.root(value)));
            !op.operands.is_empty()
        });
    }
    let custody: Vec<ParameterCustody> = (0..parameters.len())
        .map(|position| {
            if owns(position) {
                ParameterCustody::Transferred
            } else {
                ParameterCustody::Borrowed
            }
        })
        .collect();
    body.set_parameter_custody(&custody);
    let clears = frame_clear(&body);
    let borrowed: Vec<ValueId> = (0..parameters.len())
        .filter(|&position| bindings[position] == Binding::Owned && !owns(position))
        .map(|position| parameters[position])
        .collect();
    let mut exits: Vec<BlockId> = body
        .blocks
        .iter()
        .filter(|(_, block)| matches!(block.terminator, Terminator::Return { .. }))
        .map(|(&exit, _)| exit)
        .collect();
    exits.sort_unstable_by_key(|exit| exit.0);
    for exit in exits {
        let clear = clears.get(&exit);
        let captured: HashSet<ValueId> = clear
            .into_iter()
            .flat_map(|clear| clear.published.iter().copied())
            .chain(borrowed.iter().map(|&parameter| aliases.root(parameter)))
            .collect();
        let mut releases = clear.map(|clear| clear.teardown()).unwrap_or_default();
        releases.extend(&borrowed);
        let Terminator::Return { values } = &body.blocks[&exit].terminator else {
            unreachable!("frame exits are Return blocks");
        };
        let mut values = values.clone();
        let mut teardown = Vec::with_capacity(releases.len());
        for value in &mut values {
            if captured.contains(&aliases.root(*value)) {
                let capture = body.fresh_value();
                if let Some(ty) = value_type(&body, *value) {
                    body.value_types.insert(capture, ty);
                }
                teardown.push(owned_alias(*value, capture, None));
                *value = capture;
            }
        }
        teardown.extend(releases.into_iter().map(del_boundary));
        let block = body.blocks.get_mut(&exit).expect("frame exit block");
        block.ops.extend(teardown);
        block.terminator = Terminator::Return { values };
    }
    Some(body)
}

/// Whether `callee` releases a parameter the activation owns no reference for
/// (`unowned`, by position) other than by a Python `del`. Such a release ends
/// the caller's reference, which no activation can give back.
pub(super) fn releases_unowned_parameter(
    callee: &TirFunction,
    unowned: impl Fn(usize) -> bool,
) -> bool {
    let released: Vec<ValueId> = callee
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode != OpCode::DelBoundary)
        .flat_map(released_operands)
        .collect();
    if released.is_empty() {
        return false;
    }
    let aliases = build_alias_union_find(callee);
    let roots: HashSet<ValueId> = callee.blocks[&callee.entry_block]
        .args
        .iter()
        .enumerate()
        .filter(|&(position, _)| unowned(position))
        .map(|(_, parameter)| aliases.root(parameter.id))
        .collect();
    released
        .into_iter()
        .any(|value| roots.contains(&aliases.root(value)))
}

/// The operands `op` releases by itself, from the generated release table.
fn released_operands(op: &TirOp) -> Vec<ValueId> {
    match opcode_explicit_release_operands_table(op.opcode, op.operands.len()) {
        ExplicitReleaseOperands::None => Vec::new(),
        ExplicitReleaseOperands::All => op.operands.clone(),
        ExplicitReleaseOperands::One(index) => {
            op.operands.get(index).copied().into_iter().collect()
        }
    }
}

/// The Python boundary that ends the binding `value`.
fn del_boundary(value: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelBoundary,
        operands: vec![value],
        results: Vec::new(),
        attrs: AttrDict::new(),
        source_span: None,
    }
}

/// The type `func` gives `value`, an op result or a block argument.
fn value_type(func: &TirFunction, value: ValueId) -> Option<TirType> {
    func.value_types.get(&value).cloned().or_else(|| {
        func.blocks
            .values()
            .flat_map(|block| &block.args)
            .find(|argument| argument.id == value)
            .map(|argument| argument.ty.clone())
    })
}
