use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, TirOp};
use crate::tir::values::{TirValue, ValueId};

use super::exception_labels::{build_label_remap, remap_exception_label_attr};

/// The product of cloning a callee body into a caller: the block id the call's
/// predecessor half must branch into (the cloned callee entry), and the set of
/// fresh block ids that make up the cloned body (so the splicer can locate the
/// cloned `Return`-bearing blocks to rewrite into continuation branches).
pub(super) struct ClonedCallee {
    /// The fresh `BlockId` of the cloned callee's entry block. The caller's
    /// pre-call half branches here. This block has **no arguments** — the
    /// callee's parameters were bound directly to the call arguments.
    pub(super) entry: BlockId,
    /// Every fresh block id introduced by the clone, in deterministic order.
    pub(super) cloned_blocks: Vec<BlockId>,
    /// The callee `BlockId` → cloned `BlockId` map. The splicer uses it to carry
    /// the callee-side classification of each `Return` block (normal-return vs
    /// exception-exit, computed on the callee's terminator-only CFG) onto the
    /// cloned blocks when rewriting `Return`s into continuation branches.
    pub(super) block_map: HashMap<BlockId, BlockId>,
}

/// Clone an op's attribute dict while dropping the SimpleIR value-name
/// annotations (`_simple_out` and `_simple_result_N`).
///
/// Cloning a callee body remaps every `ValueId`/`BlockId` to a fresh id, but
/// these annotations are *function-local name strings* (a Python local like `x`
/// or `i`) with no id to remap — a verbatim copy carries the callee's names into
/// the caller. If a callee name collides with a caller value of a different
/// container kind, any surviving name-keyed SimpleIR consumer would resolve the
/// inlined value to the *caller's* kind — a wrong specialization, i.e. a
/// miscompile. It would likewise alias two values onto one SimpleIR slot in the
/// native TIR→SimpleIR lowering.
///
/// Dropping the names lets each inlined value fall to its unique canonical
/// (`ValueId`-derived) name, so it is classified by the authoritative
/// `ValueId`-keyed `TirType` instead. Freshly-built inlined containers keep their
/// concrete `TirType`, so the correct kind is preserved for the common case;
/// only a `DynBox`-typed inlined container loses name-keyed specialization
/// (sound — generic dispatch). LLVM `len` specialization now reads refined TIR
/// type directly, and the soundness-critical carrier `repr_by_value` is
/// `ValueId`-keyed, so neither path depends on cloned SimpleIR names.
pub(super) fn clone_attrs_without_simple_names(attrs: &AttrDict) -> AttrDict {
    attrs
        .iter()
        .filter(|(k, _)| k.as_str() != "_simple_out" && !k.starts_with("_simple_result_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Clone `callee`'s body into `caller`, minting fresh `ValueId`/`BlockId`s for
/// everything except the callee parameters, which are bound directly to
/// `arg_values` (the call's argument values, already valid in `caller`).
///
/// Returns the cloned-entry block id (which the splice's pre-call half branches
/// into) and the list of fresh block ids. The caller is responsible for actually
/// inserting the resulting blocks (they are inserted into `caller.blocks` here)
/// and for wiring the pre/cont split + rewriting cloned `Return`s — that is
/// [`splice_call_site`]'s job.
///
/// Invariants established here:
/// * The cloned entry block has **no arguments** (params bind to `arg_values`).
/// * Every callee value not a parameter gets a fresh id; uses are remapped.
/// * Cloned `Return` terminators are left *as `Return`* — the splicer rewrites
///   them into branches to the continuation (it owns the continuation id).
/// * `value_types` for cloned values transfer (remapped keys) so type facts
///   survive into the merged function.
/// * All loop metadata (`label_id_map` + `loop_roles` + `loop_pairs` +
///   `loop_break_kinds` + `loop_cond_blocks`) transfer with remapped keys.
/// * **Exception labels** are remapped to fresh caller ids. SimpleIR label ids
///   are per-function (`next_label()` resets per function), so the callee's
///   labels routinely collide numerically with the caller's. Both the cloned
///   `CheckException`/`TryStart`/`TryEnd` `"value"` attrs (transfer labels for
///   `CheckException`/`TryStart`, pairing metadata for `TryEnd`) AND the cloned
///   blocks' `label_id_map` entries are remapped through one
///   [`build_label_remap`] table so the merged function's exception-transfer
///   edges resolve to the cloned exit block (not a colliding caller block) and
///   `lower_to_simple` emits no duplicate `label N` ops.
pub(super) fn clone_function_body_with_fresh_ids(
    callee: &TirFunction,
    caller: &mut TirFunction,
    arg_values: &[ValueId],
) -> ClonedCallee {
    // Fresh exception/label remap for this clone. Allocated ABOVE the caller's
    // current max label so it cannot collide with any caller label (including the
    // fresh labels of callees already inlined into this caller — `caller` is
    // re-scanned each clone, and each clone's fresh labels were inserted into
    // `caller.label_id_map` by `transfer_loop_metadata`).
    let label_remap = build_label_remap(callee, caller);

    // Value remap: callee ValueId -> caller ValueId. Pre-seed the parameters to
    // bind directly to the call's argument values.
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::new();
    let entry = &callee.blocks[&callee.entry_block];
    debug_assert_eq!(
        entry.args.len(),
        arg_values.len(),
        "inliner: callee '{}' has {} params but call passed {} args",
        callee.name,
        entry.args.len(),
        arg_values.len()
    );
    for (param, arg) in entry.args.iter().zip(arg_values.iter()) {
        value_map.insert(param.id, *arg);
    }

    // Block remap: callee BlockId -> fresh caller BlockId. Deterministic order
    // (sorted by callee block id) so the fresh-id assignment is reproducible.
    let mut callee_block_ids: Vec<BlockId> = callee.blocks.keys().copied().collect();
    callee_block_ids.sort_by_key(|b| b.0);
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::new();
    for &bid in &callee_block_ids {
        block_map.insert(bid, caller.fresh_block());
    }

    // Mint fresh value ids for every non-parameter callee result and every
    // non-entry block argument, in a deterministic walk (blocks sorted; within a
    // block, args then ops in order).
    let fresh_for = |old: ValueId,
                     value_map: &mut HashMap<ValueId, ValueId>,
                     caller: &mut TirFunction|
     -> ValueId {
        if let Some(&existing) = value_map.get(&old) {
            return existing;
        }
        let fresh = caller.fresh_value();
        value_map.insert(old, fresh);
        fresh
    };

    for &bid in &callee_block_ids {
        let block = &callee.blocks[&bid];
        // Entry-block args are the parameters — already bound to arg_values, so
        // do NOT mint fresh ids for them. Non-entry block args get fresh ids.
        if bid != callee.entry_block {
            for arg in &block.args {
                fresh_for(arg.id, &mut value_map, caller);
            }
        }
        for op in &block.ops {
            for result in &op.results {
                fresh_for(*result, &mut value_map, caller);
            }
        }
    }

    // Helper to remap a single value (must already be in the map — every defined
    // value was assigned above; every used value is either a param, a prior
    // def, or a block arg, all of which are mapped).
    let remap = |v: ValueId, value_map: &HashMap<ValueId, ValueId>| -> ValueId {
        *value_map.get(&v).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' uses value {} with no remap (malformed SSA?)",
                callee.name, v
            )
        })
    };
    let remap_block = |b: BlockId| -> BlockId {
        *block_map.get(&b).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' references block {} with no remap",
                callee.name, b
            )
        })
    };

    // Build the cloned blocks.
    for &bid in &callee_block_ids {
        let src = &callee.blocks[&bid];
        let new_bid = remap_block(bid);

        // Cloned block arguments: empty for the entry (params bound to args),
        // remapped for every other block.
        let new_args: Vec<TirValue> = if bid == callee.entry_block {
            Vec::new()
        } else {
            src.args
                .iter()
                .map(|a| TirValue {
                    id: remap(a.id, &value_map),
                    ty: a.ty.clone(),
                })
                .collect()
        };

        // Cloned ops with operands/results remapped. The SimpleIR value-name
        // annotations are dropped (see `clone_attrs_without_simple_names`): they
        // are function-local name strings with no id to remap, so a verbatim copy
        // would carry the callee's names into the caller and collide with caller
        // values of the same name. Exception ops additionally have their handler
        // `"value"` label remapped (see `remap_exception_label_attr`) so the
        // cloned exception edge resolves to the cloned exit block, not a caller
        // block that happens to share the callee's original (per-function) label.
        let new_ops: Vec<TirOp> = src
            .ops
            .iter()
            .map(|op| {
                let mut attrs = clone_attrs_without_simple_names(&op.attrs);
                remap_exception_label_attr(op.opcode, &mut attrs, &label_remap);
                TirOp {
                    dialect: op.dialect,
                    opcode: op.opcode,
                    operands: op.operands.iter().map(|v| remap(*v, &value_map)).collect(),
                    results: op.results.iter().map(|v| remap(*v, &value_map)).collect(),
                    attrs,
                    source_span: op.source_span,
                }
            })
            .collect();

        // Cloned terminator with targets + value operands remapped. `Return`s
        // stay `Return` (the splicer rewrites them); every other terminator's
        // block targets and value args remap.
        let new_term = clone_terminator(&src.terminator, &value_map, &block_map, callee);

        caller.blocks.insert(
            new_bid,
            TirBlock {
                id: new_bid,
                args: new_args,
                ops: new_ops,
                terminator: new_term,
            },
        );
    }

    // Transfer value_types for every cloned value (remapped key). Skip params
    // (they map to caller arg values that already carry their own types).
    let entry_param_ids: HashSet<ValueId> = entry.args.iter().map(|a| a.id).collect();
    for (old, ty) in &callee.value_types {
        if entry_param_ids.contains(old) {
            continue;
        }
        if let Some(&new) = value_map.get(old) {
            caller.value_types.entry(new).or_insert_with(|| ty.clone());
        }
    }

    // Transfer loop metadata — ALL FOUR maps plus label_id_map — with remapped
    // keys (and remapped values where the value is itself a block id). Missing
    // any of these mis-describes the merged loops to LICM / BCE / the structured
    // back-conversion. `label_id_map` LABEL VALUES are remapped through
    // `label_remap` (matching the exception-op `"value"` attr remap above) so the
    // cloned blocks carry collision-free labels.
    transfer_loop_metadata(callee, caller, &block_map, &label_remap);

    ClonedCallee {
        entry: remap_block(callee.entry_block),
        cloned_blocks: callee_block_ids.iter().map(|b| remap_block(*b)).collect(),
        block_map,
    }
}

