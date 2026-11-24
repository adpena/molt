//! Checked raw integer loops: loop versioning with exact transfer (HF-857).
//!
//! A loop accumulator (`total += i`) of exact int type has no proven bound,
//! so the value-range analysis correctly refuses it a raw carrier: a bare
//! `iadd` would wrap at 2^63. Without this pass the accumulator is a boxed
//! word, and each update pays a tag test, an unbox, a rebox, reference
//! counting and a runtime fallback.
//!
//! The pass versions the loop instead:
//!
//! ```text
//! fast loop L          every selected op is CheckedAdd/CheckedMul:
//!   (r, of) = checked(a, b)
//!   cond_br of, transfer, rest_of_block
//! transfer             exact = add(a, b)    (boxed, BigInt-exact)
//!   br slow_rest        enters L' just after the op's clone
//! slow loop L'         a clone of L with plain boxed arithmetic
//! ```
//!
//! On overflow the iteration continues in the slow loop at the matching
//! point, with the exact result in place of the wrapped one. Nothing is
//! replayed, so the loop body may branch, call, store and raise. The shared
//! SSA updater ([`crate::tir::ssa_update`]) then gives every use the
//! definition that reaches it: the slow loop's first partial iteration reads
//! the fast loop's values, and code after the loop merges both loops. The
//! wrapped result is never observed, because on the overflow path the
//! transfer's exact result is the reaching definition.
//!
//! Selection is an optimistic fixpoint over the loop: an exact-int `Add` or
//! `Mul` is selected when both operands are raw-capable (inline-safe by the
//! representation authority, a constant, a selected result, or a loop phi
//! whose incomings are all raw-capable). The representation plan then makes
//! each selected result and accumulator phi a checked full-range raw carrier
//! (`Repr::RawI64FullDeopt`), and codegen emits a machine add with an
//! overflow branch.
//!
//! The outermost qualifying loop of a nest is versioned whole, so an
//! accumulator carried across an inner loop stays raw in both.
//!
//! For a target whose backend consumes final TIR, the fast loop also reads
//! each loop-invariant boxed exact int that a numeric op of the loop takes
//! (the bound `n` of `while i < n`) as its inline payload. The loop's only
//! entry tests each one once:
//!
//! ```text
//! preheader            t = IsInlineInt n ; cond_br t, enter_fast, L'
//! enter_fast           n' = UnboxInlineInt n ; br L    (L reads n')
//! ```
//!
//! A big-int bound runs the slow loop from the start. In the fast loop the
//! bound lies in the inline window, so the range analysis bounds the
//! counter and the loop's back edges, and with them a bounded-step
//! accumulator (`total += i & 7`). Selection reads those ranges: an op whose
//! exact result they place inside i64 takes no overflow check; it carries
//! the proof (`TirOp::mark_i64_exact_proof`), and codegen emits the plain
//! machine op on its full-range raw carrier. A subtraction has no checked
//! form, so it is selected only with that proof (`i * 3 - (i >> 1)`).
//!
//! Refusals (loops with handler regions, suspension points or exception
//! edges into the loop, loops over the size budget, failed SSA repair) leave
//! the loop unchanged. `MOLT_CHECKED_INT_LOOPS_STATS=1` reports each
//! versioned and refused loop to stderr and as a debug artifact.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use std::collections::BTreeMap;

use crate::tir::analysis::{AnalysisManager, LoopForest};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::clone_attrs_without_simple_names;
use crate::tir::dominators::{exception_label_to_block, exception_transfer_target};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    CheckedIntLoopRule, opcode_checked_int_loop_rule_table,
    opcode_is_exception_handler_region_table, opcode_is_state_machine_table,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::value_identity::{copy_value_source, scalar_value_source};
use crate::tir::passes::value_range::ValueRange;
use crate::tir::ssa_update::{self, RepairRefusal, Variable};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use molt_ir::backend_environment::{self as backend_env, BackendEnv};

use super::PassStats;

mod short_ascii;

/// The largest loop, in operations, that the pass clones. Versioning doubles
/// the loop's code; past this size the code growth outweighs the gain.
const MAX_VERSIONED_OPS: usize = 2048;

/// Why a loop was left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// No exact-int add or multiply with raw-capable operands feeds a loop phi.
    NoAccumulator,
    /// The loop holds a try/except region; its handler labels would need a clone.
    HandlerRegion,
    /// The loop holds a suspension point of a generator or coroutine.
    Suspension,
    /// An exception edge enters a block of the loop.
    ExceptionEdgeIntoLoop,
    /// The loop exceeds `MAX_VERSIONED_OPS`.
    TooLarge,
    /// The SSA repair refused the versioned loop.
    Repair(RepairRefusal),
}

