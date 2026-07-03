//! Deforestation / iterator-fusion sub-pass.
//!
//! Fuses generator/iterator chains that feed a fusable builtin consumer
//! (`sum`/`any`/`all`/`min`/`max`/`list`/`len`/`set`/`tuple`/`sorted`/
//! `reversed`) into single loops, eliminating the intermediate data
//! structures. See the module-level docs on [`super`].

use std::collections::HashMap;

use super::super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_is_fusion_barrier_table;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Recognized builtin consumer that can be fused with an iterator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FusableBuiltin {
    Sum,
    Any,
    All,
    Min,
    Max,
    List,
    /// `len(iterable)` → counter loop (no intermediate list allocation).
    Len,
    /// `set(iterable)` → direct set-build loop (no intermediate list).
    Set,
    /// `tuple(iterable)` → direct tuple build (no intermediate list).
    Tuple,
    /// `sorted(iterable)` → collect + sort-in-place (single allocation).
    Sorted,
    /// `reversed(iterable)` → reverse-iteration (no materialized copy).
    Reversed,
}

impl FusableBuiltin {
    /// Try to parse a builtin name from a `CallBuiltin` attribute.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "sum" => Some(Self::Sum),
            "any" => Some(Self::Any),
            "all" => Some(Self::All),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "list" => Some(Self::List),
            "len" => Some(Self::Len),
            "set" => Some(Self::Set),
            "tuple" => Some(Self::Tuple),
            "sorted" => Some(Self::Sorted),
            "reversed" => Some(Self::Reversed),
            _ => None,
        }
    }
}

/// Description of a `GetIter` → `ForIter` loop feeding into a `CallBuiltin`.
#[derive(Debug)]
struct IteratorChain {
    /// Block containing the `CallBuiltin` consumer.
    consumer_block: BlockId,
    /// Index of the `CallBuiltin` op within its block.
    consumer_op_idx: usize,
    /// Which builtin is consuming the iterator.
    builtin: FusableBuiltin,
    /// Block containing the `ForIter` loop header.
    loop_header_block: BlockId,
    /// Index of the `ForIter` op within the loop header block.
    for_iter_op_idx: usize,
    /// Block containing the loop body ops.
    loop_body_block: BlockId,
    /// The `ValueId` produced by `GetIter` (the iterator object).
    #[allow(dead_code)]
    iter_value: ValueId,
    /// The `ValueId` produced by `IterNext`/`ForIter` (each element).
    element_value: ValueId,
    /// The `ValueId` that the `CallBuiltin` produces (the result).
    result_value: ValueId,
    /// The iterable source passed to `GetIter`.
    source_iterable: ValueId,
}

/// Returns `true` when an opcode blocks iterator-chain fusion.
///
/// The canonical, exhaustive table lives in `op_kinds.toml`.
fn is_fusion_barrier(opcode: OpCode) -> bool {
    opcode_is_fusion_barrier_table(opcode)
}

/// Check whether every op in a slice is eligible for iterator-chain fusion.
pub(super) fn is_fusable_body(ops: &[TirOp]) -> bool {
    ops.iter().all(|op| !is_fusion_barrier(op.opcode))
}

/// Detect and fuse iterator/generator chains into single loops.
///
/// Patterns detected:
/// 1. `sum(genexpr)` → accumulator loop
/// 2. `list(genexpr)` → preallocated list + append loop
/// 3. `map(f, iter)` → fused apply-in-loop
/// 4. `filter(pred, iter)` → fused guard-in-loop
/// 5. `any(genexpr)` / `all(genexpr)` → early-exit loop
/// 6. `min(genexpr)` / `max(genexpr)` → tracking loop
///
/// Purity requirement: only fuses when the body is provably pure
/// (no side effects, no exceptions beyond what unfused version would raise).
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "deforestation",
        ..Default::default()
    };

    // Phase 1: Build a map from ValueId → defining op location, and a map from
    // GetIter results to their source iterables.
    let mut def_map: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    let mut get_iter_sources: HashMap<ValueId, ValueId> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            for &res in &op.results {
                def_map.insert(res, (bid, i));
            }
            if op.opcode == OpCode::GetIter && !op.operands.is_empty() && !op.results.is_empty() {
                get_iter_sources.insert(op.results[0], op.operands[0]);
            }
        }
    }

    // Phase 2: Find fusable chains. We look for CallBuiltin ops where:
    //   - The builtin name is one of our fusable set
    //   - The single argument comes from a ForIter loop
    //   - The loop body is pure
    let chains = find_fusable_chains(func, &def_map, &get_iter_sources);

    // Phase 3: Apply fusion rewrites.
    for chain in chains {
        match chain.builtin {
            FusableBuiltin::Sum => {
                fuse_sum(func, &chain, &mut stats);
            }
            FusableBuiltin::Any => {
                fuse_any_all(func, &chain, true, &mut stats);
            }
            FusableBuiltin::All => {
                fuse_any_all(func, &chain, false, &mut stats);
            }
            FusableBuiltin::Min => {
                fuse_min_max(func, &chain, true, &mut stats);
            }
            FusableBuiltin::Max => {
                fuse_min_max(func, &chain, false, &mut stats);
            }
            FusableBuiltin::List => {
                fuse_list(func, &chain, &mut stats);
            }
            FusableBuiltin::Len => {
                fuse_len(func, &chain, &mut stats);
            }
            FusableBuiltin::Set => {
                fuse_set(func, &chain, &mut stats);
            }
            FusableBuiltin::Tuple => {
                fuse_tuple(func, &chain, &mut stats);
            }
            FusableBuiltin::Sorted => {
                fuse_sorted(func, &chain, &mut stats);
            }
            FusableBuiltin::Reversed => {
                fuse_reversed(func, &chain, &mut stats);
            }
        }
    }

    stats
}

