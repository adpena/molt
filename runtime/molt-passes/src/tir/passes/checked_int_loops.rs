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
//! whose incomings are all raw-capable). An exact-int call can also seed a
//! raw lane when its boxed result carries the inline-int tag. A tag miss
//! transfers the original result into the slow loop after the call: calls,
//! side effects, and exception checks keep their original order.
//! The representation plan then makes
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
use molt_ir::repr::Repr;
use std::cell::OnceCell;
use std::collections::BTreeMap;

use crate::tir::analysis::{AnalysisManager, ExecReachable, LoopForest, PredMap};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::clone_attrs_without_simple_names;
use crate::tir::dominators::{exception_label_to_block, exception_transfer_target};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    CheckedIntLoopRule, opcode_checked_int_loop_rule_table,
    opcode_is_exception_handler_region_table, opcode_is_state_machine_table,
};
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::value_identity::{
    ScalarFacts, build_copy_map, copy_value_source, forwarded_python_value_source, resolve_copy,
};
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
    /// No loop-carried SSA value is proven to be an exact Python int.
    NoExactAccumulator,
    /// The exact accumulator cannot be fed by raw-capable definitions.
    BoxedAccumulatorInput,
    /// The loop's integer work already fits its current carriers.
    NoProfitableRewrite,
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
    let mut headers = headers.into_iter().peekable();
    while headers.peek().is_some() {
        // Keep function-wide proofs for every candidate in this revision. Only
        // an admitted rewrite needs to release the borrow and mutate the body.
        let next = {
            let facts = LoopFacts::new(func);
            headers.find_map(|header| {
                if covered.contains(&header) {
                    return None;
                }
                let body = &forest.bodies[&header];
                let result = check_loop_shape(func, body, facts.labels()).and_then(|()| {
                    if am.cached::<ValueRange>().is_none() {
                        let ranges =
                            super::value_range::compute_in_with_scalar_facts(&facts.scalar, am);
                        am.prepopulate::<ValueRange>(ranges);
                    }
                    let entry = if final_tir {
                        am.get::<ExecReachable>(func);
                        am.get::<PredMap>(func);
                        facts.guarded_entry(header, body, am)
                    } else {
                        None
                    };
                    let ranges = am.cached::<ValueRange>().expect("value ranges cached");
                    plan_version(&facts, header, body, ranges, final_tir, entry)
                });
                match result {
                    Ok(rewrite) => Some((header, body, rewrite)),
                    Err(reason) => {
                        if debug {
                            report.push((header, Err(reason)));
                        }
                        None
                    }
                }
            })
        };
        let Some((header, body, rewrite)) = next else {
            break;
        };
        let outcome = rewrite.apply(func, body);
        if let Ok(added) = outcome {
            stats.ops_added += added;
            // An exact proof alone adds no op: the attribute is the change.
            stats.values_changed += usize::from(added == 0);
            covered.extend(body.iter().copied());
            am.invalidate_cfg();
        }
        if debug {
            report.push((header, outcome));
        }
    }
    if debug {
        write_report(func, &report);
    }
    stats
}

/// Lazy projections of the same immutable TIR revision. Small rejected loops
/// inspect their own blocks without rescanning unrelated code.
struct LoopFacts<'f> {
    scalar: ScalarFacts<'f>,
    labels: OnceCell<HashMap<i64, BlockId>>,
    copies: OnceCell<HashMap<ValueId, ValueId>>,
    repr: OnceCell<HashMap<ValueId, Repr>>,
    inline_safe: OnceCell<HashSet<ValueId>>,
    constants: OnceCell<HashSet<ValueId>>,
    incomings: OnceCell<HashMap<(BlockId, usize), Vec<ValueId>>>,
}

