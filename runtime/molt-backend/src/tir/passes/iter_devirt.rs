//! List iterator devirtualization pass.
//!
//! Transforms `for x in some_list` from the iterator protocol into direct
//! index-based access, eliminating:
//!   - iterator object heap allocation (GetIter)
//!   - per-iteration `__next__` call + StopIteration check (IterNextUnboxed)
//!   - function call overhead for each element access
//!
//! Pattern matched (in TIR):
//! ```text
//!   iter_val  = GetIter(list_val)      // list_val known to be a list
//!   ...
//!   (elem, done) = IterNextUnboxed(iter_val)   // in loop header
//!   CondBranch(done, exit, body)
//! ```
//!
//! Transformed to:
//! ```text
//!   len_val = CallBuiltin("len", list_val)
//!   Branch -> header(0)
//!   header(i):
//!     cond = Lt(i, len_val)
//!     CondBranch(cond, body, exit)
//!   body:
//!     elem = Index(list_val, i)
//!     ... original body ...
//!     next_i = Add(i, 1)
//!     Branch -> header(next_i)
//! ```
//!
//! Detection: the source of `GetIter` is considered a list if:
//!   1. `TirFunction.value_types` records a `TirType::List(_)` fact, OR
//!   2. it was produced by a structural `BuildList` op, OR
//!   3. it is a list-repeat `Mul(BuildList, count)` chain.
//!
//! This runs early in the pipeline (after range_devirt, before type refinement)
//! so downstream passes can refine the index variable and element types.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;

/// Describes a recognized list-loop pattern ready for devirtualization.
struct ListLoopCandidate {
    /// Block containing the GetIter op.
    setup_block: BlockId,
    /// Index of the GetIter op within setup_block.
    get_iter_idx: usize,
    /// The ValueId of the list being iterated.
    list_val: ValueId,
    /// The ValueId produced by GetIter — the iterator.
    _iter_val: ValueId,
    /// Loop header block containing IterNextUnboxed.
    header_block: BlockId,
    /// Index of the IterNextUnboxed op within header_block.
    iter_next_idx: usize,
    /// The element ValueId produced by IterNextUnboxed (results[0]).
    elem_val: ValueId,
    /// The done-flag ValueId produced by IterNextUnboxed (results[1]).
    done_val: ValueId,
    /// The exit block (where done=true branches to).
    exit_block: BlockId,
    /// The body block (where done=false branches to).
    body_block: BlockId,
    /// Semantic container type proven by function-owned type facts or a
    /// structural list producer. Propagated to the synthesized Index op so
    /// legacy SimpleIR consumers preserve the known list shape.
    container_type: Option<String>,
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "iter_devirt",
        ..Default::default()
    };

    let candidates = find_candidates(func);
    if candidates.is_empty() {
        return stats;
    }

    for candidate in candidates {
        apply_transform(func, &candidate, &mut stats);
    }

    stats
}

/// Infer the semantic list container shape for a value.
///
/// Input authority is restricted to function-owned TIR type facts and
/// structural list producers. Legacy `container_type` attrs are deliberately
/// ignored here: they are preserved as output metadata for downstream
/// compatibility, not accepted as proof that iterator devirtualization is
/// sound.
fn infer_list_container_type(
    func: &TirFunction,
    source_val: ValueId,
    block_ids: &[BlockId],
) -> Option<String> {
    let mut seen = HashSet::new();
    infer_list_container_type_inner(func, source_val, block_ids, &mut seen)
}

fn infer_list_container_type_inner(
    func: &TirFunction,
    source_val: ValueId,
    block_ids: &[BlockId],
    seen: &mut HashSet<ValueId>,
) -> Option<String> {
    if !seen.insert(source_val) {
        return None;
    }

    if let Some(ty) = func.value_types.get(&source_val)
        && matches!(ty, TirType::List(_))
    {
        return Some("list".to_string());
    }

    for &bid in block_ids {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        for op in &block.ops {
            if !op.results.contains(&source_val) {
                continue;
            }
            // BuildList always produces a generic list.
            if op.opcode == OpCode::BuildList {
                return Some("list".to_string());
            }
            // Mul(BuildList, count) is a list repeat — inherits from operand.
            if op.opcode == OpCode::Mul && op.operands.len() == 2 {
                let (a, b) = (op.operands[0], op.operands[1]);
                if let Some(ct) = infer_list_container_type_inner(func, a, block_ids, seen) {
                    return Some(ct);
                }
                if let Some(ct) = infer_list_container_type_inner(func, b, block_ids, seen) {
                    return Some(ct);
                }
            }
            return None;
        }
    }
    None
}

