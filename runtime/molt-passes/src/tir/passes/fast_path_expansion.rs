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
//! Floor division (`FloorDiv`) and modulo (`Mod`) take the int arm, which
//! leaves for the runtime on a zero divisor so the int ZeroDivisionError is
//! raised; modulo also takes the float and mixed arms. The raw ops follow
//! Python's floor and sign rules in the backend's division lane.
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
//! A comparison also takes a full-range raw int (`RawI64FullDeopt`: the
//! counter of `for i in range(n)` when `n` is unknown), in its int arm only:
//! comparing cannot overflow, and the int's conversion to float is inexact
//! past 2**53. The slow arm boxes it with `BoxVal`.
//!
//! `Add`, `Sub` and `Mul` take a full-range raw int through a checked op:
//! `CheckedAdd` or `CheckedMul` yields the wrapped result and an overflow
//! flag, and the arm leaves for the runtime when the flag is set. A
//! subtraction adds the negated right operand: an inline payload negates
//! exactly; a full-range right operand has
//! no exact negation (`i64::MIN`) and keeps the runtime entry. Two raw
//! operands stay the backend's raw lane.
//!
//! A shift (`Shl`, `Shr`) takes an int arm only:
//!
//! ```text
//!   %r = Shr %a, %c                          ; %a boxed, %c a raw inline int
//! becomes
//!   %ta = IsInlineInt %a ; cond_br %ta, ^int, ^slow
//! ^int:   %x = UnboxInlineInt %a
//!         %ge = Ge %c, 0 ; cond_br %ge, ^hi, ^slow
//! ^hi:    %le = Le %c, 63 ; cond_br %le, ^shift, ^slow
//! ^shift: %s = Shr %x, %c                    ; floors, as Python's `>>` does
//!         %r1 = BoxVal %s ; br ^join(%r1)
//! ^slow:  %r2 = CallRuntime[molt_rshift] %a, box(%c) ; br ^join(%r2)
//! ```
//!
//! A full-range value shifts right only inside the inline window (two more
//! compares), so the machine shift's result is an inline int by construction.
//! `x << c` is the checked product `x * (1 << c)` for a count in `[0, 45]`:
//! the power of two is an inline int and the product's overflow flag sends a
//! result past i64 to the runtime. A negative count (ValueError), a larger
//! count and a big int are the runtime's.
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

use molt_ir::deterministic_hash::HashMap;

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
    let raise = super::check_exception_elim::classify::RaiseFacts::of(func);
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
                    &raise,
                )
            }
            Expansion::FloatSqrt { operand } => expand_float_sqrt(func, block, split, operand),
            Expansion::Truthiness => expand_truthiness(func, block, split),
            Expansion::GuardedCall => expand_guarded_call(func, block, split, None),
            Expansion::ProvenCall { closure } => {
                expand_guarded_call(func, block, split, Some(closure))
            }
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
    FloatSqrt {
        operand: NumOperand,
    },
    Truthiness,
    /// A guarded call whose target's linkage row admits a direct call.
    GuardedCall,
    /// A guarded call whose callee integration proved (HF-846 slice 2).
    /// `closure`: the target takes its function's closure tuple first.
    ProvenCall {
        closure: bool,
    },
}

/// What the stage needs of a guarded call's target, from its linkage row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalleeFacts {
    /// Declared parameters, the closure included.
    pub arity: usize,
    pub has_closure: bool,
}

/// The linkage facts of every target a function's guarded calls name.
pub type GuardedTargets = std::collections::BTreeMap<molt_ir::FunctionId, CalleeFacts>;

