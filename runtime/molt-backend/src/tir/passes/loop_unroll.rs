//! Loop Unrolling Pass for TIR.
//!
//! Unrolls loops with known small trip counts (compile-time constant
//! bounds) by duplicating the loop body. The unrolled body enables
//! SCCP to fold constants per-iteration and DCE to eliminate dead
//! branches, producing straight-line code for tight numeric loops.
//!
//! Only unrolls loops that meet ALL criteria:
//! 1. Trip count is a compile-time constant (from range_devirt metadata).
//! 2. Trip count <= MAX_UNROLL_TRIP_COUNT (default 8).
//! 3. Loop body has <= MAX_UNROLL_OPS ops (prevents code bloat).
//! 4. No exception handling in the loop body.
//! 5. No nested loops (only innermost loops).
//!
//! The unrolled code replaces the entire loop with straight-line ops,
//! one copy of the body per iteration with the induction variable
//! replaced by the constant iteration value.
//!
//! Reference: Muchnick ch. 17, LLVM LoopUnrollPass.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Maximum trip count for unrolling.
const MAX_UNROLL_TRIP_COUNT: i64 = 8;

/// Maximum ops in loop body for unrolling (prevents code bloat).
const MAX_UNROLL_OPS: usize = 20;

/// Metadata about a loop eligible for unrolling.
struct UnrollCandidate {
    /// The loop header block.
    header: BlockId,
    /// The loop body block (single block only).
    body: BlockId,
    /// The exit block (after the loop).
    exit: BlockId,
    /// The induction variable (block arg of the header).
    induction_var: ValueId,
    /// Start value of the induction variable.
    start: i64,
    /// Trip count (number of iterations).
    trip_count: i64,
    /// Step value per iteration.
    step: i64,
}

/// Detect range-devirtualized loops eligible for unrolling.
///
/// After range_devirt, loops have metadata attributes:
/// - "range_start", "range_stop", "range_step" on the loop header
/// - The header has a block arg for the induction variable
/// - CondBranch with the induction variable comparison
fn find_unroll_candidates(func: &TirFunction) -> Vec<UnrollCandidate> {
    let mut candidates = Vec::new();

    for (&bid, block) in &func.blocks {
        // Only consider loop headers.
        if func.loop_roles.get(&bid) != Some(&LoopRole::LoopHeader) {
            continue;
        }

        // Check for range metadata in the header's ops.
        let mut range_start: Option<i64> = None;
        let mut range_stop: Option<i64> = None;
        let mut range_step: Option<i64> = None;

        for op in &block.ops {
            if op.opcode == OpCode::ConstInt {
                if let Some(AttrValue::Str(tag)) = op.attrs.get("range_role") {
                    if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                        match tag.as_str() {
                            "start" => range_start = Some(*v),
                            "stop" => range_stop = Some(*v),
                            "step" => range_step = Some(*v),
                            _ => {}
                        }
                    }
                }
            }
        }

        // Also check: the loop header must branch conditionally.
        let (body, exit) = match &block.terminator {
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => {
                // Typically: then=body, else=exit. But it could be inverted.
                // If then_block loops back to header, it's the body.
                if func.blocks.get(then_block).is_some_and(|b| {
                    matches!(&b.terminator, Terminator::Branch { target, .. } if *target == bid)
                }) {
                    (*then_block, *else_block)
                } else {
                    (*else_block, *then_block)
                }
            }
            _ => continue,
        };

        // Need at least start and stop. Step defaults to 1.
        let start = match range_start {
            Some(s) => s,
            None => continue,
        };
        let stop = match range_stop {
            Some(s) => s,
            None => continue,
        };
        let step = range_step.unwrap_or(1);
        if step == 0 {
            continue;
        }

        // Compute trip count.
        let trip_count = if step > 0 {
            (stop - start + step - 1) / step
        } else {
            (start - stop + (-step) - 1) / (-step)
        };

        if trip_count <= 0 || trip_count > MAX_UNROLL_TRIP_COUNT {
            continue;
        }

        // Check body size.
        let body_block = match func.blocks.get(&body) {
            Some(b) => b,
            None => continue,
        };
        if body_block.ops.len() > MAX_UNROLL_OPS {
            continue;
        }

        // No exception handling.
        if func.has_exception_handling {
            continue;
        }

        // No nested loops (body should not be a loop header).
        if func
            .loop_roles
            .get(&body)
            .is_some_and(|r| matches!(r, LoopRole::LoopHeader))
        {
            continue;
        }

        // Induction variable: first block arg of the header.
        if block.args.is_empty() {
            continue;
        }
        let induction_var = block.args[0].id;

        candidates.push(UnrollCandidate {
            header: bid,
            body,
            exit,
            induction_var,
            start,
            trip_count,
            step,
        });
    }

    candidates
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "loop_unroll",
        ..Default::default()
    };

    let candidates = find_unroll_candidates(func);
    if candidates.is_empty() {
        return stats;
    }

    for candidate in candidates {
        unroll_candidate(func, &candidate, &mut stats);
    }

    stats
}

