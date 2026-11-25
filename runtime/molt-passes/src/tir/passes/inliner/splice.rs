use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::{checked_exception_observer, executable_reachable_blocks};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::ownership_lattice_min::owned_alias;
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
pub(super) fn splice_call_site(
    caller: &mut TirFunction,
    callee: &TirFunction,
    site: &CallSite,
    activations: &mut Activations,
) -> Option<BlockId> {
    let block_id = site.block;
    let op_index = site.op_index;

    let (mut call_args, call_result, multi_result, call_span, callable) = {
        let block = &caller.blocks[&block_id];
        let op = &block.ops[op_index];
        if op.opcode != OpCode::Call {
            return None;
        }
        // A proven guarded call's operand 0 is the callee's function object,
        // which the inlined activation does not read, except a closure
        // callee's environment (below): only the arguments bind.
        let call = crate::tir::call_targets::static_call(op)?;
        (
            call.arguments.to_vec(),
            op.results.first().copied(),
            op.results.len() > 1,
            op.source_span,
            call.callable,
        )
    };
    if multi_result {
        return None;
    }

    // A closure callee takes its environment first. Only a proven guarded
    // call supplies it: its operand 0 is the callee's own function object,
    // whose closure tuple is exactly what the runtime's binding would pass
    // (design 78 5.10). Eligibility admits closure callees only on final-TIR
    // targets, where `FunctionClosure` lowers. Any other call of a closure is
    // refused here: its operands carry no environment, and binding the
    // environment parameter to an argument would miscompile
    // `__molt_closure__[i]`.
    let closure_callable = if callee.has_closure_parameter() {
        Some(callable?)
    } else {
        None
    };
    let first_parameter = usize::from(closure_callable.is_some());

    // Arity must match (params bind 1:1 to args). A static call whose arg count
    // disagrees with the callee's param count is a shape we will not splice
    // (defensive - the frontend should keep these aligned, but a mismatch must
    // not produce malformed SSA).
    let callee_entry = &callee.blocks[&callee.entry_block];
    if callee_entry.args.len() != call_args.len() + first_parameter {
        return None;
    }

    // REFCOUNT guard (invariant 2).
    if call_site_has_arg_incref(&caller.blocks[&block_id], op_index, &call_args) {
        return None;
    }

    // The activation keeps the call's reference contract (invariant 2): each
    // parameter binds its argument or an owned binding alias of it, and every
    // exit clears the callee frame before it leaves. From here on `callee` is
    // that prepared body.
    let mut bindings = activations.bindings(caller, callee, site, &call_args, first_parameter);
    if closure_callable.is_some() {
        // The environment is a view the function object owns, and the inlined
        // body may outlive that object's last use in the caller. An owned
        // binding retains the tuple for the activation and releases it at the
        // frame clear.
        bindings.insert(0, Binding::Owned);
    }
    let callee = activations.prepare(callee, &bindings)?;

    // A value call requires one result from a reachable ordinary return.
    // An unwind requires a caller observer that can run without a result.
    let reachable = std::cell::OnceCell::new();
    let check = callee
        .blocks
        .values()
        .any(|block| matches!(block.terminator, Terminator::Unwind))
        .then(|| {
            checked_unwind_observer(
                caller,
                caller.blocks[&block_id].ops.get(op_index + 1),
                call_result,
            )
        })
        .flatten();
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
    let mut held_callable = None;
    if let Some(callable) = closure_callable {
        // Every refusal has been checked. Allocate the environment view only
        // now, so a refused splice preserves all caller state, including its
        // value allocator and type facts. CallerFacts sees only original
        // arguments; the splice owns the environment binding explicitly.
        // Keep the function itself alive until the activation exits, as the
        // original call did. Retaining only the environment after its load
        // permits a last-use function release between that load and retain,
        // and can run function weakref callbacks before the inlined body.
        // The existing explicit lifetime boundaries below pin this owner;
        // no new borrowing or drop-placement rule is needed.
        let held = caller.fresh_value();
        caller.value_types.insert(held, TirType::DynBox);
        binding_ops.push(owned_alias(callable, held, call_span));
        held_callable = Some(held);
        let view = caller.fresh_value();
        caller.value_types.insert(view, TirType::DynBox);
        let mut load = TirOp::new(OpCode::FunctionClosure, vec![held], vec![view]);
        load.source_span = call_span;
        binding_ops.push(load);
        call_args.insert(0, view);
    }
    let parameter_values: Vec<ValueId> = call_args
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
                Binding::Direct if proven.is_none() => argument,
                Binding::Direct => {
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
    let removed_call_opcode = all_ops.pop().map(|o| o.opcode);
    assert_eq!(
        removed_call_opcode,
        Some(OpCode::Call),
        "splice: expected to remove the Call op at {block_id:?}#{op_index}"
    );
    let mut pre_ops = all_ops;
    // The parameter bindings take the call's place.
    pre_ops.extend(binding_ops);

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

    // Insert B_pre (original id, ops 0..call, branch into cloned entry).
    caller.blocks.insert(
        block_id,
        TirBlock {
            id: block_id,
            args: pre_args,
            ops: pre_ops,
            terminator: Terminator::Branch {
                target: cloned.entry,
                args: Vec::new(),
            },
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
    Some(cont_block_id)
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
    call_result: Option<ValueId>,
) -> Option<TirOp> {
    let check = first.filter(|op| {
        op.results.is_empty() && !call_result.is_some_and(|result| op.operands.contains(&result))
    })?;
    checked_exception_observer(check, |label| {
        caller
            .label_id_map
            .iter()
            .find_map(|(&block, &value)| (value == label).then_some(BlockId(block)))
    })
    .map(|_| check.clone())
}