impl<'f> LoopFacts<'f> {
    fn new(func: &'f TirFunction) -> Self {
        Self {
            scalar: ScalarFacts::new(func),
            labels: OnceCell::new(),
            copies: OnceCell::new(),
            repr: OnceCell::new(),
            inline_safe: OnceCell::new(),
            constants: OnceCell::new(),
            incomings: OnceCell::new(),
        }
    }

    fn labels(&self) -> &HashMap<i64, BlockId> {
        self.labels
            .get_or_init(|| exception_label_to_block(self.scalar.body()))
    }

    fn representations(&self, vr: &crate::tir::ValueRangeResult) -> &HashMap<ValueId, Repr> {
        self.repr
            .get_or_init(|| crate::representation_facts::repr_by_value_from(&self.scalar, Some(vr)))
    }

    fn guarded_entry<'a>(
        &'a self,
        header: BlockId,
        body: &HashSet<BlockId>,
        am: &'a AnalysisManager,
    ) -> Option<GuardedEntry<'a>> {
        let func = self.scalar.body();
        let reachable = am.cached::<ExecReachable>().expect("reachability cached");
        let preds = am.cached::<PredMap>().expect("predecessors cached");
        let preheader = plain_preheader(func, header, body, preds, reachable, self.labels())?;
        Some(GuardedEntry {
            preheader,
            header_preds: &preds[&header],
            copies: self.copies.get_or_init(|| build_copy_map(func)),
        })
    }
}

enum LoopRewrite {
    Ascii(short_ascii::ShortAscii),
    Arithmetic {
        guard: Option<InvariantGuard>,
        selection: Selection,
    },
}

fn plan_version(
    facts: &LoopFacts<'_>,
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
    final_tir: bool,
    entry: Option<GuardedEntry<'_>>,
) -> Result<LoopRewrite, Refusal> {
    let func = facts.scalar.body();
    if let Some(entry) = entry
        && let Some(ascii) =
            short_ascii::ShortAscii::admit(facts, header, body, vr, entry.preheader)
    {
        return Ok(LoopRewrite::Ascii(ascii));
    }

    let guard = entry.and_then(|entry| invariant_guard(header, body, vr, facts, entry));
    // Only a guard that has work to do needs a speculative analysis body.
    let selection = if let Some(guard) = &guard {
        let mut fast = func.clone();
        guard.install_payloads(&mut fast);
        guard.rewrite(&mut fast, body);
        fast.blocks
            .get_mut(&guard.preheader)
            .expect("preheader exists")
            .ops
            .extend(guard.unbox_ops());
        let fast_facts = LoopFacts::new(&fast);
        let fast_vr =
            crate::tir::passes::value_range::compute_with_scalar_facts(&fast_facts.scalar);
        select(header, body, &fast_vr, &fast_facts, final_tir)?
    } else {
        select(header, body, vr, facts, final_tir)?
    };

    if guard.is_none() && selection.is_empty() {
        return Err(Refusal::NoProfitableRewrite);
    }
    Ok(LoopRewrite::Arithmetic { guard, selection })
}

impl LoopRewrite {
    fn apply(self, func: &mut TirFunction, body: &HashSet<BlockId>) -> Result<usize, Refusal> {
        let (guard, selection) = match self {
            Self::Ascii(ascii) => {
                return commit_version(func, |candidate| {
                    version(candidate, body, &Selection::default(), None, Some(&ascii))
                });
            }
            Self::Arithmetic { guard, selection } => (guard, selection),
        };
        // Proofs alone need no second loop and cannot fail.
        if guard.is_none()
            && selection.checked.is_empty()
            && selection.projections.is_empty()
            && selection.packed_loads.is_empty()
        {
            for (block, index) in selection.exact {
                func.blocks
                    .get_mut(&block)
                    .expect("selected block exists")
                    .ops[index]
                    .mark_i64_exact_proof();
            }
            for (block, index) in selection.unchecked {
                uncheck(func, block, index);
            }
            return Ok(0);
        }
        commit_version(func, |candidate| {
            if let Some(guard) = &guard {
                guard.install_payloads(candidate);
            }
            version(candidate, body, &selection, guard.as_ref(), None)
        })
    }
}