/// The speculative target a guarded call names: the compiled function its
/// frontend expected, by its typed callee. The callee word may be any
/// object at run time.
fn guarded_target(op: &TirOp) -> Option<molt_ir::FunctionId> {
    if op.opcode != OpCode::Call || op.origin != Some(molt_ir::OpKind::CallGuarded) {
        return None;
    }
    op.callee
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
        .map(|callee| {
            // The driver's table holds every row of the document, so a
            // missing target is a wiring defect, never a reason to keep the
            // slow lane in silence.
            let signature = linkage.get(callee.index()).unwrap_or_else(|| {
                panic!(
                    "guarded call in `{}` names function #{}, outside its document",
                    func.name, callee.0
                )
            });
            (
                callee,
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
    /// A raw exact int of the full i64 range (`RawI64FullDeopt`): a loop
    /// counter whose bound is unknown. Its conversion to float is inexact
    /// past 2**53, so it joins int arms only, and a sum or product of it
    /// could leave i64, so those arms use a checked op (`FullRange`).
    RawInt,
    /// A raw bool (`Repr::Bool`) of an int-valued op (`+ - *`, comparisons):
    /// the int 0 or 1, since bool subclasses int. It joins int arms only (its
    /// raw lane widens the bit), and the slow arm boxes it as the bool it is,
    /// so a runtime operator or a user `__radd__` still receives True/False.
    RawBool,
}

/// How an op's int arm takes a full-range raw int.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FullRange {
    /// No arm takes it: the op keeps its lowering.
    Declined,
    /// The arm is exact on any i64: a comparison, or a shift, whose arm
    /// guards its operands.
    Exact,
    /// The arm computes with this checked op and leaves for the runtime on
    /// overflow.
    Checked(OpCode),
    /// `a - b` is `CheckedAdd(a, -b)` where `-b` is exact.
    CheckedSub,
}

/// The largest count the `<<` arm takes: `1 << 45` is an inline int.
const SHL_COUNT_MAX: i64 = 45;
/// The largest count a machine shift of an i64 takes.
const SHR_COUNT_MAX: i64 = 63;

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
    shape: ArmShape,
    /// The arm adds the negated right operand (a full-range subtraction).
    negate_rhs: bool,
    /// The arm leaves for the runtime on a zero int divisor, which raises
    /// the int ZeroDivisionError; a division rule sets it on its two-int arm.
    zero_guard: bool,
}

/// A division rule's expansion: its two-int arm guards a zero divisor.
fn guarding_int_divisor(expansion: Option<Expansion>) -> Option<Expansion> {
    let mut expansion = expansion?;
    if let Expansion::Numeric { arms, .. } = &mut expansion {
        for arm in arms {
            arm.zero_guard = arm.payloads == [Payload::Int, Payload::Int];
        }
    }
    Some(expansion)
}

/// Expand an exact-float MathSqrt call. One ordered comparison sends
/// negatives and NaNs to the original call, which owns the exception
/// source and the portable NaN result bits.
fn expand_float_sqrt(
    func: &mut TirFunction,
    block: BlockId,
    split: Split,
    operand: NumOperand,
) -> BlockId {
    let Split {
        mut op,
        rest,
        terminator,
    } = split;
    let span = op.source_span;
    let input = op.operands[0];
    let result = op.results[0];
    let slow = func.fresh_block();
    let join = func.fresh_block();
    let domain = func.fresh_block();
    let fast = func.fresh_block();
    let raw = if matches!(operand, NumOperand::RawFloat) {
        end_block(func, block, Vec::new(), branch_to(domain));
        input
    } else {
        let inline = fresh_typed(func, TirType::Bool);
        let raw = fresh_typed(func, TirType::F64);
        let unbox = func.fresh_block();
        end_block(
            func,
            block,
            vec![new_op(OpCode::IsFloatWord, vec![input], vec![inline], span)],
            cond_branch(inline, unbox, slow),
        );
        func.blocks.insert(
            unbox,
            new_block(
                unbox,
                vec![new_op(OpCode::UnboxFloatWord, vec![input], vec![raw], span)],
                branch_to(domain),
            ),
        );
        raw
    };
    let zero = fresh_typed(func, TirType::F64);
    let nonnegative = fresh_typed(func, TirType::Bool);
    let mut zero_op = new_op(OpCode::ConstFloat, Vec::new(), vec![zero], span);
    zero_op
        .attrs
        .insert("f_value".into(), AttrValue::Float(0.0));
    func.blocks.insert(
        domain,
        new_block(
            domain,
            vec![
                zero_op,
                new_op(OpCode::Ge, vec![raw, zero], vec![nonnegative], span),
            ],
            cond_branch(nonnegative, fast, slow),
        ),
    );
    let root = fresh_typed(func, TirType::F64);
    let fast_result = fresh_typed(func, TirType::DynBox);
    let mut root_box = new_op(OpCode::BoxVal, vec![root], vec![fast_result], span);
    root_box.mark_nothrow_proof();
    func.blocks.insert(
        fast,
        new_block(
            fast,
            vec![
                new_op(OpCode::FloatSqrt, vec![raw], vec![root], span),
                root_box,
            ],
            branch(join, fast_result),
        ),
    );
    let mut slow_ops = Vec::new();
    if matches!(operand, NumOperand::RawFloat) {
        let word = fresh_typed(func, TirType::DynBox);
        let mut boxed = new_op(OpCode::BoxVal, vec![input], vec![word], span);
        boxed.mark_nothrow_proof();
        slow_ops.push(boxed);
        op.operands = vec![word];
    }
    let slow_result = fresh_typed(func, TirType::DynBox);
    op.opcode = OpCode::CallRuntime;
    op.origin = None;
    op.results = vec![slow_result];
    slow_ops.push(op);
    func.blocks
        .insert(slow, new_block(slow, slow_ops, branch(join, slow_result)));
    finish(func, block, join, result, rest, terminator);
    join
}

/// What a fast arm does around its op. The rule sets it; the arm's builder
/// reads it and never re-derives it from the opcode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArmShape {
    /// The op computes the result.
    Plain,
    /// The op (`CheckedAdd`, `CheckedMul`) yields the result and an overflow
    /// flag; the arm leaves for the runtime on overflow.
    Checked,
    /// `>>`: guard the count to [0, 63] and a full-range value to the inline
    /// window, then shift.
    ShiftRight,
    /// `<<`: guard the count to [0, 45], then a checked product with the
    /// inline power of two.
    ShiftLeft,
}

fn is_boxed_word(repr: &HashMap<ValueId, Repr>, value: ValueId) -> bool {
    matches!(
        repr.get(&value),
        Some(Repr::DynBox | Repr::MaybeBigInt | Repr::BoxedImmortal)
    )
}

