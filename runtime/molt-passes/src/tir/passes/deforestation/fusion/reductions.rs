use super::super::super::PassStats;
use super::model::IteratorChain;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};

/// Fuse `sum(genexpr)` into an accumulator loop.
///
/// Replaces the CallBuiltin(sum) with:
///   acc = ConstInt(0)
///   ForIter loop body: acc = Add(acc, element)
///   result = acc
pub(super) fn fuse_sum(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    let acc_init = func.fresh_value();
    let acc_updated = func.fresh_value();

    // Insert ConstInt(0) as the accumulator initializer before the loop.
    let init_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![acc_init],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(0));
            m
        },
        source_span: None,
    };

    // Insert Add(acc, element) in the loop body.
    let add_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![acc_init, chain.element_value],
        results: vec![acc_updated],
        attrs: AttrDict::new(),
        source_span: None,
    };

    // Replace the CallBuiltin with a Copy from the accumulator.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![acc_updated],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("sum".into()));
            m
        },
        source_span: None,
    };

    // Apply mutations.
    // 1. Insert init op before the ForIter in the header block.
    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, init_op);
    }

    // 2. Insert accumulator update in the loop body.
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(add_op);
    }

    // 3. Replace the CallBuiltin in the consumer block with the Copy.
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 2; // init + add
}

/// Fuse `any(genexpr)` or `all(genexpr)` into an early-exit loop.
///
/// For `any`: init=false, body: if element { result = true; break }
/// For `all`: init=true,  body: if !element { result = false; break }
pub(super) fn fuse_any_all(
    func: &mut TirFunction,
    chain: &IteratorChain,
    is_any: bool,
    stats: &mut PassStats,
) {
    let init_val = func.fresh_value();
    let tag = if is_any { "any" } else { "all" };
    let init_bool = !is_any; // any→false, all→true

    // ConstBool for the initializer.
    let init_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBool,
        operands: vec![],
        results: vec![init_val],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Bool(init_bool));
            m
        },
        source_span: None,
    };

    // Replace the CallBuiltin with a Copy from the init value.
    // The actual early-exit semantics are expressed by tagging the op;
    // the backend codegen will read the "fused" attr.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![init_val],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str(tag.into()));
            m.insert(
                "early_exit_on".into(),
                AttrValue::Bool(is_any), // any: exit on true; all: exit on false
            );
            m.insert(
                "element".into(),
                AttrValue::Int(chain.element_value.0 as i64),
            );
            m.insert(
                "source".into(),
                AttrValue::Int(chain.source_iterable.0 as i64),
            );
            m
        },
        source_span: None,
    };

    // Apply.
    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, init_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 1;
}

/// Fuse `min(genexpr)` or `max(genexpr)` into a tracking loop.
pub(super) fn fuse_min_max(
    func: &mut TirFunction,
    chain: &IteratorChain,
    is_min: bool,
    stats: &mut PassStats,
) {
    let tag = if is_min { "min" } else { "max" };
    let cmp_opcode = if is_min { OpCode::Lt } else { OpCode::Gt };

    let tracker = func.fresh_value();
    let cmp_result = func.fresh_value();

    // The tracker is initialized to the first element via Copy.
    let init_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![chain.element_value],
        results: vec![tracker],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str(format!("{tag}_init")));
            m
        },
        source_span: None,
    };

    // Compare current element with tracker.
    let cmp_op = TirOp {
        dialect: Dialect::Molt,
        opcode: cmp_opcode,
        operands: vec![chain.element_value, tracker],
        results: vec![cmp_result],
        attrs: AttrDict::new(),
        source_span: None,
    };

    // Replace the CallBuiltin with a Copy from the tracker.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![tracker],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str(tag.into()));
            m
        },
        source_span: None,
    };

    // Apply.
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(init_op);
        body.ops.push(cmp_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 2;
}
