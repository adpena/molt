//! Escape-state analysis and derived allocation-root facts.
//!
//! Callable effects do not establish noncapture. Only explicit local operand
//! contracts and transparent SSA transport permit an allocation to remain local.

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::call_targets::static_call;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_has_local_only_operands_table;
use crate::tir::ops::{AttrValue, TirOp};
use crate::tir::passes::value_identity::{copy_value_source, no_heap_alias_source};
use crate::tir::value_edges::ValueEdges;
use crate::tir::values::ValueId;

use super::classify::{EscapeState, is_alloc_site};

/// A copy is an exact alias; a CFG argument may select among several objects.
/// Both carry escape/finalizer obligations; exact aliases also establish
/// containment in a local allocation owner. Neither implies storage placement.
#[derive(Clone, Copy)]
struct ValueFlow {
    target: ValueId,
    exact: bool,
}

/// One transport projection for escape propagation, containment, and
/// finalizer sensitivity. The terminator owns the complete CFG-edge vocabulary.
fn value_flows(func: &TirFunction) -> ValueEdges<ValueId, ValueFlow> {
    let mut flows = ValueEdges::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(source) = no_heap_alias_source(op) {
                flows.add(
                    source,
                    ValueFlow {
                        target: op.results[0],
                        exact: true,
                    },
                );
            }
        }
        block.terminator.for_each_edge(|target, args| {
            if let Some(destination) = func.blocks.get(&target) {
                for (&source, parameter) in args.iter().zip(&destination.args) {
                    flows.add(
                        source,
                        ValueFlow {
                            target: parameter.id,
                            exact: false,
                        },
                    );
                }
            }
        });
    }
    flows
}

fn extend_forward(
    values: &mut HashSet<ValueId>,
    flows: &ValueEdges<ValueId, ValueFlow>,
    exact_only: bool,
) {
    let mut pending: Vec<_> = values.iter().copied().collect();
    while let Some(source) = pending.pop() {
        for flow in flows.from(&source) {
            if (!exact_only || flow.exact) && values.insert(flow.target) {
                pending.push(flow.target);
            }
        }
    }
}

/// Analyze every allocation use and propagate escape across aliases in both directions.
/// A callback can retain its receiver or any argument; a returned iterator can
/// retain its input without mutating it. Neither is a local-only read.
pub fn analyze(func: &TirFunction) -> HashMap<ValueId, EscapeState> {
    analyze_with_calls(func, |_, _| false)
}

/// Contextual escape lattice. Only a proved argument binding may discharge
/// its call use. Any other use of the same object still dominates that proof.
fn analyze_with_calls(
    func: &TirFunction,
    noncapturing_operand: impl FnMut(&TirOp, usize) -> bool,
) -> HashMap<ValueId, EscapeState> {
    let allocations: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| is_alloc_site(op.opcode))
        .flat_map(|op| op.results.iter().copied())
        .collect();
    analyze_roots(func, allocations.clone(), allocations, noncapturing_operand)
}

/// Capture facts for a bottom-up traversal. The resolver supplies locally
/// owned, finalized callee bodies. Discard the cache when a summarized body
/// changes. The intraprocedural alias/ownership pipeline stays conservative.
#[derive(Default)]
pub(crate) struct ParameterCapture {
    summaries: HashMap<molt_ir::FunctionId, Vec<bool>>,
}

