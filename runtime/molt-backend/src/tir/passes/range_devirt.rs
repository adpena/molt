//! Range loop devirtualization pass.
//!
//! Transforms `for i in range(...)` iterator protocol into direct while-loop
//! arithmetic, eliminating:
//!   - range object heap allocation
//!   - range_iterator heap allocation
//!   - per-iteration `__next__` call + StopIteration check
//!   - boxing/unboxing of the induction variable
//!
//! Pattern matched (in TIR):
//! ```text
//!   range_obj = CallBuiltin("range", args...)
//!   iter_val  = GetIter(range_obj)
//!   ...
//!   (elem, done) = IterNextUnboxed(iter_val)   // in loop header
//!   CondBranch(done, exit, body)
//! ```
//!
//! Transformed to:
//! ```text
//!   // start/stop/step materialized as ConstInt or forwarded values
//!   Branch -> header(start_val)
//!   header(i):
//!     cond = Lt(i, stop_val)    // Gt for negative step
//!     CondBranch(cond, body, exit)
//!   body:
//!     ... uses i ...
//!     next_i = Add(i, step_val)
//!     Branch -> header(next_i)
//! ```
//!
//! This runs EARLY in the pipeline, before type refinement, so the loop
//! variable gets typed as I64 by downstream passes.

use std::collections::HashMap;

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;

/// Describes a recognized range-loop pattern ready for devirtualization.
struct RangeLoopCandidate {
    /// Block containing the CallBuiltin("range") and GetIter ops.
    setup_block: BlockId,
    /// Index of the CallBuiltin("range") op within setup_block.
    call_range_idx: usize,
    /// Index of the GetIter op within setup_block.
    get_iter_idx: usize,
    /// The ValueId produced by CallBuiltin("range") — the range object.
    _range_obj: ValueId,
    /// The ValueId produced by GetIter — the iterator.
    _iter_val: ValueId,
    /// Loop header block containing IterNextUnboxed/ForIter.
    header_block: BlockId,
    /// Index of the IterNextUnboxed/ForIter op within header_block.
    iter_next_idx: usize,
    /// The element ValueId produced by IterNextUnboxed (results[0]).
    elem_val: ValueId,
    /// The done-flag ValueId produced by IterNextUnboxed (results[1]).
    done_val: ValueId,
    /// Whether this uses IterNextUnboxed (2 results) vs ForIter (1 result).
    _uses_unboxed: bool,
    /// Range arguments: start, stop, step ValueIds.
    start_val: ValueId,
    stop_val: ValueId,
    step_val: ValueId,
    /// Whether the step is a known constant.
    step_const: Option<i64>,
    /// The exit block (where done=true branches to).
    exit_block: BlockId,
    /// The body block (where done=false branches to).
    body_block: BlockId,
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "range_devirt",
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

/// Scan the function for range-loop patterns.
fn find_candidates(func: &TirFunction) -> Vec<RangeLoopCandidate> {
    // Phase 1: Build definition maps.
    // Map ValueId -> (block, op_index, opcode) for range-relevant ops.
    let mut call_builtin_defs: HashMap<ValueId, (BlockId, usize, Vec<ValueId>)> = HashMap::new();
    let mut get_iter_defs: HashMap<ValueId, (BlockId, usize, ValueId)> = HashMap::new();

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            match op.opcode {
                OpCode::CallBuiltin => {
                    let name = op
                        .attrs
                        .get("name")
                        .and_then(|v| match v {
                            AttrValue::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .unwrap_or("");
                    if (name == "range" || name == "builtin_range" || name == "molt_range")
                        && !op.results.is_empty()
                        && (1..=3).contains(&op.operands.len())
                    {
                        call_builtin_defs.insert(op.results[0], (bid, op_idx, op.operands.clone()));
                    }
                }
                OpCode::GetIter => {
                    if !op.operands.is_empty() && !op.results.is_empty() {
                        get_iter_defs.insert(op.results[0], (bid, op_idx, op.operands[0]));
                    }
                }
                _ => {}
            }
        }
    }

    // Phase 2: Find loop headers with IterNextUnboxed/ForIter that trace back
    // to a range call.
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

        // Find IterNextUnboxed or ForIter in the header.
        for (op_idx, op) in header_block.ops.iter().enumerate() {
            let (uses_unboxed, elem_val, done_val) = match op.opcode {
                OpCode::IterNextUnboxed if op.results.len() == 2 && !op.operands.is_empty() => {
                    (true, op.results[0], op.results[1])
                }
                _ => continue,
            };

            let iter_val = op.operands[0];

            // Trace: iter_val -> GetIter(source) -> CallBuiltin("range", args)
            let Some(&(get_iter_block, get_iter_idx, source_val)) = get_iter_defs.get(&iter_val)
            else {
                continue;
            };

            let Some(&(call_block, call_idx, ref range_args)) = call_builtin_defs.get(&source_val)
            else {
                continue;
            };

            // GetIter and CallBuiltin must be in the same block (the setup block
            // before the loop header). This ensures we can safely remove them.
            if get_iter_block != call_block {
                continue;
            }

            // Extract start, stop, step from range arguments.
            let (start_val, stop_val, step_val, step_const) =
                extract_range_args(func, &block_ids, range_args);

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

            candidates.push(RangeLoopCandidate {
                setup_block: call_block,
                call_range_idx: call_idx,
                get_iter_idx,
                _range_obj: source_val,
                _iter_val: iter_val,
                header_block: header,
                iter_next_idx: op_idx,
                elem_val,
                done_val,
                _uses_unboxed: uses_unboxed,
                start_val,
                stop_val,
                step_val,
                step_const,
                exit_block,
                body_block,
            });

            // Only process the first IterNextUnboxed per header.
            break;
        }
    }

