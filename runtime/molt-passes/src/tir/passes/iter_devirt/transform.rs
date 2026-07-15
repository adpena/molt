use crate::tir::blocks::{BlockId, LoopBreakKind, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;
use super::candidate::ListLoopCandidate;

/// Apply the list iterator devirtualization transform to a single candidate.
pub(super) fn apply_transform(
    func: &mut TirFunction,
    c: &ListLoopCandidate,
    stats: &mut PassStats,
) {
    // 1. In the setup block, replace GetIter with CallBuiltin("len", list_val).
    //    Reuse the GetIter result ValueId for the len value so we don't need to
    //    find and update all references to the iterator (there are none after
    //    we replace IterNextUnboxed).
    let len_val = func.fresh_value();
    func.value_types.insert(len_val, TirType::I64);
    let len_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallBuiltin,
        operands: vec![c.list_val],
        results: vec![len_val],
        attrs: {
            let mut a = AttrDict::new();
            a.insert("name".to_string(), AttrValue::Str("len".to_string()));
            a
        },
        source_span: None,
    };

    // Materialize ConstInt(0) for the initial index.
    let zero_val = func.fresh_value();
    func.value_types.insert(zero_val, TirType::I64);
    let zero_op = make_const_int(zero_val, 0);

    // Materialize ConstInt(1) for the index increment.
    let one_val = func.fresh_value();
    func.value_types.insert(one_val, TirType::I64);
    let one_op = make_const_int(one_val, 1);

    if let Some(block) = func.blocks.get_mut(&c.setup_block) {
        // Replace GetIter with len + constants.
        // Insert len_op at the GetIter position, then constants after.
        block.ops[c.get_iter_idx] = len_op;
        block.ops.insert(c.get_iter_idx + 1, zero_op);
        block.ops.insert(c.get_iter_idx + 2, one_op);
        stats.ops_added += 2; // len replaces GetIter (net 0), plus 2 new consts
    }

    // 2. Modify setup block terminator to pass zero_val (initial index) as
    //    block argument to the header.
    if let Some(block) = func.blocks.get_mut(&c.setup_block) {
        block.terminator.for_each_edge_mut(|target, args| {
            if *target == c.header_block {
                args.push(zero_val);
            }
        });
    }

    // 3. Transform the header block:
    //    - Add block argument for the index variable.
    //    - Replace IterNextUnboxed with Lt(i, len).
    //    - Flip CondBranch polarity (was: done->exit, !done->body;
    //      now: in_bounds->body, out_of_bounds->exit).
    let idx_var = func.fresh_value();
    func.value_types.insert(idx_var, TirType::I64);

    if let Some(header) = func.blocks.get_mut(&c.header_block) {
        // Add block argument for index variable.
        header.args.push(TirValue {
            id: idx_var,
            ty: TirType::I64,
        });

        // Replace IterNextUnboxed with Lt comparison.
        let cond_val = c.done_val; // Reuse done_val as the comparison result.
        func.value_types.insert(cond_val, TirType::Bool);
        let cmp_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Lt,
            operands: vec![idx_var, len_val],
            results: vec![cond_val],
            attrs: AttrDict::new(),
            source_span: None,
        };

        header.ops[c.iter_next_idx] = cmp_op;
        stats.values_changed += 1;

        // Flip CondBranch polarity: cond_val=true means "in bounds" (continue),
        // so then_block should be body and else_block should be exit.
        header.terminator = Terminator::CondBranch {
            cond: cond_val,
            then_block: c.body_block,
            then_args: vec![],
            else_block: c.exit_block,
            else_args: vec![],
        };
    }

    // 4. Insert Index(list_val, idx_var) -> elem_val at the start of the body
    //    block, so all uses of elem_val in the body see the correct element.
    //    Propagate container_type so the backend emits inline list access
    //    instead of a generic runtime call.
    let mut index_attrs = AttrDict::new();
    if let Some(ref ct) = c.container_type {
        index_attrs.insert("container_type".to_string(), AttrValue::Str(ct.clone()));
    }
    let index_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![c.list_val, idx_var],
        results: vec![c.elem_val],
        attrs: index_attrs,
        source_span: None,
    };

    if let Some(body) = func.blocks.get_mut(&c.body_block) {
        body.ops.insert(0, index_op);
        stats.ops_added += 1;
    }

    // 5. Add increment (i += 1) at the end of every back-edge block and pass
    //    the incremented value to the header as a block argument.
    let back_edge_blocks: Vec<BlockId> = {
        let mut result = Vec::new();
        for (&bid, block) in &func.blocks {
            if bid == c.header_block {
                continue;
            }
            let branches_to_header = block.terminator.has_successor(c.header_block);
            if branches_to_header {
                result.push(bid);
            }
        }
        result
    };

    for back_bid in back_edge_blocks {
        // Skip the setup block — it already has zero_val as the argument.
        if back_bid == c.setup_block {
            continue;
        }

        let next_val = func.fresh_value();
        func.value_types.insert(next_val, TirType::I64);

        if let Some(block) = func.blocks.get_mut(&back_bid) {
            // Insert Add(idx_var, 1) -> next_val at end of block (before terminator).
            //
            // This `+1` index counter provably cannot overflow a signed i64:
            // the header guard `Lt(idx_var, len_val)` ensures the body (and so
            // this back-edge) executes only when `idx_var <= len_val - 1`, and
            // `len_val = len(container) >= 0` is bounded by `isize::MAX < i64::MAX`.
            // Hence `idx_var + 1 <= len_val <= i64::MAX` — no wrap. Tagging the
            // increment `no_signed_wrap` lets SCEV form the IV's `AddRec`, which
            // is the seed the value-range analysis needs to prove the IV (and
            // every value derived from it) stays within the inline window —
            // exactly the same justification range_devirt uses for its `±1`
            // counted-loop increment. Without this tag the canonical
            // `for x in seq:` index counter has no proven range, blocking SROA
            // hot-loop field promotion and BCE on devirtualized iterators.
            let add_op = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![idx_var, one_val],
                results: vec![next_val],
                attrs: {
                    let mut a = AttrDict::new();
                    a.insert("no_signed_wrap".to_string(), AttrValue::Bool(true));
                    a
                },
                source_span: None,
            };
            block.ops.push(add_op);
            stats.ops_added += 1;

            // Add next_val to the branch args going to the header.
            block.terminator.for_each_edge_mut(|target, args| {
                if *target == c.header_block {
                    args.push(next_val);
                }
            });
        }
    }

    // 6. Update loop_break_kinds to reflect the new polarity.
    //    Original: done=true -> exit (BreakIfTrue).
    //    Now: cond=true -> body (continue), so exit is the else branch.
    //    Update to BreakIfFalse.
    func.loop_break_kinds
        .insert(c.header_block, LoopBreakKind::BreakIfFalse);
}

pub(super) fn make_const_int(result: ValueId, value: i64) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".to_string(), AttrValue::Int(value));
            m
        },
        source_span: None,
    }
}