/// Unroll a single loop candidate: replace header + body with a straight-line
/// "landing" block that contains `trip_count` copies of the body, each with
/// the induction variable substituted by its iteration constant. Exit-edge
/// arguments that referenced the induction variable are rewritten to the
/// final post-loop value (start + trip_count * step), and arguments that
/// referenced the body's loop-back values are rewritten to the corresponding
/// last-iteration result.
fn unroll_candidate(func: &mut TirFunction, c: &UnrollCandidate, stats: &mut PassStats) {
    // Snapshot what we need from header/body before mutating.
    let body_ops = match func.blocks.get(&c.body) {
        Some(b) => b.ops.clone(),
        None => return,
    };
    let body_back_args: Vec<ValueId> = match func.blocks.get(&c.body) {
        Some(b) => match &b.terminator {
            Terminator::Branch { target, args } if *target == c.header => args.clone(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let header_block = match func.blocks.get(&c.header) {
        Some(b) => b.clone(),
        None => return,
    };
    let orig_exit_args: Vec<ValueId> = match &header_block.terminator {
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *else_block == c.exit {
                else_args.clone()
            } else if *then_block == c.exit {
                then_args.clone()
            } else {
                return;
            }
        }
        _ => return,
    };

    // Map each header block-arg index to which body back-edge ValueId fills it.
    // body_back_args[i] is the value the body branch passes for header.args[i].
    let header_arg_ids: Vec<ValueId> = header_block.args.iter().map(|a| a.id).collect();

    let header_ops_count = header_block.ops.len();
    let body_ops_count = body_ops.len();

    // Build the landing block ops.
    let mut landing_ops: Vec<TirOp> = Vec::new();
    // Track the last iteration's remap so we can substitute exit-arg references
    // to body-loop-back values with the last clone's outputs.
    let mut last_iter_remap: HashMap<ValueId, ValueId> = HashMap::new();

    for i in 0..c.trip_count {
        let iter_value = c.start + i * c.step;
        let iter_const_id = func.fresh_value();

        let mut const_attrs = AttrDict::new();
        const_attrs.insert("value".into(), AttrValue::Int(iter_value));
        landing_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![iter_const_id],
            attrs: const_attrs,
            source_span: None,
        });

        // Per-iteration value remap: induction var → this iteration's constant,
        // then each cloned op's result gets a fresh ValueId added to the map.
        let mut remap: HashMap<ValueId, ValueId> = HashMap::new();
        remap.insert(c.induction_var, iter_const_id);

        for op in &body_ops {
            let new_results: Vec<ValueId> = op
                .results
                .iter()
                .map(|&result| {
                    let new_value = func.fresh_value();
                    remap.insert(result, new_value);
                    new_value
                })
                .collect();

            let new_operands: Vec<ValueId> = op
                .operands
                .iter()
                .map(|v| remap.get(v).copied().unwrap_or(*v))
                .collect();

            landing_ops.push(TirOp {
                dialect: op.dialect,
                opcode: op.opcode,
                operands: new_operands,
                results: new_results.clone(),
                attrs: op.attrs.clone(),
                source_span: op.source_span,
            });

            stats.ops_added += 1;
            stats.values_changed += new_results.len();
        }

        last_iter_remap = remap;
    }

    // If any exit arg references the induction variable, materialise the
    // post-loop final value (start + trip_count * step) as a ConstInt in the
    // landing block and use it as the substitution target.
    let final_value_id: Option<ValueId> = if orig_exit_args.iter().any(|v| *v == c.induction_var) {
        let final_value = c.start + c.trip_count * c.step;
        let final_id = func.fresh_value();
        let mut final_attrs = AttrDict::new();
        final_attrs.insert("value".into(), AttrValue::Int(final_value));
        landing_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![final_id],
            attrs: final_attrs,
            source_span: None,
        });
        Some(final_id)
    } else {
        None
    };

    // Substitute exit args:
    //   - References to the induction variable → final post-loop value.
    //   - References to a header block-arg whose body back-edge value is in
    //     last_iter_remap → that remapped result. (Handles cases where the
    //     exit-edge passes a body-defined value that loops back through
    //     header.args[k], i.e. the loop-carried reduction pattern.)
    let new_exit_args: Vec<ValueId> = orig_exit_args
        .iter()
        .map(|&v| {
            if v == c.induction_var {
                return final_value_id.unwrap_or(v);
            }
            // Loop-carried block arg: header.args[k] is forwarded by body's
            // back-edge as body_back_args[k]. The exit edge gets the same
            // value as the LAST iteration's body produced, which is
            // last_iter_remap[body_back_args[k]].
            if let Some(arg_idx) = header_arg_ids.iter().position(|&id| id == v) {
                if let Some(&body_value) = body_back_args.get(arg_idx) {
                    if let Some(&remapped) = last_iter_remap.get(&body_value) {
                        return remapped;
                    }
                }
            }
            v
        })
        .collect();

    // Allocate the landing block.
    let landing = func.fresh_block();
    let landing_block = TirBlock {
        id: landing,
        args: Vec::new(),
        ops: landing_ops,
        terminator: Terminator::Branch {
            target: c.exit,
            args: new_exit_args,
        },
    };
    func.blocks.insert(landing, landing_block);

    // Redirect every predecessor of the header to the landing block.
    let preds: Vec<BlockId> = func
        .blocks
        .iter()
        .filter_map(|(&bid, b)| {
            if bid == c.header || bid == c.body {
                return None;
            }
            if branches_to(&b.terminator, c.header) {
                Some(bid)
            } else {
                None
            }
        })
        .collect();
    for pred in preds {
        if let Some(b) = func.blocks.get_mut(&pred) {
            redirect_terminator(&mut b.terminator, c.header, landing);
        }
    }
    // Also remap the function entry if it pointed at the header.
    if func.entry_block == c.header {
        func.entry_block = landing;
    }

    // Drop the original header and body blocks plus their loop metadata.
    func.blocks.remove(&c.header);
    func.blocks.remove(&c.body);
    func.loop_roles.remove(&c.header);
    func.loop_pairs.remove(&c.header);
    func.loop_break_kinds.remove(&c.header);
    func.loop_cond_blocks.remove(&c.header);

    stats.ops_removed += header_ops_count + body_ops_count;
}