/// `admit_bool`: the op's result is an int or a comparison whatever its int
/// operands are (`+ - *`, comparisons), so a raw bool is the int 0 or 1, an
/// inline int. Bitwise ops refuse it: `True & True` is the bool True.
fn num_operand(
    repr: &HashMap<ValueId, Repr>,
    value: ValueId,
    admit_bool: bool,
) -> Option<NumOperand> {
    match repr.get(&value) {
        _ if is_boxed_word(repr, value) => Some(NumOperand::Boxed),
        Some(Repr::RawI64Safe) => Some(NumOperand::InlineInt),
        Some(Repr::Bool) if admit_bool => Some(NumOperand::RawBool),
        Some(Repr::FloatUnboxed) => Some(NumOperand::RawFloat),
        Some(Repr::RawI64FullDeopt) => Some(NumOperand::RawInt),
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
    // Runtime identity is decoded and verified at ingest; annotations alone
    // do not prove the carrier of an exact semantic float.
    if op.opcode == OpCode::Call
        && op.callee.is_none()
        && op.runtime == Some(molt_ir::runtime_fn_generated::RuntimeFn::MathSqrt)
        && op.operands.len() == 1
    {
        let value = op.operands[0];
        let operand = num_operand(repr, value, false)?;
        if matches!(operand, NumOperand::RawFloat)
            || (matches!(operand, NumOperand::Boxed) && exact.get(&value) == Some(&TirType::F64))
        {
            return Some(Expansion::FloatSqrt { operand });
        }
    }
    // `int` is the two-int arm's op, raw type and shape, or None where the op
    // has no such arm (true division computes on floats); `float` is the op
    // and raw type every other arm computes with in F64, or None where a float
    // operand is the runtime's (bitwise ops raise TypeError on floats).
    let numeric = |int: Option<(OpCode, TirType, ArmShape)>,
                   float: Option<(OpCode, TirType)>,
                   full: FullRange,
                   admit_bool: bool| {
        let [lhs, rhs] = op.operands[..] else {
            return None;
        };
        let operands = [
            num_operand(repr, lhs, admit_bool)?,
            num_operand(repr, rhs, admit_bool)?,
        ];
        op.numeric_runtime()?;
        let full_range = operands.contains(&NumOperand::RawInt);
        // Two raw operands are the backend's raw lane. An expansion of one
        // whose result is boxed measured 4x slower per iteration: such an op
        // is a checked loop's overflow exit, and boxing its result here gave
        // the loop's raw carriers refcount ops on every iteration.
        if !operands.contains(&NumOperand::Boxed) {
            return None;
        }
        // The int arm's op for a full-range operand, and the negation a
        // subtraction needs.
        let mut negate_rhs = false;
        let int = match (full_range, full) {
            (false, _) | (true, FullRange::Exact) => int,
            (true, FullRange::Declined) => return None,
            (true, FullRange::Checked(checked)) => Some((checked, TirType::I64, ArmShape::Checked)),
            (true, FullRange::CheckedSub) => {
                // The right operand is the boxed one (two raw operands are
                // the backend's): its inline payload negates exactly.
                if operands[1] != NumOperand::Boxed {
                    return None;
                }
                negate_rhs = true;
                Some((OpCode::CheckedAdd, TirType::I64, ArmShape::Checked))
            }
        };
        // The payloads an operand may hold: a raw value is its own kind, and a
        // boxed operand proven an exact int (or float) takes no arm that reads
        // it as the other kind.
        let admits = |i: usize, payload: Payload| match operands[i] {
            NumOperand::InlineInt | NumOperand::RawInt | NumOperand::RawBool => {
                payload == Payload::Int
            }
            NumOperand::RawFloat => payload == Payload::Float,
            NumOperand::Boxed => match exact.get(&op.operands[i]) {
                Some(TirType::F64) => payload == Payload::Float,
                Some(TirType::I64 | TirType::BigInt | TirType::Bool) => payload == Payload::Int,
                // An exact str, bytes or None word is never a number: no arm
                // tests it (`"%d" % n` and `s + t` call the runtime at once).
                Some(TirType::Str | TirType::Bytes | TirType::None) => false,
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
            // A float arm would convert the raw bool's bit as an i64 word;
            // the slow arm owns bool with float.
            if operands.contains(&NumOperand::RawBool) && payloads.contains(&Payload::Float) {
                continue;
            }
            let (fast, raw, compute, shape) = match (&int, &float, payloads) {
                (Some((fast, raw, shape)), _, [Payload::Int, Payload::Int]) => {
                    (*fast, raw.clone(), TirType::I64, *shape)
                }
                // A full-range int converts to float inexactly: int arms only.
                (_, Some(_), _) if full_range => continue,
                (_, Some((fast, raw)), _) => (*fast, raw.clone(), TirType::F64, ArmShape::Plain),
                _ => continue,
            };
            arms.push(NumericArm {
                payloads,
                compute,
                fast,
                raw,
                shape,
                negate_rhs,
                zero_guard: false,
            });
        }
        // A comparison planned as a raw bool (two exact numbers, one of them
        // a boxed word) joins its arms raw; without this case no stage and no
        // backend lane owned the op, and it fell to the runtime with a
        // truthiness call on its result.
        // An int-only op whose result the plan proves inline-safe (a range
        // proof such as `x & 63`) joins raw too; otherwise the backend saw a
        // raw result over a boxed operand and called the runtime.
        let raw_result = arms.first().is_some_and(|arm| match arm.raw {
            TirType::Bool => repr.get(&op.results[0]) == Some(&Repr::Bool),
            TirType::I64 => {
                arms.iter().all(|arm| arm.compute == TirType::I64)
                    && repr.get(&op.results[0]) == Some(&Repr::RawI64Safe)
            }
            _ => false,
        });
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
            Some((OpCode::Add, TirType::I64, ArmShape::Plain)),
            Some((OpCode::Add, TirType::F64)),
            FullRange::Checked(OpCode::CheckedAdd),
            true,
        ),
        FastPathExpansionRule::NumericSub => numeric(
            Some((OpCode::Sub, TirType::I64, ArmShape::Plain)),
            Some((OpCode::Sub, TirType::F64)),
            FullRange::CheckedSub,
            true,
        ),
        // The int product of two inline payloads can leave i64: CheckedMul
        // tests the overflow and the arm leaves for the runtime on it.
        FastPathExpansionRule::NumericMul => numeric(
            Some((OpCode::CheckedMul, TirType::I64, ArmShape::Checked)),
            Some((OpCode::Mul, TirType::F64)),
            FullRange::Checked(OpCode::CheckedMul),
            true,
        ),
        // True division computes in F64 on every arm, two ints included: two
        // inline payloads convert exactly, and one IEEE division of exact
        // operands is CPython's correctly rounded int quotient below 2**53
        // (`long_true_divide`). Raw F64 division raises on a zero divisor.
        FastPathExpansionRule::NumericTrueDiv => guarding_int_divisor(numeric(
            None,
            Some((OpCode::Div, TirType::F64)),
            FullRange::Declined,
            false,
        )),
        // `//` of two inline payloads: the quotient is no larger than the
        // dividend, so it is an inline int, and the arm's divisor guard sends
        // zero to the runtime. No float arm: no backend lowers a raw float
        // floor division. A full-range dividend declines, since
        // `i64::MIN // -1` is `2**63`.
        FastPathExpansionRule::NumericFloorDiv => guarding_int_divisor(numeric(
            Some((OpCode::FloorDiv, TirType::I64, ArmShape::Plain)),
            None,
            FullRange::Declined,
            false,
        )),
        // `%` takes the int arm behind the divisor guard and the float and
        // mixed arms, whose int payload converts exactly as CPython converts
        // it. A remainder fits i64 for any operands (`x % -1` is 0), so a
        // full-range int takes the int arm.
        FastPathExpansionRule::NumericMod => guarding_int_divisor(numeric(
            Some((OpCode::Mod, TirType::I64, ArmShape::Plain)),
            Some((OpCode::Mod, TirType::F64)),
            FullRange::Exact,
            false,
        )),
        // AND, OR and XOR of two inline-int payloads stay within the inline
        // range, so the int arm never overflows; a float operand is the
        // runtime's, which raises TypeError.
        // A full-range operand keeps the runtime entry: its result is not
        // an inline int, and no backend holds a full-range bitwise result raw.
        FastPathExpansionRule::NumericBitwise => numeric(
            Some((op.opcode, TirType::I64, ArmShape::Plain)),
            None,
            FullRange::Declined,
            false,
        ),
        // A shift of two ints; the arm guards the count (and a full-range
        // value) before the machine shift. A float operand is the runtime's,
        // which raises TypeError.
        FastPathExpansionRule::NumericShiftLeft => numeric(
            Some((OpCode::Shl, TirType::I64, ArmShape::ShiftLeft)),
            None,
            FullRange::Exact,
            false,
        ),
        FastPathExpansionRule::NumericShiftRight => numeric(
            Some((OpCode::Shr, TirType::I64, ArmShape::ShiftRight)),
            None,
            FullRange::Exact,
            false,
        ),
        // A comparison on two payloads is the same comparison on raw values;
        // float comparisons keep IEEE order, so NaN compares unequal.
        FastPathExpansionRule::NumericCompare => numeric(
            Some((op.opcode, TirType::Bool, ArmShape::Plain)),
            Some((op.opcode, TirType::Bool)),
            FullRange::Exact,
            true,
        ),
        FastPathExpansionRule::Truthiness => (op.operands.len() == 1
            && is_boxed_word(repr, op.operands[0]))
        .then_some(Expansion::Truthiness),
        // An unproven closure-taking target and an arity mismatch keep the
        // runtime lane: binding, not a static ABI, must answer them.
        FastPathExpansionRule::GuardedCall => {
            let facts = targets.get(&guarded_target(op)?)?;
            let boxed_callee = is_boxed_word(repr, op.operands[0]);
            if op.proven_target() {
                // Integration proved the callee is the target's function
                // object with an exact positional call (HF-846 slice 2); the
                // linkage row must agree, or the wiring is broken. A closure
                // target's arity counts its environment, which the call
                // takes from that function object (design 78 5.10).
                let parameters = op.operands.len() - usize::from(!facts.has_closure);
                assert!(
                    boxed_callee && facts.arity == parameters,
                    "proven call of function #{} disagrees with its linkage row",
                    guarded_target(op).map_or(u32::MAX, |id| id.0)
                );
                return Some(Expansion::ProvenCall {
                    closure: facts.has_closure,
                });
            }
            let admitted =
                !facts.has_closure && facts.arity + 1 == op.operands.len() && boxed_callee;
            admitted.then_some(Expansion::GuardedCall)
        }
        FastPathExpansionRule::None => None,
    }
}

/// Split each conditional edge that passes a raw scalar to a boxed block
/// argument.
///
/// A backend boxes a raw scalar that meets a boxed block argument in the
/// edge's source block, before the branch, so on a conditional branch the box
/// runs on both edges. A loop whose exit passes its raw carrier to a boxed
/// join boxed it on every iteration: one `molt_int_from_i64` call per
/// iteration of `for i in range(n): total += i`. A forwarding block on the
/// edge becomes the box's source block, so it runs only where the value
/// leaves. The block holds no op: an explicit `BoxVal` would be a boxed
/// observation of the binding, and the representation plan would then box
/// the whole loop carrier. Returns the edges it split.
pub fn box_raw_edge_args(func: &mut TirFunction) -> usize {
    let vr = crate::representation_facts::value_range_for(func);
    let repr = crate::representation_facts::repr_by_value_for(func, Some(&vr));
    let raw_scalar = |value: ValueId| {
        matches!(
            repr.get(&value),
            Some(Repr::RawI64Safe | Repr::RawI64FullDeopt | Repr::FloatUnboxed)
        )
    };
    let mut blocks: Vec<BlockId> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    let mut split = 0;
    for block in blocks {
        let Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } = &func.blocks[&block].terminator
        else {
            continue;
        };
        let edges = [
            (*then_block, then_args.clone()),
            (*else_block, else_args.clone()),
        ];
        let mut retargets = [None, None];
        for (side, (target, args)) in edges.into_iter().enumerate() {
            let params: Vec<ValueId> = func.blocks[&target].args.iter().map(|a| a.id).collect();
            let boxes = (0..args.len().min(params.len()))
                .any(|i| raw_scalar(args[i]) && is_boxed_word(&repr, params[i]));
            if !boxes {
                continue;
            }
            let edge = func.fresh_block();
            func.blocks.insert(
                edge,
                new_block(edge, Vec::new(), Terminator::Branch { target, args }),
            );
            retargets[side] = Some(edge);
            split += 1;
        }
        if let Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } = &mut func
            .blocks
            .get_mut(&block)
            .expect("listed block")
            .terminator
        {
            if let Some(edge) = retargets[0] {
                *then_block = edge;
                then_args.clear();
            }
            if let Some(edge) = retargets[1] {
                *else_block = edge;
                else_args.clear();
            }
        }
    }
    split
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

