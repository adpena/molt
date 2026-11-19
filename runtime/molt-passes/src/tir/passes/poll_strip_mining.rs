//! Strip-mine the back-edge poll of a counted loop (call_anatomy.md 4.8,
//! HF-847; HotSpot's loop strip mining).
//!
//! A canonical loop back edge polls for asynchronous work on every iteration:
//! a volatile load of the pending-work word and a branch. For a loop whose
//! exit test is `x < stop`, with `x` rising by at least one per iteration and
//! a body that runs no Python code of its own, the exit test can carry the
//! poll for free. The loop carries a chunk limit as a new header argument and
//! tests `x < limit` instead of `x < stop`:
//!
//! ```text
//! H(.., limit): .. G: c = lt x, limit; cond_br c, Body, B
//! B:  c2 = lt x, stop; cond_br c2, P, Exit(exit args)     (the real test)
//! P:  poll; t = lt x, THRESHOLD; cond_br t, R1, R2        (once per chunk)
//! R1: cand = add x, CHUNK; u = lt cand, stop; cond_br u, J(cand), R2
//! R2: br J(stop)
//! J(next): br H(.., next)
//! Body .. latch: br H(.., limit)                          (no poll)
//! ```
//!
//! Each entry edge passes the entry value of `x` as `limit`, so the first
//! test fails at once and the boundary block decides: an empty loop exits
//! without a poll, and any other loop polls once on entry, as its first back
//! edge did before. After a refill `limit <= stop`, so `x < limit` implies the
//! real test. The loop runs exactly the iterations it ran before; it polls
//! at least once every `CHUNK` iterations because `x` rises by at least one.
//! An infinite loop stays interruptible for the same reason.
//!
//! The boundary re-enters the header with the header's own arguments, so the
//! header-to-guard path re-runs. The pass admits only a path of pure ops for
//! that reason. It runs first in the drop phase: drop insertion then places
//! the poll's exception-edge releases at its new site, and frame-home
//! placement writes the homes there. Only backends that consume final TIR
//! get the rewrite; the SimpleIR projection rebuilds structured loops from
//! one exit test and cannot express the boundary block.

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::analysis::{AnalysisManager, LoopForest, LoopForestResult};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::exception_regions::{ExceptionOpPosition, ExceptionRegionFacts, ExceptionRegions};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{INLINE_INT47_HI, INLINE_INT47_LO, ScevExpr};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::PassStats;
use super::async_work_poll::{is_call_return_poll, latch_check_site, reachable_lexical_handler};
use super::check_exception_elim::classify::RaiseFacts;
use super::counted_loop::loop_guard_path;
use super::frame_home_placement::is_home_store;
use super::scev::{ScalarEvolution, ScevResult};
use super::value_identity::{no_heap_alias_source, scalar_value_source};

/// Iterations between polls. At a few instructions per call-free iteration
/// the added service latency is microseconds.
pub const CHUNK: i64 = 1024;

/// The largest `x` whose `x + CHUNK` stays in the inline-int47 window.
const REFILL_THRESHOLD: i64 = INLINE_INT47_HI - CHUNK;

/// One loop the pass rewrites; every position is read before any mutation.
struct Plan {
    header: BlockId,
    guard: BlockId,
    compare: (BlockId, usize),
    induction: ValueId,
    bound: ValueId,
    iv_index: usize,
    latch: BlockId,
    poll: (BlockId, usize),
    exit: BlockId,
    exit_args: Vec<ValueId>,
}

/// Why a loop keeps its per-iteration poll. One reason per rule of the
/// contract above, so a test names the rule a loop broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Resumable,
    NoBody,
    NotInnermost,
    NotOneLatch,
    NoExitTest,
    NotLessThan,
    ImpureExitPath,
    NotInlineInts,
    VariantBound,
    NoProgress,
    RunsPython,
    NoLatchPoll,
    PollPayload,
    OtherRegion,
    LaggedBinding,
}

