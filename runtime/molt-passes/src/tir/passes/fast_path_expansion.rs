//! The late lowering stage of TIR (HF-821): value-level fast paths written once,
//! as representation-level control flow, for every backend that consumes final
//! TIR. Backends then only select instructions; the NaN-box meaning of each
//! primitive comes from molt-codegen-abi's shared generators.
//!
//! `Add`, `Sub`, `Mul` and their in-place forms on boxed words take an
//! inline-int arm and a float arm. Two 47-bit payloads add and subtract
//! exactly in i64; their product may not, so the multiplication arm uses
//! `CheckedMul` and leaves for the runtime on overflow:
//!
//! ```text
//!   %r = Add %a, %b                          ; both boxed words
//! becomes
//!   %ta = IsInlineInt %a ; cond_br %ta, ^check_b, ^float_a
//! ^check_b: %tb = IsInlineInt %b ; cond_br %tb, ^int, ^float_a
//! ^int:   %x = UnboxInlineInt %a ; %y = UnboxInlineInt %b
//!         %s = Add %x, %y                    ; exact in i64: two 47-bit payloads
//!         %r1 = BoxVal %s                    ; inline if it fits, else a heap int
//!         br ^join(%r1)
//! ^float_a: %fa = IsFloatWord %a ; cond_br %fa, ^float_b, ^slow
//! ^float_b: %fb = IsFloatWord %b ; cond_br %fb, ^float, ^slow
//! ^float: %u = UnboxFloatWord %a ; %v = UnboxFloatWord %b
//!         %f = Add %u, %v ; %r2 = BoxVal %f  ; a float word is its own bits
//!         br ^join(%r2)
//! ^slow:  %r3 = CallRuntime[molt_add] %a, %b ; br ^join(%r3)
//! ^join(%r):  the rest of the block, then its terminator
//! ```
//!
//! Two mixed arms follow, one per order, for a float and an inline int: the
//! int payload converts exactly (`IntToFloat`; |x| < 2**46) and the float op
//! computes, as CPython converts the int first. True division (`Div`) has no
//! int arm: two inline ints convert and divide in F64, which is CPython's
//! correctly rounded quotient below 2**53; F64 division raises on a zero
//! divisor. An operand proven an exact int takes no arm that reads it as a
//! float payload, and one proven an exact float none that reads it as an int.
//!
//! A comparison (`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`) takes the same shape. Its
//! fast arm compares the two payloads, which order exactly as the ints do, and
//! boxes the raw `Bool` as a bool word; the slow arm calls the comparison's
//! runtime entry, which runs the rich-comparison protocol.
//!
//! One operand may instead be a raw value in the inline window (`RawI64Safe`,
//! such as the literal in `n - 1`). It needs no tag test and feeds the fast arm
//! as it is; the slow arm boxes it with `BoxInlineInt` (which cannot raise) for
//! the runtime entry, which takes words.
//!
//! Truthiness, `Bool` on a boxed word:
//!
//! ```text
//!   %t = Bool %x
//! becomes
//!   %tb = IsBoolWord %x ; cond_br %tb, ^bool, ^check_int
//! ^bool:      %t1 = UnboxBoolWord %x ; br ^join(%t1)
//! ^check_int: %ti = IsInlineInt %x ; cond_br %ti, ^int, ^slow
//! ^int:       %p = UnboxInlineInt %x ; %t2 = Ne %p, 0 ; br ^join(%t2)
//! ^slow:      %t3 = Bool %x                  ; the op's own lowering, unchanged
//! ^join(%t):  the rest of the block, then its terminator
//! ```
//!
//! The original result id becomes the join's argument, so no use is renamed.
//! No existing value changes representation: before it splits a block, the
//! stage marks each op whose integer results are proven inline
//! (`record_inline_int_proofs`), and the representation plan reads the mark. An in-place op on two ints has no
//! dunder to try, so both forms share the fast arm; the slow arm calls the op's
//! own runtime entry (`TirOp::numeric_runtime`), which tries `__i<op>__` first.
//! A failed heap box or a raising `__bool__` leaves a pending exception, which
//! the `CheckException` that already follows the op observes in the join.
//! Truthiness keeps the original `Bool` as its slow arm until `CallRuntime` can
//! carry a raw-bits result through type refinement.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    FastPathExpansionRule, opcode_fast_path_expansion_rule_table,
};
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use molt_ir::repr::Repr;

