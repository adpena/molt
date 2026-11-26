use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::executable_reachable_blocks;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::ownership_lattice_min::owned_alias;
use crate::tir::type_refine::checked_result_observer;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::activation::{Activations, Binding};
use super::call_sites::{CallSite, call_site_has_arg_incref};
use super::clone_body::clone_function_body_with_fresh_ids;

/// Splice the call site `(block, op_index)` in `caller`: replace the `Call` to
/// `callee` (an owned snapshot) with the callee's inlined body.
///
/// The callee is passed by reference rather than looked up inside, because the
/// driver holds `&mut caller` borrowed out of the module vector and Rust cannot
/// prove disjointness from a second borrow of the callee through the same
/// vector. The driver clones the callee snapshot (`callee_idx != caller_idx` is
/// guaranteed - self-calls are filtered) and hands it here.
///
/// Returns the continuation block `B_cont` if the site was inlined, `None` if
/// it was refused (refcount guard, multi-result/arity/shape mismatch, or no
/// activation that keeps the call's reference contract - all of which leave
/// the call intact, conservative-correct).
///
/// Mechanics:
/// 1. Read the `Call` op's argument operands and (optional) result value.
/// 2. Refcount guard - refuse a site with a caller-side arg `IncRef` in the <=2
///    preceding ops.
/// 3. Prepare the callee's activation for this site ([`Activations`]): which
///    parameters bind an owned `binding_alias` of their argument, each exit's
///    frame clear, and owned captures of the frame bindings a return names.
/// 4. Clone the prepared body into `caller`, each parameter bound to its
///    argument or to the argument's binding alias.
/// 5. Split the caller block at the `Call` into `B_pre` (ops `0..op_index`,
///    then the binding aliases, keeping the original block id) and a fresh
///    continuation `B_cont` (ops `op_index+1..`, taking the original
///    terminator). `B_cont`'s single block argument is the original call-result
///    value id, so every downstream use of the call result is satisfied without
///    rewriting.
/// 6. `B_pre` branches unconditionally into the cloned entry.
/// 7. Each cloned `Return { values }` becomes `Branch { target: B_cont, args:
///    values }` (or `Branch B_cont []` for a void callee with a no-arg `B_cont`).
///    Observation-only void exception exits may branch directly to the caller's
///    post-call exception target instead of joining a value-carrying continuation.
///    Every exit keeps the frame clear that precedes its `Return`.
/// 8. The original `Call` op is gone (it lived between `B_pre` and `B_cont`).
#[cfg(test)]
pub(super) fn splice_call_site(
    caller: &mut TirFunction,
    callee: &TirFunction,
    site: &CallSite,
    activations: &mut Activations,
) -> Option<BlockId> {
    splice(caller, callee, site, activations).map(|(continuation, _)| continuation)
}