/// `final_tir`: the target's backend consumes final TIR, so the fast loop may
/// test and unbox an inline int (`IsInlineInt`, `UnboxInlineInt`) and carry an
/// op whose exact result is proven in i64 as a plain machine op.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, final_tir: bool) -> PassStats {
    let mut stats = PassStats {
        name: "checked_int_loops",
        ..PassStats::default()
    };
    // Module chunks keep every value boxed (`floor_module_chunk_carriers`).
    if molt_ir::ir::is_module_chunk_function(&func.name) {
        return stats;
    }
    let forest = am.get::<LoopForest>(func).clone();
    if forest.headers.is_empty() {
        return stats;
    }
    let debug = backend_env::var(BackendEnv::MOLT_CHECKED_INT_LOOPS_STATS).as_deref() == Ok("1");
    let mut report: Vec<(BlockId, Result<usize, Refusal>)> = Vec::new();

    // Outermost loops first: a versioned loop's inner loops are versioned
    // with it. A refused loop lets its inner loops try on their own.
    let mut headers = forest.headers.clone();
    headers.sort_by_key(|header| std::cmp::Reverse(forest.bodies[header].len()));
    let mut covered: HashSet<BlockId> = HashSet::default();
    let mut ranges = None;
    for header in headers {
        if covered.contains(&header) {
            continue;
        }
        let body = &forest.bodies[&header];
        let vr = ranges.get_or_insert_with(|| am.get::<ValueRange>(func).clone());
        let outcome = try_version_loop(func, header, body, vr, final_tir);
        if let Ok(added) = outcome {
            stats.ops_added += added;
            // An exact proof alone adds no op: the attribute is the change.
            stats.values_changed += usize::from(added == 0);
            covered.extend(body.iter().copied());
            // Value ranges change with the CFG: recompute them for the next
            // loop instead of reading the cached result of the old function.
            am.invalidate_cfg();
            ranges = None;
        }
        report.push((header, outcome));
    }
    if debug {
        write_report(func, &report);
    }
    stats
}

fn try_version_loop(
    func: &mut TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
    final_tir: bool,
) -> Result<usize, Refusal> {
    check_loop_shape(func, body)?;
    let snapshot = func.clone();
    if final_tir && let Some(ascii) = short_ascii::ShortAscii::admit(func, header, body, vr) {
        let outcome = version(func, body, &Selection::default(), None, Some(&ascii));
        if outcome.is_err() {
            *func = snapshot;
        }
        return outcome;
    }
    let guard = if final_tir {
        invariant_guard(func, header, body, vr)
    } else {
        None
    };
    // With a guard, selection reads the ranges of the fast loop as it will
    // be: the invariants replaced by their payloads, whose range is the
    // inline window. The rewrite keeps every block and op index of the loop.
    let selection = match &guard {
        Some(guard) => {
            let mut fast = func.clone();
            guard.rewrite(&mut fast, body);
            fast.blocks
                .get_mut(&guard.preheader)
                .expect("preheader exists")
                .ops
                .extend(guard.unbox_ops());
            let fast_vr = <ValueRange as crate::tir::analysis::Analysis>::compute(&fast);
            select(&fast, header, body, &fast_vr, final_tir)
        }
        None => select(func, header, body, vr, final_tir),
    };
    let selection = match selection {
        Ok(selection) => selection,
        Err(refusal) => {
            *func = snapshot;
            return Err(refusal);
        }
    };
    if guard.is_none() && selection.is_empty() {
        *func = snapshot;
        return Err(Refusal::NoAccumulator);
    }
    // Proofs alone need no second loop: they hold where the ops are.
    if guard.is_none() && selection.checked.is_empty() {
        for &(block, index) in &selection.exact {
            func.blocks
                .get_mut(&block)
                .expect("selected block exists")
                .ops[index]
                .mark_i64_exact_proof();
        }
        for &(block, index) in &selection.unchecked {
            uncheck(func, block, index);
        }
        return Ok(0);
    }
    match version(func, body, &selection, guard.as_ref(), None) {
        Ok(added) => Ok(added),
        Err(refusal) => {
            *func = snapshot;
            Err(refusal)
        }
    }
}