/// Each loop header of `func` with its plan or the rule it broke.
fn plans(func: &TirFunction, am: &mut AnalysisManager) -> Vec<(BlockId, Result<Plan, Refusal>)> {
    let loops = am.get::<LoopForest>(func).clone();
    // A resumable body keeps a poll on every back edge: its loop state lives
    // in the frame across suspensions, and no measurement asks for more.
    if func
        .blocks
        .values()
        .any(|block| matches!(block.terminator, Terminator::StateDispatch { .. }))
    {
        return loops
            .headers
            .iter()
            .map(|&header| (header, Err(Refusal::Resumable)))
            .collect();
    }
    if loops.headers.is_empty() {
        return Vec::new();
    }
    let scev = am.get::<ScalarEvolution>(func).clone();
    let regions = am.get::<ExceptionRegions>(func).clone();
    let context = Context::new(func);
    loops
        .headers
        .iter()
        .map(|&header| (header, context.plan(func, &loops, &scev, &regions, header)))
        .collect()
}

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "poll_strip_mining",
        ..Default::default()
    };
    let trace = crate::tir::pass_manager::trace_func_enabled(&func.name);
    for (header, plan) in plans(func, am) {
        if trace {
            eprintln!(
                "[TIR-STRIP] {} loop {header}: {}",
                func.name,
                plan.as_ref()
                    .map_or_else(|refusal| format!("{refusal:?}"), |_| "rewritten".into())
            );
        }
        if let Ok(plan) = plan {
            stats.ops_added += apply(func, &plan);
            stats.attrs_changed += 1;
        }
    }
    stats
}

struct Context {
    predecessors: HashMap<BlockId, Vec<BlockId>>,
    raise: RaiseFacts,
    definitions: HashMap<ValueId, (BlockId, usize)>,
}