/// Expand every eligible op of `func`. Requires: the module phase is complete
/// and drops are not inserted; `func` belongs to a target whose backend
/// consumes final TIR.
pub fn run(func: &mut TirFunction, targets: &GuardedTargets) -> PassStats {
    let mut stats = PassStats {
        name: "fast_path_expansion",
        ..Default::default()
    };
    let vr = crate::representation_facts::value_range_for(func);
    let repr = crate::representation_facts::repr_by_value_for(func, Some(&vr));
    // Exact types as they stand before any block is split.
    let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
    if !func.blocks.values().any(|block| {
        block
            .ops
            .iter()
            .any(|op| expansion_of(op, &repr, &exact, targets).is_some())
    }) {
        return stats;
    }
    record_inline_int_proofs(func, &repr);
    let mut pending: Vec<BlockId> = func.blocks.keys().copied().collect();
    pending.sort_unstable_by(|a, b| b.cmp(a));
    while let Some(block) = pending.pop() {
        let Some((index, expansion)) = func.blocks[&block]
            .ops
            .iter()
            .enumerate()
            .find_map(|(index, op)| expansion_of(op, &repr, &exact, targets).map(|e| (index, e)))
        else {
            continue;
        };
        let split = split(func, block, index);
        let join = match expansion {
            Expansion::Numeric {
                arms,
                operands,
                raw_result,
            } => {
                let exact_operands = split
                    .op
                    .operands
                    .iter()
                    .map(|operand| exact.get(operand).cloned())
                    .collect();
                expand_numeric(
                    func,
                    block,
                    split,
                    &arms,
                    operands,
                    raw_result,
                    exact_operands,
                )
            }
            Expansion::Truthiness => expand_truthiness(func, block, split),
            Expansion::GuardedCall => expand_guarded_call(func, block, split),
        };
        stats.ops_added += 6;
        stats.values_changed += 1;
        // Only the join is rescanned: the slow arm keeps its op.
        pending.push(join);
    }
    stats
}

/// Record on each op the proof that its integer results fit the inline window.
///
/// The range analysis proves a loop's induction variable from the loop's
/// shape: a guard block, a body path and a latch. Expansion splits body
/// blocks, so an analysis that runs afterwards (drop insertion, a backend's
/// carrier plan) no longer recognizes the loop and would carry the variable
/// as a possible big int: reference counts on every iteration and a runtime
/// call for `i < n` and `i + 1`. The proof is about the value, not the block
/// it sits in, so it stays true; this stage is the last point where it can be
/// read, and the op is where it travels. Phis and copies need no mark: the
/// plan derives them from their sources.
fn record_inline_int_proofs(func: &mut TirFunction, repr: &HashMap<ValueId, Repr>) {
    let is_int = |func: &TirFunction, value: ValueId| {
        matches!(
            func.value_types.get(&value),
            Some(TirType::I64 | TirType::BigInt)
        )
    };
    let proven: Vec<(BlockId, usize)> = func
        .blocks
        .iter()
        .flat_map(|(&block, body)| {
            body.ops
                .iter()
                .enumerate()
                .map(move |(index, op)| (block, index, op))
        })
        .filter(|(_, _, op)| {
            let mut ints = op.results.iter().filter(|&&r| is_int(func, r)).peekable();
            ints.peek().is_some() && ints.all(|r| repr.get(r) == Some(&Repr::RawI64Safe))
        })
        .map(|(block, index, _)| (block, index))
        .collect();
    for (block, index) in proven {
        func.blocks
            .get_mut(&block)
            .expect("the block was just scanned")
            .ops[index]
            .mark_inline_int_proof();
    }
}

/// What the stage does with one op, from the generated
/// `fast_path_expansion_rules` table.
enum Expansion {
    /// Add, subtract, multiply or compare; in-place forms share the binary
    /// arms. Each arm tests its tag on the boxed operands, computes on the
    /// payloads with `fast`, and yields a value of type `raw`.
    Numeric {
        arms: Vec<NumericArm>,
        operands: [NumOperand; 2],
        /// The op's result is a raw bool: the arms join without boxing it.
        raw_result: bool,
    },
    Truthiness,
    /// A guarded call whose target's linkage row admits a direct call.
    GuardedCall,
}

/// What the stage needs of a guarded call's target, from its linkage row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalleeFacts {
    /// Declared parameters, the closure included.
    pub arity: usize,
    pub has_closure: bool,
}

/// The linkage facts of every target a function's guarded calls name.
pub type GuardedTargets = std::collections::BTreeMap<String, CalleeFacts>;

