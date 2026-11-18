//! A fused-loop reduction kernel over a generator's iterator declines.
//!
//! The frontend puts a chunk loop before `for x in it: acc += x` (and the
//! `*=`, min and max forms): each pass calls a reduction kernel (`vec_sum`,
//! `vec_prod`, `vec_min`, `vec_max`) with the loop's iterator and reads its
//! `(result, last, count, more)` outcome. The kernel admits only the runtime's
//! own list, tuple and range iterator, and declines every other object before
//! any effect with `(None, None, 0, False)`
//! (`molt-runtime` `vec_reduction_chunk`). `iter(g)` of a generator is the
//! generator, so the outcome over a generator this function allocates is that
//! constant. Folding it here removes the kernel's use of the iterator, which
//! lets the frame elision see the loop's `IterNext` as the iterator's one
//! consumer; SCCP then deletes the chunk loop.

use molt_ir::OpKind;

use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::clone::{const_int_op, const_none_op};
use super::recognize::terminator_uses;
use super::{attr_original_kind, attr_task_kind, attr_value_int, is_get_iter_op};

fn is_reduction_kernel(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && matches!(
            attr_original_kind(op),
            Some(OpKind::VecSum | OpKind::VecProd | OpKind::VecMin | OpKind::VecMax)
        )
}

fn const_bool_op(result: ValueId, value: bool) -> TirOp {
    let mut attrs = AttrDict::default();
    attrs.insert("value".into(), AttrValue::Bool(value));
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBool,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

/// The integer a `ConstInt` defines for `value`, if one does.
fn const_int_of(func: &TirFunction, value: ValueId) -> Option<i64> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::ConstInt && op.results.first() == Some(&value))
        .and_then(attr_value_int)
}

/// Every use of `outcome` is `Index(outcome, k)` with a constant `k` in
/// `0..4`: the outcome tuple never escapes, so its fields are its meaning.
fn only_field_reads(func: &TirFunction, outcome: ValueId) -> bool {
    for block in func.blocks.values() {
        for op in &block.ops {
            if !op.operands.contains(&outcome) {
                continue;
            }
            let field = (op.opcode == OpCode::Index && op.operands.first() == Some(&outcome))
                .then(|| op.operands.get(1).and_then(|&k| const_int_of(func, k)))
                .flatten();
            if !matches!(field, Some(0..=3)) || op.operands[1..].contains(&outcome) {
                return false;
            }
        }
        if terminator_uses(&block.terminator, outcome) {
            return false;
        }
    }
    true
}

/// Fold each reduction kernel over the iterator of a generator `func`
/// allocates to the kernel's declined outcome. Returns whether `func` changed.
pub(super) fn decline_reduction_kernels(func: &mut TirFunction) -> bool {
    // The iterators of this function's own generators.
    let frames: Vec<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::AllocTask && attr_task_kind(op) == Some("generator"))
        .filter_map(|op| op.results.first().copied())
        .collect();
    if frames.is_empty() {
        return false;
    }
    let iterators: Vec<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| is_get_iter_op(op) && op.operands.first().is_some_and(|v| frames.contains(v)))
        .filter_map(|op| op.results.first().copied())
        .collect();
    let outcomes: Vec<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| {
            is_reduction_kernel(op) && op.operands.first().is_some_and(|v| iterators.contains(v))
        })
        .filter_map(|op| op.results.first().copied())
        .filter(|&outcome| only_field_reads(func, outcome))
        .collect();
    if outcomes.is_empty() {
        return false;
    }

    // Each field read becomes the declined outcome's field.
    let mut folds: Vec<(ValueId, i64)> = Vec::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Index
                && op.operands.first().is_some_and(|v| outcomes.contains(v))
                && let Some(&result) = op.results.first()
                && let Some(field) = op.operands.get(1).and_then(|&k| const_int_of(func, k))
            {
                folds.push((result, field));
            }
        }
    }
    for block in func.blocks.values_mut() {
        for op in block.ops.iter_mut() {
            let Some(&result) = op.results.first() else {
                continue;
            };
            let Some(&(_, field)) = folds.iter().find(|(value, _)| *value == result) else {
                continue;
            };
            let mut folded = match field {
                0 | 1 => const_none_op(result),
                2 => const_int_op(result, 0),
                _ => const_bool_op(result, false),
            };
            folded.inherit_source_from(op);
            *op = folded;
        }
        block.ops.retain(|op| {
            !(is_reduction_kernel(op) && op.results.first().is_some_and(|v| outcomes.contains(v)))
        });
    }
    for (result, field) in folds {
        let ty = match field {
            0 | 1 => TirType::None,
            2 => TirType::I64,
            _ => TirType::Bool,
        };
        func.value_types.insert(result, ty);
    }
    for outcome in outcomes {
        func.value_types.remove(&outcome);
    }
    true
}