impl Context {
    fn new(func: &TirFunction) -> Self {
        let mut definitions = HashMap::default();
        for (&block_id, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                for &result in &op.results {
                    definitions.insert(result, (block_id, index));
                }
            }
        }
        Self {
            predecessors: crate::tir::dominators::build_pred_map(func),
            raise: RaiseFacts::of(func),
            definitions,
        }
    }

    fn defining_op<'f>(&self, func: &'f TirFunction, value: ValueId) -> Option<&'f TirOp> {
        let &(block, index) = self.definitions.get(&value)?;
        func.blocks.get(&block)?.ops.get(index)
    }

    /// The late lowering stage proved this value fits the inline-int47 window.
    fn inline_proven(&self, func: &TirFunction, value: ValueId) -> bool {
        self.defining_op(func, value).is_some_and(|op| {
            op.has_inline_int_proof()
                || (op.opcode == OpCode::ConstInt
                    && matches!(op.attrs.get("value"),
                        Some(AttrValue::Int(v)) if (INLINE_INT47_LO..=INLINE_INT47_HI).contains(v)))
        })
    }

    fn plan(
        &self,
        func: &TirFunction,
        loops: &LoopForestResult,
        scev: &ScevResult,
        regions: &ExceptionRegionFacts,
        header: BlockId,
    ) -> Result<Plan, Refusal> {
        let body = loops.bodies.get(&header).ok_or(Refusal::NoBody)?;
        // Innermost: an inner loop's back edge keeps the outer chunk's latency
        // unbounded by CHUNK.
        if loops
            .headers
            .iter()
            .any(|other| *other != header && body.contains(other))
        {
            return Err(Refusal::NotInnermost);
        }
        let latch = self
            .single_latch(func, header, body)
            .ok_or(Refusal::NotOneLatch)?;

        // The exit test: `lt x, stop`, continuing on true.
        let path = loop_guard_path(func, header, body, None).ok_or(Refusal::NoExitTest)?;
        let guard = *path.last().ok_or(Refusal::NoExitTest)?;
        let Terminator::CondBranch {
            cond,
            then_block,
            else_block,
            else_args,
            ..
        } = &func.blocks[&guard].terminator
        else {
            return Err(Refusal::NoExitTest);
        };
        if !body.contains(then_block) || body.contains(else_block) {
            return Err(Refusal::NotLessThan);
        }
        // The compare may sit on any block of the path (a `while` test is
        // computed before the blocks that lead to its branch).
        let compare = path
            .iter()
            .find_map(|block_id| {
                func.blocks[block_id]
                    .ops
                    .iter()
                    .position(|op| op.results.first() == Some(cond))
                    .map(|index| (*block_id, index))
            })
            .ok_or(Refusal::NotLessThan)?;
        let compare_op = &func.blocks[&compare.0].ops[compare.1];
        let [induction, bound] = compare_op.operands[..] else {
            return Err(Refusal::NotLessThan);
        };
        if compare_op.opcode != OpCode::Lt || self.use_count(func, *cond) != 1 {
            return Err(Refusal::NotLessThan);
        }
        // Re-entering the header re-runs this path: only pure ops may sit on it.
        let pure = path.iter().all(|block_id| {
            func.blocks[block_id]
                .ops
                .iter()
                .enumerate()
                .all(|(index, op)| {
                    (*block_id, index) == compare
                        || matches!(op.opcode, OpCode::FrameSetLocation | OpCode::ConstInt)
                        || re_executable_copy(op)
                })
        });
        if !pure {
            return Err(Refusal::ImpureExitPath);
        }
        // Both sides are raw ints in the inline window, and the bound is
        // defined outside the loop.
        if !self.inline_proven(func, induction) || !self.inline_proven(func, bound) {
            return Err(Refusal::NotInlineInts);
        }
        if self
            .definitions
            .get(&bound)
            .is_none_or(|(block, _)| body.contains(block))
        {
            return Err(Refusal::VariantBound);
        }
        // Progress: `x` rises by a constant of at least one per iteration.
        match scev.scev_of(induction) {
            ScevExpr::AddRec {
                step, loop_header, ..
            } if loop_header == header && matches!(*step, ScevExpr::Constant(c) if c >= 1) => {}
            _ => return Err(Refusal::NoProgress),
        }
        let iv_index = self
            .header_argument_of(func, header, induction)
            .ok_or(Refusal::NoProgress)?;

        // The body runs no Python code of its own: one poll, at the latch.
        let poll = self.single_latch_poll(func, latch, body)?;
        if !func.blocks[&poll.0].ops[poll.1].operands.is_empty() {
            return Err(Refusal::PollPayload);
        }
        // The boundary polls before the body binds this iteration's values, so
        // every binding the body stores must reach the header as a parameter.
        // A `for` loop binds the counter and passes `counter + step`: its
        // binding lags the parameter, and frame-home placement would carry the
        // lagged value and a body-ran flag through the hot loop, costing more
        // than the poll (int_arith 11.0 -> 12.0 instructions per iteration).
        // Rematerializing it as `counter - step` is the next slice (A2).
        let Terminator::Branch {
            args: latch_args, ..
        } = &func.blocks[&latch].terminator
        else {
            return Err(Refusal::NotOneLatch);
        };
        let carried: HashSet<ValueId> = latch_args
            .iter()
            .map(|&arg| self.canonical(func, arg))
            .collect();
        let lagged = body.iter().any(|block_id| {
            func.blocks[block_id].ops.iter().any(|op| {
                is_home_store(op)
                    && op
                        .operands
                        .first()
                        .is_some_and(|&value| !carried.contains(&self.canonical(func, value)))
            })
        });
        if lagged {
            return Err(Refusal::LaggedBinding);
        }
        // The boundary sits in the poll's lexical region.
        let handler = |block, op_index| {
            reachable_lexical_handler(regions, func, ExceptionOpPosition { block, op_index })
        };
        let at_guard = handler(guard, func.blocks[&guard].ops.len());
        if at_guard.is_none() || at_guard != handler(poll.0, poll.1) {
            return Err(Refusal::OtherRegion);
        }
        Ok(Plan {
            header,
            guard,
            compare,
            induction,
            bound,
            iv_index,
            latch,
            poll,
            exit: *else_block,
            exit_args: else_args.clone(),
        })
    }

    fn single_latch(
        &self,
        func: &TirFunction,
        header: BlockId,
        body: &HashSet<BlockId>,
    ) -> Option<BlockId> {
        let mut latches = self
            .predecessors
            .get(&header)?
            .iter()
            .filter(|pred| body.contains(pred));
        let latch = *latches.next()?;
        if latches.next().is_some() {
            return None;
        }
        matches!(&func.blocks[&latch].terminator,
            Terminator::Branch { target, .. } if *target == header)
        .then_some(latch)
    }

    /// The one marked poll of the body, which must be the latch's own poll;
    /// any other poll means the body calls code that can run Python.
    fn single_latch_poll(
        &self,
        func: &TirFunction,
        latch: BlockId,
        body: &HashSet<BlockId>,
    ) -> Result<(BlockId, usize), Refusal> {
        let mut found = None;
        for block_id in body {
            for (index, op) in func.blocks[block_id].ops.iter().enumerate() {
                if is_call_return_poll(op) || op.opcode == OpCode::AsyncWorkPending {
                    return Err(Refusal::RunsPython);
                }
                if op.is_async_work_poll() {
                    if found.is_some() {
                        return Err(Refusal::RunsPython);
                    }
                    found = Some((*block_id, index));
                }
            }
        }
        let poll = found.ok_or(Refusal::NoLatchPoll)?;
        (latch_check_site(func, latch, &self.predecessors, &self.raise) == Some(poll))
            .then_some(poll)
            .ok_or(Refusal::NoLatchPoll)
    }

    /// The scalar value `value` forwards, through copies and publications.
    fn canonical(&self, func: &TirFunction, mut value: ValueId) -> ValueId {
        for _ in 0..=self.definitions.len() {
            match self.defining_op(func, value).and_then(scalar_value_source) {
                Some(source) => value = source,
                None => break,
            }
        }
        value
    }

    /// The header argument `value` forwards, through scalar copies.
    fn header_argument_of(
        &self,
        func: &TirFunction,
        header: BlockId,
        mut value: ValueId,
    ) -> Option<usize> {
        let args = &func.blocks[&header].args;
        for _ in 0..=self.definitions.len() {
            if let Some(index) = args.iter().position(|arg| arg.id == value) {
                return Some(index);
            }
            value = scalar_value_source(self.defining_op(func, value)?)?;
        }
        None
    }

    fn use_count(&self, func: &TirFunction, value: ValueId) -> usize {
        func.blocks
            .values()
            .map(|block| {
                let mut uses = block
                    .ops
                    .iter()
                    .map(|op| op.operands.iter().filter(|v| **v == value).count())
                    .sum::<usize>();
                block
                    .terminator
                    .for_each_value(|v| uses += usize::from(v == value));
                uses
            })
            .sum()
    }
}