/// The speculative target a guarded call names: the compiled function its
/// frontend expected. The callee word may be any object at run time.
fn guarded_target(op: &TirOp) -> Option<&str> {
    if op.opcode != OpCode::Call || op.origin != Some(molt_ir::OpKind::CallGuarded) {
        return None;
    }
    ["s_value", "_var"]
        .iter()
        .find_map(|key| match op.attrs.get(*key) {
            Some(AttrValue::Str(symbol)) if !symbol.is_empty() => Some(symbol.as_str()),
            _ => None,
        })
}

/// Project the signatures of the targets `func`'s guarded calls name.
pub fn guarded_targets(
    func: &TirFunction,
    linkage: Option<&molt_ir::linkage_abi::FunctionSignatures>,
) -> GuardedTargets {
    let Some(linkage) = linkage else {
        return GuardedTargets::new();
    };
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(guarded_target)
        .map(|symbol| {
            // The driver's table holds every function the unit defines or
            // declares, so a missing target is a wiring defect, never a
            // reason to keep the slow lane in silence.
            let signature = linkage.get(symbol).unwrap_or_else(|| {
                panic!(
                    "guarded call in `{}` names `{symbol}`, which has no source signature",
                    func.name
                )
            });
            (
                symbol.to_owned(),
                CalleeFacts {
                    arity: signature.arity,
                    has_closure: signature.has_closure,
                },
            )
        })
        .collect()
}

/// How an integer operand reaches the fast arm: a boxed word is tag-tested
/// and unboxed; an inline-safe raw value is used as it is. Two values in the
/// inline window sum exactly in i64.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NumOperand {
    /// A boxed word: each arm tests its tag.
    Boxed,
    /// A raw value in the inline-int window (`RawI64Safe`).
    InlineInt,
    /// A raw f64 (`FloatUnboxed`).
    RawFloat,
}

/// The payload one operand holds in a fast arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Payload {
    Int,
    Float,
}

/// One fast arm: the payload each operand must hold, the type it computes in
/// (`I64` for two ints; `F64` otherwise, an int payload converted exactly by
/// `IntToFloat`), the op it computes with, and the raw type that op yields.
#[derive(Clone, Debug)]
struct NumericArm {
    payloads: [Payload; 2],
    compute: TirType,
    fast: OpCode,
    raw: TirType,
}

fn is_boxed_word(repr: &HashMap<ValueId, Repr>, value: ValueId) -> bool {
    matches!(
        repr.get(&value),
        Some(Repr::DynBox | Repr::MaybeBigInt | Repr::BoxedImmortal)
    )
}

fn num_operand(repr: &HashMap<ValueId, Repr>, value: ValueId) -> Option<NumOperand> {
    match repr.get(&value) {
        _ if is_boxed_word(repr, value) => Some(NumOperand::Boxed),
        Some(Repr::RawI64Safe) => Some(NumOperand::InlineInt),
        Some(Repr::FloatUnboxed) => Some(NumOperand::RawFloat),
        _ => None,
    }
}