/// The loop-invariant boxed exact ints the fast loop reads as inline
/// payloads, and the loop's only entry, which tests them.
struct InvariantGuard {
    header: BlockId,
    /// The loop's only predecessor outside it; it ends in a plain branch to
    /// the header.
    preheader: BlockId,
    /// Each invariant the preheader tests and the payload it unboxes.
    payloads: Vec<(ValueId, ValueId)>,
    /// Each operand the fast loop reads as a payload: an invariant, or a
    /// header parameter that carries one unchanged (`header_invariants`).
    reads: HashMap<ValueId, ValueId>,
}

impl InvariantGuard {
    /// Read each invariant's payload in the numeric ops of `body`.
    fn rewrite(&self, func: &mut TirFunction, body: &HashSet<BlockId>) {
        let payload = &self.reads;
        for bid in body {
            let block = func.blocks.get_mut(bid).expect("loop block exists");
            for op in block
                .ops
                .iter_mut()
                .filter(|op| takes_inline_payload(op.opcode))
            {
                for operand in &mut op.operands {
                    if let Some(&raw) = payload.get(operand) {
                        *operand = raw;
                    }
                }
            }
        }
    }

    /// Replace the preheader's branch to the header with one tag test per
    /// invariant; a failed test enters `slow_header` with the same arguments,
    /// and the last success unboxes every payload and enters the fast loop.
    /// Returns the operations added.
    fn enter(&self, func: &mut TirFunction, slow_header: BlockId) -> usize {
        let preheader = func
            .blocks
            .get_mut(&self.preheader)
            .expect("preheader exists");
        let entry = std::mem::replace(&mut preheader.terminator, Terminator::Unreachable);
        let Terminator::Branch { target, args } = entry else {
            unreachable!("the invariant guard's preheader ends in a plain branch");
        };
        debug_assert_eq!(target, self.header);
        let mut test_block = self.preheader;
        for &(boxed, _) in &self.payloads {
            let inline = func.fresh_value();
            func.value_types.insert(inline, TirType::Bool);
            let next = func.fresh_block();
            let mut test = TirOp::new(OpCode::IsInlineInt, vec![boxed], vec![inline]);
            test.mark_nothrow_proof();
            let block = func.blocks.entry(test_block).or_insert_with(|| TirBlock {
                id: test_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Unreachable,
            });
            block.ops.push(test);
            block.terminator = Terminator::CondBranch {
                cond: inline,
                then_block: next,
                then_args: vec![],
                else_block: slow_header,
                else_args: args.clone(),
            };
            test_block = next;
        }
        let mut unbox = self.unbox_ops();
        for op in &mut unbox {
            op.mark_nothrow_proof();
        }
        let added = 2 * self.payloads.len();
        func.blocks.insert(
            test_block,
            TirBlock {
                id: test_block,
                args: vec![],
                ops: unbox,
                terminator: Terminator::Branch {
                    target: self.header,
                    args,
                },
            },
        );
        added
    }

    /// `payload = UnboxInlineInt invariant` for each invariant.
    fn unbox_ops(&self) -> Vec<TirOp> {
        self.payloads
            .iter()
            .map(|&(boxed, raw)| TirOp::new(OpCode::UnboxInlineInt, vec![boxed], vec![raw]))
            .collect()
    }
}

/// An op that reads an invariant's inline payload in the fast loop. With a
/// boxed other operand the late lowering stage takes the payload raw, with no
/// tag test. With a raw other operand the op needs a raw lane for any result:
/// a comparison has one, and an add or multiply of exact ints is selected
/// here (checked, or exact in i64). Other ops keep the word: a shift, bitwise
/// op or subtraction of two raw ints whose result is not proven inline boxes
/// both and calls the runtime every iteration (`bench_loop_invariant_shift`
/// measured 7x slower).
fn takes_inline_payload(opcode: OpCode) -> bool {
    use crate::tir::op_kinds_generated::{
        FastPathExpansionRule, opcode_fast_path_expansion_rule_table,
    };
    matches!(
        opcode_fast_path_expansion_rule_table(opcode),
        FastPathExpansionRule::NumericAdd
            | FastPathExpansionRule::NumericMul
            | FastPathExpansionRule::NumericCompare
    )
}