fn branch_to(target: BlockId) -> Terminator {
    Terminator::Branch {
        target,
        args: Vec::new(),
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

/// Supply the lexical environment that a raw direct call leaves implicit.
/// Each local construction defines a fresh SSA name for its capture. The
/// caller's own environment is the initial definition on a recursive edge.
/// SSA repair merges reaching definitions at joins without rebinding another
/// use of the original capture. The physical call then has exactly one
/// operand per declared parameter, including the borrowed environment.
pub fn lower_static_call_arguments(
    func: &mut TirFunction,
    owner: molt_ir::FunctionId,
    signatures: &[molt_ir::ir::ExternFunctionSignature],
) -> bool {
    use crate::tir::call_targets::direct_callee;
    use crate::tir::ssa_update::{Variable, repair};
    use std::collections::BTreeMap;

    let mut environments = BTreeMap::new();
    for op in func.blocks.values().flat_map(|block| &block.ops) {
        let Some(target) = direct_callee(op) else {
            continue;
        };
        let signature = &signatures[target.index()];
        // First-class TIR may already spell its physical arguments. Source
        // calls omit the lexical environment at the transport boundary.
        if op.origin.is_none() && op.operands.len() == signature.arity {
            continue;
        }
        assert_eq!(
            op.operands.len() + usize::from(signature.has_closure),
            signature.arity,
            "static call argument ABI mismatch for function #{} in {}",
            target.index(),
            func.name
        );
        if signature.has_closure {
            environments.entry(target).or_insert_with(|| Variable {
                names: Vec::new(),
                ty: TirType::DynBox,
            });
        }
    }
    if environments.is_empty() {
        return false;
    }

    let mut definitions = Vec::new();
    if environments.contains_key(&owner) {
        let incoming = func.blocks[&func.entry_block].args[0].id;
        definitions.push((func.entry_block, 0, owner, incoming, None));
    }
    for (&block, body) in &func.blocks {
        for (index, op) in body.ops.iter().enumerate() {
            if op.origin == Some(molt_ir::OpKind::FuncNewClosure)
                && let Some(target) = op.callee
                && environments.contains_key(&target)
            {
                definitions.push((block, index + 1, target, op.operands[0], op.source_span));
            }
        }
    }
    definitions.sort_by_key(|&(block, index, ..)| std::cmp::Reverse((block, index)));
    for (block, index, target, capture, span) in definitions {
        let name = fresh_typed(func, TirType::DynBox);
        let alias = new_op(OpCode::Copy, vec![capture], vec![name], span);
        func.blocks
            .get_mut(&block)
            .unwrap()
            .ops
            .insert(index, alias);
        environments.get_mut(&target).unwrap().names.push(name);
    }
    for op in func.blocks.values_mut().flat_map(|block| &mut block.ops) {
        if let Some(target) = direct_callee(op)
            && !(op.origin.is_none() && op.operands.len() == signatures[target.index()].arity)
            && let Some(variable) = environments.get(&target)
        {
            let seed = variable.names.first().unwrap_or_else(|| {
                panic!(
                    "{} has no lexical environment for function #{}",
                    func.name,
                    target.index()
                )
            });
            let custody = op.argument_custody();
            op.operands.insert(0, *seed);
            if let Some(mut custody) = custody {
                custody.insert(0, molt_ir::ParameterCustody::Borrowed);
                op.set_argument_custody(&custody);
            }
        }
    }
    let variables: Vec<_> = environments.into_values().collect();
    repair(func, &variables).unwrap_or_else(|cause| {
        panic!(
            "{}: no reaching closure environment on a static call edge: {cause:?}",
            func.name
        )
    });
    true
}

/// Charge ordinary compiled calls whose target can reenter Python. The callee's
/// finalized body proves when admission can be omitted. An extern has no such
/// proof. Guarded calls own admission in their guarded expansion.
pub(crate) fn charge_static_calls(func: &mut TirFunction, leaves: &molt_ir::FunctionSet) -> bool {
    let mut sites: Vec<_> = func
        .blocks
        .iter()
        .flat_map(|(&block, body)| {
            body.ops.iter().enumerate().filter_map(move |(index, op)| {
                crate::tir::call_targets::direct_callee(op)
                    .filter(|&id| !leaves.contains(id))
                    .map(|_| (block, index))
            })
        })
        .collect();
    // Split later sites first so indices in the original block stay valid.
    sites.sort_unstable_by(|a, b| b.cmp(a));
    let changed = !sites.is_empty();
    for (block, index) in sites {
        let Split {
            op,
            rest,
            terminator,
        } = split(func, block, index);
        let span = op.source_span;
        let admitted = fresh_typed(func, TirType::Bool);
        let enter = func.fresh_block();
        let refuse = func.fresh_block();
        end_block(
            func,
            block,
            vec![new_op(
                OpCode::RecursionAdmits,
                vec![],
                vec![admitted],
                span,
            )],
            cond_branch(admitted, enter, refuse),
        );

        let mut raise = runtime_call(
            molt_ir::runtime_fn_generated::RuntimeFn::RaiseRecursionError,
            vec![],
            vec![],
            span,
        );
        if let Some(source) = op.attrs.get("exception_source") {
            raise
                .attrs
                .insert("exception_source".into(), source.clone());
        }
        let mut failure = vec![raise];
        // A source handler may catch this refusal in the calling function.
        // The refusal supplies None for an exceptional edge carrying the
        // unproduced call result, and has fresh SSA definitions of its own.
        if let Some(check) = rest
            .first()
            .filter(|check| check.opcode == OpCode::CheckException)
        {
            let mut check = check.clone();
            if check
                .operands
                .iter()
                .any(|value| op.results.contains(value))
            {
                let none = fresh_typed(func, TirType::None);
                failure.push(new_op(OpCode::ConstNone, vec![], vec![none], span));
                for value in &mut check.operands {
                    if op.results.contains(value) {
                        *value = none;
                    }
                }
            }
            for result in &mut check.results {
                let ty = func
                    .value_types
                    .get(result)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                *result = fresh_typed(func, ty);
            }
            failure.push(check);
        }
        // No callee adopted its arguments. Lifetime finalization sees the
        // refusal edge and releases every outstanding owner exactly once.
        func.blocks.insert(
            refuse,
            new_block(refuse, failure, Terminator::Return { values: vec![] }),
        );
        let mut entered = Vec::with_capacity(rest.len() + 3);
        entered.push(new_op(OpCode::RecursionCount, vec![], vec![], span));
        entered.push(op);
        entered.push(new_op(OpCode::RecursionRelease, vec![], vec![], span));
        entered.extend(rest);
        func.blocks
            .insert(enter, new_block(enter, entered, terminator));
        for cond in func.loop_cond_blocks.values_mut() {
            if *cond == block {
                *cond = enter;
            }
        }
    }
    changed
}

/// Every op an expansion creates states its own raise contract, from the one
/// instance classifier, over the values it creates (whose types it knows):
/// boxing a bool or float, a raw compare, an unbox after its tag test cannot
/// raise and carries the no-throw proof; anything that can (boxing a raw int
/// that may leave the inline window, a float division) keeps the original
/// op's exception source, as the slow arm's call does. Unmarked, such an op
/// read as a new origin source beside the slow arm's unwind source, and the
/// exception-region verifier rejected the join (HF-891: a `<` on two boxed
/// words in a generator's nested range loop).
fn state_fast_arm_raise_proofs(func: &TirFunction, fast_ops: &mut [TirOp], original: &TirOp) {
    let source = original.attrs.get("exception_source").cloned();
    for fast in fast_ops.iter_mut() {
        if fast.has_nothrow_proof() {
            continue;
        }
        if crate::tir::op_semantics::op_instance_effects_for_op(fast, &func.value_types).nothrow {
            fast.mark_nothrow_proof();
        } else if let Some(source) = &source {
            fast.attrs.insert("exception_source".into(), source.clone());
        }
    }
}

/// A fast arm under construction: its current block and that block's ops.
struct FastArm<'a> {
    block: BlockId,
    ops: Vec<TirOp>,
    slow: BlockId,
    original: &'a TirOp,
}

impl FastArm<'_> {
    /// End the current block on `holds`: the arm goes on in a new block where
    /// it is `expected`, and leaves for the slow arm otherwise.
    fn guard(&mut self, func: &mut TirFunction, holds: ValueId, expected: bool) {
        let next = func.fresh_block();
        state_fast_arm_raise_proofs(func, &mut self.ops, self.original);
        let end = if expected {
            cond_branch(holds, next, self.slow)
        } else {
            cond_branch(holds, self.slow, next)
        };
        func.blocks.insert(
            self.block,
            new_block(self.block, std::mem::take(&mut self.ops), end),
        );
        self.block = next;
    }

    /// Leave for the slow arm unless `lo <= value <= hi`.
    fn within(&mut self, func: &mut TirFunction, value: ValueId, lo: i64, hi: i64) {
        let span = self.original.source_span;
        for (compare, bound) in [(OpCode::Ge, lo), (OpCode::Le, hi)] {
            let limit = fresh_typed(func, TirType::I64);
            let holds = fresh_typed(func, TirType::Bool);
            self.ops.push(const_int(limit, bound, span));
            self.ops
                .push(new_op(compare, vec![value, limit], vec![holds], span));
            self.guard(func, holds, true);
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
    raise: &super::check_exception_elim::classify::RaiseFacts,
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
    let cannot_raise = !raise.op_may_raise(&op);
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
        let mut code = FastArm {
            block: fast_block,
            ops: Vec::new(),
            slow,
            original: &op,
        };
        let mut payloads = operands;
        for i in 0..2 {
            if kinds[i] == NumOperand::Boxed {
                let (_, unbox_op, payload_ty) = payload_ops(arm.payloads[i]);
                payloads[i] = fresh_typed(func, payload_ty);
                code.ops
                    .push(new_op(unbox_op, vec![operands[i]], vec![payloads[i]], span));
            }
        }
        // Two ints divided by zero raise the int message ("division by
        // zero", "integer modulo by zero"), not a float lane's: a zero int
        // divisor leaves for the runtime with the original words.
        if arm.zero_guard {
            let zero = fresh_typed(func, TirType::I64);
            let nonzero = fresh_typed(func, TirType::Bool);
            code.ops.push(const_int(zero, 0, span));
            code.ops.push(new_op(
                OpCode::Ne,
                vec![payloads[1], zero],
                vec![nonzero],
                span,
            ));
            code.guard(func, nonzero, true);
        }
        // A machine shift takes a count in [0, 63]; a negative count raises
        // and a larger one is the runtime's. `>>` of a value in the inline
        // window stays in it; `<<` multiplies by an inline power of two.
        match arm.shape {
            ArmShape::ShiftRight => {
                code.within(func, payloads[1], 0, SHR_COUNT_MAX);
                if kinds[0] == NumOperand::RawInt {
                    code.within(
                        func,
                        payloads[0],
                        molt_ir::tir::numeric_facts::INLINE_INT47_LO,
                        molt_ir::tir::numeric_facts::INLINE_INT47_HI,
                    );
                }
            }
            ArmShape::ShiftLeft => {
                code.within(func, payloads[1], 0, SHL_COUNT_MAX);
                let one = fresh_typed(func, TirType::I64);
                let power = fresh_typed(func, TirType::I64);
                code.ops.push(const_int(one, 1, span));
                let mut shift = new_op(OpCode::Shl, vec![one, payloads[1]], vec![power], span);
                shift.mark_inline_int_proof();
                shift.mark_nothrow_proof();
                code.ops.push(shift);
                payloads[1] = power;
            }
            ArmShape::Plain | ArmShape::Checked => {}
        }
        // `0 - payload` is exact in i64 for an inline payload.
        if arm.negate_rhs {
            let zero = fresh_typed(func, TirType::I64);
            let negated = fresh_typed(func, TirType::I64);
            code.ops.push(const_int(zero, 0, span));
            code.ops.push(new_op(
                OpCode::Sub,
                vec![zero, payloads[1]],
                vec![negated],
                span,
            ));
            payloads[1] = negated;
        }
        for i in 0..2 {
            // A float arm reads an int payload as its exact f64.
            if arm.compute == TirType::F64 && arm.payloads[i] == Payload::Int {
                let converted = fresh_typed(func, TirType::F64);
                code.ops.push(new_op(
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
        match arm.shape {
            ArmShape::Checked | ArmShape::ShiftLeft => {
                // `x << c` is the checked product `x * (1 << c)`.
                let checked = if arm.shape == ArmShape::ShiftLeft {
                    OpCode::CheckedMul
                } else {
                    arm.fast
                };
                let overflow = fresh_typed(func, TirType::Bool);
                code.ops.push(new_op(
                    checked,
                    payloads.to_vec(),
                    vec![raw, overflow],
                    span,
                ));
                code.guard(func, overflow, false);
            }
            ArmShape::ShiftRight => {
                // The guards hold here: the count is a machine count and the
                // value is in the inline window, so the result is too.
                let mut shift = new_op(arm.fast, payloads.to_vec(), vec![raw], span);
                shift.mark_inline_int_proof();
                shift.mark_nothrow_proof();
                code.ops.push(shift);
            }
            ArmShape::Plain => code
                .ops
                .push(new_op(arm.fast, payloads.to_vec(), vec![raw], span)),
        }
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
            code.ops.push(box_op);
            boxed_result
        };
        state_fast_arm_raise_proofs(func, &mut code.ops, &op);
        func.blocks.insert(
            code.block,
            new_block(code.block, code.ops, branch(join, fast_result)),
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
            NumOperand::RawFloat | NumOperand::RawInt | NumOperand::RawBool => OpCode::BoxVal,
        };
        words[i] = fresh_typed(func, TirType::DynBox);
        let mut box_op = new_op(boxing, vec![operands[i]], vec![words[i]], span);
        // As the fast arm's result box: an op on two exact numbers cannot
        // raise, so its replacement states no new exception source.
        if cannot_raise {
            box_op.mark_nothrow_proof();
        }
        slow_ops.push(box_op);
    }
    // Otherwise boxing a full-range int may allocate: it keeps the original's
    // source.
    state_fast_arm_raise_proofs(func, &mut slow_ops, &op);
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
    // Two exact numbers compare to a bool word, and a range-proven int result
    // is an inline-int word: read its payload, as the fast arms' raw value, so
    // the join carries one representation.
    let slow_result = if raw_result {
        let (raw_type, unbox) = if arms[0].raw == TirType::I64 {
            (TirType::I64, OpCode::UnboxInlineInt)
        } else {
            (TirType::Bool, OpCode::UnboxBoolWord)
        };
        let raw = fresh_typed(func, raw_type);
        slow_ops.push(new_op(unbox, vec![slow_result], vec![raw], span));
        raw
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
            attrs: AttrDict::from_iter([("value".into(), AttrValue::Int(0))]),
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
/// ^enter:  %d = RecursionAdmits ; cond_br %d, ^direct, ^refuse
/// ^direct: RecursionCount ; %r1 = DirectCall[@target] %f, args... ; RecursionRelease
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
///
/// A `proven` call (HF-846 slice 2) has no `^code` and no `^slow`: its block
/// ends in `^enter`'s recursion check. Integration proved the callee is the
/// target's function object, with an exact positional call of a
/// positional-only signature and matching entry custody, so the admission
/// call and the code guard would always pass. When that target takes a
/// closure (`proven == Some(true)`, design 78 5.10), `^direct` first reads the
/// callee's closure tuple and passes it after the callable, where the
/// target's entry takes its environment:
///
/// ```text
/// ^direct: RecursionCount ; %env = FunctionClosure %f
///          %r1 = DirectCall[@target] %f, %env, args... ; RecursionRelease
/// ```
///
/// The tuple is the one the runtime's binding would pass, read live, so a
/// write to one of its cells is seen by the callee. Both `%f` and `%env` are
/// borrowed; `%f` owns `%env` and outlives the call.
fn expand_guarded_call(
    func: &mut TirFunction,
    block: BlockId,
    split: Split,
    proven: Option<bool>,
) -> BlockId {
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

    if proven.is_some() {
        end_block(func, block, vec![], branch_to(enter));
    } else {
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
                {
                    // The admission answer is a machine word, 0 or 1: the
                    // exact facts every raise question reads must know the
                    // compare on it cannot raise.
                    let mut admission = runtime_call(
                        RuntimeFn::FunctionDirectCallEligible,
                        vec![callee, nargs, shape_word],
                        vec![eligible],
                        span,
                    );
                    admission.mark_exact_result_proof(&TirType::I64);
                    admission
                },
                const_int(zero, 0, span),
                new_op(OpCode::Ne, vec![eligible, zero], vec![admitted], span),
            ],
            cond_branch(admitted, code, slow),
        );

        let same_code = fresh_typed(func, TirType::Bool);
        let mut code_is = new_op(OpCode::CodeIs, vec![callee], vec![same_code], span);
        code_is.callee = op.callee;
        func.blocks.insert(
            code,
            new_block(code, vec![code_is], cond_branch(same_code, enter, slow)),
        );
    }

    let admitted = fresh_typed(func, TirType::Bool);
    func.blocks.insert(
        enter,
        new_block(
            enter,
            vec![new_op(
                OpCode::RecursionAdmits,
                vec![],
                vec![admitted],
                span,
            )],
            cond_branch(admitted, direct, refuse),
        ),
    );

    let direct_result = fresh_typed(func, TirType::DynBox);
    let mut call = op.clone();
    call.opcode = OpCode::DirectCall;
    call.origin = None;
    call.results = vec![direct_result];
    let mut direct_ops = vec![new_op(OpCode::RecursionCount, vec![], vec![], span)];
    let custody = op.argument_custody().map(|mut custody| {
        custody[0] = crate::ir::ParameterCustody::Borrowed;
        custody
    });
    if proven == Some(true) {
        let environment = fresh_typed(func, TirType::DynBox);
        direct_ops.push(new_op(
            OpCode::FunctionClosure,
            vec![callee],
            vec![environment],
            span,
        ));
        call.operands.insert(1, environment);
        if let Some(mut custody) = custody {
            custody.insert(1, crate::ir::ParameterCustody::Borrowed);
            call.set_argument_custody(&custody);
        }
    } else if let Some(custody) = custody {
        call.set_argument_custody(&custody);
    }
    direct_ops.push(call);
    direct_ops.push(new_op(OpCode::RecursionRelease, vec![], vec![], span));
    func.blocks.insert(
        direct,
        new_block(direct, direct_ops, branch(join, direct_result)),
    );

    let none = fresh_typed(func, TirType::None);
    let mut raise = runtime_call(RuntimeFn::RaiseRecursionError, vec![], vec![], span);
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

    if proven.is_none() {
        let slow_result = fresh_typed(func, TirType::DynBox);
        let mut slow_call = op;
        slow_call.results = vec![slow_result];
        func.blocks.insert(
            slow,
            new_block(slow, vec![slow_call], branch(join, slow_result)),
        );
    }
    finish(func, block, join, result, rest, terminator);
    join
}

#[cfg(test)]
mod tests;