fn expansion_of(
    op: &TirOp,
    repr: &HashMap<ValueId, Repr>,
    exact: &HashMap<ValueId, TirType>,
    targets: &GuardedTargets,
) -> Option<Expansion> {
    if !op.has_valid_shape() || op.results.len() != 1 {
        return None;
    }
    // `int` is the two-int arm's op and raw type, or None where the op has no
    // such arm (true division computes on floats); `float` is the op and raw
    // type every other arm computes with in F64.
    let numeric = |int: Option<(OpCode, TirType)>, float: (OpCode, TirType)| {
        let [lhs, rhs] = op.operands[..] else {
            return None;
        };
        let operands = [num_operand(repr, lhs)?, num_operand(repr, rhs)?];
        // Two raw operands are the backend's raw lane, not a boxed fast path.
        if !operands.contains(&NumOperand::Boxed) || op.numeric_runtime().is_none() {
            return None;
        }
        // The payloads an operand may hold: a raw value is its own kind, and a
        // boxed operand proven an exact int (or float) takes no arm that reads
        // it as the other kind.
        let admits = |i: usize, payload: Payload| match operands[i] {
            NumOperand::InlineInt => payload == Payload::Int,
            NumOperand::RawFloat => payload == Payload::Float,
            NumOperand::Boxed => match exact.get(&op.operands[i]) {
                Some(TirType::F64) => payload == Payload::Float,
                Some(TirType::I64 | TirType::BigInt | TirType::Bool) => payload == Payload::Int,
                _ => true,
            },
        };
        let mut arms = Vec::new();
        for payloads in [
            [Payload::Int, Payload::Int],
            [Payload::Float, Payload::Float],
            [Payload::Float, Payload::Int],
            [Payload::Int, Payload::Float],
        ] {
            if !(admits(0, payloads[0]) && admits(1, payloads[1])) {
                continue;
            }
            let (fast, raw, compute) = match (&int, payloads) {
                (Some((fast, raw)), [Payload::Int, Payload::Int]) => {
                    (*fast, raw.clone(), TirType::I64)
                }
                _ => (float.0, float.1.clone(), TirType::F64),
            };
            arms.push(NumericArm {
                payloads,
                compute,
                fast,
                raw,
            });
        }
        // A comparison planned as a raw bool (two exact numbers, one of them
        // a boxed word) joins its arms raw; without this case no stage and no
        // backend lane owned the op, and it fell to the runtime with a
        // truthiness call on its result.
        let raw_result = arms.first().is_some_and(|arm| arm.raw == TirType::Bool)
            && repr.get(&op.results[0]) == Some(&Repr::Bool);
        (!arms.is_empty() && (raw_result || is_boxed_word(repr, op.results[0]))).then_some(
            Expansion::Numeric {
                arms,
                operands,
                raw_result,
            },
        )
    };
    match opcode_fast_path_expansion_rule_table(op.opcode) {
        FastPathExpansionRule::NumericAdd => numeric(
            Some((OpCode::Add, TirType::I64)),
            (OpCode::Add, TirType::F64),
        ),
        FastPathExpansionRule::NumericSub => numeric(
            Some((OpCode::Sub, TirType::I64)),
            (OpCode::Sub, TirType::F64),
        ),
        // The int product of two inline payloads can leave i64: CheckedMul
        // tests the overflow and the arm leaves for the runtime on it.
        FastPathExpansionRule::NumericMul => numeric(
            Some((OpCode::CheckedMul, TirType::I64)),
            (OpCode::Mul, TirType::F64),
        ),
        // True division computes in F64 on every arm, two ints included: two
        // inline payloads convert exactly, and one IEEE division of exact
        // operands is CPython's correctly rounded int quotient below 2**53
        // (`long_true_divide`). Raw F64 division raises on a zero divisor.
        FastPathExpansionRule::NumericTrueDiv => numeric(None, (OpCode::Div, TirType::F64)),
        // A comparison on two payloads is the same comparison on raw values;
        // float comparisons keep IEEE order, so NaN compares unequal.
        FastPathExpansionRule::NumericCompare => {
            numeric(Some((op.opcode, TirType::Bool)), (op.opcode, TirType::Bool))
        }
        FastPathExpansionRule::Truthiness => (op.operands.len() == 1
            && is_boxed_word(repr, op.operands[0]))
        .then_some(Expansion::Truthiness),
        // A closure-taking target and an arity mismatch keep the runtime lane:
        // binding, not a static ABI, must answer them.
        FastPathExpansionRule::GuardedCall => {
            let facts = targets.get(guarded_target(op)?)?;
            (!facts.has_closure
                && facts.arity + 1 == op.operands.len()
                && is_boxed_word(repr, op.operands[0]))
            .then_some(Expansion::GuardedCall)
        }
        FastPathExpansionRule::None => None,
    }
}

fn new_op(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    span: Option<(u32, u32)>,
) -> TirOp {
    TirOp {
        source_span: span,
        ..TirOp::new(opcode, operands, results)
    }
}

fn fresh_typed(func: &mut TirFunction, ty: TirType) -> ValueId {
    let value = func.fresh_value();
    func.value_types.insert(value, ty);
    value
}

fn new_block(id: BlockId, ops: Vec<TirOp>, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args: Vec::new(),
        ops,
        terminator,
    }
}

fn branch(target: BlockId, value: ValueId) -> Terminator {
    Terminator::Branch {
        target,
        args: vec![value],
    }
}

fn cond_branch(cond: ValueId, then_block: BlockId, else_block: BlockId) -> Terminator {
    Terminator::CondBranch {
        cond,
        then_block,
        then_args: Vec::new(),
        else_block,
        else_args: Vec::new(),
    }
}

/// The op being expanded and what followed it in its block.
struct Split {
    op: TirOp,
    rest: Vec<TirOp>,
    terminator: Terminator,
}

