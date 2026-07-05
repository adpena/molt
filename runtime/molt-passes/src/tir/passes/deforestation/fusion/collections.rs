use super::super::super::PassStats;
use super::model::IteratorChain;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};

/// Fuse `list(genexpr)` into a preallocated list + append loop.
pub(super) fn fuse_list(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    let list_val = func.fresh_value();

    // BuildList creates the empty list.
    let build_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildList,
        operands: vec![],
        results: vec![list_val],
        attrs: AttrDict::new(),
        source_span: None,
    };

    // StoreIndex appends element to list in the loop body.
    let store_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreIndex,
        operands: vec![list_val, chain.element_value],
        results: vec![],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("list_append".into()));
            m
        },
        source_span: None,
    };

    // Replace the CallBuiltin with a Copy from the list.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_val],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("list".into()));
            m
        },
        source_span: None,
    };

    // Apply.
    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, build_op);
    }
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(store_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 2;
}

/// Fuse `len(iterable)` into a counter loop — no intermediate list allocation.
///
/// Replaces `len(CallBuiltin)` with:
///   counter = ConstInt(0)
///   ForIter loop body: counter = Add(counter, ConstInt(1))
///   result = counter
///
/// This eliminates the entire intermediate list that `len([x for x in data])`
/// would otherwise allocate just to count its elements.
pub(super) fn fuse_len(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    let counter_init = func.fresh_value();
    let one_val = func.fresh_value();
    let counter_updated = func.fresh_value();

    // ConstInt(0) as the counter initializer.
    let init_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![counter_init],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(0));
            m
        },
        source_span: None,
    };

    // ConstInt(1) for the increment.
    let one_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![one_val],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(1));
            m
        },
        source_span: None,
    };

    // Add(counter, 1) in the loop body.
    let add_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![counter_init, one_val],
        results: vec![counter_updated],
        attrs: AttrDict::new(),
        source_span: None,
    };

    // Replace CallBuiltin with Copy from counter.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![counter_updated],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("len".into()));
            m
        },
        source_span: None,
    };

    // Apply mutations.
    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, init_op);
    }
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(one_op);
        body.ops.push(add_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 3; // init + one + add
}

/// Fuse `set(iterable)` into a direct set-build loop.
///
/// Replaces the CallBuiltin(set) with:
///   s = BuildSet()
///   ForIter loop body: StoreIndex(s, element) [set.add semantics]
///   result = s
pub(super) fn fuse_set(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    let set_val = func.fresh_value();

    let build_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSet,
        operands: vec![],
        results: vec![set_val],
        attrs: AttrDict::new(),
        source_span: None,
    };

    // StoreIndex adds element to set in the loop body.
    let store_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreIndex,
        operands: vec![set_val, chain.element_value],
        results: vec![],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("set_add".into()));
            m
        },
        source_span: None,
    };

    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![set_val],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("set".into()));
            m
        },
        source_span: None,
    };

    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, build_op);
    }
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(store_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 2;
}

/// Fuse `tuple(iterable)` into a direct tuple build.
///
/// Replaces CallBuiltin(tuple) with:
///   tmp_list = BuildList()
///   ForIter loop body: StoreIndex(tmp_list, element)
///   result = BuildTuple from tmp_list [tagged for backend conversion]
///
/// The backend recognizes the "fused=tuple" tag and emits a list→tuple
/// conversion after the loop, avoiding double allocation.
pub(super) fn fuse_tuple(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    let list_val = func.fresh_value();

    let build_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildList,
        operands: vec![],
        results: vec![list_val],
        attrs: AttrDict::new(),
        source_span: None,
    };

    let store_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreIndex,
        operands: vec![list_val, chain.element_value],
        results: vec![],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("tuple_append".into()));
            m
        },
        source_span: None,
    };

    // The result is a Copy with fused=tuple tag — the backend converts
    // the accumulated list to a tuple.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_val],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("tuple".into()));
            m
        },
        source_span: None,
    };

    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, build_op);
    }
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(store_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 2;
}

/// Fuse `sorted(iterable)` into a single collect + sort-in-place.
///
/// Instead of: list(iterable) → sorted(list) [two allocations],
/// emit: collect into list → sort list in-place → result = list.
pub(super) fn fuse_sorted(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    let list_val = func.fresh_value();

    let build_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildList,
        operands: vec![],
        results: vec![list_val],
        attrs: AttrDict::new(),
        source_span: None,
    };

    let store_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreIndex,
        operands: vec![list_val, chain.element_value],
        results: vec![],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("sorted_append".into()));
            m
        },
        source_span: None,
    };

    // Copy with fused=sorted — the backend calls sort-in-place on the
    // list after the loop, returning the sorted list directly.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_val],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("sorted".into()));
            m
        },
        source_span: None,
    };

    if let Some(header) = func.blocks.get_mut(&chain.loop_header_block) {
        header.ops.insert(chain.for_iter_op_idx, build_op);
    }
    if let Some(body) = func.blocks.get_mut(&chain.loop_body_block) {
        body.ops.push(store_op);
    }
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
    stats.ops_added += 2;
}

/// Fuse `reversed(iterable)` into reverse-order iteration.
///
/// Tags the iteration chain so the backend emits a reverse-index loop
/// instead of materializing an intermediate reversed copy.
pub(super) fn fuse_reversed(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
    // Replace CallBuiltin(reversed) with a tagged Copy that tells the
    // backend to reverse the iteration direction on the source iterable.
    let copy_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![chain.source_iterable],
        results: vec![chain.result_value],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("fused".into(), AttrValue::Str("reversed".into()));
            m.insert(
                "source".into(),
                AttrValue::Int(chain.source_iterable.0 as i64),
            );
            m
        },
        source_span: None,
    };

    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block)
        && chain.consumer_op_idx < consumer.ops.len()
    {
        consumer.ops[chain.consumer_op_idx] = copy_op;
    }

    stats.values_changed += 1;
}