/// SSA repair may fail after editing the CFG. Build only admitted versions
/// speculatively and publish a replacement only after its repair succeeds.
fn commit_version(
    func: &mut TirFunction,
    build: impl FnOnce(&mut TirFunction) -> Result<usize, Refusal>,
) -> Result<usize, Refusal> {
    let mut candidate = func.clone();
    let added = build(&mut candidate)?;
    *func = candidate;
    Ok(added)
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
    fn install_payloads(&self, func: &mut TirFunction) {
        for &(_, raw) in &self.payloads {
            assert_eq!(func.fresh_value(), raw, "invariant payload reservation");
            func.value_types.insert(raw, TirType::I64);
        }
    }

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

/// Find the invariants of the loop at `header` and reserve their payload IDs.
/// `None` when no numeric op of the loop takes a boxed exact int defined
/// outside it, or the loop has no single plain entry edge.
fn invariant_guard(
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
    facts: &LoopFacts<'_>,
    entry: GuardedEntry<'_>,
) -> Option<InvariantGuard> {
    let func = facts.scalar.body();
    let preheader = entry.preheader;
    let mut defined: HashSet<ValueId> = HashSet::default();
    for bid in body {
        let block = &func.blocks[bid];
        defined.extend(block.args.iter().map(|arg| arg.id));
        defined.extend(block.ops.iter().flat_map(|op| op.results.iter().copied()));
    }
    let exact = facts.scalar.types();
    let carried = header_invariants(func, header, body, entry, &defined);
    let mut blocks: Vec<BlockId> = body.iter().copied().collect();
    blocks.sort_by_key(|block| block.0);
    let mut payloads = Vec::new();
    let mut payload_by_invariant = HashMap::default();
    let mut read_payloads = HashMap::default();
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
                if read_payloads.contains_key(&operand)
                    || !matches!(exact.get(&operand), Some(TirType::I64 | TirType::BigInt))
                    || !matches!(
                        facts.representations(vr).get(&operand),
                        Some(Repr::DynBox | Repr::MaybeBigInt)
                    )
                {
                    continue;
                }
                let raw = if let Some(&raw) = payload_by_invariant.get(&tested) {
                    raw
                } else {
                    let raw = ValueId(
                        func.next_value
                            .checked_add(u32::try_from(payloads.len()).ok()?)?,
                    );
                    payloads.push((tested, raw));
                    payload_by_invariant.insert(tested, raw);
                    raw
                };
                read_payloads.insert(operand, raw);
            }
        }
    }
    if payloads.is_empty() {
        return None;
    }
    Some(InvariantGuard {
        header,
        preheader,
        payloads,
        reads: read_payloads,
    })
}

/// A plain branch that dominates all executable entries into the loop.
#[derive(Clone, Copy)]
struct GuardedEntry<'cfg> {
    preheader: BlockId,
    header_preds: &'cfg [BlockId],
    copies: &'cfg HashMap<ValueId, ValueId>,
}