    candidates
}

/// Extract (start, stop, step) ValueIds from range call arguments.
/// For range(n): start=ConstInt(0), stop=n, step=ConstInt(1)
/// For range(a, b): start=a, stop=b, step=ConstInt(1)
/// For range(a, b, c): start=a, stop=b, step=c
///
/// Returns (start, stop, step, step_const) where step_const is Some(v) if step
/// is a known constant.
fn extract_range_args(
    func: &TirFunction,
    block_ids: &[BlockId],
    range_args: &[ValueId],
) -> (ValueId, ValueId, ValueId, Option<i64>) {
    // Try to find constant values for the arguments.
    let const_map = build_const_map(func, block_ids);

    match range_args.len() {
        1 => {
            // range(stop) -> start=0, stop=args[0], step=1
            let stop = range_args[0];
            // We'll create start=0 and step=1 as new constants during transform.
            // Use sentinel ValueId(u32::MAX) to signal "needs materialization".
            (
                ValueId(u32::MAX - 1), // placeholder: start=0
                stop,
                ValueId(u32::MAX), // placeholder: step=1
                Some(1),
            )
        }
        2 => {
            // range(start, stop) -> step=1
            let start = range_args[0];
            let stop = range_args[1];
            (
                start,
                stop,
                ValueId(u32::MAX), // placeholder: step=1
                Some(1),
            )
        }
        3 => {
            // range(start, stop, step)
            let start = range_args[0];
            let stop = range_args[1];
            let step = range_args[2];
            let step_const = const_map.get(&step).copied();
            (start, stop, step, step_const)
        }
        _ => unreachable!("range_args len already validated as 1..=3"),
    }
}

/// Build a map of ValueId -> constant i64 value for all ConstInt ops.
fn build_const_map(func: &TirFunction, block_ids: &[BlockId]) -> HashMap<ValueId, i64> {
    let mut map = HashMap::new();
    for &bid in block_ids {
        if let Some(block) = func.blocks.get(&bid) {
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt && !op.results.is_empty()
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                        map.insert(op.results[0], *v);
                    }
            }
        }
    }
    map
}