/// Determine if a value is known to be a list from typed facts or its defining op.
fn is_list_source(func: &TirFunction, source_val: ValueId, block_ids: &[BlockId]) -> bool {
    infer_list_container_type(func, source_val, block_ids).is_some()
}

/// Scan the function for list-loop patterns.
fn find_candidates(func: &TirFunction) -> Vec<ListLoopCandidate> {
    // Phase 1: Build definition map for GetIter ops.
    // Map iter_val -> (setup_block, op_index, source_val)
    let mut get_iter_defs: HashMap<ValueId, (BlockId, usize, ValueId)> = HashMap::new();

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            if op.opcode == OpCode::GetIter && !op.operands.is_empty() && !op.results.is_empty() {
                get_iter_defs.insert(op.results[0], (bid, op_idx, op.operands[0]));
            }
        }
    }

    // Phase 2: Find loop headers with IterNextUnboxed that trace back to
    // a GetIter on a known list.
    let loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| (*role == LoopRole::LoopHeader).then_some(*bid))
        .collect();

    let mut candidates = Vec::new();

    for header in loop_headers {
        let Some(header_block) = func.blocks.get(&header) else {
            continue;
        };

        for (op_idx, op) in header_block.ops.iter().enumerate() {
            let (elem_val, done_val) = match op.opcode {
                OpCode::IterNextUnboxed if op.results.len() == 2 && !op.operands.is_empty() => {
                    (op.results[0], op.results[1])
                }
                _ => continue,
            };

            let iter_val = op.operands[0];

            // Trace: iter_val -> GetIter(source)
            let Some(&(setup_block, get_iter_idx, source_val)) = get_iter_defs.get(&iter_val)
            else {
                continue;
            };

            // Check if source is known to be a list from typed facts or a
            // structural list producer. Transport-only container metadata is
            // intentionally ignored.
            if !is_list_source(func, source_val, &block_ids) {
                // Not a list — skip. This avoids transforming dict/set/generator
                // iteration which has different semantics.
                continue;
            }

            // Determine the container_type for the synthesized Index op from
            // the same typed/structural proof used for devirtualization.
            let container_type = infer_list_container_type(func, source_val, &block_ids);

            // Reject if source_val is defined INSIDE the loop (mutation risk).
            // The list must be defined before the loop header.
            let source_in_loop = {
                let mut in_loop = false;
                // Check if source is defined in the header or body.
                // A conservative check: if defined in setup_block, it's fine.
                // If defined elsewhere, check if that block has a LoopRole.
                'outer: for &bid in &block_ids {
                    if bid == setup_block {
                        continue;
                    }
                    if let Some(block) = func.blocks.get(&bid) {
                        for def_op in &block.ops {
                            if def_op.results.contains(&source_val) {
                                // Check if this block is part of the loop.
                                if func.loop_roles.contains_key(&bid) && bid != header {
                                    // Defined in a loop-related block that isn't
                                    // the header — could be the body.
                                    in_loop = true;
                                }
                                break 'outer;
                            }
                        }
                    }
                }
                in_loop
            };

            if source_in_loop {
                continue;
            }

            // The header's terminator must be a CondBranch on done_val.
            let (exit_block, body_block) = match &header_block.terminator {
                Terminator::CondBranch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } if *cond == done_val => {
                    // done=true -> then_block (exit), done=false -> else_block (body)
                    (*then_block, *else_block)
                }
                _ => continue,
            };

            candidates.push(ListLoopCandidate {
                setup_block,
                get_iter_idx,
                list_val: source_val,
                _iter_val: iter_val,
                header_block: header,
                iter_next_idx: op_idx,
                elem_val,
                done_val,
                exit_block,
                body_block,
                container_type,
            });

            // Only process the first IterNextUnboxed per header.
            break;
        }
    }

    candidates
}