/// Remove the op at `index` and everything after it from `block`; the caller
/// gives the block its new tail and terminator.
fn split(func: &mut TirFunction, block: BlockId, index: usize) -> Split {
    let original = func.blocks.get_mut(&block).expect("pending block exists");
    let rest = original.ops.split_off(index + 1);
    let op = original.ops.pop().expect("eligible op at index");
    let terminator = std::mem::replace(
        &mut original.terminator,
        Terminator::Branch {
            target: block,
            args: Vec::new(),
        },
    );
    Split {
        op,
        rest,
        terminator,
    }
}

/// End `block` with `ops` and `terminator`.
fn end_block(func: &mut TirFunction, block: BlockId, ops: Vec<TirOp>, terminator: Terminator) {
    let original = func.blocks.get_mut(&block).expect("pending block exists");
    original.ops.extend(ops);
    original.terminator = terminator;
}

/// Insert the join: it binds the original result and holds the rest of the
/// original block. A loop whose break test ended that block finds it here.
fn finish(
    func: &mut TirFunction,
    block: BlockId,
    join: BlockId,
    result: ValueId,
    rest: Vec<TirOp>,
    terminator: Terminator,
) {
    let ty = func
        .value_types
        .get(&result)
        .cloned()
        .unwrap_or(TirType::DynBox);
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue { id: result, ty }],
            ops: rest,
            terminator,
        },
    );
    for cond_block in func.loop_cond_blocks.values_mut() {
        if *cond_block == block {
            *cond_block = join;
        }
    }
}

