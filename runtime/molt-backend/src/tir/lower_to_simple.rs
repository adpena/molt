//! TIR → SimpleIR back-conversion scaffold.
//!
//! This module provides the bridge that allows TIR optimization passes to
//! benefit Cranelift and WASM backends without rewriting them.
//!
//! # Phase 1 (current)
//! Basic linearization: visits blocks in reverse-postorder, converts block
//! arguments at join points to `store_var` ops, and maps TIR terminators back
//! to SimpleIR control-flow markers.
//!
//! # Phase 2
//! Full round-trip with type annotations, phi elimination, and all OpCode
//! mappings.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;

use super::blocks::{BlockId, LoopBreakKind, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

thread_local! {
    static VALUE_NAME_OVERRIDES: RefCell<HashMap<ValueId, String>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a [`TirFunction`] back to a linear sequence of [`OpIR`] entries
/// suitable for the existing Cranelift/WASM/Luau backends.
///
/// The conversion linearises blocks in reverse-postorder (entry first, then
/// successors), emitting:
/// - A `label` op at the start of each non-entry block.
/// - `store_var` ops for block arguments at join points.
/// - One [`OpIR`] per [`TirOp`] in the block.
/// - Control-flow [`OpIR`] ops derived from the block's [`Terminator`].
///
/// When a `types` map is provided, the back-conversion propagates TIR type
/// refinement results into SimpleIR fast-path flags (`fast_int`, `fast_float`,
/// `type_hint`, `stack_eligible`), closing the optimisation gap where type
/// information was previously lost.
pub fn lower_to_simple_ir(func: &TirFunction, types: &HashMap<ValueId, TirType>) -> Vec<OpIR> {
    VALUE_NAME_OVERRIDES.with(|overrides| {
        let mut overrides = overrides.borrow_mut();
        overrides.clear();
        if let Some(entry_block) = func.blocks.get(&func.entry_block) {
            for (idx, arg) in entry_block.args.iter().enumerate() {
                if let Some(name) = func.param_names.get(idx) {
                    overrides.insert(arg.id, name.clone());
                }
            }
        }
    });

    let mut out = Vec::new();

    // Compute block visit order (reverse-postorder from entry).
    let rpo = reverse_postorder(func);

    // Build a BlockId → label_id mapping.  Blocks that have an original
    // SimpleIR label value (stored in label_id_map during lifting) reuse that
    // value so that check_exception / jump / br_if targets still match.
    // Blocks without a mapped label (e.g. blocks created by TIR optimisation
    // passes, or CFG blocks whose original label value coincides with another
    // block's fallback) are assigned fresh IDs guaranteed not to collide.
    let label_id_for_block: HashMap<BlockId, i64> = {
        let used_ids: HashSet<i64> = func.label_id_map.values().copied().collect();
        let max_used = used_ids.iter().copied().max().unwrap_or(0);
        let max_bid = func.blocks.keys().map(|b| b.0 as i64).max().unwrap_or(0);
        let mut next_fresh = max_used.max(max_bid) + 1;
        let mut mapping = HashMap::new();
        for bid in func.blocks.keys() {
            if let Some(&label_val) = func.label_id_map.get(&bid.0) {
                mapping.insert(*bid, label_val);
            } else {
                while used_ids.contains(&next_fresh) {
                    next_fresh += 1;
                }
                mapping.insert(*bid, next_fresh);
                next_fresh += 1;
            }
        }
        mapping
    };
    let block_label_id =
        |bid: &BlockId| -> i64 { label_id_for_block.get(bid).copied().unwrap_or(bid.0 as i64) };

    // Collect block argument info for all blocks so we can generate
    // `store_var` assignments at branch sites.
    // Map: (source_block, target_block) → Vec<(arg_value, param_var_name)>
    // We synthesise variable names for block arguments as "_bb<id>_arg<n>".

    // Build param-variable names for every block that has args.
    let block_param_vars: HashMap<BlockId, Vec<String>> = func
        .blocks
        .iter()
        .map(|(bid, block)| {
            let vars: Vec<String> = block
                .args
                .iter()
                .enumerate()
                .map(|(i, _)| format!("_bb{}_arg{}", bid.0, i))
                .collect();
            (*bid, vars)
        })
        .collect();

    let mut loop_region_blocks: HashSet<BlockId> = HashSet::new();
    let mut header_body_chain: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for bid in &rpo {
        let role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::LoopHeader {
            continue;
        }
        let Some(block) = func.blocks.get(bid) else {
            continue;
        };
        let break_kind = func
            .loop_break_kinds
            .get(bid)
            .copied()
            .unwrap_or(LoopBreakKind::BreakIfTrue);

        let mut region: HashSet<BlockId> = HashSet::new();
        let mut stack: Vec<BlockId> = Vec::new();
        let mut exit_block: Option<BlockId> = None;

        match &block.terminator {
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                let body_seed = match break_kind {
                    LoopBreakKind::BreakIfTrue => *else_block,
                    LoopBreakKind::BreakIfFalse => *then_block,
                };
                exit_block = Some(match break_kind {
                    LoopBreakKind::BreakIfTrue => *then_block,
                    LoopBreakKind::BreakIfFalse => *else_block,
                });
                stack.push(body_seed);
            }
            Terminator::Branch { target, .. } => {
                if let Some(cond_block) = func.blocks.get(target)
                    && let Terminator::CondBranch {
                        then_block,
                        else_block,
                        ..
                    } = &cond_block.terminator
                {
                    let body_seed = match break_kind {
                        LoopBreakKind::BreakIfTrue => *else_block,
                        LoopBreakKind::BreakIfFalse => *then_block,
                    };
                    exit_block = Some(match break_kind {
                        LoopBreakKind::BreakIfTrue => *then_block,
                        LoopBreakKind::BreakIfFalse => *else_block,
                    });
                    stack.push(*target);
                    stack.push(body_seed);
                }
            }
            _ => {}
        }

        while let Some(region_bid) = stack.pop() {
            if !region.insert(region_bid) {
                continue;
            }
            let Some(region_block) = func.blocks.get(&region_bid) else {
                continue;
            };
            for succ in block_successors(region_block) {
                if Some(succ) == exit_block || succ == *bid {
                    continue;
                }
                stack.push(succ);
            }
        }

        let chain: Vec<BlockId> = rpo
            .iter()
            .filter(|candidate| region.contains(candidate))
            .copied()
            .collect();
        loop_region_blocks.extend(chain.iter().copied());
        header_body_chain.insert(*bid, chain);
    }

    // Build the emission order: RPO but with loop exit blocks deferred
    // until after their loop region.  Without this, RPO can place the
    // exit block before the loop body, causing the native backend to
    // execute after-loop code before the loop body.
    //
    // We do NOT modify RPO.  Instead, during emission we skip exit blocks
    // when encountered in RPO and emit them immediately after loop_end.
    // This is tracked in `deferred_exits`: header_bid → exit_bid.
    let mut deferred_exits: HashMap<BlockId, BlockId> = HashMap::new();
    // Re-scan to build exit map.
    for bid in &rpo {
        let role = func.loop_roles.get(bid).cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::LoopHeader { continue; }
        let Some(block) = func.blocks.get(bid) else { continue; };
        let break_kind = func.loop_break_kinds.get(bid).copied()
            .unwrap_or(LoopBreakKind::BreakIfTrue);
        match &block.terminator {
            Terminator::CondBranch { then_block, else_block, .. } => {
                let exit = match break_kind {
                    LoopBreakKind::BreakIfTrue => *then_block,
                    LoopBreakKind::BreakIfFalse => *else_block,
                };
                deferred_exits.insert(*bid, exit);
            }
            _ => {}
        }
    }
    // Set of exit blocks that should be deferred.
    let deferred_exit_set: HashSet<BlockId> = deferred_exits.values().copied().collect();

    for bid in &rpo {
        // Skip deferred exit blocks — they'll be emitted after loop_end.
        if deferred_exit_set.contains(bid) && !loop_region_blocks.contains(bid) {
            continue;
        }

        // Skip blocks in loop regions — they're emitted inline within
        // the loop header's loop_start/loop_end region.
        let loop_role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if loop_region_blocks.contains(bid) && loop_role != super::blocks::LoopRole::LoopHeader {
            // Emit only the label for external references (check_exception
            // targets etc), but skip ops (already inlined in the header).
            if *bid != func.entry_block {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(bid)),
                    ..OpIR::default()
                });
            }
            continue;
        }

        let block = match func.blocks.get(bid) {
            Some(b) => b,
            None => continue,
        };

        // Emit loop_start before loop header blocks.
        if loop_role == super::blocks::LoopRole::LoopHeader {
            // Emit a label for the loop header so that check_exception
            // targets referencing this block's label ID remain valid.
            if *bid != func.entry_block {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(bid)),
                    ..OpIR::default()
                });
            }
            out.push(OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            });
        }

        // Loop headers fall through from the preheader into the structured
        // region; re-entering through a label would execute loop_index_start
        // on every iteration.
        if *bid != func.entry_block && loop_role != super::blocks::LoopRole::LoopHeader {
            out.push(OpIR {
                kind: "label".to_string(),
                value: Some(block_label_id(bid)),
                ..OpIR::default()
            });

            // Load block argument variables into SSA-named vars.
            if let Some(param_vars) = block_param_vars.get(bid) {
                for (i, var_name) in param_vars.iter().enumerate() {
                    if i < block.args.len() {
                        out.push(OpIR {
                            kind: "load_var".to_string(),
                            var: Some(var_name.clone()),
                            out: Some(value_var(block.args[i].id)),
                            ..OpIR::default()
                        });
                    }
                }
            }
        }

        // Helper: emit a block's ops with type annotation.
        let emit_block_ops = |block: &TirBlock, out: &mut Vec<OpIR>| {
            for op in &block.ops {
                if let Some(mut opir) = lower_op(op) {
                    annotate_type_flags(&mut opir, op, types);
                    out.push(opir);
                }
            }
        };

        // For loop headers: emit ALL blocks in the loop region as a fused
        // sequential region: loop_start → [all region ops] → loop_continue → loop_end.
        // Block args are threaded via VALUE_NAME_OVERRIDES so ops reference
        // predecessor values directly, avoiding label/jump/store_var/load_var.
        if loop_role == super::blocks::LoopRole::LoopHeader {
            // Helper: emit a block's ops inline, mapping block args to predecessor values.
            let emit_block_inline = |blk: &TirBlock, pred_args: &[ValueId], out: &mut Vec<OpIR>| {
                // Map block arg ValueIds to predecessor branch arg values.
                // Resolve value_var names BEFORE taking the mutable borrow
                // to avoid RefCell double-borrow panics.
                let resolved: Vec<(ValueId, String)> = blk
                    .args
                    .iter()
                    .enumerate()
                    .filter_map(|(i, arg)| {
                        pred_args.get(i).map(|&pred| (arg.id, value_var(pred)))
                    })
                    .collect();
                VALUE_NAME_OVERRIDES.with(|overrides| {
                    let mut map = overrides.borrow_mut();
                    for (id, name) in &resolved {
                        map.insert(*id, name.clone());
                    }
                });
                for op in &blk.ops {
                    if let Some(mut opir) = lower_op(op) {
                        annotate_type_flags(&mut opir, op, types);
                        out.push(opir);
                    }
                }
            };

            let break_kind = func
                .loop_break_kinds
                .get(bid)
                .copied()
                .unwrap_or(LoopBreakKind::BreakIfTrue);
            let mut region_chain = header_body_chain.get(bid).cloned().unwrap_or_default();
            let mut cond_data: Option<(ValueId, BlockId, Vec<ValueId>, BlockId, Vec<ValueId>)> = None;

            match &block.terminator {
                Terminator::CondBranch {
                    cond,
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                } => {
                    emit_block_ops(block, &mut out);
                    cond_data = Some((
                        *cond,
                        *then_block,
                        then_args.clone(),
                        *else_block,
                        else_args.clone(),
                    ));
                }
                Terminator::Branch { target, args } => {
                    emit_block_ops(block, &mut out);
                    if region_chain.first() == Some(target)
                        && let Some(cond_block) = func.blocks.get(target)
                        && let Terminator::CondBranch {
                            cond,
                            then_block,
                            then_args,
                            else_block,
                            else_args,
                        } = &cond_block.terminator
                    {
                        emit_block_inline(cond_block, args, &mut out);
                        region_chain.remove(0);
                        cond_data = Some((
                            *cond,
                            *then_block,
                            then_args.clone(),
                            *else_block,
                            else_args.clone(),
                        ));
                    }
                }
                _ => {
                    emit_block_ops(block, &mut out);
                }
            }

            if let Some((cond, then_block, then_args, else_block, else_args)) = cond_data {
                let (after_block, after_args, body_block, body_args) = match break_kind {
                    LoopBreakKind::BreakIfTrue => {
                        (then_block, then_args, else_block, else_args)
                    }
                    LoopBreakKind::BreakIfFalse => {
                        (else_block, else_args, then_block, then_args)
                    }
                };

                emit_block_arg_stores(after_block, &after_args, &block_param_vars, &mut out);
                out.push(OpIR {
                    kind: match break_kind {
                        LoopBreakKind::BreakIfTrue => "loop_break_if_true".to_string(),
                        LoopBreakKind::BreakIfFalse => "loop_break_if_false".to_string(),
                    },
                    args: Some(vec![value_var(cond)]),
                    ..OpIR::default()
                });

                let mut prev_args: Vec<ValueId> = body_args.clone();
                if region_chain.is_empty() {
                    if let Some(body_block_ir) = func.blocks.get(&body_block) {
                        emit_block_inline(body_block_ir, &prev_args, &mut out);
                        if let Terminator::Branch { target, args } = &body_block_ir.terminator {
                            emit_block_arg_stores(*target, args, &block_param_vars, &mut out);
                        }
                    }
                } else {
                    for region_bid in &region_chain {
                        if let Some(region_block) = func.blocks.get(region_bid) {
                            emit_block_inline(region_block, &prev_args, &mut out);
                            prev_args = match &region_block.terminator {
                                Terminator::Branch { args, .. } => args.clone(),
                                Terminator::CondBranch { then_args, else_args, .. } => {
                                    if body_block == *region_bid {
                                        body_args.clone()
                                    } else if region_chain
                                        .iter()
                                        .position(|candidate| candidate == region_bid)
                                        .and_then(|idx| region_chain.get(idx + 1))
                                        == Some(&body_block)
                                    {
                                        then_args.clone()
                                    } else {
                                        else_args.clone()
                                    }
                                }
                                _ => vec![],
                            };
                        }
                    }
                    if let Some(last_bid) = region_chain.last()
                        && let Some(last_block) = func.blocks.get(last_bid)
                        && let Terminator::Branch { target, args } = &last_block.terminator
                    {
                        emit_block_arg_stores(*target, args, &block_param_vars, &mut out);
                    }
                }

                out.push(OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                });
                out.push(OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                });

                // Emit the deferred exit block immediately after loop_end
                // so the after-loop code follows the loop body, not precedes it.
                if let Some(exit_bid) = deferred_exits.get(bid) {
                    if let Some(exit_block) = func.blocks.get(exit_bid) {
                        // Emit exit block label.
                        out.push(OpIR {
                            kind: "label".to_string(),
                            value: Some(block_label_id(exit_bid)),
                            ..OpIR::default()
                        });
                        if let Some(param_vars) = block_param_vars.get(exit_bid) {
                            for (i, var_name) in param_vars.iter().enumerate() {
                                if i < exit_block.args.len() {
                                    out.push(OpIR {
                                        kind: "load_var".to_string(),
                                        var: Some(var_name.clone()),
                                        out: Some(value_var(exit_block.args[i].id)),
                                        ..OpIR::default()
                                    });
                                }
                            }
                        }
                        // Emit exit block ops.
                        emit_block_ops(exit_block, &mut out);
                        // Emit exit block terminator.
                        let original_has_ret = func.attrs.get("_original_has_ret")
                            .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                            .unwrap_or(false);
                        let exit_role = func.loop_roles.get(exit_bid).cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        emit_terminator(
                            exit_block,
                            &block_param_vars,
                            &block_label_id,
                            &func.loop_roles,
                            &mut out,
                            original_has_ret,
                            exit_role,
                        );
                    }
                }
            } else {
                let original_has_ret = func.attrs.get("_original_has_ret")
                    .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                    .unwrap_or(false);
                emit_terminator(
                    block,
                    &block_param_vars,
                    &block_label_id,
                    &func.loop_roles,
                    &mut out,
                    original_has_ret,
                    loop_role,
                );
            }
        } else {
            // Non-loop block: emit ops normally.
            emit_block_ops(block, &mut out);
            // Non-loop block: emit terminator normally.
            let original_has_ret = func.attrs.get("_original_has_ret")
                .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                .unwrap_or(false);
            emit_terminator(
                block,
                &block_param_vars,
                &block_label_id,
                &func.loop_roles,
                &mut out,
                original_has_ret,
                loop_role,
            );
        }
    }

    VALUE_NAME_OVERRIDES.with(|overrides| overrides.borrow_mut().clear());

    out
}