/// Clone a terminator, remapping value operands and block targets. `Return`
/// terminators are cloned verbatim (values remapped) — the splicer rewrites them
/// into branches once it owns the continuation block id.
fn clone_terminator(
    term: &Terminator,
    value_map: &HashMap<ValueId, ValueId>,
    block_map: &HashMap<BlockId, BlockId>,
    callee: &TirFunction,
) -> Terminator {
    let rv = |v: ValueId| -> ValueId {
        *value_map.get(&v).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' terminator uses value {} with no remap",
                callee.name, v
            )
        })
    };
    let rb = |b: BlockId| -> BlockId {
        *block_map.get(&b).unwrap_or_else(|| {
            panic!(
                "inliner: callee '{}' terminator targets block {} with no remap",
                callee.name, b
            )
        })
    };
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: rb(*target),
            args: args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: rv(*cond),
            then_block: rb(*then_block),
            then_args: then_args.iter().map(|v| rv(*v)).collect(),
            else_block: rb(*else_block),
            else_args: else_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: rv(*value),
            cases: cases
                .iter()
                .map(|(c, blk, args)| (*c, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(s, blk, args)| (*s, rb(*blk), args.iter().map(|v| rv(*v)).collect()))
                .collect(),
            default: rb(*default),
            default_args: default_args.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|v| rv(*v)).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

