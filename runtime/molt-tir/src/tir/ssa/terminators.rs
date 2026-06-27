use super::*;
use std::collections::HashMap;
impl<'a> SsaContext<'a> {
    pub(super) fn block_for_label(&self, label_id: i64) -> Option<usize> {
        self.cfg.blocks.iter().enumerate().find_map(|(bid, block)| {
            let has_label = (block.start_op..block.end_op).any(|op_idx| {
                let op = &self.ops[op_idx];
                matches!(op.kind.as_str(), "label" | "state_label") && op.value == Some(label_id)
            });
            has_label.then_some(bid)
        })
    }
    pub(super) fn build_terminator(
        &mut self,
        bid: usize,
        var_stacks: &HashMap<String, Vec<ValueId>>,
        block_ops: &mut Vec<TirOp>,
    ) -> Terminator {
        let bb = &self.cfg.blocks[bid];
        let last_op_idx = bb.end_op.saturating_sub(1);
        let last_op = if bb.start_op < bb.end_op {
            Some(&self.ops[last_op_idx])
        } else {
            None
        };

        let succs = &self.cfg.successors[bid];

        // Determine terminator kind from the last op.
        let kind = last_op.map(|o| o.kind.as_str()).unwrap_or("");

        match kind {
            "ret" | "ret_void" | "return" => {
                let mut values = Vec::new();
                if (kind == "ret" || kind == "return")
                    && let Some(op) = last_op
                {
                    // Canonical ret surface:
                    // - multi-value returns use `op.args`
                    // - single-value returns may carry the value in `op.var`
                    //   and/or redundantly in `op.args`
                    //
                    // Treat `op.args` as authoritative when present; only
                    // fall back to `op.var` when args are absent. Otherwise
                    // the roundtrip path duplicates the first return value.
                    let candidates: Vec<&String> = if let Some(ref args) = op.args {
                        args.iter().collect()
                    } else if let Some(ref v) = op.var {
                        vec![v]
                    } else {
                        Vec::new()
                    };
                    for a in candidates {
                        if is_variable(a)
                            && let Some(vid) = self.resolve_known_var(a, var_stacks)
                        {
                            values.push(vid);
                        }
                    }
                }
                Terminator::Return { values }
            }

            "jump" | "goto" | "loop_break" => {
                if let Some(&target_bid) = succs.first() {
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            "if" | "br_if" | "loop_break_if_true" | "loop_break_if_false" => {
                // Resolve the condition.
                let cond = last_op
                    .and_then(|op| {
                        op.args.as_ref().and_then(|a| a.first()).and_then(|a| {
                            if is_variable(a) {
                                self.resolve_known_var(a, var_stacks)
                            } else {
                                None
                            }
                        })
                    })
                    .or(self.undef_value)
                    .unwrap_or_else(|| {
                        self.undef_value
                            .expect("SSA undef value must be initialized")
                    });

                if succs.len() >= 2 {
                    // Successor ordering is preserved from cfg.rs:
                    //   if:                   succs[0] = fall-through (TRUE), succs[1] = else (FALSE)
                    //   br_if:                succs[0] = fall-through (FALSE), succs[1] = branch target (TRUE)
                    //   loop_break_if_true:   succs[0] = break target (TRUE), succs[1] = fall-through (FALSE)
                    //   loop_break_if_false:  succs[0] = fall-through (TRUE), succs[1] = break target (FALSE)
                    //
                    // CondBranch convention: then_block = TRUE path, else_block = FALSE path.
                    let last_kind = last_op.map(|op| op.kind.as_str()).unwrap_or("");
                    // Successors are sorted numerically by sort_unstable(),
                    // so we identify then/else by semantic meaning, not position.
                    // Fall-through = bid+1.
                    let fall_through = bid + 1;
                    let (then_bid, else_bid) = match last_kind {
                        "if" => {
                            // For structured `if`, TRUE = fall-through ONLY when
                            // the THEN body is non-empty.  When the next block
                            // begins with `else`, the THEN body is empty and
                            // cfg.rs routes the TRUE edge directly to the
                            // matching `end_if` block (skipping the else body).
                            // The fall-through (`bid + 1`) is then the FALSE
                            // path (the else block).  Treating fall-through as
                            // TRUE in that case swaps the THEN/ELSE bodies and
                            // miscompiles `if cond: pass / else: <body>`
                            // patterns the frontend emits for guarded cleanup
                            // sequences (e.g. module-level `del eg` after an
                            // `except*` handler).
                            let next_starts_with_else = self
                                .cfg
                                .blocks
                                .get(fall_through)
                                .map(|b| {
                                    b.start_op < self.ops.len()
                                        && self.ops[b.start_op].kind == "else"
                                })
                                .unwrap_or(false);
                            if next_starts_with_else {
                                // Empty THEN: fall-through is the else block (FALSE).
                                if succs[0] == fall_through {
                                    (succs[1], succs[0])
                                } else {
                                    (succs[0], succs[1])
                                }
                            } else {
                                // Non-empty THEN: fall-through is the then body (TRUE).
                                if succs[0] == fall_through {
                                    (succs[0], succs[1])
                                } else {
                                    (succs[1], succs[0])
                                }
                            }
                        }
                        "loop_break_if_true" => {
                            // TRUE = break target (non-fall-through)
                            if succs[0] == fall_through {
                                (succs[1], succs[0])
                            } else {
                                (succs[0], succs[1])
                            }
                        }
                        "loop_break_if_false" => {
                            // TRUE = continue (fall-through)
                            if succs[0] == fall_through {
                                (succs[0], succs[1])
                            } else {
                                (succs[1], succs[0])
                            }
                        }
                        _ => {
                            // br_if: TRUE = branch target, FALSE = fall-through
                            if succs[0] == fall_through {
                                (succs[1], succs[0])
                            } else {
                                (succs[0], succs[1])
                            }
                        }
                    };
                    let then_args = self.collect_branch_args(then_bid, var_stacks);
                    let else_args = self.collect_branch_args(else_bid, var_stacks);
                    Terminator::CondBranch {
                        cond,
                        then_block: BlockId(then_bid as u32),
                        then_args,
                        else_block: BlockId(else_bid as u32),
                        else_args,
                    }
                } else if succs.len() == 1 {
                    let target_bid = succs[0];
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            "loop_break_if_exception" => {
                // Conditional loop break gated on the runtime exception flag.
                // Materialize a non-foldable `ExceptionPending` op into the
                // block body to read the flag, then branch: TRUE (pending) →
                // break target (loop exit), FALSE → fall-through (continue).
                //
                // Successor ordering matches `loop_break_if_true` (cfg.rs adds
                // the break target FIRST, fall-through SECOND), so the TRUE edge
                // is the non-fall-through successor.
                if succs.len() >= 2 {
                    let cond = self.fresh_value_typed();
                    block_ops.push(TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ExceptionPending,
                        operands: vec![],
                        results: vec![cond],
                        attrs: AttrDict::new(),
                        source_span: None,
                    });
                    let fall_through = bid + 1;
                    // TRUE = break target (non-fall-through), FALSE = continue.
                    let (then_bid, else_bid) = if succs[0] == fall_through {
                        (succs[1], succs[0])
                    } else {
                        (succs[0], succs[1])
                    };
                    let then_args = self.collect_branch_args(then_bid, var_stacks);
                    let else_args = self.collect_branch_args(else_bid, var_stacks);
                    Terminator::CondBranch {
                        cond,
                        then_block: BlockId(then_bid as u32),
                        then_args,
                        else_block: BlockId(else_bid as u32),
                        else_args,
                    }
                } else if succs.len() == 1 {
                    // Degenerate: only one successor survived (e.g. the loop body
                    // was proven to fall through).  Branch unconditionally; the
                    // flag read would be dead, so it is intentionally omitted.
                    let target_bid = succs[0];
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            "state_switch" => {
                // The `_poll` state-machine dispatch.  State 0 (initial entry)
                // falls through to the next block (the function's first-entry
                // continuation = `default`); every saved resume state dispatches
                // to the matching suspend op's resume continuation.  The dispatch
                // value is implicit (read from the frame header at lowering time),
                // so this terminator carries no SSA condition value — only the
                // per-edge block-argument incomings, supplied here from the var
                // stacks live at the dispatch point so phi finalization fills the
                // resume-block phis on the dispatch edge (mirror `check_exception`
                // arg supply, but fanning out to every resume block).
                let default_bid = succs.first().copied();
                let Some(default_bid) = default_bid else {
                    // A `state_switch` with no fall-through successor is malformed
                    // (the initial-entry path must exist); be conservative.
                    return Terminator::Unreachable;
                };
                let default_args = self.collect_branch_args(default_bid, var_stacks);
                let mut cases: Vec<(i64, BlockId, Vec<ValueId>)> = Vec::new();
                for &(switch_bid, resume_bid, state_id) in &self.cfg.state_resume_edges {
                    if switch_bid != bid {
                        continue;
                    }
                    let args = self.collect_branch_args(resume_bid, var_stacks);
                    cases.push((state_id, BlockId(resume_bid as u32), args));
                }
                // Deterministic case order (by state id), matching the backends'
                // switch construction.
                cases.sort_by_key(|(state, _, _)| *state);
                Terminator::StateDispatch {
                    cases,
                    default: BlockId(default_bid as u32),
                    default_args,
                }
            }

            "check_exception" => {
                // check_exception terminates the block with an implicit branch
                // to both the fallthrough (no exception) and the exception
                // handler. The check_exception OP itself (emitted in the block
                // body) carries the handler branch args via its operands
                // (see translate_op). The terminator only needs to branch to
                // the fallthrough successor — the handler edge is implicit.
                //
                // We also store args for the handler block here so that when
                // lower_to_simple emits the fallthrough jump, the handler
                // block's arguments are still correct.
                if !succs.is_empty() {
                    let target_bid = succs[0];
                    let args = self.collect_branch_args(target_bid, var_stacks);
                    Terminator::Branch {
                        target: BlockId(target_bid as u32),
                        args,
                    }
                } else {
                    Terminator::Unreachable
                }
            }

            _ => {
                // Default: fall-through to successor(s).
                match succs.len() {
                    0 => Terminator::Unreachable,
                    1 => {
                        let target_bid = succs[0];
                        let args = self.collect_branch_args(target_bid, var_stacks);
                        Terminator::Branch {
                            target: BlockId(target_bid as u32),
                            args,
                        }
                    }
                    _ => {
                        // Multiple successors from a non-branch op — branch to
                        // fallthrough, exception handler args are passed via the
                        // check_exception op's inline branch arguments.
                        let target_bid = succs[0];
                        let args = self.collect_branch_args(target_bid, var_stacks);
                        Terminator::Branch {
                            target: BlockId(target_bid as u32),
                            args,
                        }
                    }
                }
            }
        }
    }
    pub(super) fn collect_branch_args(
        &self,
        target_bid: usize,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Vec<ValueId> {
        self.block_arg_vars[target_bid]
            .iter()
            .map(|var| {
                // Use the current stack-top definition for this variable.
                // If the variable has no reaching definition on this path
                // (e.g., a loop-body variable at the loop entry edge), use
                // the shared undef value.  This is correct SSA semantics:
                // on the first iteration the value is undefined, and the
                // loop header's phi merges undef (entry edge) with the
                // actual value (back-edge).
                self.resolve_known_var(var, var_stacks)
                    .or(self.undef_value)
                    .expect("SSA undef value must be initialized")
            })
            .collect()
    }
}