/// A copy the boundary may run again: a no-heap move, or an owned alias that
/// cannot fail or call back (drop insertion, which runs after this pass,
/// releases each instance). A copy that publishes or mutates is not one.
fn re_executable_copy(op: &TirOp) -> bool {
    use crate::tir::op_kinds_generated::{
        copy_kind_is_callback_free_table, copy_kind_is_nothrow_table,
        copy_kind_mints_owned_alias_ref_table,
    };
    no_heap_alias_source(op).is_some()
        || (op.opcode == OpCode::Copy
            && op.operands.len() == 1
            && op.results.len() == 1
            && op.origin.is_some_and(|kind| {
                copy_kind_mints_owned_alias_ref_table(kind)
                    && copy_kind_is_callback_free_table(kind)
                    && copy_kind_is_nothrow_table(kind)
            }))
}

fn typed(func: &mut TirFunction, ty: TirType) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, ty);
    value
}

fn const_int(func: &mut TirFunction, value: i64) -> (ValueId, TirOp) {
    let result = typed(func, TirType::I64);
    let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(value));
    op.mark_inline_int_proof();
    (result, op)
}

/// A raw compare of two inline ints: it cannot raise.
fn less_than(func: &mut TirFunction, lhs: ValueId, rhs: ValueId) -> (ValueId, TirOp) {
    let result = typed(func, TirType::Bool);
    let mut op = TirOp::new(OpCode::Lt, vec![lhs, rhs], vec![result]);
    op.mark_nothrow_proof();
    (result, op)
}

fn insert_block(
    func: &mut TirFunction,
    id: BlockId,
    args: Vec<TirValue>,
    ops: Vec<TirOp>,
    terminator: Terminator,
) {
    func.blocks.insert(
        id,
        TirBlock {
            id,
            args,
            ops,
            terminator,
        },
    );
}

