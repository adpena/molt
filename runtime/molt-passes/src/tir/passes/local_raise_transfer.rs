//! A raise its own function catches becomes a branch (HF-906 S1).
//!
//! The frontend lowers `raise v` as `RAISE v` (publish `v` as the pending
//! exception) then a check that transfers to the try's handler block `H`.
//! `H` reads the exception back (`EXCEPTION_LAST_PENDING`) and clears it. When
//! the raise and its handler are in one function, that round trip through the
//! thread record is an interpreter's: HotSpot C2 and Graal turn a throw caught
//! in the same compilation unit into a jump, and Swift's SIL passes a thrown
//! error to its handler as a block argument.
//!
//! This pass splits `H` after its pending read and clear:
//!
//! ```text
//! H(args):  try_end L; p = last_pending; clear; br H2(args, p)
//! H2(args', exc): <the rest of H, with args -> args', p -> exc>
//! ```
//!
//! Every exception edge still enters `H`. A local raise instead chains the
//! handled exception into `v.__context__` (`molt_exception_chain_context`, what
//! `molt_raise` does before it publishes), keeps its check (the chain can fail
//! with a pending error, which the handler then receives as before), closes the
//! region with `try_end L` as normal completion does, and branches to `H2` with
//! the check's handler arguments and `v`.
//!
//! Rewrite raises while their original operation positions are still valid.
//! Then split their handlers and rename all dominated uses in one traversal.
//! The original handler prefix and its new forwarding branch keep the old
//! argument and pending value names.
//!
//! Only a `v` that `molt_exception_prepare_raise` produced qualifies: that is a
//! normalized exception instance, exactly what `molt_raise` would publish.

use molt_ir::OpKind;
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::runtime_fn_generated::RuntimeFn;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;

mod builtin_kinds_generated;
mod prepare;

/// A `raise v` whose check, the block's last op, transfers to label `label`.
struct LocalRaise {
    block: BlockId,
    raise: usize,
    label: i64,
    exception: ValueId,
}

fn label_of(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

fn is_unwind_check(op: &TirOp) -> bool {
    op.opcode == OpCode::CheckException
        && matches!(
            op.attrs.get("exception_edge"),
            Some(AttrValue::Int(kind)) if *kind == ExceptionEdgeKind::Unwind as i64
        )
}

fn is_copy_of(op: &TirOp, kind: OpKind) -> bool {
    op.opcode == OpCode::Copy && op.origin == Some(kind)
}

/// The handler's pending read, when `H` opens `try_end L; p = last_pending; clear`.
fn handler_pending_read(block: &TirBlock, label: i64) -> Option<ValueId> {
    let [try_end, read, clear, ..] = block.ops.as_slice() else {
        return None;
    };
    (try_end.opcode == OpCode::TryEnd
        && label_of(try_end) == Some(label)
        && is_copy_of(read, OpKind::ExceptionLastPending)
        && read.results.len() == 1
        && is_copy_of(clear, OpKind::ExceptionClear))
    .then(|| read.results[0])
}

fn local_raises(func: &TirFunction) -> Vec<LocalRaise> {
    let mut prepared: HashSet<ValueId> = HashSet::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Call && op.runtime == Some(RuntimeFn::ExceptionPrepareRaise) {
                prepared.extend(op.results.iter().copied());
            }
        }
    }
    let mut found = Vec::new();
    for block in func.blocks.values() {
        let ops = &block.ops;
        let Some(raise) = ops.len().checked_sub(2) else {
            continue;
        };
        let (raise_op, check) = (&ops[raise], &ops[raise + 1]);
        if raise_op.opcode != OpCode::Raise || !is_unwind_check(check) {
            continue;
        }
        let ([exception], Some(label)) = (raise_op.operands.as_slice(), label_of(check)) else {
            continue;
        };
        if prepared.contains(exception) {
            found.push(LocalRaise {
                block: block.id,
                raise,
                label,
                exception: *exception,
            });
        }
    }
    found.sort_by_key(|site| (site.label, site.block.0));
    found
}

pub fn run(func: &mut TirFunction, target: &crate::tir::target_info::TargetInfo) -> PassStats {
    let mut stats = PassStats {
        name: "local_raise_transfer",
        ..Default::default()
    };
    transfer_local_raises(func, &mut stats);
    prepare::specialize(func, target, &mut stats);
    stats
}