/// Find the invariants of the loop at `header` and allocate their payloads.
/// `None` when no numeric op of the loop takes a boxed exact int defined
/// outside it, or the loop has no single plain entry edge.
fn invariant_guard(
    func: &mut TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
) -> Option<InvariantGuard> {
    let preheader = plain_preheader(func, header, body)?;
    let mut defined: HashSet<ValueId> = HashSet::default();
    for bid in body {
        let block = &func.blocks[bid];
        defined.extend(block.args.iter().map(|arg| arg.id));
        defined.extend(block.ops.iter().flat_map(|op| op.results.iter().copied()));
    }
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    let repr = crate::representation_facts::repr_by_value_for(func, Some(vr));
    let carried = header_invariants(func, header, body, preheader, &defined);
    let mut blocks: Vec<BlockId> = body.iter().copied().collect();
    blocks.sort_by_key(|block| block.0);
    // Each operand read as a payload, with the invariant the guard tests.
    let mut reads: Vec<(ValueId, ValueId)> = Vec::new();
    for bid in blocks {
        for op in &func.blocks[&bid].ops {
            if !takes_inline_payload(op.opcode) {
                continue;
            }
            for &operand in &op.operands {
                let tested = if defined.contains(&operand) {
                    carried.get(&operand).copied()
                } else {
                    Some(operand)
                };
                let Some(tested) = tested else {
                    continue;
                };
                // An exact int of any magnitude: the range stop of
                // `for i in range(n)` is `n.__index__()`.
                if matches!(exact.get(&operand), Some(TirType::I64 | TirType::BigInt))
                    && matches!(
                        repr.get(&operand),
                        Some(molt_ir::repr::Repr::DynBox | molt_ir::repr::Repr::MaybeBigInt)
                    )
                    && !reads.iter().any(|(read, _)| *read == operand)
                {
                    reads.push((operand, tested));
                }
            }
        }
    }
    if reads.is_empty() {
        return None;
    }
    let mut payloads: Vec<(ValueId, ValueId)> = Vec::new();
    let mut read_payloads: HashMap<ValueId, ValueId> = HashMap::default();
    for (operand, tested) in reads {
        let raw = match payloads.iter().find(|(boxed, _)| *boxed == tested) {
            Some(&(_, raw)) => raw,
            None => {
                let raw = func.fresh_value();
                func.value_types.insert(raw, TirType::I64);
                payloads.push((tested, raw));
                raw
            }
        };
        read_payloads.insert(operand, raw);
    }
    Some(InvariantGuard {
        header,
        preheader,
        payloads,
        reads: read_payloads,
    })
}

fn plain_preheader(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
) -> Option<BlockId> {
    let labels = exception_label_to_block(func);
    // The lift keeps unreachable `loop_end -> header` edges as loop metadata;
    // only an executable edge enters the loop.
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);
    let mut entries = func.blocks.values().filter(|block| {
        reachable.contains(&block.id)
            && !body.contains(&block.id)
            && block.terminator.successors().contains(&header)
    });
    let preheader = entries.next()?;
    if entries.next().is_some()
        || !matches!(preheader.terminator, Terminator::Branch { target, .. } if target == header)
        || crate::tir::dominators::exception_successors(preheader, &labels).contains(&header)
    {
        return None;
    }
    Some(preheader.id)
}

/// The header parameters that carry one value defined outside the loop
/// unchanged, each with that value: the entry edge passes it, and every back
/// edge passes it again or the parameter itself, through value-preserving
/// copies (a fused generator's slot for its range stop, held by binding
/// aliases). Such a parameter is the invariant in every iteration.
fn header_invariants(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    preheader: BlockId,
    defined: &HashSet<ValueId>,
) -> HashMap<ValueId, ValueId> {
    let mut copy_of: HashMap<ValueId, ValueId> = HashMap::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Copy
                && let ([source], [result]) = (op.operands.as_slice(), op.results.as_slice())
            {
                copy_of.insert(*result, *source);
            }
        }
    }
    let root = |mut value: ValueId| {
        let mut steps = 0;
        while let Some(&source) = copy_of.get(&value) {
            value = source;
            steps += 1;
            if steps > 64 {
                break;
            }
        }
        value
    };
    let params: Vec<ValueId> = func.blocks[&header].args.iter().map(|arg| arg.id).collect();
    let mut entry: Option<Vec<ValueId>> = None;
    let mut back: Vec<Vec<ValueId>> = Vec::new();
    let mut blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for bid in blocks {
        func.blocks[&bid].terminator.for_each_edge(|target, args| {
            if target != header {
                return;
            }
            if bid == preheader {
                entry = Some(args.to_vec());
            } else if body.contains(&bid) {
                back.push(args.to_vec());
            }
        });
    }
    let Some(entry) = entry else {
        return HashMap::default();
    };
    let mut carried = HashMap::default();
    for (index, &param) in params.iter().enumerate() {
        let Some(&first) = entry.get(index) else {
            continue;
        };
        let invariant = root(first);
        if defined.contains(&invariant) {
            continue;
        }
        if back.iter().all(|args| {
            args.get(index)
                .is_some_and(|&arg| arg == param || root(arg) == invariant || root(arg) == param)
        }) {
            carried.insert(param, first);
        }
    }
    carried
}