/// A guarded splice retains the caller's distinct post-call service.
pub(super) fn splice(
    caller: &mut TirFunction,
    callee: &TirFunction,
    site: &CallSite,
    activations: &mut Activations,
) -> Option<(BlockId, bool)> {
    let block_id = site.block;
    let op_index = site.op_index;

    let (mut call_args, call_result, call_span, captured_callable, first_parameter) = {
        let block = &caller.blocks[&block_id];
        let op = &block.ops[op_index];
        if op.opcode != OpCode::Call || op.results.len() > 1 {
            return None;
        }
        // The invocation's function is an owner even when it supplies no
        // closure environment to the callee.
        let call = crate::tir::call_targets::static_call(op)?;
        let arguments = call.bind(callee)?;
        let first = arguments.first_positional();
        if callee.blocks[&callee.entry_block].args.len() != first + arguments.positional.len() {
            return None;
        }
        (
            arguments.positional.to_vec(),
            op.results.first().copied(),
            op.source_span,
            call.callable,
            first,
        )
    };

    // REFCOUNT guard (invariant 2).
    if call_site_has_arg_incref(&caller.blocks[&block_id], op_index, &call_args) {
        return None;
    }

    // The activation keeps the call's reference contract (invariant 2): each
    // parameter binds its argument or an owned binding alias of it, and every
    // exit clears the callee frame before it leaves. From here on `callee` is
    // that prepared body.
    let mut bindings = activations.bindings(caller, callee, site, &call_args, first_parameter);
    let observable = callee.execution_context == molt_ir::ExecutionContextPolicy::Local;
    let hit = if observable {
        if caller.execution_context != molt_ir::ExecutionContextPolicy::Local
            || call_result.is_none()
            || first_parameter != 0
        {
            return None;
        }
        let callable = captured_callable?;
        let plan = activations.packed_read(site.callee, callee)?;
        Some(super::packed_splice::Hit::prepare(
            callee, plan, callable, &call_args, &bindings,
        )?)
    } else {
        None
    };
    if first_parameter != 0 {
        // The environment is a view the function object owns, and the inlined
        // body may outlive that object's last use in the caller. An owned
        // binding retains the tuple for the activation and releases it at the
        // frame clear.
        bindings.insert(0, Binding::Owned);
    }
    let success_only =
        call_result.is_none_or(|result| activations.checked_result_is_success_only(result));
    let needs_callable_owner =
        captured_callable.is_some_and(|value| !activations.caller_keeps_alive(site, value));
    let callee = if let Some(hit) = &hit {
        &hit.body
    } else {
        activations.prepare(site.callee, callee, &bindings)?
    };

    // A value call requires one result from a reachable ordinary return.
    // An unwind requires a caller observer that can run without a result.
    let reachable = std::cell::OnceCell::new();
    let call_observer = (observable
        || callee
            .blocks
            .values()
            .any(|block| matches!(block.terminator, Terminator::Unwind)))
    .then(|| {
        checked_unwind_observer(
            caller,
            caller.blocks[&block_id].ops.get(op_index + 1),
            success_only,
        )
    })
    .flatten();
    // A depth refusal precedes binding the arguments and entering the virtual
    // frame. The original caller's Origin must capture it.
    if observable
        && call_observer.as_ref().is_none_or(|check| {
            check.exception_edge_kind() != Some(ExceptionEdgeKind::Origin)
                || check.has_checked_invocation_binding()
        })
    {
        return None;
    }
    let mut check = call_observer.clone();
    if observable && let Some(check) = &mut check {
        check.set_exception_edge_kind(ExceptionEdgeKind::Unwind);
    }
    for block in callee.blocks.values() {
        let incompatible = match &block.terminator {
            Terminator::Return { values } => call_result.is_some() && values.is_empty(),
            Terminator::Unwind => check.is_none(),
            Terminator::Branch { .. }
            | Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::StateDispatch { .. }
            | Terminator::Unreachable => false,
        };
        if incompatible
            && reachable
                .get_or_init(|| executable_reachable_blocks(callee))
                .contains(&block.id)
        {
            return None;
        }
    }

    // Bind each parameter to its argument, or to an owned binding alias of it
    // placed where the call was.
    // A specialized clone's parameter types (`exact_param_types`) are what
    // the type flow proved of this site's arguments, a whole-program fact
    // the caller's own solver cannot derive again. The binding carries it as
    // the binding op's `exact_result_proof`, so the inlined body keeps the
    // facts its clone was compiled under.
    let callee_entry = &callee.blocks[&callee.entry_block];
    let mut binding_ops = Vec::new();
    let held_callable = captured_callable
        .filter(|_| needs_callable_owner)
        .map(|callable| {
            // A temporary callable must survive the invocation, including callee
            // cleanup. Its last release can run weakref callbacks or destroy an
            // object owned by the function. Keep this use even for frameless bodies.
            let held = caller.fresh_value();
            caller.value_types.insert(held, TirType::DynBox);
            binding_ops.push(owned_alias(callable, held, call_span));
            held
        });
    if first_parameter != 0 {
        let callable = held_callable
            .or(captured_callable)
            .expect("bound closure arguments require their function");
        let view = caller.fresh_value();
        caller.value_types.insert(view, TirType::DynBox);
        let mut load = TirOp::new(OpCode::FunctionClosure, vec![callable], vec![view]);
        load.source_span = call_span;
        binding_ops.push(load);
        call_args.insert(0, view);
    }
    let mut parameter_values: Vec<ValueId> = call_args
        .iter()
        .zip(&bindings)
        .zip(&callee_entry.args)
        .enumerate()
        .map(|(position, ((&argument, &binding), parameter))| {
            let proven = callee
                .exact_param_types
                .get(position)
                .filter(|ty| **ty != TirType::DynBox);
            match binding {
                Binding::Direct | Binding::Move if proven.is_none() => argument,
                Binding::Direct | Binding::Move => {
                    let bound = caller.fresh_value();
                    let mut op = TirOp::new(OpCode::Copy, vec![argument], vec![bound]);
                    op.source_span = call_span;
                    if !op.mark_exact_result_proof(proven.expect("checked")) {
                        return argument;
                    }
                    caller.value_types.insert(bound, parameter.ty.clone());
                    binding_ops.push(op);
                    bound
                }
                Binding::Owned => {
                    let alias = caller.fresh_value();
                    caller.value_types.insert(alias, parameter.ty.clone());
                    let mut op = owned_alias(argument, alias, call_span);
                    if let Some(ty) = proven {
                        let _ = op.mark_exact_result_proof(ty);
                    }
                    binding_ops.push(op);
                    alias
                }
            }
        })
        .collect();
    let guard = hit.as_ref().map(|hit| {
        let entered = caller.fresh_block();
        let miss = caller.fresh_block();
        let (first, carry) = hit.guard(caller, entered, miss, call_span);
        parameter_values.extend(carry);
        (first, entered, miss)
    });
    if observable {
        parameter_values.push(
            held_callable
                .or(captured_callable)
                .expect("a prepared observable body requires its captured function"),
        );
    }

    // Clone the prepared body into the caller (params -> bindings).
    let cloned = clone_function_body_with_fresh_ids(callee, caller, &parameter_values);

    // Split the caller block. Take the original block out, partition its ops.
    let original = caller
        .blocks
        .remove(&block_id)
        .expect("splice: caller block vanished");
    let TirBlock {
        id: _,
        args: pre_args,
        ops: mut all_ops,
        terminator: original_term,
    } = original;

    // Ops after the call become the continuation block's ops.
    let cont_ops = all_ops.split_off(op_index + 1);
    // Remove the `Call` op itself (now the last element of `all_ops`).
    let removed_call = all_ops.pop().expect("an accepted splice has a call");
    assert_eq!(
        removed_call.opcode,
        OpCode::Call,
        "splice: expected to remove the Call op at {block_id:?}#{op_index}"
    );
    let mut pre_ops = all_ops;
    // The continuation block takes a single argument = the original call result
    // value id (when the call produced a value). A void call -> no-arg cont.
    let cont_block_id = caller.fresh_block();
    let cont_args: Vec<TirValue> = match call_result {
        Some(result) => {
            let ty = caller
                .value_types
                .get(&result)
                .cloned()
                .or_else(|| callee_return_value_type(callee))
                .unwrap_or(TirType::DynBox);
            vec![TirValue { id: result, ty }]
        }
        None => Vec::new(),
    };
    let cont_arity = cont_args.len();
    debug_assert!(cont_arity <= 1);
    for &cloned_bid in &cloned.cloned_blocks {
        let unwind = matches!(caller.blocks[&cloned_bid].terminator, Terminator::Unwind);
        let Some(values) = caller.blocks[&cloned_bid].terminator.exit_values() else {
            continue;
        };
        let branch_args = values.first().copied().filter(|_| cont_arity == 1);
        let block = caller
            .blocks
            .get_mut(&cloned_bid)
            .expect("cloned activation exit");

        if observable {
            let mut release = TirOp::new(OpCode::RecursionRelease, vec![], vec![]);
            release.source_span = call_span;
            block.ops.push(release);
        }
        if let Some(held) = held_callable {
            let mut release = TirOp::new(OpCode::DelBoundary, vec![held], vec![]);
            release.source_span = call_span;
            block.ops.push(release);
        }
        if unwind {
            if let Some(observer) = check.as_ref() {
                // Keep the caller's traceback and poll even on a spliced exit.
                block.ops.push(observer.clone());
            } else {
                // Only structurally retained unreachable callee exits arrive here.
                block.terminator = Terminator::Unreachable;
            }
        } else if cont_arity == 1 && branch_args.is_none() {
            block.terminator = Terminator::Unreachable;
        } else {
            block.terminator = Terminator::Branch {
                target: cont_block_id,
                args: branch_args.into_iter().collect(),
            };
        }
    }

    let enter_body = Terminator::Branch {
        target: cloned.entry,
        args: Vec::new(),
    };
    let enter = if let Some((first, entered, _)) = guard {
        // The admitted path owns one depth charge through the callee's
        // normal or exceptional clear, just like a physical guarded call.
        let mut count = TirOp::new(OpCode::RecursionCount, vec![], vec![]);
        count.source_span = call_span;
        binding_ops.insert(0, count);
        caller.blocks.insert(
            entered,
            TirBlock {
                id: entered,
                args: vec![],
                ops: binding_ops,
                terminator: enter_body,
            },
        );
        // FrameEntryAdmits includes recursion depth, code ownership and arena room.
        Terminator::Branch {
            target: first,
            args: vec![],
        }
    } else {
        pre_ops.extend(binding_ops);
        enter_body
    };
    // The original block id remains the entry to this invocation.
    caller.blocks.insert(
        block_id,
        TirBlock {
            id: block_id,
            args: pre_args,
            ops: pre_ops,
            terminator: enter,
        },
    );

    // Insert B_cont (continuation: the cont arg + the post-call ops + original
    // terminator).
    caller.blocks.insert(
        cont_block_id,
        TirBlock {
            id: cont_block_id,
            args: cont_args,
            ops: cont_ops,
            terminator: original_term,
        },
    );

    caller.move_loop_terminator(block_id, cont_block_id);
    if let Some((_, _, miss)) = guard {
        super::packed_splice::ordinary_call(caller, miss, cont_block_id, removed_call);
    }
    Some((cont_block_id, guard.is_some()))
}

/// The type the callee returns, derived from its `Return` terminators'
/// value_types (best-effort, for annotating the continuation block arg).
fn callee_return_value_type(callee: &TirFunction) -> Option<TirType> {
    for block in callee.blocks.values() {
        if let Terminator::Return { values } = &block.terminator
            && let Some(v) = values.first()
            && let Some(ty) = callee.value_types.get(v)
        {
            return Some(ty.clone());
        }
    }
    if callee.return_abi.returns_value() {
        return Some(callee.return_type.clone());
    }
    None
}

fn checked_unwind_observer(
    caller: &TirFunction,
    first: Option<&TirOp>,
    success_only: bool,
) -> Option<TirOp> {
    let check = first?;
    checked_result_observer(caller, &std::cell::OnceCell::new(), check)?;
    // Splicing defines the result only on the ordinary continuation. An
    // unwind copies this observer into a block that never enters that
    // continuation, so neither its payload nor any handler/merged cleanup
    // may read the physical call word. Reuse the checked-result use census:
    // inspecting only the observer's operands misses direct downstream reads.
    if !success_only {
        return None;
    }
    Some(check.clone())
}