fn transfer_local_raises(func: &mut TirFunction, stats: &mut PassStats) {
    let sites = local_raises(func);
    if sites.is_empty() {
        return;
    }
    let mut label_blocks: HashMap<i64, Option<BlockId>> = HashMap::default();
    for (&block, &label) in &func.label_id_map {
        // A label two blocks claim names no unique handler.
        label_blocks
            .entry(label)
            .and_modify(|unique| *unique = None)
            .or_insert(Some(BlockId(block)));
    }
    let mut splits = Vec::new();
    let mut continuations = HashMap::default();
    for site in sites {
        let continuation = continuations.entry(site.label).or_insert_with(|| {
            let handler = label_blocks.get(&site.label).copied().flatten()?;
            let block = func.blocks.get(&handler)?;
            let pending = handler_pending_read(block, site.label)?;
            let arity = block.args.len();
            let continuation = func.fresh_block();
            splits.push((handler, continuation, pending));
            stats.ops_added += 1;
            Some((continuation, arity))
        });
        if let Some((continuation, arity)) = *continuation {
            transfer(func, &site, continuation, arity);
            stats.ops_removed += 1;
            stats.ops_added += 2;
        }
    }
    if splits.is_empty() {
        return;
    }

    let mut replacements = HashMap::default();
    let mut moved = HashMap::default();
    for (handler, continuation, pending) in splits {
        split_handler(func, handler, continuation, pending, &mut replacements);
        moved.insert(handler, continuation);
    }
    func.remap_loop_terminators(&moved);
    for block in func.blocks.values_mut() {
        let rename = |value: &mut ValueId| {
            if let Some(&(handler, replacement)) = replacements.get(value)
                && block.id != handler
            {
                *value = replacement;
            }
        };
        for op in &mut block.ops {
            op.operands.iter_mut().for_each(rename);
        }
        block.terminator.for_each_value_mut(rename);
    }
}

/// Split a handler after its pending read. Uses below it are renamed in batch.
fn split_handler(
    func: &mut TirFunction,
    handler: BlockId,
    continuation: BlockId,
    pending: ValueId,
    replacements: &mut HashMap<ValueId, (BlockId, ValueId)>,
) {
    let old_args: Vec<TirValue> = func.blocks[&handler].args.clone();
    let mut new_args = Vec::with_capacity(old_args.len() + 1);
    for arg in &old_args {
        let fresh = func.fresh_value();
        func.value_types.insert(fresh, arg.ty.clone());
        replacements.insert(arg.id, (handler, fresh));
        new_args.push(TirValue {
            id: fresh,
            ty: arg.ty.clone(),
        });
    }
    let exception = func.fresh_value();
    let exception_ty = func
        .value_types
        .get(&pending)
        .cloned()
        .unwrap_or(TirType::DynBox);
    func.value_types.insert(exception, exception_ty.clone());
    replacements.insert(pending, (handler, exception));
    new_args.push(TirValue {
        id: exception,
        ty: exception_ty,
    });

    let block = func.blocks.get_mut(&handler).expect("handler block");
    let rest = block.ops.split_off(3);
    let terminator = std::mem::replace(
        &mut block.terminator,
        Terminator::Branch {
            target: continuation,
            args: old_args
                .iter()
                .map(|arg| arg.id)
                .chain(std::iter::once(pending))
                .collect(),
        },
    );
    func.blocks.insert(
        continuation,
        TirBlock {
            id: continuation,
            args: new_args,
            ops: rest,
            terminator,
        },
    );
}