fn check_loop_shape(func: &TirFunction, body: &HashSet<BlockId>) -> Result<(), Refusal> {
    let labels = exception_label_to_block(func);
    let mut ops = 0usize;
    for bid in body {
        let block = &func.blocks[bid];
        ops += block.ops.len();
        for op in &block.ops {
            if opcode_is_exception_handler_region_table(op.opcode) {
                return Err(Refusal::HandlerRegion);
            }
            if opcode_is_state_machine_table(op.opcode) {
                return Err(Refusal::Suspension);
            }
            if exception_transfer_target(op, &labels).is_some_and(|target| body.contains(&target)) {
                return Err(Refusal::ExceptionEdgeIntoLoop);
            }
        }
        if matches!(block.terminator, Terminator::StateDispatch { .. }) {
            return Err(Refusal::Suspension);
        }
    }
    if body.contains(&func.entry_block) {
        return Err(Refusal::ExceptionEdgeIntoLoop);
    }
    if ops > MAX_VERSIONED_OPS {
        return Err(Refusal::TooLarge);
    }
    Ok(())
}

/// The checked opcode for an exact-int arithmetic opcode.
fn checked_opcode(opcode: OpCode) -> Option<OpCode> {
    match opcode_checked_int_loop_rule_table(opcode) {
        CheckedIntLoopRule::CheckedAdd => Some(OpCode::CheckedAdd),
        CheckedIntLoopRule::CheckedMul => Some(OpCode::CheckedMul),
        CheckedIntLoopRule::ExactSub | CheckedIntLoopRule::None => None,
    }
}