fn expand_numeric(
    func: &mut TirFunction,
    block: BlockId,
    split: Split,
    arms: &[NumericArm],
    kinds: [NumOperand; 2],
    raw_result: bool,
    exact_operands: Option<Vec<TirType>>,
) -> BlockId {
    let Split {
        op,
        rest,
        terminator,
    } = split;
    let operands = [op.operands[0], op.operands[1]];
    let result = op.results[0];
    let span = op.source_span;
    let runtime = op
        .numeric_runtime()
        .expect("eligible ops have a runtime entry");
    // An op on two exact numbers cannot raise, and the check that would have
    // observed it is gone. The ops that replace it box a result or call the
    // runtime, which the op tables declare as raising; unmarked, each would be
    // a new exception source in front of a check that expects none.
    let cannot_raise = !super::check_exception_elim::classify::op_may_raise(
        &func.value_types,
        &HashMap::new(),
        &op,
    );
    // The op's own exact result type, from its operands' exact types: an add
    // of two exact ints is an exact int. The runtime call that replaces it is
    // typed "an object" by its contract, and the join would lose the fact.
    let exact_result = exact_operands
        .and_then(|types| crate::tir::op_semantics::op_instance_facts(op.opcode, &types))
        .and_then(|facts| facts.result_type);
    let boxed: Vec<(usize, ValueId)> = (0..2)
        .filter(|&i| kinds[i] == NumOperand::Boxed)
        .map(|i| (i, operands[i]))
        .collect();

    let slow = func.fresh_block();
    let join = func.fresh_block();
    // Each arm: one tag test per boxed operand, then its fast block. A failed
    // test goes to the next arm's first test, and the last arm's to the slow
    // arm. The first test ends the original block.
    let arm_entries: Vec<BlockId> = (0..arms.len())
        .map(|index| {
            if index == 0 {
                block
            } else {
                func.fresh_block()
            }
        })
        .collect();
    let mut fast_results = Vec::new();
    for (index, arm) in arms.iter().enumerate() {
        let on_failure = arm_entries.get(index + 1).copied().unwrap_or(slow);
        let fast_block = func.fresh_block();
        let payload_ops = |payload: Payload| match payload {
            Payload::Int => (OpCode::IsInlineInt, OpCode::UnboxInlineInt, TirType::I64),
            Payload::Float => (OpCode::IsFloatWord, OpCode::UnboxFloatWord, TirType::F64),
        };
        let checks: Vec<BlockId> = std::iter::once(arm_entries[index])
            .chain((1..boxed.len()).map(|_| func.fresh_block()))
            .collect();
        for (k, &(index, operand)) in boxed.iter().enumerate() {
            let holds = fresh_typed(func, TirType::Bool);
            let next = checks.get(k + 1).copied().unwrap_or(fast_block);
            let (test_op, _, _) = payload_ops(arm.payloads[index]);
            let test = vec![new_op(test_op, vec![operand], vec![holds], span)];
            let check_end = cond_branch(holds, next, on_failure);
            if checks[k] == block {
                end_block(func, block, test, check_end);
            } else {
                func.blocks
                    .insert(checks[k], new_block(checks[k], test, check_end));
            }
        }

        let mut fast_ops = Vec::new();
        let mut payloads = operands;
        for i in 0..2 {
            if kinds[i] == NumOperand::Boxed {
                let (_, unbox_op, payload_ty) = payload_ops(arm.payloads[i]);
                payloads[i] = fresh_typed(func, payload_ty);
                fast_ops.push(new_op(unbox_op, vec![operands[i]], vec![payloads[i]], span));
            }
        }
        // Two ints divided by zero raise the int message ("division by
        // zero"), not F64 division's: a zero int divisor leaves for the
        // runtime with the original words.
        let mut fast_block = fast_block;
        if arm.fast == OpCode::Div && arm.payloads == [Payload::Int, Payload::Int] {
            let zero = fresh_typed(func, TirType::I64);
            let nonzero = fresh_typed(func, TirType::Bool);
            fast_ops.push(const_int(zero, 0, span));
            fast_ops.push(new_op(
                OpCode::Ne,
                vec![payloads[1], zero],
                vec![nonzero],
                span,
            ));
            let divide_block = func.fresh_block();
            func.blocks.insert(
                fast_block,
                new_block(
                    fast_block,
                    std::mem::take(&mut fast_ops),
                    cond_branch(nonzero, divide_block, slow),
                ),
            );
            fast_block = divide_block;
        }
        for i in 0..2 {
            // A float arm reads an int payload as its exact f64.
            if arm.compute == TirType::F64 && arm.payloads[i] == Payload::Int {
                let converted = fresh_typed(func, TirType::F64);
                fast_ops.push(new_op(
                    OpCode::IntToFloat,
                    vec![payloads[i]],
                    vec![converted],
                    span,
                ));
                payloads[i] = converted;
            }
        }
        let raw = fresh_typed(func, arm.raw.clone());
        // A checked op yields an overflow flag beside its value; on overflow
        // the arm leaves for the runtime, which computes the big int.
        let fast_block = if arm.fast == OpCode::CheckedMul {
            let overflow = fresh_typed(func, TirType::Bool);
            fast_ops.push(new_op(
                arm.fast,
                payloads.to_vec(),
                vec![raw, overflow],
                span,
            ));
            let box_block = func.fresh_block();
            func.blocks.insert(
                fast_block,
                new_block(
                    fast_block,
                    std::mem::take(&mut fast_ops),
                    cond_branch(overflow, slow, box_block),
                ),
            );
            box_block
        } else {
            fast_ops.push(new_op(arm.fast, payloads.to_vec(), vec![raw], span));
            fast_block
        };
        let fast_result = if raw_result {
            raw
        } else {
            let boxed_result = fresh_typed(func, TirType::DynBox);
            let mut box_op = new_op(OpCode::BoxVal, vec![raw], vec![boxed_result], span);
            if cannot_raise {
                box_op.mark_nothrow_proof();
            }
            // The boxed word is the op's result: an exact number stays one.
            if let Some(ty) = &exact_result {
                box_op.mark_exact_result_proof(ty);
            }
            fast_ops.push(box_op);
            boxed_result
        };
        func.blocks.insert(
            fast_block,
            new_block(fast_block, fast_ops, branch(join, fast_result)),
        );
        fast_results.push(fast_result);
    }

    // The slow arm is the original op's semantics: it keeps its attributes
    // (exception source, location) and calls the op's typed runtime entry,
    // which takes boxed words, so a raw operand is boxed first: an inline int
    // by BoxInlineInt, a float by BoxVal. Neither allocates nor raises, so no
    // Origin source mixes into the join's check.
    let mut slow_ops = Vec::new();
    let mut words = operands;
    for i in 0..2 {
        let boxing = match kinds[i] {
            NumOperand::Boxed => continue,
            NumOperand::InlineInt => OpCode::BoxInlineInt,
            NumOperand::RawFloat => OpCode::BoxVal,
        };
        words[i] = fresh_typed(func, TirType::DynBox);
        slow_ops.push(new_op(boxing, vec![operands[i]], vec![words[i]], span));
    }
    let slow_result = fresh_typed(func, TirType::DynBox);
    let mut call = op;
    call.opcode = OpCode::CallRuntime;
    call.origin = None;
    call.runtime = Some(runtime);
    call.operands = words.to_vec();
    call.results = vec![slow_result];
    if cannot_raise {
        call.mark_nothrow_proof();
    }
    if let Some(ty) = &exact_result {
        call.mark_exact_result_proof(ty);
    }
    slow_ops.push(call);
    // Two exact numbers compare to a bool word: read its low bit, as the fast
    // arms' raw bool, so the join carries one representation.
    let slow_result = if raw_result {
        let truth = fresh_typed(func, TirType::Bool);
        slow_ops.push(new_op(
            OpCode::UnboxBoolWord,
            vec![slow_result],
            vec![truth],
            span,
        ));
        truth
    } else {
        slow_result
    };
    func.blocks
        .insert(slow, new_block(slow, slow_ops, branch(join, slow_result)));
    finish(func, block, join, result, rest, terminator);
    join
}

