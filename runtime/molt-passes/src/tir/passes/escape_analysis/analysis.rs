//! Escape-state analysis and derived allocation-root facts.
//!
//! Callable effects do not establish noncapture. Only explicit local operand
//! contracts and transparent SSA transport permit an allocation to remain local.

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_has_local_only_operands_table;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::value_edges::ValueEdges;
use crate::tir::values::ValueId;

use super::classify::{EscapeState, is_alloc_site, is_pure_move_copy};

/// A copy is an exact alias; a CFG argument may select among several objects.
/// Both carry escape/finalizer obligations; exact aliases also establish
/// containment in a local allocation owner. Neither implies storage placement.
#[derive(Clone, Copy)]
struct ValueFlow {
    target: ValueId,
    exact: bool,
}

fn is_transparent_copy(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && op.operands.len() == 1
        && op.results.len() == 1
        && is_pure_move_copy(op.origin)
}

/// One transport projection for escape propagation, containment, and
/// finalizer sensitivity. The terminator owns the complete CFG-edge vocabulary.
fn value_flows(func: &TirFunction) -> ValueEdges<ValueId, ValueFlow> {
    let mut flows = ValueEdges::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if is_transparent_copy(op) {
                flows.add(
                    op.operands[0],
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
    let mut tracked: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| is_alloc_site(op.opcode))
        .flat_map(|op| op.results.iter().copied())
        .collect();
    if tracked.is_empty() {
        return HashMap::default();
    }
    let flows = value_flows(func);
    // A may-alias CFG parameter can also name an external object. Only exact
    // allocation aliases can establish containment in a function-local owner.
    let mut local_owners = tracked.clone();
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
            if is_transparent_copy(op) || opcode_has_local_only_operands_table(op.opcode) {
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
            for operand in &op.operands {
                if let Some(state) = escapes.get_mut(operand) {
                    *state = EscapeState::GlobalEscape;
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
        .filter_map(|(&value, state)| (*state == EscapeState::GlobalEscape).then_some(value))
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
        for &child in retained.take_from(&parent) {
            if let Some(state) = escapes.get_mut(&child)
                && *state != EscapeState::GlobalEscape
            {
                *state = EscapeState::GlobalEscape;
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