/// Select the loop's raw-capable arithmetic. `NoAccumulator` when no header
/// phi of the loop is raw-capable.
fn select(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
    final_tir: bool,
) -> Result<Selection, Refusal> {
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    let is_int = |value: &ValueId| exact.get(value) == Some(&TirType::I64);
    let inline_safe = crate::representation_facts::raw_i64_safe_values_for(func, vr);
    let mut constants: HashSet<ValueId> = HashSet::default();
    let mut arithmetic: Vec<(BlockId, usize, ValueId, [ValueId; 2])> = Vec::new();
    let mut checked_before: Vec<(BlockId, usize, ValueId, [ValueId; 2])> = Vec::new();
    let mut copies: Vec<(ValueId, ValueId)> = Vec::new();
    let mut phis: Vec<(BlockId, usize, ValueId)> = Vec::new();
    for (&bid, block) in &func.blocks {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt && op.results.first().is_some_and(is_int) {
                constants.insert(op.results[0]);
            }
        }
        if !body.contains(&bid) {
            continue;
        }
        for (index, arg) in block.args.iter().enumerate() {
            if is_int(&arg.id) {
                phis.push((bid, index, arg.id));
            }
        }
        for (index, op) in block.ops.iter().enumerate() {
            let rule = opcode_checked_int_loop_rule_table(op.opcode);
            if rule != CheckedIntLoopRule::None
                && let ([lhs, rhs], [result]) = (&op.operands[..], &op.results[..])
                && let (lhs, rhs, result) = (*lhs, *rhs, *result)
                && is_int(&lhs)
                && is_int(&rhs)
                && is_int(&result)
                && !inline_safe.contains(&result)
                // An op with no checked form is raw only with the proof.
                && (rule != CheckedIntLoopRule::ExactSub
                    || final_tir && vr.range_of(result).is_bounded())
            {
                arithmetic.push((bid, index, result, [lhs, rhs]));
            } else if is_checked(op.opcode)
                && let ([lhs, rhs], [result, _]) = (&op.operands[..], &op.results[..])
            {
                // A loop versioned before (a type-flow clone of a versioned
                // body) already checks this op; its result is a raw carrier.
                checked_before.push((bid, index, *result, [*lhs, *rhs]));
            } else if let Some(source) = copy_value_source(op).or_else(|| scalar_value_source(op))
                && let [result] = op.results[..]
                && is_int(&result)
            {
                copies.push((result, source));
            }
        }
    }
    let incomings = crate::representation_facts::block_arg_incomings_for(func);

    // Optimistic fixpoint: assume every candidate raw-capable, then drop each
    // one with an operand or incoming that is not.
    let mut capable: HashSet<ValueId> =
        arithmetic.iter().map(|&(_, _, result, _)| result).collect();
    capable.extend(copies.iter().map(|&(result, _)| result));
    capable.extend(phis.iter().map(|&(_, _, phi)| phi));
    let checked_results: HashSet<ValueId> = checked_before
        .iter()
        .map(|&(_, _, result, _)| result)
        .collect();
    let raw = |value: &ValueId, capable: &HashSet<ValueId>| {
        capable.contains(value)
            || inline_safe.contains(value)
            || constants.contains(value)
            || checked_results.contains(value)
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &(_, _, result, operands) in &arithmetic {
            if capable.contains(&result) && !operands.iter().all(|v| raw(v, &capable)) {
                capable.remove(&result);
                changed = true;
            }
        }
        for &(result, source) in &copies {
            if capable.contains(&result) && !raw(&source, &capable) {
                capable.remove(&result);
                changed = true;
            }
        }
        for &(block, index, phi) in &phis {
            if capable.contains(&phi)
                && !incomings
                    .get(&(block, index))
                    .is_some_and(|srcs| !srcs.is_empty() && srcs.iter().all(|v| raw(v, &capable)))
            {
                capable.remove(&phi);
                changed = true;
            }
        }
    }
    let header_args: Vec<ValueId> = func.blocks[&header].args.iter().map(|arg| arg.id).collect();
    if !header_args.iter().any(|arg| capable.contains(arg)) {
        return Err(Refusal::NoAccumulator);
    }
    let mut selection = Selection::default();
    for &(block, index, result, _) in &arithmetic {
        if !capable.contains(&result) {
            continue;
        }
        if final_tir && vr.range_of(result).is_bounded() {
            selection.exact.push((block, index));
        } else {
            selection.checked.push((block, index));
        }
    }
    // A checked op whose operands are raw and whose exact result the ranges
    // place inside i64 loses its check.
    for &(block, index, result, operands) in &checked_before {
        if final_tir
            && vr.range_of(result).is_bounded()
            && operands.iter().all(|v| raw(v, &capable))
        {
            selection.unchecked.push((block, index));
        }
    }
    selection
        .checked
        .sort_by_key(|&(block, index)| (block.0, index));
    selection
        .exact
        .sort_by_key(|&(block, index)| (block.0, index));
    selection
        .unchecked
        .sort_by_key(|&(block, index)| (block.0, index));
    Ok(selection)
}

/// The selected arithmetic ops, as (block, op index), in deterministic order.
#[derive(Default)]
struct Selection {
    /// Ops the fast loop checks for overflow.
    checked: Vec<(BlockId, usize)>,
    /// Ops whose exact result the ranges place inside i64: a plain machine op.
    exact: Vec<(BlockId, usize)>,
    /// Checked ops of an earlier versioning whose exact result the ranges
    /// now place inside i64: each becomes a plain op with the proof.
    unchecked: Vec<(BlockId, usize)>,
}

impl Selection {
    fn is_empty(&self) -> bool {
        self.checked.is_empty() && self.exact.is_empty() && self.unchecked.is_empty()
    }
}

/// A checked op: its first result is the wrapped value, its second the
/// overflow flag.
fn is_checked(opcode: OpCode) -> bool {
    use crate::tir::op_kinds_generated::{
        ReprRawI64FullDeoptSeedRule, opcode_repr_raw_i64_full_deopt_seed_rule_table,
    };
    opcode_repr_raw_i64_full_deopt_seed_rule_table(opcode)
        == ReprRawI64FullDeoptSeedRule::CheckedResultZero
}