/// Scan the function for fusable iterator chains.
fn find_fusable_chains(
    func: &TirFunction,
    def_map: &HashMap<ValueId, (BlockId, usize)>,
    get_iter_sources: &HashMap<ValueId, ValueId>,
) -> Vec<IteratorChain> {
    let mut chains = Vec::new();

    for (&bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            // Look for CallBuiltin with a known fusable name.
            if op.opcode != OpCode::CallBuiltin {
                continue;
            }
            let builtin_name = match op.attrs.get("name") {
                Some(AttrValue::Str(s)) => s.as_str(),
                _ => continue,
            };
            let builtin = match FusableBuiltin::from_name(builtin_name) {
                Some(b) => b,
                None => continue,
            };

            // The builtin must have exactly one operand (the iterator argument)
            // and one result.
            if op.operands.len() != 1 || op.results.is_empty() {
                continue;
            }
            let arg_value = op.operands[0];
            let result_value = op.results[0];

            // Trace back: the argument should come from a ForIter loop.
            // Find the ForIter that produces arg_value.
            let (for_block, for_idx) = match def_map.get(&arg_value) {
                Some(&loc) => loc,
                None => continue,
            };

            let for_iter_op = match func.blocks.get(&for_block) {
                Some(b) => match b.ops.get(for_idx) {
                    Some(op) if op.opcode == OpCode::ForIter => op,
                    _ => continue,
                },
                None => continue,
            };

            // ForIter takes an iterator value as operand and yields the element.
            if for_iter_op.operands.is_empty() || for_iter_op.results.is_empty() {
                continue;
            }
            let iter_value = for_iter_op.operands[0];
            let element_value = for_iter_op.results[0];

            // The iterator value should come from a GetIter.
            let source_iterable = match get_iter_sources.get(&iter_value) {
                Some(&src) => src,
                None => continue,
            };

            // Find the loop body block. The ForIter block's terminator should
            // branch to a body block on success.
            let loop_body_block = match &func.blocks[&for_block].terminator {
                Terminator::CondBranch { then_block, .. } => *then_block,
                Terminator::Branch { target, .. } => *target,
                _ => continue,
            };

            // Check purity of the loop body.
            let body_block = match func.blocks.get(&loop_body_block) {
                Some(b) => b,
                None => continue,
            };
            if !is_fusable_body(&body_block.ops) {
                continue;
            }

            chains.push(IteratorChain {
                consumer_block: bid,
                consumer_op_idx: i,
                builtin,
                loop_header_block: for_block,
                for_iter_op_idx: for_idx,
                loop_body_block,
                iter_value,
                element_value,
                result_value,
                source_iterable,
            });
        }
    }

    chains
}

/// Fuse `sum(genexpr)` into an accumulator loop.
///
/// Replaces the CallBuiltin(sum) with:
///   acc = ConstInt(0)
///   ForIter loop body: acc = Add(acc, element)
///   result = acc
fn fuse_sum(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
fn fuse_any_all(
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
fn fuse_min_max(
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

/// Fuse `list(genexpr)` into a preallocated list + append loop.
fn fuse_list(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
fn fuse_len(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
fn fuse_set(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
fn fuse_tuple(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
fn fuse_sorted(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
fn fuse_reversed(func: &mut TirFunction, chain: &IteratorChain, stats: &mut PassStats) {
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