fn expand_truthiness(func: &mut TirFunction, block: BlockId, split: Split) -> BlockId {
    let Split {
        op,
        rest,
        terminator,
    } = split;
    let value = op.operands[0];
    let result = op.results[0];
    let span = op.source_span;

    let bool_block = func.fresh_block();
    let check_int = func.fresh_block();
    let int_block = func.fresh_block();
    let slow = func.fresh_block();
    let join = func.fresh_block();
    let is_bool = fresh_typed(func, TirType::Bool);
    let bool_truth = fresh_typed(func, TirType::Bool);
    let is_int = fresh_typed(func, TirType::Bool);
    let payload = fresh_typed(func, TirType::I64);
    let zero = fresh_typed(func, TirType::I64);
    let int_truth = fresh_typed(func, TirType::Bool);
    let slow_truth = fresh_typed(func, TirType::Bool);

    end_block(
        func,
        block,
        vec![new_op(OpCode::IsBoolWord, vec![value], vec![is_bool], span)],
        cond_branch(is_bool, bool_block, check_int),
    );
    let unbox = vec![new_op(
        OpCode::UnboxBoolWord,
        vec![value],
        vec![bool_truth],
        span,
    )];
    func.blocks.insert(
        bool_block,
        new_block(bool_block, unbox, branch(join, bool_truth)),
    );
    let check = vec![new_op(OpCode::IsInlineInt, vec![value], vec![is_int], span)];
    func.blocks.insert(
        check_int,
        new_block(check_int, check, cond_branch(is_int, int_block, slow)),
    );
    let int_ops = vec![
        new_op(OpCode::UnboxInlineInt, vec![value], vec![payload], span),
        TirOp {
            attrs: AttrDict::from([("value".into(), AttrValue::Int(0))]),
            ..new_op(OpCode::ConstInt, vec![], vec![zero], span)
        },
        new_op(OpCode::Ne, vec![payload, zero], vec![int_truth], span),
    ];
    func.blocks.insert(
        int_block,
        new_block(int_block, int_ops, branch(join, int_truth)),
    );
    let mut slow_op = op;
    slow_op.results = vec![slow_truth];
    func.blocks.insert(
        slow,
        new_block(slow, vec![slow_op], branch(join, slow_truth)),
    );
    finish(func, block, join, result, rest, terminator);
    join
}

fn const_int(value: ValueId, constant: i64, span: Option<(u32, u32)>) -> TirOp {
    let mut op = new_op(OpCode::ConstInt, vec![], vec![value], span);
    op.attrs.insert("value".into(), AttrValue::Int(constant));
    op
}

fn runtime_call(
    runtime: molt_ir::runtime_fn_generated::RuntimeFn,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    span: Option<(u32, u32)>,
) -> TirOp {
    let mut op = new_op(OpCode::CallRuntime, operands, results, span);
    op.runtime = Some(runtime);
    op
}