/// Validate that every label referenced by jump/br_if/check_exception exists
/// as a label op in the output.  Returns false if any reference is dangling.
pub fn validate_labels(ops: &[crate::ir::OpIR]) -> bool {
    let mut defined_labels: HashSet<i64> = HashSet::new();
    let mut referenced_labels: HashSet<i64> = HashSet::new();
    for op in ops {
        match op.kind.as_str() {
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    defined_labels.insert(id);
                }
            }
            "jump" | "br_if" | "check_exception" => {
                if let Some(id) = op.value {
                    referenced_labels.insert(id);
                }
            }
            _ => {}
        }
    }
    referenced_labels.is_subset(&defined_labels)
}

// ---------------------------------------------------------------------------
// Op lowering
// ---------------------------------------------------------------------------

/// Convert a single TirOp to an OpIR. Returns None for ops that are
/// dialect-internal and have no SimpleIR equivalent (yet).
fn lower_op(op: &TirOp) -> Option<OpIR> {
    // Map result (if any) to output variable.
    let out_var = op.results.first().map(|v| value_var(*v));

    match op.opcode {
        // Constants.
        OpCode::ConstInt => Some(OpIR {
            kind: "const".to_string(),
            value: attr_int(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstFloat => Some(OpIR {
            kind: "const_float".to_string(),
            f_value: attr_float(&op.attrs, "f_value").or_else(|| attr_float(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstStr => Some(OpIR {
            kind: "const_str".to_string(),
            s_value: attr_str(&op.attrs, "s_value").or_else(|| attr_str(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBool => Some(OpIR {
            kind: "const_bool".to_string(),
            // The SSA lift stores the value as AttrValue::Int (from OpIR.value),
            // while SCCP-generated const_bool ops store it as AttrValue::Bool.
            // Handle both representations to avoid silently converting true→false.
            value: Some(match op.attrs.get("value") {
                Some(AttrValue::Bool(b)) => {
                    if *b {
                        1
                    } else {
                        0
                    }
                }
                Some(AttrValue::Int(i)) => {
                    if *i != 0 {
                        1
                    } else {
                        0
                    }
                }
                _ => 0,
            }),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstNone => Some(OpIR {
            kind: "const_none".to_string(),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBytes => Some(OpIR {
            kind: "const_bytes".to_string(),
            bytes: attr_bytes(&op.attrs, "bytes").or_else(|| attr_bytes(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),

        // Arithmetic.
        OpCode::Add => Some(binary_op("add", op, out_var)),
        OpCode::Sub => Some(binary_op("sub", op, out_var)),
        OpCode::Mul => Some(binary_op("mul", op, out_var)),
        OpCode::InplaceAdd => Some(binary_op("inplace_add", op, out_var)),
        OpCode::InplaceSub => Some(binary_op("inplace_sub", op, out_var)),
        OpCode::InplaceMul => Some(binary_op("inplace_mul", op, out_var)),
        OpCode::Div => Some(binary_op("div", op, out_var)),
        OpCode::FloorDiv => Some(binary_op("floor_div", op, out_var)),
        OpCode::Mod => Some(binary_op("mod", op, out_var)),
        OpCode::Pow => Some(binary_op("pow", op, out_var)),
        OpCode::Neg => Some(unary_op("neg", op, out_var)),
        OpCode::Pos => Some(unary_op("pos", op, out_var)),

        // Comparison.
        OpCode::Eq => Some(binary_op("eq", op, out_var)),
        OpCode::Ne => Some(binary_op("ne", op, out_var)),
        OpCode::Lt => Some(binary_op("lt", op, out_var)),
        OpCode::Le => Some(binary_op("le", op, out_var)),
        OpCode::Gt => Some(binary_op("gt", op, out_var)),
        OpCode::Ge => Some(binary_op("ge", op, out_var)),
        OpCode::Is => Some(binary_op("is", op, out_var)),
        OpCode::IsNot => Some(binary_op("is_not", op, out_var)),
        OpCode::In => Some(binary_op("in", op, out_var)),
        OpCode::NotIn => Some(binary_op("not_in", op, out_var)),

        // Bitwise.
        OpCode::BitAnd => Some(binary_op("bit_and", op, out_var)),
        OpCode::BitOr => Some(binary_op("bit_or", op, out_var)),
        OpCode::BitXor => Some(binary_op("bit_xor", op, out_var)),
        OpCode::BitNot => Some(unary_op("bit_not", op, out_var)),
        OpCode::Shl => Some(binary_op("shl", op, out_var)),
        OpCode::Shr => Some(binary_op("shr", op, out_var)),

        // Boolean.
        OpCode::And => Some(binary_op("and", op, out_var)),
        OpCode::Or => Some(binary_op("or", op, out_var)),
        OpCode::Not => Some(unary_op("not", op, out_var)),

        // Memory.
        OpCode::LoadAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "get_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out: out_var,
                ic_index: attr_int(&op.attrs, "ic_index"),
                ..OpIR::default()
            })
        }
        OpCode::StoreAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "set_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out: out_var,
                ..OpIR::default()
            })
        }
        OpCode::Index => {
            let kind = attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "index".to_string());
            Some(binary_op(&kind, op, out_var))
        }
        OpCode::StoreIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "store_index".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out: out_var,
                ..OpIR::default()
            })
        }

        // Call — s_value holds the target function name, value holds the code_id.
        // Recover the original SimpleIR kind (call_func, call_indirect, etc.)
        // if it was preserved during the SSA lift.
        OpCode::Call => {
            let kind = attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "call".to_string());
            Some(OpIR {
                kind,
                s_value: attr_str(&op.attrs, "s_value"),
                args: Some(operand_args(op)),
                out: out_var,
                value: attr_int(&op.attrs, "value"),
                ..OpIR::default()
            })
        }
        OpCode::CallMethod => Some(OpIR {
            kind: "call_method".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallBuiltin => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "call_builtin".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name"),
                out: out_var,
                ..OpIR::default()
            })
        }

        // Box/unbox — no-ops at SimpleIR level (type info discarded).
        OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard => {
            if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Copy: either a genuine copy_var or a passthrough for an unknown op
        // whose original kind was preserved in attrs.
        OpCode::Copy => {
            if let Some(original_kind) = attr_str(&op.attrs, "_original_kind") {
                if original_kind == "unpack_sequence" {
                    let mut args = operand_args(op);
                    args.extend(op.results.iter().map(|v| value_var(*v)));
                    return Some(OpIR {
                        kind: original_kind,
                        args: Some(args),
                        value: attr_int(&op.attrs, "value"),
                        f_value: attr_float(&op.attrs, "f_value"),
                        s_value: attr_str(&op.attrs, "s_value"),
                        bytes: attr_bytes(&op.attrs, "bytes"),
                        var: attr_str(&op.attrs, "_var"),
                        fast_int: attr_bool(&op.attrs, "_fast_int"),
                        fast_float: attr_bool(&op.attrs, "_fast_float"),
                        type_hint: attr_str(&op.attrs, "_type_hint"),
                        task_kind: attr_str(&op.attrs, "task_kind"),
                        container_type: attr_str(&op.attrs, "container_type"),
                        ic_index: attr_int(&op.attrs, "ic_index"),
                        raw_int: attr_bool(&op.attrs, "raw_int"),
                        ..OpIR::default()
                    });
                }
                // Passthrough: reconstruct the original SimpleIR op with all fields.
                Some(OpIR {
                    kind: original_kind,
                    args: if op.operands.is_empty() {
                        None
                    } else {
                        Some(operand_args(op))
                    },
                    out: out_var,
                    value: attr_int(&op.attrs, "value"),
                    f_value: attr_float(&op.attrs, "f_value"),
                    s_value: attr_str(&op.attrs, "s_value"),
                    bytes: attr_bytes(&op.attrs, "bytes"),
                    var: attr_str(&op.attrs, "_var"),
                    fast_int: attr_bool(&op.attrs, "_fast_int"),
                    fast_float: attr_bool(&op.attrs, "_fast_float"),
                    type_hint: attr_str(&op.attrs, "_type_hint"),
                    task_kind: attr_str(&op.attrs, "task_kind"),
                    container_type: attr_str(&op.attrs, "container_type"),
                    ic_index: attr_int(&op.attrs, "ic_index"),
                    raw_int: attr_bool(&op.attrs, "raw_int"),
                    ..OpIR::default()
                })
            } else if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Build containers.
        OpCode::BuildList => Some(OpIR {
            kind: "build_list".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildDict => Some(OpIR {
            kind: "build_dict".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildTuple => Some(OpIR {
            kind: "build_tuple".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSet => Some(OpIR {
            kind: "build_set".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSlice => Some(OpIR {
            kind: "build_slice".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Iteration.
        OpCode::GetIter => Some(unary_op("get_iter", op, out_var)),
        OpCode::IterNext => Some(unary_op("iter_next", op, out_var)),
        OpCode::IterNextUnboxed => {
            // Emit as iter_next_unboxed with two output vars:
            // results[0] = value, results[1] = done_flag.
            let val_var = op.results.first().map(|v| value_var(*v));
            let done_var = op.results.get(1).map(|v| value_var(*v));
            Some(OpIR {
                kind: "iter_next_unboxed".to_string(),
                args: Some(operand_args(op)),
                out: done_var,
                var: val_var,
                ..OpIR::default()
            })
        }
        OpCode::ForIter => Some(OpIR {
            kind: "for_iter".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Generator.
        OpCode::Yield => Some(OpIR {
            kind: "yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::YieldFrom => Some(OpIR {
            kind: "yield_from".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Exception.
        OpCode::Raise => Some(OpIR {
            kind: "raise".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::CheckException => Some(OpIR {
            kind: "check_exception".to_string(),
            // Emit with None args (matching the original structured IR format).
            // The Cranelift backend manages live-value state implicitly from
            // the structured control flow context. Emitting the TIR operands
            // (which are all block-argument values captured at exception
            // boundaries) causes the backend to generate incorrect exception
            // handling state with inflated argument lists.
            args: None,
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::TryStart => Some(OpIR {
            kind: "try_start".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::TryEnd => Some(OpIR {
            kind: "try_end".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateBlockStart => Some(OpIR {
            kind: "state_block_start".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateBlockEnd => Some(OpIR {
            kind: "state_block_end".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),

        // Import.
        OpCode::Import => Some(OpIR {
            kind: "import".to_string(),
            s_value: attr_str(&op.attrs, "module"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ImportFrom => Some(OpIR {
            kind: "import_from".to_string(),
            s_value: attr_str(&op.attrs, "name"),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Refcount and allocation — preserve for native backend.
        OpCode::IncRef => Some(OpIR {
            kind: "inc_ref".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::DecRef => Some(OpIR {
            kind: "dec_ref".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::Alloc => Some(OpIR {
            kind: "alloc".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            s_value: attr_str(&op.attrs, "s_value"),
            ..OpIR::default()
        }),
        OpCode::StackAlloc => Some(OpIR {
            kind: "stack_alloc".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::Free => Some(OpIR {
            kind: "free".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),

        // SCF ops — handled separately via terminators in Phase 2.
        OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield => None,

        // Deopt — emit a hint but not critical.
        OpCode::Deopt => Some(OpIR {
            kind: "deopt".to_string(),
            ..OpIR::default()
        }),

        // Remaining attribute ops.
        OpCode::DelAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "del_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                ..OpIR::default()
            })
        }
        OpCode::DelIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "del_index".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                ..OpIR::default()
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Terminator emission
// ---------------------------------------------------------------------------

fn emit_terminator(
    block: &TirBlock,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    loop_roles: &HashMap<BlockId, super::blocks::LoopRole>,
    out: &mut Vec<OpIR>,
    original_has_ret: bool,
    loop_role: super::blocks::LoopRole,
) {
    match &block.terminator {
        Terminator::Return { values } => {
            if values.is_empty() {
                if original_has_ret {
                    let ret_name = format!("_ret_none_{}", out.len());
                    out.push(OpIR {
                        kind: "const_none".to_string(),
                        out: Some(ret_name.clone()),
                        ..OpIR::default()
                    });
                    out.push(OpIR {
                        kind: "ret".to_string(),
                        var: Some(ret_name.clone()),
                        args: Some(vec![ret_name]),
                        ..OpIR::default()
                    });
                } else {
                    out.push(OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    });
                }
            } else {
                // The native backend reads the return value from `op.var`,
                // not from `op.args`.  Set both for compatibility.
                out.push(OpIR {
                    kind: "ret".to_string(),
                    var: Some(value_var(values[0])),
                    args: Some(values.iter().map(|v| value_var(*v)).collect()),
                    ..OpIR::default()
                });
            }
        }

        Terminator::Branch { target, args } => {
            if loop_role == super::blocks::LoopRole::LoopEnd {
                // Loop back-edge: emit loop_continue + loop_end instead
                // of a plain jump.  The native backend uses these markers
                // to construct the Cranelift loop back-edge.
                emit_block_arg_stores(*target, args, block_param_vars, out);
                out.push(OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                });
                out.push(OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                });
            } else if loop_roles.get(target).cloned()
                == Some(super::blocks::LoopRole::LoopHeader)
            {
                // Structured loop entry falls through from the preheader into
                // loop_start. The header label exists only for external
                // references such as check_exception targets; the preheader
                // itself must not jump to it or re-enter above loop setup.
                emit_block_arg_stores(*target, args, block_param_vars, out);
            } else {
                // If the block ends with a check_exception op, the native
                // backend handles the fallthrough implicitly — suppress the
                // jump so the next block's ops follow sequentially.  This
                // prevents TIR block boundaries from fragmenting loop bodies
                // with spurious jump/label pairs.
                let last_op_is_check_exception = block.ops.last()
                    .map(|op| op.opcode == OpCode::CheckException)
                    .unwrap_or(false);
                emit_block_arg_stores(*target, args, block_param_vars, out);
                if !last_op_is_check_exception {
                    out.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(block_label_id(target)),
                        ..OpIR::default()
                    });
                }
            }
        }

        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            if loop_role == super::blocks::LoopRole::LoopHeader {
                // Loop header conditional: the "then" branch exits the
                // loop (break), the "else" branch continues into the body.
                // Store then-args so the after-loop block gets correct values
                // when the break is taken.
                emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                // Emit loop_break_if_true which the native backend uses
                // to construct the loop exit branch.
                out.push(OpIR {
                    kind: "loop_break_if_true".to_string(),
                    args: Some(vec![value_var(*cond)]),
                    ..OpIR::default()
                });
                // Fall through to body — store else-args for the body block.
                emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
            } else {
                // Generic conditional branch.
                emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "br_if".to_string(),
                    args: Some(vec![value_var(*cond)]),
                    value: Some(block_label_id(then_block)),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(else_block)),
                    ..OpIR::default()
                });
            }
        }

        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            // Emit a chain of br_if checks for each case, then jump to default.
            for (case_val, target, case_args) in cases {
                out.push(OpIR {
                    kind: "switch_case".to_string(),
                    args: Some(vec![value_var(*value)]),
                    value: Some(*case_val),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*target, case_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(target)),
                    ..OpIR::default()
                });
            }
            emit_block_arg_stores(*default, default_args, block_param_vars, out);
            out.push(OpIR {
                kind: "jump".to_string(),
                value: Some(block_label_id(default)),
                ..OpIR::default()
            });
        }

        Terminator::Unreachable => {
            out.push(OpIR {
                kind: "unreachable".to_string(),
                ..OpIR::default()
            });
        }
    }
}

/// Emit `store_var` ops to pass `values` to the target block's arg variables.
fn emit_block_arg_stores(
    target: BlockId,
    values: &[ValueId],
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    if values.is_empty() {
        return;
    }
    if let Some(param_vars) = block_param_vars.get(&target) {
        for (i, val) in values.iter().enumerate() {
            if let Some(var_name) = param_vars.get(i) {
                out.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(var_name.clone()),
                    args: Some(vec![value_var(*val)]),
                    ..OpIR::default()
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RPO traversal
// ---------------------------------------------------------------------------

fn reverse_postorder(func: &TirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut postorder: Vec<BlockId> = Vec::new();
    let mut stack: Vec<(BlockId, bool)> = vec![(func.entry_block, false)];

    while let Some((bid, processed)) = stack.pop() {
        if processed {
            postorder.push(bid);
            continue;
        }
        if visited.contains(&bid) {
            continue;
        }
        visited.insert(bid);
        stack.push((bid, true));

        if let Some(block) = func.blocks.get(&bid) {
            // Push successors in reverse order for correct DFS.
            let succs = successors_of(block);
            for succ in succs.into_iter().rev() {
                if !visited.contains(&succ) {
                    stack.push((succ, false));
                }
            }
        }
    }

    postorder.reverse();

    // Append any blocks not reachable via normal control flow (e.g. exception
    // handler blocks only reachable via check_exception implicit edges).
    // These must still appear in the output so the native backend can create
    // state_blocks for their labels.
    if visited.len() < func.blocks.len() {
        let mut unreachable: Vec<BlockId> = func
            .blocks
            .keys()
            .filter(|bid| !visited.contains(bid))
            .copied()
            .collect();
        // Sort for deterministic output.
        unreachable.sort_by_key(|bid| bid.0);
        postorder.extend(unreachable);
    }

    postorder
}

fn successors_of(block: &TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut succs = vec![*default];
            for (_, target, _) in cases {
                succs.push(*target);
            }
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

// ---------------------------------------------------------------------------
// Type annotation propagation
// ---------------------------------------------------------------------------

/// Annotate a SimpleIR [`OpIR`] with fast-path flags derived from TIR type
/// refinement results.  This is the critical bridge that makes TIR type
/// analysis visible to downstream backends (Cranelift, WASM, Luau).
fn annotate_type_flags(opir: &mut OpIR, tir_op: &TirOp, types: &HashMap<ValueId, TirType>) {
    // If the op already has type metadata from the original IR (preserved
    // through the passthrough path), respect it — the original frontend
    // annotation is authoritative for ops the type refiner doesn't understand
    // (iter, list_new, etc.).  Only apply type refinement when there's no
    // existing annotation or when the op is a known arithmetic/comparison op.
    let has_original_hint = opir.type_hint.is_some()
        || opir.fast_int.is_some()
        || opir.fast_float.is_some();

    // Only apply TIR type refinement to ops where the refiner's inference is
    // trustworthy.  For passthrough ops (iter, iter_next, list_new, etc.) the
    // refiner may incorrectly infer I64 for results that are actually tuples
    // or lists.  Restrict refinement to known arithmetic/comparison/const ops.
    let is_refinable = matches!(
        tir_op.opcode,
        OpCode::Add | OpCode::Sub | OpCode::Mul | OpCode::Div
        | OpCode::FloorDiv | OpCode::Mod | OpCode::Pow | OpCode::Neg | OpCode::Pos
        | OpCode::InplaceAdd | OpCode::InplaceSub | OpCode::InplaceMul
        | OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge
        | OpCode::ConstInt | OpCode::ConstFloat | OpCode::ConstBool
        | OpCode::BoxVal | OpCode::UnboxVal
        | OpCode::Not | OpCode::And | OpCode::Or
        | OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor
        | OpCode::Shl | OpCode::Shr
    );

    // Look up the type of the first result value (most ops have 0 or 1 result).
    if !has_original_hint && is_refinable {
        if let Some(&result_id) = tir_op.results.first() {
            match types.get(&result_id) {
                Some(TirType::I64) => {
                    opir.fast_int = Some(true);
                }
                Some(TirType::F64) => {
                    opir.fast_float = Some(true);
                }
                Some(ty @ TirType::Bool)
                | Some(ty @ TirType::Str)
                | Some(ty @ TirType::Bytes)
                | Some(ty @ TirType::List(_))
                | Some(ty @ TirType::Dict(_, _))
                | Some(ty @ TirType::Set(_))
                | Some(ty @ TirType::Tuple(_))
                | Some(ty @ TirType::BigInt) => {
                    opir.type_hint = Some(type_to_hint_string(ty));
                }
                // DynBox, Never, Union, Box, Func, Ptr — no hint to propagate.
                _ => {}
            }
        }
    }

    // Propagate StackAlloc: if the TIR op is StackAlloc, mark the SimpleIR op
    // so the native backend can emit stack allocation instead of heap allocation.
    if tir_op.opcode == OpCode::StackAlloc {
        opir.stack_eligible = Some(true);
    }

    // Preserve original fast_int / fast_float / type_hint from the input IR
    // when the type refiner did not produce a more specific type.  This ensures
    // the round-trip is lossless even when type refinement yields DynBox.
    if opir.fast_int.is_none() && attr_bool(&tir_op.attrs, "_fast_int") == Some(true) {
        opir.fast_int = Some(true);
    }
    if opir.fast_float.is_none() && attr_bool(&tir_op.attrs, "_fast_float") == Some(true) {
        opir.fast_float = Some(true);
    }
    if opir.type_hint.is_none()
        && let Some(th) = attr_str(&tir_op.attrs, "_type_hint")
    {
        opir.type_hint = Some(th);
    }
}

/// Convert a TIR type to a human-readable hint string for the backend.
/// Collect all successor BlockIds from a block's terminator.
fn block_successors(block: &TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch { then_block, else_block, .. } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut succs: Vec<BlockId> = cases.iter().map(|c| c.1).collect();
            succs.push(*default);
            succs
        }
        _ => vec![],
    }
}

fn type_to_hint_string(ty: &TirType) -> String {
    match ty {
        TirType::I64 => "int".to_string(),
        TirType::F64 => "float".to_string(),
        TirType::Bool => "bool".to_string(),
        TirType::Str => "str".to_string(),
        TirType::Bytes => "bytes".to_string(),
        TirType::None => "none".to_string(),
        TirType::List(_) => "list".to_string(),
        TirType::Dict(_, _) => "dict".to_string(),
        TirType::Set(_) => "set".to_string(),
        TirType::Tuple(_) => "tuple".to_string(),
        TirType::BigInt => "bigint".to_string(),
        TirType::Func(_) => "func".to_string(),
        TirType::Ptr(_) => "ptr".to_string(),
        TirType::Box(inner) => format!("box<{}>", type_to_hint_string(inner)),
        TirType::Union(members) => {
            let parts: Vec<String> = members.iter().map(type_to_hint_string).collect();
            format!("union<{}>", parts.join(","))
        }
        TirType::DynBox => "any".to_string(),
        TirType::Never => "never".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

/// Synthesise a SimpleIR variable name from a ValueId.
fn value_var(id: ValueId) -> String {
    VALUE_NAME_OVERRIDES.with(|overrides| {
        overrides
            .borrow()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("_v{}", id.0))
    })
}

fn binary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

fn unary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

fn operand_args(op: &TirOp) -> Vec<String> {
    op.operands.iter().map(|v| value_var(*v)).collect()
}

fn attr_int(attrs: &super::ops::AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key) {
        Some(AttrValue::Int(i)) => Some(*i),
        _ => None,
    }
}

fn attr_float(attrs: &super::ops::AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key) {
        Some(AttrValue::Float(f)) => Some(*f),
        _ => None,
    }
}

fn attr_str(attrs: &super::ops::AttrDict, key: &str) -> Option<String> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn attr_bool(attrs: &super::ops::AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key) {
        Some(AttrValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn attr_bytes(attrs: &super::ops::AttrDict, key: &str) -> Option<Vec<u8>> {
    match attrs.get(key) {
        Some(AttrValue::Bytes(b)) => Some(b.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn add_function() -> TirFunction {
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

        let result = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        func
    }

    #[test]
    fn linearize_simple_function_compiles() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        // Must produce at least one op.
        assert!(!ops.is_empty(), "expected non-empty ops for add function");
    }

    #[test]
    fn linearize_emits_return() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let has_ret = ops.iter().any(|o| o.kind == "ret" || o.kind == "ret_void");
        assert!(has_ret, "expected a return op, got: {:?}", ops);
    }

    #[test]
    fn empty_tir_return_preserves_original_ret_signature() {
        let mut func = TirFunction::new("ret_none".into(), vec![], TirType::DynBox);
        func.attrs
            .insert("_original_has_ret".into(), AttrValue::Bool(true));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return { values: vec![] };

        let ops = lower_to_simple_ir(&func, &HashMap::new());

        assert!(
            !ops.iter().any(|op| op.kind == "ret_void"),
            "roundtrip must not downgrade original `ret` to `ret_void`: {ops:?}"
        );
        let ret_op = ops
            .iter()
            .find(|op| op.kind == "ret")
            .expect("roundtrip must synthesize `ret None`");
        let none_op = ops
            .iter()
            .find(|op| op.kind == "const_none")
            .expect("roundtrip must synthesize a const_none return value");
        let none_name = none_op
            .out
            .as_deref()
            .expect("const_none must define an output var");
        assert_eq!(
            ret_op.var.as_deref(),
            Some(none_name),
            "ret must use the synthesized None value"
        );
        assert_eq!(
            ret_op.args.as_ref().and_then(|args| args.first()).map(String::as_str),
            Some(none_name),
            "ret args must also reference the synthesized None value"
        );
    }

    #[test]
    fn ret_op_has_var_set() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let ret_op = ops
            .iter()
            .find(|o| o.kind == "ret")
            .expect("expected a ret op");
        assert!(
            ret_op.var.is_some(),
            "ret op must have `var` set for the native backend; got: {:?}",
            ret_op
        );
    }

    /// Integration test: full TIR round-trip preserves `ret` var field.
    /// This simulates the frontend's `def add(a,b): return a+b` IR.
    #[test]
    fn tir_round_trip_preserves_ret_var() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;
        use crate::tir::type_refine;

        let func_ir = FunctionIR {
            name: "add".into(),
            params: vec!["a".into(), "b".into()],
            ops: vec![
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["a".into(), "b".into()]),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("v0".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
        };

        let mut tir_func = lower_to_tir(&func_ir);
        type_refine::refine_types(&mut tir_func);
        let type_map = type_refine::extract_type_map(&tir_func);
        let round_tripped = lower_to_simple_ir(&tir_func, &type_map);

        let ret_op = round_tripped
            .iter()
            .find(|o| o.kind == "ret")
            .expect("TIR round-trip must preserve the ret op");
        assert!(
            ret_op.var.is_some(),
            "TIR round-trip must set `var` on ret op for native backend; got: {:?}",
            ret_op,
        );
    }

    #[test]
    fn linearize_emits_add_op() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let has_add = ops.iter().any(|o| o.kind == "add");
        assert!(has_add, "expected an 'add' op, got: {:?}", ops);
    }

    #[test]
    fn linearize_multi_block_emits_labels() {
        // Build: func @branch(bool) -> i64 with two successor blocks.
        let mut func = TirFunction::new("branch".into(), vec![TirType::Bool], TirType::I64);

        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };

        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(1));
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: attrs1,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v1] },
            },
        );

        let mut attrs2 = AttrDict::new();
        attrs2.insert("value".into(), AttrValue::Int(0));
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: attrs2,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v2] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let label_count = ops.iter().filter(|o| o.kind == "label").count();
        // Should have labels for bb1 and bb2.
        assert!(
            label_count >= 2,
            "expected >=2 labels for multi-block function, got {}: {:?}",
            label_count,
            ops
        );
    }

    #[test]
    fn value_var_naming() {
        assert_eq!(value_var(ValueId(0)), "_v0");
        assert_eq!(value_var(ValueId(42)), "_v42");
    }

    /// Verify that TIR type refinement results are propagated back to SimpleIR
    /// fast-path flags.  This is the critical test for the type-propagation fix:
    /// ops that TIR proves are I64 must have `fast_int = Some(true)` in the
    /// output SimpleIR.
    #[test]
    fn type_propagation_sets_fast_int_on_arithmetic() {
        use crate::tir::type_refine::{extract_type_map, refine_types};

        // Build: func @add_ints() -> I64
        //   %0 = const_int 10
        //   %1 = const_int 20
        //   %2 = add %0, %1
        //   return %2
        let mut func = TirFunction::new("add_ints".into(), vec![], TirType::I64);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let mut attrs0 = AttrDict::new();
        attrs0.insert("value".into(), AttrValue::Int(10));
        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(20));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v0],
            attrs: attrs0,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v1],
            attrs: attrs1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![v0, v1],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };

        // Run type refinement.
        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        // Verify the type map has I64 for all three values.
        assert_eq!(type_map.get(&v0), Some(&TirType::I64), "v0 should be I64");
        assert_eq!(type_map.get(&v1), Some(&TirType::I64), "v1 should be I64");
        assert_eq!(
            type_map.get(&v2),
            Some(&TirType::I64),
            "v2 should be I64 (add of two I64s)"
        );

        // Lower to SimpleIR with the type map.
        let ops = lower_to_simple_ir(&func, &type_map);

        // The 'add' op in the output must have fast_int = Some(true).
        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty(), "expected an 'add' op in output");
        for add_op in &add_ops {
            assert_eq!(
                add_op.fast_int,
                Some(true),
                "add op should have fast_int=true after type propagation, got: {:?}",
                add_op
            );
        }

        // The const ops must also have fast_int = Some(true).
        let const_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "const").collect();
        assert!(const_ops.len() >= 2, "expected at least 2 const ops");
        for const_op in &const_ops {
            assert_eq!(
                const_op.fast_int,
                Some(true),
                "const int op should have fast_int=true, got: {:?}",
                const_op
            );
        }
    }

    /// Verify that F64 types produce fast_float flags.
    #[test]
    fn type_propagation_sets_fast_float_on_float_arithmetic() {
        use crate::tir::type_refine::{extract_type_map, refine_types};

        let mut func = TirFunction::new("add_floats".into(), vec![], TirType::F64);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let mut attrs0 = AttrDict::new();
        attrs0.insert("f_value".into(), AttrValue::Float(1.5));
        let mut attrs1 = AttrDict::new();
        attrs1.insert("f_value".into(), AttrValue::Float(2.5));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![v0],
            attrs: attrs0,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![v1],
            attrs: attrs1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![v0, v1],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);
        let ops = lower_to_simple_ir(&func, &type_map);

        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty());
        for add_op in &add_ops {
            assert_eq!(
                add_op.fast_float,
                Some(true),
                "float add should have fast_float=true, got: {:?}",
                add_op
            );
        }
    }

    /// Verify that comparison ops get type_hint="bool" (not fast_int/fast_float).
    #[test]
    fn type_propagation_sets_type_hint_for_bool() {
        use crate::tir::type_refine::{extract_type_map, refine_types};

        let mut func = TirFunction::new("cmp".into(), vec![], TirType::Bool);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let mut attrs0 = AttrDict::new();
        attrs0.insert("value".into(), AttrValue::Int(1));
        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(2));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v0],
            attrs: attrs0,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v1],
            attrs: attrs1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Eq,
            operands: vec![v0, v1],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);
        let ops = lower_to_simple_ir(&func, &type_map);

        let eq_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "eq").collect();
        assert!(!eq_ops.is_empty());
        for eq_op in &eq_ops {
            assert_eq!(
                eq_op.type_hint.as_deref(),
                Some("bool"),
                "eq op should have type_hint='bool', got: {:?}",
                eq_op
            );
            // Bool should NOT set fast_int or fast_float.
            assert!(eq_op.fast_int.is_none(), "bool op should not have fast_int");
            assert!(
                eq_op.fast_float.is_none(),
                "bool op should not have fast_float"
            );
        }
    }

    /// Verify that no type map (empty) means no flags are set.
    #[test]
    fn empty_type_map_sets_no_flags() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty());
        for add_op in &add_ops {
            assert!(
                add_op.fast_int.is_none(),
                "empty type map should not set fast_int"
            );
            assert!(
                add_op.fast_float.is_none(),
                "empty type map should not set fast_float"
            );
            assert!(
                add_op.type_hint.is_none(),
                "empty type map should not set type_hint"
            );
        }
    }

    #[test]
    fn tir_round_trip_preserves_guarded_field_set_offset() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "guarded_store".into(),
            params: vec![
                "obj".into(),
                "class_bits".into(),
                "expected".into(),
                "value".into(),
            ],
            ops: vec![OpIR {
                kind: "guarded_field_set".into(),
                args: Some(vec![
                    "obj".into(),
                    "class_bits".into(),
                    "expected".into(),
                    "value".into(),
                ]),
                s_value: Some("x".into()),
                value: Some(24),
                ..OpIR::default()
            }],
            param_types: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());
        let store_op = round_tripped
            .iter()
            .find(|op| op.kind == "guarded_field_set")
            .expect("expected guarded_field_set after TIR round-trip");

        assert_eq!(store_op.s_value.as_deref(), Some("x"));
        assert_eq!(store_op.value, Some(24));
    }

    #[test]
    fn tir_round_trip_preserves_guarded_field_get_offset() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "guarded_load".into(),
            params: vec!["obj".into(), "class_bits".into(), "expected".into()],
            ops: vec![OpIR {
                kind: "guarded_field_get".into(),
                args: Some(vec!["obj".into(), "class_bits".into(), "expected".into()]),
                s_value: Some("x".into()),
                value: Some(24),
                out: Some("loaded".into()),
                ..OpIR::default()
            }],
            param_types: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());
        let load_op = round_tripped
            .iter()
            .find(|op| op.kind == "guarded_field_get")
            .expect("expected guarded_field_get after TIR round-trip");

        assert_eq!(load_op.s_value.as_deref(), Some("x"));
        assert_eq!(load_op.value, Some(24));
        assert!(
            load_op.out.is_some(),
            "guarded_field_get must preserve an output"
        );
    }

    /// Integration test: TIR round-trip preserves loop markers (loop_start, loop_end).
    /// Simulates a `while i < 3: total += i; i += 1` pattern.
    #[test]
    fn tir_round_trip_preserves_loop_markers() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "while_loop".into(),
            params: vec!["n".into()],
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                // loop_start: header
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                // condition: i < n
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["i".into(), "n".into()]),
                    out: Some("cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["cond".into()]),
                    ..OpIR::default()
                },
                // body: total += i
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["total".into(), "i".into()]),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                // body: i += 1
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["i".into(), "one".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                // loop_end: back-edge
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                // after loop
                OpIR {
                    kind: "ret".into(),
                    var: Some("total".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
        };

        let tir_func = lower_to_tir(&func_ir);

        // Verify loop roles were detected.
        let has_header = tir_func
            .loop_roles
            .values()
            .any(|r| *r == super::super::blocks::LoopRole::LoopHeader);
        let has_end = tir_func
            .loop_roles
            .values()
            .any(|r| *r == super::super::blocks::LoopRole::LoopEnd);
        assert!(
            has_header,
            "expected a LoopHeader role; loop_roles = {:?}",
            tir_func.loop_roles
        );
        assert!(
            has_end,
            "expected a LoopEnd role; loop_roles = {:?}",
            tir_func.loop_roles
        );

        let type_map = HashMap::new();
        let round_tripped = lower_to_simple_ir(&tir_func, &type_map);

        // Must contain a structured loop exit op, not a state-machine branch.
        let has_loop_break = round_tripped
            .iter()
            .any(|o| o.kind == "loop_break_if_false");
        assert!(
            has_loop_break,
            "round-tripped while loop must contain loop_break_if_false for the loop condition; ops: {:?}",
            round_tripped
                .iter()
                .map(|o| o.kind.as_str())
                .collect::<Vec<_>>()
        );

        // Structured loop round-trips must use loop_continue/loop_end for the
        // back-edge rather than a state-machine jump to the header label.
        let has_loop_continue = round_tripped.iter().any(|o| o.kind == "loop_continue");
        assert!(
            has_loop_continue,
            "round-tripped while loop must contain loop_continue"
        );
        let header_label = round_tripped
            .iter()
            .find(|o| o.kind == "label")
            .and_then(|o| o.value);
        let has_back_edge_jump = header_label.is_some_and(|label| {
            round_tripped
                .iter()
                .any(|o| o.kind == "jump" && o.value == Some(label))
        });
        assert!(
            !has_back_edge_jump,
            "round-tripped while loop must not lower the back-edge as jump-to-header"
        );

        // Must still have a ret op.
        let has_ret = round_tripped.iter().any(|o| o.kind == "ret");
        assert!(
            has_ret,
            "round-tripped ops must contain ret; ops: {:?}",
            round_tripped
                .iter()
                .map(|o| o.kind.as_str())
                .collect::<Vec<_>>()
        );

        // Label validation must pass.
        assert!(
            validate_labels(&round_tripped),
            "label validation failed on round-tripped while loop"
        );
    }

    /// Regression test: counted loops must not re-enter above loop_index_start.
    /// Otherwise the induction variable resets every iteration and the loop hangs.
    #[test]
    fn tir_round_trip_keeps_loop_index_start_out_of_backedge_path() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "counted_loop".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(3),
                    out: Some("limit".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("zero".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_index_start".into(),
                    args: Some(vec!["zero".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["i".into(), "limit".into()]),
                    out: Some("cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["i".into(), "one".into()]),
                    out: Some("next_i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_index_next".into(),
                    args: Some(vec!["next_i".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());

        let loop_start_idx = round_tripped
            .iter()
            .position(|op| op.kind == "loop_start")
            .expect("expected loop_start after round-trip");
        let loop_index_start_idx = round_tripped
            .iter()
            .position(|op| op.kind == "loop_index_start")
            .expect("expected loop_index_start after round-trip");
        assert!(
            round_tripped[loop_start_idx + 1..loop_index_start_idx]
                .iter()
                .all(|op| op.kind != "label" && op.kind != "jump" && op.kind != "br_if"),
            "counted loop must not place control-flow re-entry before loop_index_start; ops: {:?}",
            round_tripped
                .iter()
                .map(|op| op.kind.as_str())
                .collect::<Vec<_>>()
        );
    }
}
