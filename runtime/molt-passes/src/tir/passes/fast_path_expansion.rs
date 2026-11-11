//! The late lowering stage of TIR (HF-821): value-level fast paths written once,
//! as representation-level control flow, for every backend that consumes final
//! TIR. Backends then only select instructions; the NaN-box meaning of each
//! primitive comes from molt-codegen-abi's shared generators.
//!
//! Integer `Add`, `Sub`, `InplaceAdd` and `InplaceSub` on two boxed words:
//!
//! ```text
//!   %r = Add %a, %b                          ; both boxed words
//! becomes
//!   %ta = IsInlineInt %a ; cond_br %ta, ^check_b, ^slow
//! ^check_b: %tb = IsInlineInt %b ; cond_br %tb, ^fast, ^slow
//! ^fast:  %x = UnboxInlineInt %a ; %y = UnboxInlineInt %b
//!         %s = Add %x, %y                    ; exact in i64: two 47-bit payloads
//!         %r1 = BoxVal %s                    ; inline if it fits, else a heap int
//!         br ^join(%r1)
//! ^slow:  %r2 = CallRuntime[molt_add] %a, %b ; br ^join(%r2)
//! ^join(%r):  the rest of the block, then its terminator
//! ```
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
//! The original result id becomes the join's argument, so no use is renamed and
//! no existing value changes representation. An in-place op on two ints has no
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
    let mut pending: Vec<BlockId> = func.blocks.keys().copied().collect();
    pending.sort_unstable_by(|a, b| b.cmp(a));
    while let Some(block) = pending.pop() {
        let Some((index, expansion)) = func.blocks[&block]
            .ops
            .iter()
            .enumerate()
            .find_map(|(index, op)| expansion_of(op, &repr, targets).map(|e| (index, e)))
        else {
            continue;
        };
        let split = split(func, block, index);
        let join = match expansion {
            Expansion::IntArith {
                fast,
                raw,
                operands,
            } => expand_int_arith(func, block, split, fast, raw, operands),
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

/// What the stage does with one op, from the generated
/// `fast_path_expansion_rules` table.
enum Expansion {
    /// Integer add, subtract or comparison; in-place forms share the binary
    /// fast arm. `fast` computes on the payloads and yields a `raw` value.
    IntArith {
        fast: OpCode,
        raw: TirType,
        operands: [IntOperand; 2],
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum IntOperand {
    Boxed,
    InlineSafe,
}

fn is_boxed_word(repr: &HashMap<ValueId, Repr>, value: ValueId) -> bool {
    matches!(
        repr.get(&value),
        Some(Repr::DynBox | Repr::MaybeBigInt | Repr::BoxedImmortal)
    )
}

fn int_operand(repr: &HashMap<ValueId, Repr>, value: ValueId) -> Option<IntOperand> {
    if is_boxed_word(repr, value) {
        Some(IntOperand::Boxed)
    } else if repr.get(&value) == Some(&Repr::RawI64Safe) {
        Some(IntOperand::InlineSafe)
    } else {
        None
    }
}

fn expansion_of(
    op: &TirOp,
    repr: &HashMap<ValueId, Repr>,
    targets: &GuardedTargets,
) -> Option<Expansion> {
    if !op.has_valid_shape() || op.results.len() != 1 {
        return None;
    }
    let int_arith = |fast, raw| {
        let [lhs, rhs] = op.operands[..] else {
            return None;
        };
        let operands = [int_operand(repr, lhs)?, int_operand(repr, rhs)?];
        // Two raw operands are the backend's raw lane, not a boxed fast path.
        (operands.contains(&IntOperand::Boxed)
            && is_boxed_word(repr, op.results[0])
            && op.numeric_runtime().is_some())
        .then_some(Expansion::IntArith {
            fast,
            raw,
            operands,
        })
    };
    match opcode_fast_path_expansion_rule_table(op.opcode) {
        FastPathExpansionRule::IntAdd => int_arith(OpCode::Add, TirType::I64),
        FastPathExpansionRule::IntSub => int_arith(OpCode::Sub, TirType::I64),
        // A comparison on two payloads is the same comparison on raw values.
        FastPathExpansionRule::IntCompare => int_arith(op.opcode, TirType::Bool),
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

fn expand_int_arith(
    func: &mut TirFunction,
    block: BlockId,
    split: Split,
    fast: OpCode,
    raw_ty: TirType,
    kinds: [IntOperand; 2],
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
    let boxed: Vec<ValueId> = (0..2)
        .filter(|&i| kinds[i] == IntOperand::Boxed)
        .map(|i| operands[i])
        .collect();

    let fast_block = func.fresh_block();
    let slow = func.fresh_block();
    let join = func.fresh_block();
    // One tag test per boxed operand: the first ends the original block, a
    // second gets its own block; every false edge goes to the slow arm.
    let checks: Vec<BlockId> = std::iter::once(block)
        .chain((1..boxed.len()).map(|_| func.fresh_block()))
        .collect();
    for (k, &operand) in boxed.iter().enumerate() {
        let is_int = fresh_typed(func, TirType::Bool);
        let next = checks.get(k + 1).copied().unwrap_or(fast_block);
        let test = vec![new_op(
            OpCode::IsInlineInt,
            vec![operand],
            vec![is_int],
            span,
        )];
        let check_end = cond_branch(is_int, next, slow);
        if k == 0 {
            end_block(func, block, test, check_end);
        } else {
            func.blocks
                .insert(checks[k], new_block(checks[k], test, check_end));
        }
    }

    let mut fast_ops = Vec::new();
    let mut payloads = operands;
    for i in 0..2 {
        if kinds[i] == IntOperand::Boxed {
            payloads[i] = fresh_typed(func, TirType::I64);
            fast_ops.push(new_op(
                OpCode::UnboxInlineInt,
                vec![operands[i]],
                vec![payloads[i]],
                span,
            ));
        }
    }
    let raw = fresh_typed(func, raw_ty);
    let fast_result = fresh_typed(func, TirType::DynBox);
    fast_ops.push(new_op(fast, payloads.to_vec(), vec![raw], span));
    fast_ops.push(new_op(OpCode::BoxVal, vec![raw], vec![fast_result], span));
    func.blocks.insert(
        fast_block,
        new_block(fast_block, fast_ops, branch(join, fast_result)),
    );

    // The slow arm is the original op's semantics: it keeps its attributes
    // (exception source, location) and calls the op's typed runtime entry,
    // which takes boxed words, so an inline-safe operand is boxed first, by
    // BoxInlineInt: a fresh raise there would mix an Origin source into the
    // join's check, which keeps the original op's declared source.
    let mut slow_ops = Vec::new();
    let mut words = operands;
    for i in 0..2 {
        if kinds[i] == IntOperand::InlineSafe {
            words[i] = fresh_typed(func, TirType::DynBox);
            slow_ops.push(new_op(
                OpCode::BoxInlineInt,
                vec![operands[i]],
                vec![words[i]],
                span,
            ));
        }
    }
    let slow_result = fresh_typed(func, TirType::DynBox);
    let mut call = op;
    call.opcode = OpCode::CallRuntime;
    call.origin = None;
    call.runtime = Some(runtime);
    call.operands = words.to_vec();
    call.results = vec![slow_result];
    slow_ops.push(call);
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