/// ```text
///   %r = Call[guarded @target] %f, args...
/// becomes
///   %e = CallRuntime[direct_call_eligible] %f, nargs, shape ; cond_br %e != 0, ^code, ^slow
/// ^code:   %c = CodeIs[@target] %f ; cond_br %c, ^enter, ^slow
/// ^enter:  %d = CallRuntime[recursion_enter_fast] ; cond_br %d != 0, ^direct, ^refuse
/// ^direct: %r1 = DirectCall[@target] %f, args... ; CallRuntime[recursion_exit_fast]
///          br ^join(%r1)
/// ^refuse: CallRuntime[raise_recursion_error] ; %n = ConstNone ; br ^join(%n)
/// ^slow:   %r2 = Call[guarded @target] %f, args...   ; the op, unchanged
/// ^join(%r): the rest of the block, then its terminator
/// ```
///
/// The admission ops cannot raise. The direct call and the RecursionError
/// raise keep the call's exception source, so the check after the join reads
/// one source. The direct call takes the call's argument custody but borrows
/// the callable, which therefore outlives the call; on the refused path no op
/// takes the inputs, so drop insertion releases them there.
fn expand_guarded_call(func: &mut TirFunction, block: BlockId, split: Split) -> BlockId {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    let Split {
        op,
        rest,
        terminator,
    } = split;
    let callee = op.operands[0];
    let result = op.results[0];
    let span = op.source_span;
    let supplied = (op.operands.len() - 1) as i64;
    // Bit 0: the entry takes a closure (never, here). Bit 1: the entry's
    // custody must match this instruction's adoption of its arguments.
    let shape = if op.argument_custody().is_some() {
        0b10
    } else {
        0
    };

    let [code, enter, direct, refuse, slow, join] = [(); 6].map(|()| func.fresh_block());

    let nargs = fresh_typed(func, TirType::I64);
    let shape_word = fresh_typed(func, TirType::I64);
    let eligible = fresh_typed(func, TirType::I64);
    let zero = fresh_typed(func, TirType::I64);
    let admitted = fresh_typed(func, TirType::Bool);
    end_block(
        func,
        block,
        vec![
            const_int(nargs, supplied, span),
            const_int(shape_word, shape, span),
            runtime_call(
                RuntimeFn::FunctionDirectCallEligible,
                vec![callee, nargs, shape_word],
                vec![eligible],
                span,
            ),
            const_int(zero, 0, span),
            new_op(OpCode::Ne, vec![eligible, zero], vec![admitted], span),
        ],
        cond_branch(admitted, code, slow),
    );

    let same_code = fresh_typed(func, TirType::Bool);
    let mut code_is = new_op(OpCode::CodeIs, vec![callee], vec![same_code], span);
    for key in ["s_value", "_var"] {
        if let Some(target) = op.attrs.get(key) {
            code_is.attrs.insert(key.into(), target.clone());
        }
    }
    func.blocks.insert(
        code,
        new_block(code, vec![code_is], cond_branch(same_code, enter, slow)),
    );

    let depth_ok = fresh_typed(func, TirType::I64);
    let zero_depth = fresh_typed(func, TirType::I64);
    let entered = fresh_typed(func, TirType::Bool);
    func.blocks.insert(
        enter,
        new_block(
            enter,
            vec![
                runtime_call(RuntimeFn::RecursionEnterFast, vec![], vec![depth_ok], span),
                const_int(zero_depth, 0, span),
                new_op(OpCode::Ne, vec![depth_ok, zero_depth], vec![entered], span),
            ],
            cond_branch(entered, direct, refuse),
        ),
    );

    let direct_result = fresh_typed(func, TirType::DynBox);
    let mut call = op.clone();
    call.opcode = OpCode::DirectCall;
    call.origin = None;
    call.results = vec![direct_result];
    if let Some(mut custody) = op.argument_custody() {
        custody[0] = crate::ir::ParameterCustody::Borrowed;
        call.set_argument_custody(&custody);
    }
    func.blocks.insert(
        direct,
        new_block(
            direct,
            vec![
                call,
                runtime_call(RuntimeFn::RecursionExitFast, vec![], vec![], span),
            ],
            branch(join, direct_result),
        ),
    );

    let raised = fresh_typed(func, TirType::I64);
    let none = fresh_typed(func, TirType::None);
    let mut raise = runtime_call(RuntimeFn::RaiseRecursionError, vec![], vec![raised], span);
    if let Some(source) = op.attrs.get("exception_source") {
        raise
            .attrs
            .insert("exception_source".into(), source.clone());
    }
    func.blocks.insert(
        refuse,
        new_block(
            refuse,
            vec![raise, new_op(OpCode::ConstNone, vec![], vec![none], span)],
            branch(join, none),
        ),
    );

    let slow_result = fresh_typed(func, TirType::DynBox);
    let mut slow_call = op;
    slow_call.results = vec![slow_result];
    func.blocks.insert(
        slow,
        new_block(slow, vec![slow_call], branch(join, slow_result)),
    );
    finish(func, block, join, result, rest, terminator);
    join
}

#[cfg(test)]
mod tests;