/// Returns `true` if the terminator references `target` as any successor.
fn branches_to(term: &Terminator, target: BlockId) -> bool {
    match term {
        Terminator::Branch { target: t, .. } => *t == target,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == target || *else_block == target,
        Terminator::Switch { cases, default, .. } => {
            *default == target || cases.iter().any(|(_, b, _)| *b == target)
        }
        _ => false,
    }
}

/// Replace every successor reference to `from` with `to` in `term`.
fn redirect_terminator(term: &mut Terminator, from: BlockId, to: BlockId) {
    match term {
        Terminator::Branch { target, .. } => {
            if *target == from {
                *target = to;
            }
        }
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => {
            if *then_block == from {
                *then_block = to;
            }
            if *else_block == from {
                *else_block = to;
            }
        }
        Terminator::Switch { cases, default, .. } => {
            for (_, b, _) in cases.iter_mut() {
                if *b == from {
                    *b = to;
                }
            }
            if *default == from {
                *default = to;
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    fn const_int_op(result: ValueId, value: i64) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(value));
                m
            },
            source_span: None,
        }
    }

    fn range_metadata_op(result: ValueId, role: &str, value: i64) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(value));
                m.insert("range_role".into(), AttrValue::Str(role.into()));
                m
            },
            source_span: None,
        }
    }

    fn add_op(lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    struct TestLoop {
        func: TirFunction,
        header: BlockId,
        body: BlockId,
        exit: BlockId,
        body_result: ValueId,
    }

    fn build_test_loop(
        start: i64,
        stop: i64,
        step: i64,
        body_op_count: usize,
        exit_args: impl FnOnce(ValueId, ValueId) -> Vec<ValueId>,
    ) -> TestLoop {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let ind_var = func.fresh_value();
        let cond = func.fresh_value();
        let start_val = func.fresh_value();
        let stop_val = func.fresh_value();
        let step_val = func.fresh_value();
        let external = func.fresh_value();
        let body_op_result = func.fresh_value();

        // Entry → header
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int_op(external, 10));
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![],
            };
        }

        // Header: range metadata + CondBranch
        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![TirValue {
                    id: ind_var,
                    ty: TirType::I64,
                }],
                ops: vec![
                    range_metadata_op(start_val, "start", start),
                    range_metadata_op(stop_val, "stop", stop),
                    range_metadata_op(step_val, "step", step),
                    const_int_op(cond, 1),
                ],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: exit_args(ind_var, body_op_result),
                },
            },
        );

        // Body: one op, loops back to header
        let mut body_ops = vec![add_op(ind_var, external, body_op_result)];
        for _ in 1..body_op_count {
            let extra_result = func.fresh_value();
            body_ops.push(add_op(ind_var, external, extra_result));
        }

        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![body_op_result],
                },
            },
        );

        // Exit
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.loop_roles.insert(header, LoopRole::LoopHeader);

        TestLoop {
            func,
            header,
            body,
            exit,
            body_result: body_op_result,
        }
    }

    fn attr_int(op: &TirOp, name: &str) -> Option<i64> {
        match op.attrs.get(name) {
            Some(AttrValue::Int(value)) => Some(*value),
            _ => None,
        }
    }

    #[test]
    fn unrolls_small_range_loop_into_four_body_copies() {
        let TestLoop {
            mut func,
            header,
            body,
            ..
        } = build_test_loop(0, 4, 1, 1, |_, _| vec![]);

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 4);
        assert_eq!(stats.ops_added, 4);
        assert_eq!(stats.ops_removed, 5);
        assert!(!func.blocks.contains_key(&header));
        assert!(!func.blocks.contains_key(&body));
        assert!(!func.loop_roles.contains_key(&header));

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, args } => {
                assert!(args.is_empty());
                *target
            }
            _ => panic!("entry should branch to unrolled landing block"),
        };
        assert_ne!(landing, header);
        assert_ne!(landing, body);

        let landing_block = &func.blocks[&landing];
        let add_count = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::Add)
            .count();
        assert_eq!(add_count, 4, "body Add op should be cloned once per trip");

        let iteration_constants: Vec<_> = landing_block
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| attr_int(op, "value"))
            .collect();
        assert_eq!(iteration_constants, vec![0, 1, 2, 3]);
    }

    #[test]
    fn substitutes_exit_arg_induction_var_with_final_iteration_value() {
        let TestLoop {
            mut func,
            exit,
            ..
        } = build_test_loop(2, 10, 2, 1, |ind_var, _| vec![ind_var]);

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 4);

        let landing = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to unrolled landing block"),
        };
        let landing_block = &func.blocks[&landing];
        let branch_args = match &landing_block.terminator {
            Terminator::Branch { target, args } => {
                assert_eq!(*target, exit);
                args
            }
            _ => panic!("landing should branch to exit"),
        };
        assert_eq!(branch_args.len(), 1);

        let final_value_op = landing_block
            .ops
            .iter()
            .find(|op| op.results == vec![branch_args[0]])
            .expect("final induction value should be materialized in landing block");
        assert_eq!(final_value_op.opcode, OpCode::ConstInt);
        assert_eq!(attr_int(final_value_op, "value"), Some(10));
    }

    #[test]
    fn does_not_unroll_body_larger_than_max_unroll_ops() {
        let TestLoop {
            mut func,
            header,
            body,
            body_result,
            ..
        } = build_test_loop(0, 4, 1, MAX_UNROLL_OPS + 1, |_, _| vec![]);
        let entry_target_before = match &func.blocks[&func.entry_block].terminator {
            Terminator::Branch { target, .. } => *target,
            _ => panic!("entry should branch to header"),
        };

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_added, 0);
        assert_eq!(stats.ops_removed, 0);
        assert_eq!(entry_target_before, header);
        assert!(func.blocks.contains_key(&header));
        assert!(func.blocks.contains_key(&body));
        assert!(func.loop_roles.contains_key(&header));
        assert_eq!(
            func.blocks[&body].ops[0].results,
            vec![body_result],
            "oversized body should remain intact"
        );
    }
}