/// Apply the list iterator devirtualization transform to a single candidate.
fn apply_transform(func: &mut TirFunction, c: &ListLoopCandidate, stats: &mut PassStats) {
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
        match &mut block.terminator {
            Terminator::Branch { args, target } if *target == c.header_block => {
                args.push(zero_val);
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == c.header_block {
                    then_args.push(zero_val);
                }
                if *else_block == c.header_block {
                    else_args.push(zero_val);
                }
            }
            _ => {}
        }
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
            let branches_to_header = match &block.terminator {
                Terminator::Branch { target, .. } => *target == c.header_block,
                Terminator::CondBranch {
                    then_block,
                    else_block,
                    ..
                } => *then_block == c.header_block || *else_block == c.header_block,
                _ => false,
            };
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
            match &mut block.terminator {
                Terminator::Branch { target, args } if *target == c.header_block => {
                    args.push(next_val);
                }
                Terminator::CondBranch {
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                    ..
                } => {
                    if *then_block == c.header_block {
                        then_args.push(next_val);
                    }
                    if *else_block == c.header_block {
                        else_args.push(next_val);
                    }
                }
                _ => {}
            }
        }
    }

    // 6. Update loop_break_kinds to reflect the new polarity.
    //    Original: done=true -> exit (BreakIfTrue).
    //    Now: cond=true -> body (continue), so exit is the else branch.
    //    Update to BreakIfFalse.
    func.loop_break_kinds
        .insert(c.header_block, LoopBreakKind::BreakIfFalse);
}

