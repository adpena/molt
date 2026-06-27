use std::collections::HashSet;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::{CfgEdgePolicy, reachable_blocks_with};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, dead_placeholder_const_for_type};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::call_sites::{CallSite, call_site_has_arg_incref};
use super::clone_body::clone_function_body_with_fresh_ids;
use super::eligibility::is_closure;

/// Splice the call site `(block, op_index)` in `caller`: replace the `Call` to
/// `callee` (an owned snapshot) with the callee's inlined body.
///
/// The callee is passed by reference rather than looked up inside, because the
/// driver holds `&mut caller` borrowed out of the module vector and Rust cannot
/// prove disjointness from a second borrow of the callee through the same
/// vector. The driver clones the callee snapshot (`callee_idx != caller_idx` is
/// guaranteed — self-calls are filtered) and hands it here.
///
/// Returns `true` if the site was inlined, `false` if it was refused (refcount
/// guard, multi-result/arity/shape mismatch — all of which leave the call
/// intact, conservative-correct).
///
/// Mechanics:
/// 1. Read the `Call` op's argument operands and (optional) result value.
/// 2. Refcount guard — refuse a site with a caller-side arg `IncRef` in the ≤2
///    preceding ops.
/// 3. Clone the callee body (params bound to the call args) into `caller`.
/// 4. Split the caller block at the `Call` into `B_pre` (ops `0..op_index`,
///    keeping the original block id) and a fresh continuation `B_cont` (ops
///    `op_index+1..`, taking the original terminator). `B_cont`'s single block
///    argument is the original call-result value id, so every downstream use of
///    the call result is satisfied without rewriting.
/// 5. `B_pre` branches unconditionally into the cloned entry.
/// 6. Each cloned `Return { values }` becomes `Branch { target: B_cont, args:
///    values }` (or `Branch B_cont []` for a void callee with a no-arg `B_cont`).
/// 7. The original `Call` op is gone (it lived between `B_pre` and `B_cont`).
pub(super) fn splice_call_site(
    caller: &mut TirFunction,
    callee: &TirFunction,
    site: &CallSite,
) -> bool {
    let block_id = site.block;
    let op_index = site.op_index;

    let (call_args, call_result, multi_result): (Vec<ValueId>, Option<ValueId>, bool) = {
        let block = &caller.blocks[&block_id];
        let op = &block.ops[op_index];
        if op.opcode != OpCode::Call {
            return false;
        }
        (
            op.operands.clone(),
            op.results.first().copied(),
            op.results.len() > 1,
        )
    };
    if multi_result {
        return false;
    }

    // DEFENSE-IN-DEPTH: never splice a closure here. `is_inlineable` already
    // excludes closures (see `is_closure`), so a closure should never reach this
    // splice. But the arity guard below CANNOT distinguish a closure from a
    // legitimate same-arity call — a closure's leading `__molt_closure__` param
    // re-balances against the `Call`'s leading function-value operand, so the
    // guard would pass (false match) and the splice would bind the env-param to
    // the function object, miscompiling `__molt_closure__[i]` into a subscript of
    // a function. Refuse structurally so a future `is_inlineable` change cannot
    // silently re-open the hole. Refusal (not panic) keeps a release build sound;
    // the debug assert flags the invariant violation in tests.
    debug_assert!(
        !is_closure(callee),
        "splice_call_site: closure '{}' reached splice — is_inlineable must \
         exclude closures (the arity guard cannot, its env-param re-balances \
         the operand count)",
        callee.name
    );
    if is_closure(callee) {
        return false;
    }

    // Arity must match (params bind 1:1 to args). A static call whose arg count
    // disagrees with the callee's param count is a shape we will not splice
    // (defensive — the frontend should keep these aligned, but a mismatch must
    // not produce malformed SSA).
    let callee_entry = &callee.blocks[&callee.entry_block];
    if callee_entry.args.len() != call_args.len() {
        return false;
    }

    // REFCOUNT guard (invariant 2).
    if call_site_has_arg_incref(&caller.blocks[&block_id], op_index, &call_args) {
        return false;
    }

    // Classify the callee's `Return` blocks on its **terminator-only** CFG:
    //  * NORMAL return — reachable from entry through terminator edges. Carries
    //    the function's actual return value.
    //  * EXCEPTION EXIT — reachable ONLY via implicit exception edges
    //    (`CheckException` → function-exit). A `ret_void` "propagate the pending
    //    flag" exit; it carries no value.
    // This classification (computed on the callee, before any mutation) drives
    // both the pre-check below and the placeholder padding in the rewrite loop.
    let normal_reachable = reachable_blocks_with(callee, CfgEdgePolicy::TerminatorOnly);

    // Return-arity compatibility pre-check (BEFORE any mutation, so a refusal
    // leaves `caller` byte-identical — no fragile mid-splice rollback). The
    // continuation carries one argument iff the call produces a value. Every
    // NORMAL-return site must then carry a value: a value call demands exactly
    // one returned value from each normal return. A normal return that carries
    // *no* value while the call expects one is a frontend-shape mismatch we
    // refuse rather than fabricate a value for. (An EXCEPTION-EXIT carries no
    // value by construction; it is handled by placeholder padding, not refused —
    // refusing it would re-dormant the inliner on every value-returning
    // observation-only callee, which is the whole point of this arc.)
    let call_wants_value = call_result.is_some();
    if call_wants_value {
        for (bid, block) in &callee.blocks {
            if let Terminator::Return { values } = &block.terminator
                && normal_reachable.contains(bid)
                && values.is_empty()
            {
                return false;
            }
        }
    }

    // Clone the callee body into the caller (params → call args).
    let cloned = clone_function_body_with_fresh_ids(callee, caller, &call_args);

    // The cloned block ids of the callee's EXCEPTION-EXIT blocks (reached only via
    // exception edges). Their cloned `Return`s need placeholder padding when the
    // continuation carries a value.
    let exception_exit_clones: HashSet<BlockId> = callee
        .blocks
        .keys()
        .filter(|bid| !normal_reachable.contains(bid))
        .filter_map(|bid| cloned.block_map.get(bid).copied())
        .collect();

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
    let pre_ops = all_ops;

    // The continuation block takes a single argument = the original call result
    // value id (when the call produced a value). A void call → no-arg cont.
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

    // Rewrite each cloned `Return { values }` into a branch to the continuation.
    //  * A NORMAL return (value call): branch with the returned value — the
    //    pre-check guarantees it carries one.
    //  * An EXCEPTION-EXIT return (`ret_void`) into a value-carrying continuation:
    //    synthesize a representation-matched DEAD placeholder for the missing
    //    continuation arg. The value is provably dead — `B_cont`'s first op is the
    //    caller's post-call `CheckException`, which re-observes the pending flag
    //    and reroutes before the call result is ever used — so the placeholder is
    //    never read. `verify_block_args` checks only arity; the typed placeholder
    //    keeps the continuation phi's representation clean for codegen.
    //  * A void call (cont_arity 0): branch with no args (any returned value, on
    //    the normal or exception path, is discarded — the call discarded it too).
    let cont_arity = cont_args.len();
    let cont_ty: Option<TirType> = cont_args.first().map(|a| a.ty.clone());
    debug_assert!(
        cont_arity <= 1,
        "continuation arity is 0 (void) or 1 (value)"
    );
    for &cloned_bid in &cloned.cloned_blocks {
        let return_values: Option<Vec<ValueId>> = match &caller.blocks[&cloned_bid].terminator {
            Terminator::Return { values } => Some(values.clone()),
            _ => None,
        };
        let Some(values) = return_values else {
            continue;
        };

        let branch_args: Vec<ValueId> = match (cont_arity, values.first()) {
            (0, _) => Vec::new(),
            (1, Some(&v)) => vec![v],
            (1, None) => {
                // Void return into a value-carrying continuation. The pre-check
                // refused any NORMAL return that carries no value, so this is
                // exclusively an exception-exit.
                debug_assert!(
                    exception_exit_clones.contains(&cloned_bid),
                    "void return survived the pre-check in a non-exception-exit block"
                );
                let ty = cont_ty.clone().unwrap_or(TirType::DynBox);
                let placeholder = caller.fresh_value();
                caller.value_types.entry(placeholder).or_insert(ty.clone());
                let const_op = dead_placeholder_const_for_type(&ty, placeholder);
                caller
                    .blocks
                    .get_mut(&cloned_bid)
                    .expect("cloned block missing")
                    .ops
                    .push(const_op);
                vec![placeholder]
            }
            _ => unreachable!("continuation arity is 0 or 1 (debug-asserted)"),
        };
        caller
            .blocks
            .get_mut(&cloned_bid)
            .expect("cloned block missing")
            .terminator = Terminator::Branch {
            target: cont_block_id,
            args: branch_args,
        };
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

    true
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
    if callee.return_type != TirType::None {
        return Some(callee.return_type.clone());
    }
    None
}
