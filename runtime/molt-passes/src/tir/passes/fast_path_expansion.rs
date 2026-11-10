//! The late lowering stage of TIR (HF-821): value-level fast paths written once,
//! as representation-level control flow, for every backend that consumes final
//! TIR. Backends then only select instructions; the NaN-box meaning of each
//! primitive comes from molt-codegen-abi's shared generators.
//!
//! First slice: integer `Add`, `Sub`, `InplaceAdd` and `InplaceSub` on two
//! boxed-word operands.
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
//! The original result id becomes the join's argument, so no use is renamed and
//! no existing value changes representation. An in-place op on two ints has no
//! dunder to try, so both forms share the fast arm; the slow arm calls the op's
//! own runtime entry (`TirOp::numeric_runtime`), which tries `__i<op>__` first.
//! A failed heap box leaves `None` with a pending exception, which the
//! `CheckException` that already follows the op observes in the join.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    FastPathExpansionRule, opcode_fast_path_expansion_rule_table,
};
use crate::tir::ops::{OpCode, TirOp};
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
        let Some(index) = func.blocks[&block]
            .ops
            .iter()
            .position(|op| is_eligible(op, &repr))
        else {
            continue;
        };
        let join = expand(func, block, index);
        stats.ops_added += 6;
        stats.values_changed += 1;
        pending.push(join);
    }
    stats
}

fn is_boxed_word(repr: &HashMap<ValueId, Repr>, value: ValueId) -> bool {
    matches!(
        repr.get(&value),
        Some(Repr::DynBox | Repr::MaybeBigInt | Repr::BoxedImmortal)
    )
}

/// The fast-arm opcode of an op the stage expands, from the generated
/// `fast_path_expansion_rules` table: the in-place forms share the binary arm.
fn fast_arm(opcode: OpCode) -> Option<OpCode> {
    match opcode_fast_path_expansion_rule_table(opcode) {
        FastPathExpansionRule::IntAdd => Some(OpCode::Add),
        FastPathExpansionRule::IntSub => Some(OpCode::Sub),
        FastPathExpansionRule::None => None,
    }
}

fn is_eligible(op: &TirOp, repr: &HashMap<ValueId, Repr>) -> bool {
    fast_arm(op.opcode).is_some()
        && op.has_valid_shape()
        && op.operands.len() == 2
        && op.results.len() == 1
        && op
            .operands
            .iter()
            .all(|&operand| is_boxed_word(repr, operand))
        && is_boxed_word(repr, op.results[0])
        && op.numeric_runtime().is_some()
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

/// Split `block` at its eligible op `index` into the diamond above. Returns the
/// join block, which holds the rest of the original block.
fn expand(func: &mut TirFunction, block: BlockId, index: usize) -> BlockId {
    let (op, rest, terminator) = {
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
        (op, rest, terminator)
    };
    let [lhs, rhs] = op.operands[..] else {
        unreachable!("eligible ops are binary")
    };
    let result = op.results[0];
    let result_type = func
        .value_types
        .get(&result)
        .cloned()
        .unwrap_or(TirType::DynBox);
    let span = op.source_span;
    let runtime = op
        .numeric_runtime()
        .expect("eligible ops have a runtime entry");
    let fast_opcode = fast_arm(op.opcode).expect("eligible ops have a fast arm");

    let check_rhs = func.fresh_block();
    let fast = func.fresh_block();
    let slow = func.fresh_block();
    let join = func.fresh_block();

    let lhs_is_int = fresh_typed(func, TirType::Bool);
    let rhs_is_int = fresh_typed(func, TirType::Bool);
    let lhs_payload = fresh_typed(func, TirType::I64);
    let rhs_payload = fresh_typed(func, TirType::I64);
    let sum = fresh_typed(func, TirType::I64);
    let fast_result = fresh_typed(func, TirType::DynBox);
    let slow_result = fresh_typed(func, TirType::DynBox);

    {
        let original = func.blocks.get_mut(&block).expect("pending block exists");
        original.ops.push(new_op(
            OpCode::IsInlineInt,
            vec![lhs],
            vec![lhs_is_int],
            span,
        ));
        original.terminator = Terminator::CondBranch {
            cond: lhs_is_int,
            then_block: check_rhs,
            then_args: Vec::new(),
            else_block: slow,
            else_args: Vec::new(),
        };
    }
    func.blocks.insert(
        check_rhs,
        TirBlock {
            id: check_rhs,
            args: Vec::new(),
            ops: vec![new_op(
                OpCode::IsInlineInt,
                vec![rhs],
                vec![rhs_is_int],
                span,
            )],
            terminator: Terminator::CondBranch {
                cond: rhs_is_int,
                then_block: fast,
                then_args: Vec::new(),
                else_block: slow,
                else_args: Vec::new(),
            },
        },
    );
    func.blocks.insert(
        fast,
        TirBlock {
            id: fast,
            args: Vec::new(),
            ops: vec![
                new_op(OpCode::UnboxInlineInt, vec![lhs], vec![lhs_payload], span),
                new_op(OpCode::UnboxInlineInt, vec![rhs], vec![rhs_payload], span),
                new_op(fast_opcode, vec![lhs_payload, rhs_payload], vec![sum], span),
                new_op(OpCode::BoxVal, vec![sum], vec![fast_result], span),
            ],
            terminator: Terminator::Branch {
                target: join,
                args: vec![fast_result],
            },
        },
    );
    // The slow arm is the original op's semantics: it keeps its attributes
    // (exception source, location) and calls the op's typed runtime entry.
    let mut call = op;
    call.opcode = OpCode::CallRuntime;
    call.origin = None;
    call.runtime = Some(runtime);
    call.results = vec![slow_result];
    func.blocks.insert(
        slow,
        TirBlock {
            id: slow,
            args: Vec::new(),
            ops: vec![call],
            terminator: Terminator::Branch {
                target: join,
                args: vec![slow_result],
            },
        },
    );
    func.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![TirValue {
                id: result,
                ty: result_type,
            }],
            ops: rest,
            terminator,
        },
    );
    // The original block's terminator now ends the join: a loop whose break
    // test lived there finds it in the join.
    for cond_block in func.loop_cond_blocks.values_mut() {
        if *cond_block == block {
            *cond_block = join;
        }
    }
    join
}

#[cfg(test)]
mod tests;