impl ParameterCapture {
    pub(crate) fn analyze<'a>(
        &mut self,
        caller: &TirFunction,
        body: impl Fn(molt_ir::FunctionId) -> Option<&'a TirFunction>,
    ) -> HashMap<ValueId, EscapeState> {
        analyze_with_calls(caller, |op, operand| {
            self.noncapturing_operand(op, operand, &body)
        })
    }

    /// Only checked positional bindings to a locally owned callee can carry a
    /// body proof. The callable and transferred arguments retain their own call
    /// admission and destruction paths.
    fn noncapturing_operand<'a>(
        &mut self,
        op: &TirOp,
        operand: usize,
        body: impl FnOnce(molt_ir::FunctionId) -> Option<&'a TirFunction>,
    ) -> bool {
        let Some(call) = static_call(op) else {
            return false;
        };
        let Some(callee) = body(call.callee) else {
            return false;
        };
        let Some(bound) = call.bind(callee) else {
            return false;
        };
        let Some(position) = operand.checked_sub(usize::from(call.callable.is_some())) else {
            return false;
        };
        if op
            .attrs
            .contains_key(crate::tir::ops::ARGUMENT_CUSTODY_ATTR)
        {
            return false;
        }
        self.summaries
            .entry(call.callee)
            .or_insert_with(|| noncapturing_parameters(callee))
            .get(bound.first_positional() + position)
            == Some(&true)
    }
}

/// Which physical parameters a completed callee body cannot retain. A return,
/// heap write, opaque use or finalizer is a capture boundary, including through
/// copies and block arguments. A frame, suspension or callback may publish an
/// argument without naming it at the observation site. An activation owning a
/// parameter can run a finalizer on implicit teardown.
///
/// The caller uses this only to price an inline site. The call and the splice
/// independently own admission, memory effects, and reference custody.
fn noncapturing_parameters(func: &TirFunction) -> Vec<bool> {
    use super::super::frame_elision::{op_observer, terminator_observer};
    use crate::tir::blocks::Terminator;
    let entry = &func.blocks[&func.entry_block];
    if func.execution_context != molt_ir::ExecutionContextPolicy::None
        || func.frame_locals_exposed()
        || func.is_codegen_partition()
        || func.has_state_machine()
        || func.has_exception_handlers()
        || entry.args.len() != func.param_types.len()
        || (0..entry.args.len())
            .any(|i| func.parameter_custody(i) == molt_ir::ParameterCustody::Transferred)
    {
        return Vec::new();
    }
    // Empty type evidence is conservative. An annotated operand cannot rule
    // out a callback through a protocol, a branch or a release.
    let types = HashMap::default();
    if func.blocks.values().any(|block| {
        matches!(block.terminator, Terminator::Unwind)
            || terminator_observer(&block.terminator, &types).is_some()
            || block
                .ops
                .iter()
                .any(|op| !op.has_valid_shape() || op_observer(op, &types).is_some())
    }) {
        return Vec::new();
    }
    let roots = entry.args.iter().map(|arg| arg.id).collect();
    // Formal arguments can alias caller-owned objects. No formal is itself a
    // fresh storage owner, so a store into another parameter captures.
    let escapes = analyze_roots(func, roots, HashSet::default(), |_, _| false);
    entry
        .args
        .iter()
        .map(|arg| escapes.get(&arg.id) == Some(&EscapeState::NoEscape))
        .collect()
}