/// Apply the range devirtualization transform to a single candidate.
fn apply_transform(func: &mut TirFunction, c: &RangeLoopCandidate, stats: &mut PassStats) {
    // 1. Materialize start and step constants if needed (placeholders).
    let start_val = if c.start_val.0 == u32::MAX - 1 {
        // Materialize ConstInt(0) for range(stop).
        let val = func.fresh_value();
        let const_op = make_const_int(val, 0);
        // Insert before the CallBuiltin in the setup block.
        if let Some(block) = func.blocks.get_mut(&c.setup_block) {
            block.ops.insert(c.call_range_idx, const_op);
        }
        stats.ops_added += 1;
        val
    } else {
        c.start_val
    };

    // Recalculate indices after potential insertion.
    // If we inserted a start const, call_range_idx and get_iter_idx shift by 1.
    let offset = if c.start_val.0 == u32::MAX - 1 { 1 } else { 0 };

    let step_val = if c.step_val.0 == u32::MAX {
        // Materialize ConstInt(1) for step=1.
        let val = func.fresh_value();
        let const_op = make_const_int(val, 1);
        if let Some(block) = func.blocks.get_mut(&c.setup_block) {
            block.ops.insert(c.call_range_idx + offset, const_op);
        }
        stats.ops_added += 1;
        val
    } else {
        c.step_val
    };

    let offset2 = offset + if c.step_val.0 == u32::MAX { 1 } else { 0 };

    // 2. Remove CallBuiltin("range") and GetIter from setup block.
    //    We must remove them in reverse index order to preserve indices.
    if let Some(block) = func.blocks.get_mut(&c.setup_block) {
        let call_idx = c.call_range_idx + offset2;
        let iter_idx = c.get_iter_idx + offset2;

        // Remove in reverse order (higher index first).
        let (first_remove, second_remove) = if call_idx > iter_idx {
            (call_idx, iter_idx)
        } else {
            (iter_idx, call_idx)
        };

        if first_remove < block.ops.len() {
            block.ops.remove(first_remove);
            stats.ops_removed += 1;
        }
        if second_remove < block.ops.len() {
            block.ops.remove(second_remove);
            stats.ops_removed += 1;
        }
    }

    // 3. Modify setup block terminator to pass start_val as block argument
    //    to the header.
    if let Some(block) = func.blocks.get_mut(&c.setup_block) {
        match &mut block.terminator {
            Terminator::Branch { args, target } if *target == c.header_block => {
                args.push(start_val);
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == c.header_block {
                    then_args.push(start_val);
                }
                if *else_block == c.header_block {
                    else_args.push(start_val);
                }
            }
            _ => {}
        }
    }

    // 4. Transform the header block:
    //    - Add block argument for the induction variable (reusing elem_val).
    //    - Replace IterNextUnboxed with Lt/Gt comparison.
    //    - Flip CondBranch polarity (was: done->exit, !done->body;
    //      now: cond_true->body, cond_false->exit).
    let ind_var = c.elem_val; // Reuse the same ValueId for the induction variable.

    if let Some(header) = func.blocks.get_mut(&c.header_block) {
        // Add block argument for induction variable.
        header.args.push(TirValue {
            id: ind_var,
            ty: TirType::I64,
        });

        // Replace IterNextUnboxed with comparison op.
        // For positive step (or unknown): i < stop
        // For negative step: i > stop
        let cmp_opcode = match c.step_const {
            Some(s) if s < 0 => OpCode::Gt,
            _ => OpCode::Lt,
        };

        let cond_val = c.done_val; // Reuse done_val as the comparison result.
        let cmp_op = TirOp {
            dialect: Dialect::Molt,
            opcode: cmp_opcode,
            operands: vec![ind_var, c.stop_val],
            results: vec![cond_val],
            attrs: {
                let mut a = AttrDict::new();
                a.insert("_fast_int".to_string(), AttrValue::Bool(true));
                a
            },
            source_span: None,
        };

        header.ops[c.iter_next_idx] = cmp_op;
        stats.values_changed += 1;

        // Flip CondBranch polarity: cond_val=true means "in range" (continue),
        // so then_block should be body and else_block should be exit.
        header.terminator = Terminator::CondBranch {
            cond: cond_val,
            then_block: c.body_block,
            then_args: vec![],
            else_block: c.exit_block,
            else_args: vec![],
        };
    }

    // 5. Add increment (i += step) at the end of the loop body and pass
    //    the incremented value back to the header as a block argument.
    //    We need to find ALL blocks that branch back to the header (not just
    //    body_block — there may be continue paths or nested structures).
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
        // Skip the setup block — it already has start_val as the argument.
        if back_bid == c.setup_block {
            continue;
        }

        let next_val = func.fresh_value();

        if let Some(block) = func.blocks.get_mut(&back_bid) {
            // Insert Add(ind_var, step_val) -> next_val at end of block (before terminator).
            //
            // When |step| == 1, the addition provably cannot overflow a
            // signed i64:
            //   - For step=+1: the loop guard `i < stop` ensures
            //     `i <= stop - 1`, so `i + 1 <= stop` which is valid i64.
            //   - For step=-1: the loop guard `i > stop` ensures
            //     `i >= stop + 1`, so `i - 1 >= stop` which is valid i64.
            //
            // This `no_signed_wrap` attribute tells the LLVM lowering to
            // emit `add nsw` which enables SCEV, induction variable
            // strength reduction, and loop vectorization.
            let nsw_safe = matches!(c.step_const, Some(1) | Some(-1));
            let add_op = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![ind_var, step_val],
                results: vec![next_val],
                attrs: {
                    let mut a = AttrDict::new();
                    a.insert("_fast_int".to_string(), AttrValue::Bool(true));
                    if nsw_safe {
                        a.insert("no_signed_wrap".to_string(), AttrValue::Bool(true));
                    }
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
    //    The original loop had BreakIfTrue (done=true -> exit).
    //    Now we have cond=true -> body (continue), so exit is the else branch.
    //    Update to BreakIfFalse.
    use crate::tir::blocks::LoopBreakKind;
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

    fn make_call_builtin(name: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("name".to_string(), AttrValue::Str(name.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    fn make_const(result: ValueId, value: i64) -> TirOp {
        make_const_int(result, value)
    }

    /// Build a function matching the pattern:
    ///
    /// ```python
    /// for i in range(n):
    ///     body_op(i)
    /// ```
    ///
    /// TIR layout:
    ///   bb0 (entry): ConstInt(n), CallBuiltin("range", n), GetIter, Branch -> bb1
    ///   bb1 (header): IterNextUnboxed(iter) -> (elem, done), CondBranch(done, bb3, bb2)
    ///   bb2 (body): some_op(elem), Branch -> bb1
    ///   bb3 (exit): Return
    fn build_range_for_loop(range_args: &[i64]) -> TirFunction {
        let mut func = TirFunction::new("test_range".into(), vec![], TirType::None);

        // Entry block values.
        let mut range_arg_vals = Vec::new();
        let mut entry_ops = Vec::new();

        for &arg in range_args {
            let val = func.fresh_value();
            entry_ops.push(make_const(val, arg));
            range_arg_vals.push(val);
        }

        let range_obj = func.fresh_value();
        entry_ops.push(make_call_builtin(
            "range",
            range_arg_vals.clone(),
            vec![range_obj],
        ));

        let iter_val = func.fresh_value();
        entry_ops.push(make_op(OpCode::GetIter, vec![range_obj], vec![iter_val]));

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
    fn devirt_range_single_arg() {
        // for i in range(10): body
        let mut func = build_range_for_loop(&[10]);
        let stats = run(&mut func);

        // Should have transformed.
        assert!(
            stats.ops_removed > 0 || stats.ops_added > 0 || stats.values_changed > 0,
            "pass should have transformed the range loop"
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

        // Header should have a block argument (induction variable).
        assert_eq!(header.args.len(), 1, "header should have induction var arg");
        assert_eq!(header.args[0].ty, TirType::I64);

        // Entry block should not have CallBuiltin("range") or GetIter.
        let entry = &func.blocks[&BlockId(0)];
        assert!(
            !entry.ops.iter().any(|op| op.opcode == OpCode::CallBuiltin),
            "CallBuiltin('range') should be removed"
        );
        assert!(
            !entry.ops.iter().any(|op| op.opcode == OpCode::GetIter),
            "GetIter should be removed"
        );

        // Body block should have an Add for the induction variable increment.
        let body_id = BlockId(2);
        let body = &func.blocks[&body_id];
        // There should be 2 Add ops: original body op + increment.
        let add_count = body
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        assert_eq!(
            add_count, 2,
            "body should have original Add + increment Add"
        );

        // The body's branch back to header should carry the next induction value.
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
    fn devirt_range_two_args() {
        // for i in range(5, 20): body
        let mut func = build_range_for_loop(&[5, 20]);
        let stats = run(&mut func);

        assert!(
            stats.values_changed > 0,
            "pass should transform range(start, stop)"
        );

        let header = &func.blocks[&BlockId(1)];
        assert!(header.ops.iter().any(|op| op.opcode == OpCode::Lt));

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn devirt_range_three_args_positive_step() {
        // for i in range(0, 100, 3): body
        let mut func = build_range_for_loop(&[0, 100, 3]);
        let stats = run(&mut func);

        assert!(
            stats.values_changed > 0,
            "pass should transform range(start, stop, step)"
        );

        let header = &func.blocks[&BlockId(1)];
        // Positive step -> Lt.
        assert!(header.ops.iter().any(|op| op.opcode == OpCode::Lt));

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn devirt_range_three_args_negative_step() {
        // for i in range(10, 0, -1): body
        let mut func = build_range_for_loop(&[10, 0, -1]);
        let stats = run(&mut func);

        assert!(
            stats.values_changed > 0,
            "pass should transform range with negative step"
        );

        let header = &func.blocks[&BlockId(1)];
        // Negative step -> Gt.
        assert!(
            header.ops.iter().any(|op| op.opcode == OpCode::Gt),
            "negative step should use Gt comparison"
        );

        crate::tir::verify::verify_function(&func).expect("verification should pass");
    }

    #[test]
    fn no_devirt_non_range_loop() {
        // A loop with GetIter on a non-range source should not be transformed.
        let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::None);

        let param = ValueId(0);
        let iter_val = func.fresh_value();

        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        // Entry: GetIter on parameter (not range).
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
            "non-range loop should not be transformed"
        );
        assert_eq!(stats.values_changed, 0);
    }

    #[test]
    fn devirt_preserves_loop_break_kind() {
        let mut func = build_range_for_loop(&[10]);
        run(&mut func);

        // After devirt, the loop should have BreakIfFalse
        // (cond=true means continue, false means exit).
        assert_eq!(
            func.loop_break_kinds.get(&BlockId(1)),
            Some(&LoopBreakKind::BreakIfFalse)
        );
    }
}