fn plain_preheader(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    preds: &HashMap<BlockId, Vec<BlockId>>,
    reachable: &HashSet<BlockId>,
    labels: &HashMap<i64, BlockId>,
) -> Option<BlockId> {
    let mut preheader = None;
    for &block in body {
        for &pred in preds.get(&block)? {
            // The lift can keep an unreachable loop-end marker as metadata.
            if body.contains(&pred) || !reachable.contains(&pred) {
                continue;
            }
            if block != header || preheader.replace(pred).is_some() {
                return None;
            }
        }
    }
    let block = &func.blocks[&preheader?];
    (matches!(block.terminator, Terminator::Branch { target, .. } if target == header)
        && !crate::tir::dominators::exception_successors(block, labels).contains(&header))
    .then_some(block.id)
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
    guarded_entry: GuardedEntry<'_>,
    defined: &HashSet<ValueId>,
) -> HashMap<ValueId, ValueId> {
    let root = |value| resolve_copy(guarded_entry.copies, value);
    let params: Vec<ValueId> = func.blocks[&header].args.iter().map(|arg| arg.id).collect();
    let mut entry: Option<Vec<ValueId>> = None;
    let mut back: Vec<Vec<ValueId>> = Vec::new();
    for &bid in guarded_entry.header_preds {
        func.blocks[&bid].terminator.for_each_edge(|target, args| {
            if target != header {
                return;
            }
            if bid == guarded_entry.preheader {
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

fn check_loop_shape(
    func: &TirFunction,
    body: &HashSet<BlockId>,
    labels: &HashMap<i64, BlockId>,
) -> Result<(), Refusal> {
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
            if exception_transfer_target(op, labels).is_some_and(|target| body.contains(&target)) {
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

/// Select raw-capable loop arithmetic and its required input proofs.
fn select(
    header: BlockId,
    body: &HashSet<BlockId>,
    vr: &crate::tir::ValueRangeResult,
    facts: &LoopFacts<'_>,
    final_tir: bool,
) -> Result<Selection, Refusal> {
    let func = facts.scalar.body();
    let exact = facts.scalar.types();
    let is_int = |value: &ValueId| exact.get(value) == Some(&TirType::I64);
    let header_args = &func.blocks[&header].args;
    if !header_args.iter().any(|arg| is_int(&arg.id)) {
        return Err(Refusal::NoExactAccumulator);
    }
    let inline_safe = facts
        .inline_safe
        .get_or_init(|| crate::representation_facts::raw_i64_safe_values_from(&facts.scalar, vr));
    let constants = facts.constants.get_or_init(|| {
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .filter(|op| op.opcode == OpCode::ConstInt)
            .filter_map(|op| op.results.first().copied())
            .filter(is_int)
            .collect()
    });
    let mut arithmetic: Vec<(BlockId, usize, ValueId, [ValueId; 2])> = Vec::new();
    let mut checked_before: Vec<(BlockId, usize, ValueId, [ValueId; 2])> = Vec::new();
    let mut call_results = HashMap::default();
    let mut copies: Vec<(ValueId, ValueId)> = Vec::new();
    let mut phis: Vec<(BlockId, usize, ValueId)> = Vec::new();
    for &bid in body {
        let block = &func.blocks[&bid];
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
            } else if final_tir
                && matches!(op.opcode, OpCode::Call | OpCode::CallRuntime)
                && let [result] = op.results[..]
                && is_int(&result)
                && !inline_safe.contains(&result)
            {
                call_results.insert(result, (bid, index));
            } else if let Some(source) =
                copy_value_source(op).or_else(|| forwarded_python_value_source(op))
                && let [result] = op.results[..]
                && is_int(&result)
            {
                copies.push((result, source));
            }
        }
    }
    let incomings = facts
        .incomings
        .get_or_init(|| crate::representation_facts::block_arg_incomings_for(func));

    // Start at the greatest fixed point and propagate each failed dependency
    // only to the candidates that use it. A seed stays raw even when its
    // candidate definition does not qualify for this loop rewrite.
    let mut capable: HashSet<ValueId> =
        arithmetic.iter().map(|&(_, _, result, _)| result).collect();
    capable.extend(copies.iter().map(|&(result, _)| result));
    capable.extend(phis.iter().map(|&(_, _, phi)| phi));
    let checked_results: HashSet<ValueId> = checked_before
        .iter()
        .map(|&(_, _, result, _)| result)
        .collect();
    let raw_seed = |value: &ValueId| {
        inline_safe.contains(value)
            || constants.contains(value)
            || checked_results.contains(value)
            || call_results.contains_key(value)
    };
    let raw =
        |value: &ValueId, capable: &HashSet<ValueId>| capable.contains(value) || raw_seed(value);
    let mut dependents: HashMap<ValueId, Vec<ValueId>> = HashMap::default();
    let mut rejected = Vec::new();
    let mut register = |result, inputs: &[ValueId]| {
        if inputs.is_empty() || inputs.iter().any(|input| !raw(input, &capable)) {
            rejected.push(result);
        } else {
            for &input in inputs.iter().filter(|input| !raw_seed(input)) {
                dependents.entry(input).or_default().push(result);
            }
        }
    };
    for &(_, _, result, operands) in &arithmetic {
        register(result, &operands);
    }
    for &(result, source) in &copies {
        register(result, std::slice::from_ref(&source));
    }
    for &(block, index, phi) in &phis {
        let inputs = incomings
            .get(&(block, index))
            .map_or(&[][..], Vec::as_slice);
        register(phi, inputs);
    }
    while let Some(result) = rejected.pop() {
        if capable.remove(&result)
            && let Some(users) = dependents.remove(&result)
        {
            rejected.extend(users);
        }
    }
    if !header_args.iter().any(|arg| capable.contains(&arg.id)) {
        return Err(Refusal::BoxedAccumulatorInput);
    }
    let mut selection = Selection::default();
    let mut numeric_inputs = Vec::new();
    for &(block, index, result, operands) in &arithmetic {
        if !capable.contains(&result) {
            continue;
        }
        if !call_results.is_empty() {
            numeric_inputs.extend(operands);
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
            if !call_results.is_empty() {
                numeric_inputs.extend(operands);
            }
        }
    }
    // Project only call results reaching selected numerical work. Copies and
    // phis may forward the same value through more than one block.
    if !call_results.is_empty() {
        let numeric_only = if !numeric_inputs.is_empty()
            && call_results
                .values()
                .any(|&(block, index)| is_packed_buffer_read(&func.blocks[&block].ops[index]))
        {
            packed_numeric_demand(func, body, &capable, &numeric_inputs)
        } else {
            HashSet::default()
        };
        let mut sources: HashMap<ValueId, Vec<ValueId>> = HashMap::default();
        for (result, source) in copies {
            sources.entry(result).or_default().push(source);
        }
        for (block, index, phi) in phis {
            if let Some(inputs) = incomings.get(&(block, index)) {
                sources.entry(phi).or_default().extend(inputs);
            }
        }
        let mut seen = HashSet::default();
        while let Some(value) = numeric_inputs.pop() {
            if !seen.insert(value) {
                continue;
            }
            if let Some(&site) = call_results.get(&value) {
                let (block, index) = site;
                let ops = &func.blocks[&block].ops;
                let call = &ops[index];
                // An escaped Python reference or a handler payload can observe
                // the physical boxed result. Only proven numeric uses may
                // consume the packed word without constructing that object.
                if is_packed_buffer_read(call)
                    && numeric_only.contains(&value)
                    && ops.get(index + 1).is_some_and(|check| {
                        !check.has_checked_invocation_binding()
                            && check.exception_edge_kind()
                                == Some(crate::tir::op_kinds_generated::ExceptionEdgeKind::Origin)
                            && crate::tir::type_refine::checked_result_observer(
                                func,
                                &facts.labels,
                                check,
                            )
                            .is_some()
                    })
                {
                    selection.packed_loads.push(site);
                } else {
                    selection.projections.push(site);
                }
            }
            if let Some(inputs) = sources.get(&value) {
                numeric_inputs.extend(inputs);
            }
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

fn is_packed_buffer_read(op: &TirOp) -> bool {
    crate::tir::call_targets::direct_boxed_runtime(op)
        == Some(molt_ir::runtime_fn_generated::RuntimeFn::Buffer2dGet)
        && op.argument_custody().is_none()
}

/// Discharge every actual use, backwards from selected numeric operands.
/// A forwarding edge is admitted only after all uses of its destination have
/// been discharged. Cycles without an independently discharged exit remain
/// boxed. Unknown uses (including handler payloads) stay in the use census.
fn packed_numeric_demand(
    func: &TirFunction,
    body: &HashSet<BlockId>,
    capable: &HashSet<ValueId>,
    numeric_inputs: &[ValueId],
) -> HashSet<ValueId> {
    let mut remaining = super::dce::build_use_counts(func);
    let mut ready = Vec::new();
    let discharge = |value, remaining: &mut HashMap<ValueId, usize>, ready: &mut Vec<ValueId>| {
        if let Some(count) = remaining.get_mut(&value) {
            debug_assert!(*count > 0, "each use is discharged once");
            *count -= 1;
            if *count == 0 {
                ready.push(value);
            }
        }
    };
    for &value in numeric_inputs {
        discharge(value, &mut remaining, &mut ready);
    }
    if ready.is_empty() || func.frame_locals_exposed() || func.has_state_machine() {
        return ready.into_iter().collect();
    }
    let labels = exception_label_to_block(func);
    let exceptional: HashSet<_> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(|op| exception_transfer_target(op, &labels))
        .collect();
    let mut predecessors: HashMap<ValueId, Vec<ValueId>> = HashMap::default();
    for bid in body {
        let block = &func.blocks[bid];
        for op in &block.ops {
            // Value forwarding alone does not remove Python custody or a
            // frame home. Restrict to structural copies, using their shared
            // shape authority (including repeated identical operands).
            if matches!(op.origin, None | Some(molt_ir::OpKind::Copy))
                && copy_value_source(op).is_some()
                && capable.contains(&op.results[0])
            {
                predecessors
                    .entry(op.results[0])
                    .or_default()
                    .extend(op.operands.iter().copied());
            }
        }
        if matches!(block.terminator, Terminator::StateDispatch { .. }) {
            continue;
        }
        block.terminator.for_each_edge(|target, inputs| {
            if !body.contains(&target) || exceptional.contains(&target) {
                return;
            }
            let args = &func.blocks[&target].args;
            if args.len() != inputs.len() {
                return;
            }
            for (arg, &input) in args.iter().zip(inputs) {
                if capable.contains(&arg.id) {
                    predecessors.entry(arg.id).or_default().push(input);
                }
            }
        });
    }
    let mut proven = HashSet::default();
    while let Some(value) = ready.pop() {
        if !proven.insert(value) {
            continue;
        }
        if let Some(inputs) = predecessors.get(&value) {
            for &input in inputs {
                discharge(input, &mut remaining, &mut ready);
            }
        }
    }
    proven
}

/// The selected arithmetic ops, as (block, op index), in deterministic order.
#[derive(Default)]
struct Selection {
    /// Ops the fast loop checks for overflow.
    checked: Vec<(BlockId, usize)>,
    /// Exact-int calls whose fast successors consume the inline payload.
    projections: Vec<(BlockId, usize)>,
    /// A packed hit supplies the selected arithmetic directly; a miss runs
    /// the original load and its exception transfer in the boxed clone.
    packed_loads: Vec<(BlockId, usize)>,
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

#[derive(Clone, Copy)]
enum FastCheck {
    Overflow,
    CallResult,
    PackedLoad,
}

/// A checked op: its first result is the wrapped value, its second the
/// overflow flag.
pub(super) fn is_checked(opcode: OpCode) -> bool {
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

    // 3. Split after each checked operation in both loops.
    let mut by_block: BTreeMap<u32, Vec<(usize, FastCheck)>> = BTreeMap::new();
    for (checks, kind) in [
        (&selection.checked, FastCheck::Overflow),
        (&selection.projections, FastCheck::CallResult),
        (&selection.packed_loads, FastCheck::PackedLoad),
    ] {
        for &(block, index) in checks {
            by_block.entry(block.0).or_default().push((index, kind));
        }
    }
    let mut variables: HashMap<ValueId, Vec<ValueId>> = defined
        .iter()
        .filter(|(value, _)| !unchecked_flags.contains(value))
        .map(|(value, _)| (*value, vec![*value, value_map[value]]))
        .collect();
    for (block, mut indices) in by_block {
        let fast = BlockId(block);
        let slow = block_map[&fast];
        indices.sort_unstable_by_key(|&(index, _)| std::cmp::Reverse(index));
        for (index, check) in indices {
            let ops = &func.blocks[&fast].ops;
            let original = ops[index].clone();
            // Keep the exception observation adjacent to its call on both
            // paths. The inline lane only speculates on a successful result.
            let split = index
                + 1
                + usize::from(
                    matches!(check, FastCheck::CallResult)
                        && ops
                            .get(index + 1)
                            .is_some_and(|op| op.opcode == OpCode::CheckException),
                );
            let result = original.results[0];
            let flag = func.fresh_value();
            func.value_types.insert(flag, TirType::Bool);
            // A packed miss enters before the original call in its slow
            // clone. That clone defines its own result at the original point.
            let alternate = if matches!(check, FastCheck::PackedLoad) {
                None
            } else {
                let value = func.fresh_value();
                func.value_types.insert(value, TirType::I64);
                Some(value)
            };
            let fast_rest = func.fresh_block();
            let slow_rest = func.fresh_block();

            let slow_block = func.blocks.get_mut(&slow).expect("cloned block exists");
            let slow_split = if matches!(check, FastCheck::PackedLoad) {
                index
            } else {
                split
            };
            let slow_ops = slow_block.ops.split_off(slow_split);
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

            let (on_true, on_false) = match check {
                FastCheck::Overflow => {
                    let transfer = func.fresh_block();
                    func.blocks.insert(
                        transfer,
                        TirBlock {
                            id: transfer,
                            args: vec![],
                            ops: vec![TirOp {
                                results: vec![
                                    alternate.expect("overflow transfer defines an exact result"),
                                ],
                                attrs: clone_attrs_without_simple_names(&original.attrs),
                                ..original.clone()
                            }],
                            terminator: Terminator::Branch {
                                target: slow_rest,
                                args: vec![],
                            },
                        },
                    );
                    added += 1;
                    (transfer, fast_rest)
                }
                FastCheck::CallResult | FastCheck::PackedLoad => (fast_rest, slow_rest),
            };
            let fast_block = func.blocks.get_mut(&fast).expect("selected block exists");
            let mut rest_ops = fast_block.ops.split_off(split);
            let checked = &mut fast_block.ops[index];
            match check {
                FastCheck::Overflow => {
                    checked.opcode = checked_opcode(original.opcode).expect("selected arithmetic");
                    checked.results.push(flag);
                }
                FastCheck::PackedLoad => {
                    let mut load = TirOp::new(
                        OpCode::TryBuffer2dLoadI64,
                        original.operands,
                        vec![result, flag],
                    );
                    load.source_span = original.source_span;
                    *checked = load;
                }
                FastCheck::CallResult => {
                    let alternate = alternate.expect("boxed call defines a physical result");
                    checked.results[0] = alternate;
                    fast_block.ops.push(TirOp::new(
                        OpCode::IsInlineInt,
                        vec![alternate],
                        vec![flag],
                    ));
                    rest_ops.insert(
                        0,
                        TirOp::new(OpCode::UnboxInlineInt, vec![alternate], vec![result]),
                    );
                    added += 2;
                }
            };
            let rest_terminator = std::mem::replace(
                &mut fast_block.terminator,
                Terminator::CondBranch {
                    cond: flag,
                    then_block: on_true,
                    then_args: vec![],
                    else_block: on_false,
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
            if let Some(alternate) = alternate {
                variables
                    .get_mut(&result)
                    .expect("selected result is a loop value")
                    .push(alternate);
            }
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