fn analyze_roots(
    func: &TirFunction,
    mut tracked: HashSet<ValueId>,
    mut local_owners: HashSet<ValueId>,
    mut noncapturing_operand: impl FnMut(&TirOp, usize) -> bool,
) -> HashMap<ValueId, EscapeState> {
    if tracked.is_empty() {
        return HashMap::default();
    }
    let flows = value_flows(func);
    // A may-alias CFG parameter can also name an external object. Only exact
    // allocation aliases can establish containment in a function-local owner.
    extend_forward(&mut local_owners, &flows, true);
    extend_forward(&mut tracked, &flows, false);
    let mut escapes: HashMap<ValueId, EscapeState> = tracked
        .iter()
        .map(|&value| (value, EscapeState::NoEscape))
        .collect();
    // Finalizers may resurrect their receiver and objects retained by it.
    // Their retained graph must remain externally observable to alias consumers.
    let mut finalizers = finalizer_seeds(func);
    extend_forward(&mut finalizers, &flows, false);
    for value in finalizers {
        if let Some(state) = escapes.get_mut(&value) {
            *state = EscapeState::GlobalEscape;
        }
    }
    // The parent can retain a child through an explicitly offset-keyed field.
    // Generic attribute/index stores may call Python and never enter this lane.
    let mut retained = ValueEdges::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if copy_value_source(op).is_some() || opcode_has_local_only_operands_table(op.opcode) {
                continue;
            }
            if let Some((target, _offset)) = op.plain_typed_slot_store() {
                let value = op.operands[1];
                if tracked.contains(&value) {
                    if local_owners.contains(&target) {
                        retained.add(target, value);
                    } else {
                        escapes.insert(value, EscapeState::GlobalEscape);
                    }
                }
                continue;
            }
            // Fail closed for calls, overloaded operations, iteration, generic
            // heap accesses, retaining results, and unclassified future opcodes.
            for (index, operand) in op.operands.iter().enumerate() {
                if let Some(state) = escapes.get_mut(operand) {
                    *state =
                        if *state != EscapeState::GlobalEscape && noncapturing_operand(op, index) {
                            EscapeState::ArgEscape
                        } else {
                            EscapeState::GlobalEscape
                        };
                }
            }
        }
        // Returned values escape; a tracked heap condition can dispatch Python.
        // Edge arguments remain governed by value_flows, not this direct-use rule.
        block.terminator.for_each_direct_value(|value| {
            if let Some(state) = escapes.get_mut(&value) {
                *state = EscapeState::GlobalEscape;
            }
        });
    }
    let mut pending: Vec<_> = escapes
        .iter()
        .filter_map(|(&value, state)| (*state != EscapeState::NoEscape).then_some(value))
        .collect();
    if pending.is_empty() {
        return escapes;
    }
    // Escape is an object obligation. An alias can carry it to the object
    // or to any other alias, while containment only carries it to the child.
    for &source in &tracked {
        for flow in flows.from(&source) {
            retained.add(source, flow.target);
            retained.add(flow.target, source);
        }
    }
    while let Some(parent) = pending.pop() {
        let parent_state = escapes[&parent];
        for &child in retained.from(&parent) {
            if let Some(state) = escapes.get_mut(&child)
                && *state < parent_state
            {
                *state = parent_state;
                pending.push(child);
            }
        }
    }
    escapes
}

/// Returns `true` when an op produces a finalizer-bearing instance. The frontend
/// stamps `defines_del=true` after resolving `__del__` through the class MRO,
/// excluding `object`; devirtualized allocation and generic class instantiation
/// both transport that same fact.
///
/// A finalizer can resurrect its receiver beyond the current frame. Such a
/// result must keep both heap storage and its final reference-counted release;
/// no local-use proof can replace the finalizer's external callback contract.
pub(crate) fn op_result_defines_del(op: &TirOp) -> bool {
    !op.results.is_empty() && matches!(op.attrs.get("defines_del"), Some(AttrValue::Bool(true)))
}

/// The set of allocation roots positively known to define a `__del__` finalizer,
/// transitively through pure SSA-move copies and CFG arguments. This is the single
/// FinalizerSensitive seed fact (design 27). Absence is not a negative proof:
/// runtime classes can gain a finalizer, and opaque producers may omit metadata.
/// Consumers may use membership to reject an optimization, never nonmembership
/// alone to prove callback-free destruction or erase ownership.
///
/// Such an instance MUST stay heap-allocated with a live refcount so the
/// finalizer-aware `dec_ref_ptr` dispatches `__del__` at the last drop; it must
/// therefore be excluded from complete scalar replacement. Ordinary class
/// allocations retain their owned-result RC regardless of escape state.
///
/// Refcount elimination preserves final releases and no longer substitutes
/// direct `Free` operations for object destruction.
///
/// The requirement flows forward through exact copies and every CFG edge, so
/// it reaches every value that may name the finalizer-bearing object.
pub(crate) fn finalizer_alloc_roots(func: &TirFunction) -> HashSet<ValueId> {
    let mut del_required = finalizer_seeds(func);
    if !del_required.is_empty() {
        extend_forward(&mut del_required, &value_flows(func), false);
    }
    del_required
}

fn finalizer_seeds(func: &TirFunction) -> HashSet<ValueId> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op_result_defines_del(op))
        .flat_map(|op| op.results.iter().copied())
        .collect()
}