/// Transfer `label_id_map` + `loop_roles` + `loop_pairs` + `loop_break_kinds` +
/// `loop_cond_blocks` from the callee into the caller, remapping every block-id
/// key (and any block-id-valued entry) through `block_map`. `label_id_map` LABEL
/// VALUES are additionally remapped through `label_remap` (the same table that
/// rewrote the cloned exception ops' `"value"` attrs), so a cloned block's label
/// matches the cloned exception edge that targets it and cannot collide with a
/// caller label that shared the callee's original per-function label id.
fn transfer_loop_metadata(
    callee: &TirFunction,
    caller: &mut TirFunction,
    block_map: &HashMap<BlockId, BlockId>,
    label_remap: &HashMap<i64, i64>,
) {
    // label_id_map is keyed by BlockId.0 (a raw u32). Remap the key through the
    // block map AND the label value through `label_remap` so the cloned
    // exception/jump targets carry collision-free labels in the merged function.
    for (old_block_u32, label_val) in &callee.label_id_map {
        if let Some(new_bid) = block_map.get(&BlockId(*old_block_u32)) {
            let new_label = label_remap.get(label_val).copied().unwrap_or(*label_val);
            caller.label_id_map.entry(new_bid.0).or_insert(new_label);
        }
    }
    // loop_roles: BlockId -> LoopRole.
    for (old_bid, role) in &callee.loop_roles {
        if let Some(new_bid) = block_map.get(old_bid) {
            caller
                .loop_roles
                .entry(*new_bid)
                .or_insert_with(|| clone_loop_role(role));
        }
    }
    // loop_pairs: header BlockId -> end BlockId (both remap).
    for (old_header, old_end) in &callee.loop_pairs {
        if let (Some(new_header), Some(new_end)) =
            (block_map.get(old_header), block_map.get(old_end))
        {
            caller.loop_pairs.entry(*new_header).or_insert(*new_end);
        }
    }
    // loop_break_kinds: header BlockId -> LoopBreakKind.
    for (old_header, kind) in &callee.loop_break_kinds {
        if let Some(new_header) = block_map.get(old_header) {
            caller
                .loop_break_kinds
                .entry(*new_header)
                .or_insert(clone_loop_break_kind(kind));
        }
    }
    // loop_cond_blocks: header BlockId -> cond BlockId (both remap).
    for (old_header, old_cond) in &callee.loop_cond_blocks {
        if let (Some(new_header), Some(new_cond)) =
            (block_map.get(old_header), block_map.get(old_cond))
        {
            caller
                .loop_cond_blocks
                .entry(*new_header)
                .or_insert(*new_cond);
        }
    }
}

fn clone_loop_role(role: &LoopRole) -> LoopRole {
    match role {
        LoopRole::None => LoopRole::None,
        LoopRole::LoopHeader => LoopRole::LoopHeader,
        LoopRole::LoopEnd => LoopRole::LoopEnd,
    }
}

fn clone_loop_break_kind(kind: &LoopBreakKind) -> LoopBreakKind {
    *kind
}
