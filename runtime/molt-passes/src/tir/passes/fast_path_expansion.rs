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
pub fn run(func: &mut TirFunction) -> PassStats {
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
            .find_map(|(index, op)| expansion_of(op, &repr).map(|e| (index, e)))
        else {
            continue;
        };
        let split = split(func, block, index);
        let join = match expansion {
            Expansion::IntArith { fast } => expand_int_arith(func, block, split, fast),
            Expansion::Truthiness => expand_truthiness(func, block, split),
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
#[derive(Clone, Copy)]
enum Expansion {
    /// Integer add or subtract; in-place forms share the binary fast arm.
    IntArith {
        fast: OpCode,
    },
    Truthiness,
}

fn is_boxed_word(repr: &HashMap<ValueId, Repr>, value: ValueId) -> bool {
    matches!(
        repr.get(&value),
        Some(Repr::DynBox | Repr::MaybeBigInt | Repr::BoxedImmortal)
    )
}

fn expansion_of(op: &TirOp, repr: &HashMap<ValueId, Repr>) -> Option<Expansion> {
    if !op.has_valid_shape()
        || op.results.len() != 1
        || !op
            .operands
            .iter()
            .all(|&operand| is_boxed_word(repr, operand))
    {
        return None;
    }
    let int_arith = |fast| {
        (op.operands.len() == 2
            && is_boxed_word(repr, op.results[0])
            && op.numeric_runtime().is_some())
        .then_some(Expansion::IntArith { fast })
    };
    match opcode_fast_path_expansion_rule_table(op.opcode) {
        FastPathExpansionRule::IntAdd => int_arith(OpCode::Add),
        FastPathExpansionRule::IntSub => int_arith(OpCode::Sub),
        FastPathExpansionRule::Truthiness => {
            (op.operands.len() == 1).then_some(Expansion::Truthiness)
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

fn expand_int_arith(func: &mut TirFunction, block: BlockId, split: Split, fast: OpCode) -> BlockId {
    let Split {
        op,
        rest,
        terminator,
    } = split;
    let [lhs, rhs] = op.operands[..] else {
        unreachable!("integer arithmetic is binary")
    };
    let result = op.results[0];
    let span = op.source_span;
    let runtime = op
        .numeric_runtime()
        .expect("eligible ops have a runtime entry");

    let check_rhs = func.fresh_block();
    let fast_block = func.fresh_block();
    let slow = func.fresh_block();
    let join = func.fresh_block();
    let lhs_is_int = fresh_typed(func, TirType::Bool);
    let rhs_is_int = fresh_typed(func, TirType::Bool);
    let lhs_payload = fresh_typed(func, TirType::I64);
    let rhs_payload = fresh_typed(func, TirType::I64);
    let sum = fresh_typed(func, TirType::I64);
    let fast_result = fresh_typed(func, TirType::DynBox);
    let slow_result = fresh_typed(func, TirType::DynBox);

    end_block(
        func,
        block,
        vec![new_op(
            OpCode::IsInlineInt,
            vec![lhs],
            vec![lhs_is_int],
            span,
        )],
        cond_branch(lhs_is_int, check_rhs, slow),
    );
    let check = vec![new_op(
        OpCode::IsInlineInt,
        vec![rhs],
        vec![rhs_is_int],
        span,
    )];
    func.blocks.insert(
        check_rhs,
        new_block(check_rhs, check, cond_branch(rhs_is_int, fast_block, slow)),
    );
    let fast_ops = vec![
        new_op(OpCode::UnboxInlineInt, vec![lhs], vec![lhs_payload], span),
        new_op(OpCode::UnboxInlineInt, vec![rhs], vec![rhs_payload], span),
        new_op(fast, vec![lhs_payload, rhs_payload], vec![sum], span),
        new_op(OpCode::BoxVal, vec![sum], vec![fast_result], span),
    ];
    func.blocks.insert(
        fast_block,
        new_block(fast_block, fast_ops, branch(join, fast_result)),
    );
    // The slow arm is the original op's semantics: it keeps its attributes
    // (exception source, location) and calls the op's typed runtime entry.
    let mut call = op;
    call.opcode = OpCode::CallRuntime;
    call.origin = None;
    call.runtime = Some(runtime);
    call.results = vec![slow_result];
    func.blocks
        .insert(slow, new_block(slow, vec![call], branch(join, slow_result)));
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

#[cfg(test)]
mod tests;