/// Drop the overflow check of the checked op at `index`, the last op of
/// `block`, whose flag ends the block: the op becomes its plain form with the
/// proof, and the block goes on to its no-overflow successor. Returns the
/// flag, or `None` (and changes nothing) for any other shape.
fn uncheck(func: &mut TirFunction, block: BlockId, index: usize) -> Option<ValueId> {
    let block = func.blocks.get_mut(&block)?;
    if index + 1 != block.ops.len() {
        return None;
    }
    let [_, flag] = block.ops[index].results[..] else {
        return None;
    };
    let Terminator::CondBranch {
        cond,
        else_block,
        else_args,
        ..
    } = &block.terminator
    else {
        return None;
    };
    if *cond != flag {
        return None;
    }
    let rest = Terminator::Branch {
        target: *else_block,
        args: else_args.clone(),
    };
    // The plain op computes what the checked op's first result holds; the
    // value-range registry names that operation for both.
    use crate::tir::op_kinds_generated::{
        ValueRangeTransferRule, opcode_value_range_transfer_rule_table,
    };
    let plain = match opcode_value_range_transfer_rule_table(block.ops[index].opcode) {
        ValueRangeTransferRule::Add => OpCode::Add,
        ValueRangeTransferRule::Mul => OpCode::Mul,
        _ => return None,
    };
    block.terminator = rest;
    let op = &mut block.ops[index];
    op.opcode = plain;
    op.results.truncate(1);
    op.mark_i64_exact_proof();
    Some(flag)
}