fn make_const_int(result: ValueId, value: i64) -> TirOp {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopBreakKind, LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_op_with_container(
        opcode: OpCode,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
        container_type: &str,
    ) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert(
            "container_type".to_string(),
            AttrValue::Str(container_type.to_string()),
        );
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    /// Build a function matching `for x in some_list: body_op(x)`.
    ///
    /// TIR layout:
    ///   bb0 (entry): BuildList(...), GetIter(list), Branch -> bb1
    ///   bb1 (header): IterNextUnboxed(iter) -> (elem, done), CondBranch(done, bb3, bb2)
    ///   bb2 (body): some_op(elem), Branch -> bb1
    ///   bb3 (exit): Return
    fn build_list_for_loop(use_build_list: bool) -> TirFunction {
        let mut func = TirFunction::new("test_list_iter".into(), vec![], TirType::None);

        let list_val = func.fresh_value();
        let iter_val = func.fresh_value();

        let mut entry_ops = Vec::new();

        if use_build_list {
            // Create list via BuildList.
            let elem_a = func.fresh_value();
            let elem_b = func.fresh_value();
            entry_ops.push(make_const_int(elem_a, 1));
            entry_ops.push(make_const_int(elem_b, 2));
            entry_ops.push(make_op(
                OpCode::BuildList,
                vec![elem_a, elem_b],
                vec![list_val],
            ));
        } else {
            // Simulate a list from a call with container_type annotation.
            // Use a dummy operand so the verifier accepts the CallBuiltin.
            let dummy = func.fresh_value();
            entry_ops.push(make_const_int(dummy, 0));
            let mut attrs = AttrDict::new();
            attrs.insert("name".to_string(), AttrValue::Str("get_data".to_string()));
            attrs.insert(
                "container_type".to_string(),
                AttrValue::Str("list".to_string()),
            );
            entry_ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CallBuiltin,
                operands: vec![dummy],
                results: vec![list_val],
                attrs,
                source_span: None,
            });
        }

        entry_ops.push(make_op(OpCode::GetIter, vec![list_val], vec![iter_val]));

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        // Patch entry block.
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = entry_ops;
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        // Header block: IterNextUnboxed.
        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();

        let header_block = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header_block);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        // Body block: use elem, branch back.
        let body_result = func.fresh_value();
        let body_block = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::Add,
                vec![elem_val, elem_val],
                vec![body_result],
            )],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body_block);

        // Exit block.
        let exit_block = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit_block);
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        func
    }

    #[test]
    fn devirt_list_from_build_list() {
        let mut func = build_list_for_loop(true);
        let stats = run(&mut func);

        // Should have transformed.
        assert!(
            stats.ops_added > 0 || stats.values_changed > 0,
            "pass should have transformed the list loop"
        );

        // Header should no longer contain IterNextUnboxed.
        let header_id = BlockId(1);
        let header = &func.blocks[&header_id];
        assert!(
            !header
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::IterNextUnboxed),
            "IterNextUnboxed should be replaced"
        );

        // Header should contain Lt comparison.
        assert!(
            header.ops.iter().any(|op| op.opcode == OpCode::Lt),
            "header should have Lt comparison"
        );

        // Header should have a block argument (index variable).
        assert_eq!(header.args.len(), 1, "header should have index var arg");
        assert_eq!(header.args[0].ty, TirType::I64);
        assert_eq!(
            func.value_types.get(&header.args[0].id),
            Some(&TirType::I64),
            "index block arg must be mirrored into function value types"
        );
        let cmp_result = header
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Lt)
            .and_then(|op| op.results.first())
            .copied()
            .expect("list devirt comparison result");
        assert_eq!(
            func.value_types.get(&cmp_result),
            Some(&TirType::Bool),
            "list-loop comparison result must carry a bool type fact"
        );

        // Entry block should not have GetIter.
        let entry = &func.blocks[&BlockId(0)];
        assert!(
            !entry.ops.iter().any(|op| op.opcode == OpCode::GetIter),
            "GetIter should be replaced with len"
        );

        // Entry block should have CallBuiltin("len").
        assert!(
            entry.ops.iter().any(|op| {
                op.opcode == OpCode::CallBuiltin
                    && op.attrs.get("name") == Some(&AttrValue::Str("len".to_string()))
            }),
            "entry should have CallBuiltin('len')"
        );

        // Body block should have Index op at position 0.
        let body_id = BlockId(2);
        let body = &func.blocks[&body_id];
        assert_eq!(
            body.ops[0].opcode,
            OpCode::Index,
            "body should start with Index op"
        );

        // Body should have Add for the index increment.
        let add_count = body
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        assert_eq!(
            add_count, 2,
            "body should have original Add + increment Add"
        );
        let increment_result = body
            .ops
            .iter()
            .rev()
            .find(|op| op.opcode == OpCode::Add)
            .and_then(|op| op.results.first())
            .copied()
            .expect("list index increment result");
        assert_eq!(
            func.value_types.get(&increment_result),
            Some(&TirType::I64),
            "list index increment result must carry an i64 type fact"
        );

        // The body's branch back to header should carry the next index value.
        if let Terminator::Branch { target, args } = &body.terminator {
            assert_eq!(*target, header_id);
            assert_eq!(args.len(), 1, "back-edge should carry incremented value");
        } else {
            panic!("body should branch to header");
        }

        // Verify function passes TIR verification.
        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn no_devirt_from_container_type_only() {
        let mut func = build_list_for_loop(false);
        let stats = run(&mut func);

        assert_eq!(
            stats.values_changed, 0,
            "transport-only container_type=list must not prove list iteration"
        );
        assert!(
            func.blocks[&BlockId(1)]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::IterNextUnboxed),
            "transport-only list metadata must leave iterator protocol intact"
        );

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn devirt_list_from_function_value_type() {
        let mut func = build_list_for_loop(false);
        func.value_types
            .insert(ValueId(0), TirType::List(Box::new(TirType::DynBox)));
        let stats = run(&mut func);

        assert!(
            stats.values_changed > 0,
            "function-owned TirType::List fact should transform list loop"
        );

        let header = &func.blocks[&BlockId(1)];
        let cmp_op = header
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Lt)
            .expect("typed-list devirt should synthesize Lt compare");
        assert!(
            !cmp_op.attrs.contains_key("_fast_int"),
            "list compare must use value_types for scalar proof, not _fast_int attrs"
        );
        let cmp_result = cmp_op
            .results
            .first()
            .copied()
            .expect("list compare result");
        assert_eq!(
            func.value_types.get(&cmp_result),
            Some(&TirType::Bool),
            "list compare result must carry a bool type fact"
        );
        let entry = &func.blocks[&func.entry_block];
        let len_op = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::CallBuiltin)
            .expect("typed-list devirt should synthesize len call");
        assert!(
            !len_op.attrs.contains_key("_fast_int"),
            "synthesized len must use value_types for scalar proof, not _fast_int attrs"
        );
        let body = &func.blocks[&BlockId(2)];
        let index_op = body
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Index)
            .expect("typed-list devirt should synthesize Index");
        assert_eq!(
            index_op.attrs.get("container_type"),
            Some(&AttrValue::Str("list".to_string())),
            "synthesized Index should carry semantic list metadata derived from the typed proof"
        );
        let increment_op = body
            .ops
            .iter()
            .rev()
            .find(|op| op.opcode == OpCode::Add)
            .expect("typed-list devirt should synthesize index increment");
        assert!(
            !increment_op.attrs.contains_key("_fast_int"),
            "list increment must use value_types for scalar proof, not _fast_int attrs"
        );
        let increment_result = increment_op
            .results
            .first()
            .copied()
            .expect("list increment result");
        assert_eq!(
            func.value_types.get(&increment_result),
            Some(&TirType::I64),
            "list increment result must carry an i64 type fact"
        );

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn no_devirt_from_legacy_list_int_container_type() {
        let mut func = build_list_for_loop(false);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let source_op = entry
            .ops
            .iter_mut()
            .find(|op| op.opcode == OpCode::CallBuiltin)
            .expect("source op exists");
        source_op.attrs.insert(
            "container_type".to_string(),
            AttrValue::Str("list_int".to_string()),
        );

        let stats = run(&mut func);

        assert_eq!(
            stats.values_changed, 0,
            "legacy flat-list storage must not be accepted as semantic container metadata"
        );
        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn no_devirt_from_get_iter_container_type_only() {
        // Transport-only container_type on the GetIter op itself is not proof.
        let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::None);

        let param = ValueId(0);
        let iter_val = func.fresh_value();

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        // Entry: GetIter with container_type="list" on param (not BuildList).
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![make_op_with_container(
                OpCode::GetIter,
                vec![param],
                vec![iter_val],
                "list",
            )];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let header = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body);

        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit);
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "GetIter container_type=list must not prove list iteration"
        );
        assert!(
            func.blocks[&header_id]
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::IterNextUnboxed),
            "transport-only GetIter metadata must leave iterator protocol intact"
        );
    }

    #[test]
    fn devirt_list_from_typed_param() {
        let mut func = TirFunction::new(
            "test".into(),
            vec![TirType::List(Box::new(TirType::DynBox))],
            TirType::None,
        );

        let param = ValueId(0);
        let iter_val = func.fresh_value();

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![make_op(OpCode::GetIter, vec![param], vec![iter_val])];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let header = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body);

        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit);
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run(&mut func);
        assert!(
            stats.values_changed > 0,
            "typed list parameter should devirtualize without container_type metadata"
        );

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn no_devirt_non_list_loop() {
        // A loop with GetIter on a non-list source should not be transformed.
        let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::None);

        let param = ValueId(0);
        let iter_val = func.fresh_value();

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        // Entry: GetIter on parameter (not list, no container_type).
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![make_op(OpCode::GetIter, vec![param], vec![iter_val])];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let header = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body);

        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit);

        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "non-list loop should not be transformed"
        );
        assert_eq!(stats.values_changed, 0);
    }

    #[test]
    fn devirt_preserves_loop_break_kind() {
        let mut func = build_list_for_loop(true);
        run(&mut func);

        // After devirt, the loop should have BreakIfFalse.
        assert_eq!(
            func.loop_break_kinds.get(&BlockId(1)),
            Some(&LoopBreakKind::BreakIfFalse)
        );
    }

    #[test]
    fn no_devirt_dict_with_container_type() {
        // A loop with GetIter on a dict should not be transformed.
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);

        let dict_val = func.fresh_value();
        let iter_val = func.fresh_value();

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                make_op(OpCode::BuildDict, vec![], vec![dict_val]),
                make_op_with_container(OpCode::GetIter, vec![dict_val], vec![iter_val], "dict"),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let header = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body);

        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit);

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "dict loop should not be transformed"
        );
    }

    #[test]
    fn devirt_list_repeat_mul_build_list() {
        // `for x in [True] * n` should be devirtualized.
        // Source: Mul(BuildList([True]), n) — recognized as a list via
        // is_list_source tracing through Mul to BuildList.
        let mut func = TirFunction::new("test_mul_list".into(), vec![], TirType::None);

        let true_val = func.fresh_value();
        let list_1 = func.fresh_value();
        let n = func.fresh_value();
        let is_prime = func.fresh_value();
        let iter_val = func.fresh_value();

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_const_int(n, 100),
                make_op(OpCode::Mul, vec![list_1, n], vec![is_prime]),
                make_op(OpCode::GetIter, vec![is_prime], vec![iter_val]),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let header = TirBlock {
            id: header_id,
            args: vec![],
            ops: vec![make_op(
                OpCode::IterNextUnboxed,
                vec![iter_val],
                vec![elem_val, done_val],
            )],
            terminator: Terminator::CondBranch {
                cond: done_val,
                then_block: exit_id,
                then_args: vec![],
                else_block: body_id,
                else_args: vec![],
            },
        };
        func.blocks.insert(header_id, header);
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        let body = TirBlock {
            id: body_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![],
            },
        };
        func.blocks.insert(body_id, body);

        let exit = TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
        func.blocks.insert(exit_id, exit);
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run(&mut func);
        assert!(
            stats.values_changed > 0,
            "Mul(BuildList, count) should be recognized as a list for iter_devirt"
        );

        // The body should now contain an Index op with container_type="list".
        let body_block = &func.blocks[&body_id];
        let index_op = body_block.ops.iter().find(|op| op.opcode == OpCode::Index);
        assert!(index_op.is_some(), "Body must contain synthesized Index op");
        let idx_op = index_op.unwrap();
        assert_eq!(
            idx_op.attrs.get("container_type"),
            Some(&AttrValue::Str("list".to_string())),
            "Synthesized Index must carry container_type=list"
        );

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }
}