/// Rewrite one planned loop. Returns the number of ops added.
fn apply(func: &mut TirFunction, plan: &Plan) -> usize {
    let header_params: Vec<ValueId> = func.blocks[&plan.header]
        .args
        .iter()
        .map(|arg| arg.id)
        .collect();
    let limit = typed(func, TirType::I64);
    func.blocks
        .get_mut(&plan.header)
        .expect("planned header exists")
        .args
        .push(TirValue {
            id: limit,
            ty: TirType::I64,
        });

    // Entry edges start with `limit = x`: the first test fails and the
    // boundary decides. The latch carries `limit` unchanged.
    let entries: Vec<BlockId> = func
        .blocks
        .iter()
        .filter(|(id, block)| **id != plan.latch && block.terminator.has_successor(plan.header))
        .map(|(id, _)| *id)
        .collect();
    for entry in entries {
        let header = plan.header;
        let iv_index = plan.iv_index;
        func.blocks
            .get_mut(&entry)
            .unwrap()
            .terminator
            .for_each_edge_mut(|target, args| {
                if *target == header {
                    args.push(args[iv_index]);
                }
            });
    }
    match &mut func.blocks.get_mut(&plan.latch).unwrap().terminator {
        Terminator::Branch { args, .. } => args.push(limit),
        _ => unreachable!("the planned latch ends in a branch to the header"),
    }

    // The poll moves to the boundary; the latch keeps a plain observation
    // that check elimination may delete.
    let poll = {
        let check = &mut func.blocks.get_mut(&plan.poll.0).unwrap().ops[plan.poll.1];
        let moved = check.clone();
        check.clear_async_work_poll();
        moved
    };

    let boundary = func.fresh_block();
    let service = func.fresh_block();
    let refill = func.fresh_block();
    let clamp = func.fresh_block();
    let join = func.fresh_block();

    // G: test against the chunk limit; on failure, go to the boundary.
    {
        func.blocks.get_mut(&plan.compare.0).unwrap().ops[plan.compare.1].operands[1] = limit;
        let guard = func.blocks.get_mut(&plan.guard).unwrap();
        let Terminator::CondBranch {
            else_block,
            else_args,
            ..
        } = &mut guard.terminator
        else {
            unreachable!("the planned guard ends in a conditional branch");
        };
        *else_block = boundary;
        else_args.clear();
    }

    // B: the real exit test.
    let (real, real_test) = less_than(func, plan.induction, plan.bound);
    insert_block(
        func,
        boundary,
        vec![],
        vec![real_test],
        Terminator::CondBranch {
            cond: real,
            then_block: service,
            then_args: vec![],
            else_block: plan.exit,
            else_args: plan.exit_args.clone(),
        },
    );

    // P: poll once, then refill without leaving the inline window.
    let (threshold, threshold_op) = const_int(func, REFILL_THRESHOLD);
    let (room, room_test) = less_than(func, plan.induction, threshold);
    insert_block(
        func,
        service,
        vec![],
        vec![poll, threshold_op, room_test],
        Terminator::CondBranch {
            cond: room,
            then_block: refill,
            then_args: vec![],
            else_block: clamp,
            else_args: vec![],
        },
    );

    // R1: `x < REFILL_THRESHOLD`, so `x + CHUNK` fits the window.
    let (chunk, chunk_op) = const_int(func, CHUNK);
    let candidate = typed(func, TirType::I64);
    let mut add = TirOp::new(OpCode::Add, vec![plan.induction, chunk], vec![candidate]);
    add.mark_inline_int_proof();
    add.mark_nothrow_proof();
    let (short, short_test) = less_than(func, candidate, plan.bound);
    insert_block(
        func,
        refill,
        vec![],
        vec![chunk_op, add, short_test],
        Terminator::CondBranch {
            cond: short,
            then_block: join,
            then_args: vec![candidate],
            else_block: clamp,
            else_args: vec![],
        },
    );

    // R2: the chunk reaches the bound.
    insert_block(
        func,
        clamp,
        vec![],
        vec![],
        Terminator::Branch {
            target: join,
            args: vec![plan.bound],
        },
    );

    // J: re-enter the header with its own arguments and the new limit.
    let next = typed(func, TirType::I64);
    let mut args = header_params;
    args.push(next);
    insert_block(
        func,
        join,
        vec![TirValue {
            id: next,
            ty: TirType::I64,
        }],
        vec![],
        Terminator::Branch {
            target: plan.header,
            args,
        },
    );
    7
}

#[cfg(test)]
#[path = "poll_strip_mining_tests.rs"]
mod tests;