/// Clone the loop, check the selected ops, wire each overflow to the slow
/// loop, enter the fast loop through the invariant guard, and repair SSA.
/// Returns the number of operations added.
fn version(
    func: &mut TirFunction,
    body: &HashSet<BlockId>,
    selection: &Selection,
    guard: Option<&InvariantGuard>,
    ascii: Option<&short_ascii::ShortAscii>,
) -> Result<usize, Refusal> {
    let selected = &selection.checked[..];
    let mut blocks: Vec<BlockId> = body.iter().copied().collect();
    blocks.sort_by_key(|block| block.0);

    // 1. The slow loop: fresh ids for every block and every value it defines.
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::default();
    for &bid in &blocks {
        block_map.insert(bid, func.fresh_block());
    }
    let mut value_map: HashMap<ValueId, ValueId> = HashMap::default();
    let mut defined: Vec<(ValueId, TirType)> = Vec::new();
    for &bid in &blocks {
        let block = &func.blocks[&bid];
        let block_values: Vec<(ValueId, TirType)> = block
            .args
            .iter()
            .map(|arg| (arg.id, arg.ty.clone()))
            .chain(block.ops.iter().flat_map(|op| {
                op.results.iter().map(|&result| {
                    let ty = func
                        .value_types
                        .get(&result)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    (result, ty)
                })
            }))
            .collect();
        for (value, ty) in block_values {
            let fresh = func.fresh_value();
            if let Some(known) = func.value_types.get(&value).cloned() {
                func.value_types.insert(fresh, known);
            }
            value_map.insert(value, fresh);
            defined.push((value, ty));
        }
    }
    let map_value = |value: ValueId| value_map.get(&value).copied().unwrap_or(value);
    let mut added = 0usize;
    for &bid in &blocks {
        let source = &func.blocks[&bid];
        let mut terminator = source.terminator.clone();
        terminator.for_each_value_mut(|value| *value = map_value(*value));
        terminator.for_each_edge_mut(|target, _| {
            if let Some(&cloned) = block_map.get(target) {
                *target = cloned;
            }
        });
        let clone = TirBlock {
            id: block_map[&bid],
            args: source
                .args
                .iter()
                .map(|arg| TirValue {
                    id: map_value(arg.id),
                    ty: arg.ty.clone(),
                })
                .collect(),
            ops: source
                .ops
                .iter()
                .map(|op| TirOp {
                    operands: op.operands.iter().map(|&v| map_value(v)).collect(),
                    results: op.results.iter().map(|&v| map_value(v)).collect(),
                    attrs: clone_attrs_without_simple_names(&op.attrs),
                    ..op.clone()
                })
                .collect(),
            terminator,
        };
        added += clone.ops.len();
        func.blocks.insert(clone.id, clone);
    }

    // 2. The fast loop alone holds the facts it was selected under: its exact
    //    ops carry their proof, and its numeric ops read the invariants'
    //    payloads. Both precede the splits, which move ops between blocks.
    for &(block, index) in &selection.exact {
        func.blocks
            .get_mut(&block)
            .expect("selected block exists")
            .ops[index]
            .mark_i64_exact_proof();
    }
    // An earlier versioning's check the ranges now discharge: its clone keeps
    // the check, and its flag leaves the fast loop.
    let unchecked_flags: Vec<ValueId> = selection
        .unchecked
        .iter()
        .filter_map(|&(block, index)| uncheck(func, block, index))
        .collect();
    if let Some(guard) = guard {
        guard.rewrite(func, body);
    }

    // 3. Check each selected op; split its block in both loops after it.
    let mut by_block: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for &(block, index) in selected {
        by_block.entry(block.0).or_default().push(index);
    }
    let mut variables: HashMap<ValueId, Vec<ValueId>> = defined
        .iter()
        .filter(|(value, _)| !unchecked_flags.contains(value))
        .map(|(value, _)| (*value, vec![*value, value_map[value]]))
        .collect();
    for (block, mut indices) in by_block {
        let fast = BlockId(block);
        let slow = block_map[&fast];
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for index in indices {
            let original = func.blocks[&fast].ops[index].clone();
            let result = original.results[0];
            let flag = func.fresh_value();
            func.value_types.insert(flag, TirType::Bool);
            let exact = func.fresh_value();
            func.value_types.insert(exact, TirType::I64);
            let fast_rest = func.fresh_block();
            let slow_rest = func.fresh_block();
            let transfer = func.fresh_block();

            let fast_block = func.blocks.get_mut(&fast).expect("selected block exists");
            let rest_ops = fast_block.ops.split_off(index + 1);
            let checked = &mut fast_block.ops[index];
            checked.opcode = checked_opcode(original.opcode).expect("selected op is arithmetic");
            checked.results.push(flag);
            let rest_terminator = std::mem::replace(
                &mut fast_block.terminator,
                Terminator::CondBranch {
                    cond: flag,
                    then_block: transfer,
                    then_args: vec![],
                    else_block: fast_rest,
                    else_args: vec![],
                },
            );
            func.blocks.insert(
                fast_rest,
                TirBlock {
                    id: fast_rest,
                    args: vec![],
                    ops: rest_ops,
                    terminator: rest_terminator,
                },
            );
            func.move_loop_terminator(fast, fast_rest);

            let slow_block = func.blocks.get_mut(&slow).expect("cloned block exists");
            let slow_ops = slow_block.ops.split_off(index + 1);
            let slow_terminator = std::mem::replace(
                &mut slow_block.terminator,
                Terminator::Branch {
                    target: slow_rest,
                    args: vec![],
                },
            );
            func.blocks.insert(
                slow_rest,
                TirBlock {
                    id: slow_rest,
                    args: vec![],
                    ops: slow_ops,
                    terminator: slow_terminator,
                },
            );

            func.blocks.insert(
                transfer,
                TirBlock {
                    id: transfer,
                    args: vec![],
                    ops: vec![TirOp {
                        results: vec![exact],
                        attrs: clone_attrs_without_simple_names(&original.attrs),
                        ..original.clone()
                    }],
                    terminator: Terminator::Branch {
                        target: slow_rest,
                        args: vec![],
                    },
                },
            );
            variables
                .get_mut(&result)
                .expect("selected result is a loop value")
                .push(exact);
            added += 1;
        }
    }

    // 4. The loop's entry tests each invariant: the fast loop runs when every
    //    one is an inline int, and the slow loop from the start otherwise.
    if let Some(guard) = guard {
        added += guard.enter(func, block_map[&guard.header]);
    }

    if let Some(ascii) = ascii {
        added += ascii.version_fast(func, body, &block_map);
    }

    // 5. Every use reads the definition that reaches it.
    let types: HashMap<ValueId, TirType> = defined.into_iter().collect();
    let mut repair: Vec<Variable> = variables
        .into_iter()
        .map(|(value, names)| Variable {
            ty: types[&value].clone(),
            names,
        })
        .collect();
    repair.sort_by_key(|variable| variable.names[0].0);
    ssa_update::repair(func, &repair).map_err(Refusal::Repair)?;
    Ok(added)
}

fn write_report(func: &TirFunction, report: &[(BlockId, Result<usize, Refusal>)]) {
    let versioned = report.iter().filter(|(_, outcome)| outcome.is_ok()).count();
    let mut text = format!(
        "[checked_int_loops] func '{}': {} versioned, {} refused\n",
        func.name,
        versioned,
        report.len() - versioned
    );
    for (header, outcome) in report {
        match outcome {
            Ok(added) => text.push_str(&format!(
                "  versioned loop @ block {}: {added} ops added\n",
                header.0
            )),
            Err(refusal) => text.push_str(&format!(
                "  refused loop @ block {}: {refusal:?}\n",
                header.0
            )),
        }
    }
    eprint!("{text}");
    let sanitized: String = func
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("checked_int_loops/{sanitized}.txt"),
        text,
    );
}

#[cfg(test)]
mod tests;