fn transfer(func: &mut TirFunction, site: &LocalRaise, h2: BlockId, arity: usize) {
    let chained = func.fresh_value();
    func.value_types.insert(chained, TirType::DynBox);
    let block = func.blocks.get_mut(&site.block).expect("raise block");
    let handler_args = block.ops[site.raise + 1].operands.clone();
    debug_assert_eq!(
        handler_args.len(),
        arity,
        "check passes the handler's arguments"
    );
    let raise = &block.ops[site.raise];
    let mut chain = TirOp::new(OpCode::Call, vec![site.exception], vec![chained]);
    chain.runtime = Some(RuntimeFn::ExceptionChainContext);
    chain.attrs.insert(
        "s_value".into(),
        AttrValue::Str(RuntimeFn::ExceptionChainContext.symbol().to_string()),
    );
    chain.source_span = raise.source_span;
    for key in [
        crate::tir::ops::SOURCE_LINE_ATTR,
        crate::tir::ops::SOURCE_COL_ATTR,
        crate::tir::ops::SOURCE_END_COL_ATTR,
    ] {
        if let Some(value) = raise.attrs.get(key) {
            chain.attrs.insert(key.into(), value.clone());
        }
    }
    let mut try_end = TirOp::new(OpCode::TryEnd, vec![], vec![]);
    try_end
        .attrs
        .insert("value".into(), AttrValue::Int(site.label));
    block.ops[site.raise] = chain;
    // The check now observes the chain call: an error it raises originates
    // here, so the edge is an origin edge that records this frame.
    block.ops[site.raise + 1].attrs.insert(
        "exception_edge".into(),
        AttrValue::Int(ExceptionEdgeKind::Origin as i64),
    );
    block.ops.push(try_end);
    block.terminator = Terminator::Branch {
        target: h2,
        args: handler_args
            .into_iter()
            .chain(std::iter::once(site.exception))
            .collect(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_ir::ir::FunctionReturnAbi;

    #[test]
    fn splits_nested_raise_sites_before_renaming_uses() {
        let mut func = TirFunction::new(
            "nested".into(),
            vec![TirType::DynBox],
            TirType::DynBox,
            FunctionReturnAbi::Value,
        );
        let entry = func.entry_block;
        let inner = func.fresh_block();
        let outer = func.fresh_block();
        let seed = func.blocks[&entry].args[0].id;
        let inner_arg = func.fresh_value();
        let outer_arg = func.fresh_value();
        let inner_pending = func.fresh_value();
        let outer_pending = func.fresh_value();
        let first = func.fresh_value();
        let second = func.fresh_value();
        let setup = |source, result, label, argument| {
            let mut prepare = TirOp::new(OpCode::Call, vec![source, seed], vec![result]);
            prepare.runtime = Some(RuntimeFn::ExceptionPrepareRaise);
            let mut check = TirOp::new(OpCode::CheckException, vec![argument], vec![]);
            check.attrs.insert("value".into(), AttrValue::Int(label));
            check.attrs.insert(
                "exception_edge".into(),
                AttrValue::Int(ExceptionEdgeKind::Unwind as i64),
            );
            [
                prepare,
                TirOp::new(OpCode::Raise, vec![result], vec![]),
                check,
            ]
        };
        func.blocks.get_mut(&entry).unwrap().ops = setup(seed, first, 10, seed).into();
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Unwind;
        for (id, label, arg, pending) in [
            (inner, 10, inner_arg, inner_pending),
            (outer, 20, outer_arg, outer_pending),
        ] {
            func.label_id_map.insert(id.0, label);
            let mut end = TirOp::new(OpCode::TryEnd, vec![], vec![]);
            end.attrs.insert("value".into(), AttrValue::Int(label));
            let read = TirOp::new(OpCode::Copy, vec![], vec![pending])
                .with_origin(OpKind::ExceptionLastPending);
            let clear =
                TirOp::new(OpCode::Copy, vec![], vec![]).with_origin(OpKind::ExceptionClear);
            let mut ops = vec![end, read, clear];
            let terminator = if id == inner {
                ops.extend(setup(pending, second, 20, arg));
                Terminator::Unwind
            } else {
                Terminator::Return {
                    values: vec![pending],
                }
            };
            func.blocks.insert(
                id,
                TirBlock {
                    id,
                    args: vec![TirValue {
                        id: arg,
                        ty: TirType::DynBox,
                    }],
                    ops,
                    terminator,
                },
            );
        }
        for value in [
            inner_arg,
            outer_arg,
            inner_pending,
            outer_pending,
            first,
            second,
        ] {
            func.value_types.insert(value, TirType::DynBox);
        }

        let mut stats = PassStats::default();
        transfer_local_raises(&mut func, &mut stats);
        assert_eq!(stats.ops_removed, 2);
        let Terminator::Branch {
            target: inner_body,
            args: incoming,
        } = &func.blocks[&entry].terminator
        else {
            panic!("local transfer to inner handler");
        };
        assert_eq!(incoming, &[seed, first]);
        let body = &func.blocks[inner_body];
        assert_eq!(body.ops[0].operands[0], body.args[1].id);
        assert_eq!(body.ops[1].runtime, Some(RuntimeFn::ExceptionChainContext));
        let Terminator::Branch {
            target: outer_body,
            args: forwarded,
        } = &body.terminator
        else {
            panic!("local transfer to outer handler");
        };
        assert_eq!(forwarded, &[body.args[0].id, second]);
        let Terminator::Branch { target, args } = &func.blocks[&inner].terminator else {
            panic!("ordinary pending ingress");
        };
        assert_eq!(target, inner_body);
        assert_eq!(args, &[inner_arg, inner_pending]);
        let body = &func.blocks[outer_body];
        assert!(
            matches!(&body.terminator, Terminator::Return { values } if values == &[body.args[1].id])
        );
    }
}
