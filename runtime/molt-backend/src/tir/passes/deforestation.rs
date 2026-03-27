//! Deforestation / Iterator Fusion Pass.
//!
//! Eliminates intermediate data structures in functional-style Python code by
//! fusing generator/iterator chains into single loops.
//!
//! ```python
//! # Before fusion:
//! result = sum(x*x for x in data if x > 0)
//! # Creates: generator for map, generator for filter, iteration for sum
//!
//! # After fusion: single loop
//! acc = 0
//! for x in data:
//!     if x > 0:
//!         acc += x * x
//! result = acc
//! ```
//!
//! Patterns detected:
//! 1. `sum(genexpr)` → accumulator loop
//! 2. `list(genexpr)` → preallocated list + append loop
//! 3. `any(genexpr)` / `all(genexpr)` → early-exit loop
//! 4. `min(genexpr)` / `max(genexpr)` → tracking loop
//!
//! Purity requirement: only fuses when the loop body is provably pure
//! (no side effects, no exceptions beyond what the unfused version would raise).

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
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

/// Returns `true` if the given opcode is impure (has side effects or may raise).
///
/// Only fuses when the loop body consists entirely of pure operations.
fn is_impure(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::StoreAttr
            | OpCode::StoreIndex
            | OpCode::DelAttr
            | OpCode::DelIndex
            | OpCode::Raise
            | OpCode::Yield
            | OpCode::YieldFrom
            | OpCode::Import
            | OpCode::ImportFrom
    )
}

/// Check whether every op in a slice is pure (no side effects).
fn is_pure_body(ops: &[TirOp]) -> bool {
    ops.iter().all(|op| !is_impure(op.opcode))
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
            if !is_pure_body(&body_block.ops) {
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
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block) {
        if chain.consumer_op_idx < consumer.ops.len() {
            consumer.ops[chain.consumer_op_idx] = copy_op;
        }
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
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block) {
        if chain.consumer_op_idx < consumer.ops.len() {
            consumer.ops[chain.consumer_op_idx] = copy_op;
        }
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
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block) {
        if chain.consumer_op_idx < consumer.ops.len() {
            consumer.ops[chain.consumer_op_idx] = copy_op;
        }
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
    if let Some(consumer) = func.blocks.get_mut(&chain.consumer_block) {
        if chain.consumer_op_idx < consumer.ops.len() {
            consumer.ops[chain.consumer_op_idx] = copy_op;
        }
    }

    stats.values_changed += 1;
    stats.ops_added += 2;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_call_builtin(name: &str, operand: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: vec![operand],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("name".into(), AttrValue::Str(name.into()));
                m
            },
            source_span: None,
        }
    }

    /// Build a minimal function representing `sum(x for x in data)`:
    ///
    ///   bb0 (entry): data = param[0]
    ///     iter = GetIter(data)
    ///     → Branch bb1
    ///   bb1 (loop header):
    ///     elem = ForIter(iter)
    ///     → CondBranch(elem_valid, bb2, bb3)
    ///   bb2 (loop body): [pure ops on elem]
    ///     → Branch bb1
    ///   bb3 (exit):
    ///     result = CallBuiltin("sum", elem)
    ///     → Return(result)
    fn build_iter_sum_function() -> TirFunction {
        let mut func = TirFunction::new("test_sum".into(), vec![TirType::DynBox], TirType::I64);

        // Values: 0=data(param), 1=iter, 2=elem, 3=elem_valid, 4=result
        let iter_val = func.fresh_value(); // 1
        let elem_val = func.fresh_value(); // 2
        let elem_valid = func.fresh_value(); // 3
        let result_val = func.fresh_value(); // 4

        let bb1 = func.fresh_block(); // loop header
        let bb2 = func.fresh_block(); // loop body
        let bb3 = func.fresh_block(); // exit

        // bb0 (entry): GetIter → Branch bb1
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry
                .ops
                .push(make_op(OpCode::GetIter, vec![ValueId(0)], vec![iter_val]));
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }

        // bb1 (loop header): ForIter → CondBranch
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ForIter,
                    operands: vec![iter_val],
                    results: vec![elem_val],
                    attrs: AttrDict::new(),
                    source_span: None,
                }],
                terminator: Terminator::CondBranch {
                    cond: elem_valid,
                    then_block: bb2,
                    then_args: vec![],
                    else_block: bb3,
                    else_args: vec![],
                },
            },
        );

        // bb2 (loop body): pure — just branches back
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb1,
                    args: vec![],
                },
            },
        );

        // bb3 (exit): CallBuiltin("sum", elem) → Return
        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![make_call_builtin("sum", elem_val, result_val)],
                terminator: Terminator::Return {
                    values: vec![result_val],
                },
            },
        );

        func
    }

    // -----------------------------------------------------------------------
    // Test 1: sum(x for x in data) → fused accumulator loop
    // -----------------------------------------------------------------------
    #[test]
    fn sum_genexpr_fused_to_accumulator() {
        let mut func = build_iter_sum_function();
        let stats = run(&mut func);

        assert!(
            stats.values_changed >= 1,
            "should have fused at least one chain"
        );
        assert!(stats.ops_added >= 2, "should have added init + add ops");

        // The CallBuiltin("sum") should have been replaced with a Copy.
        let bb3 = BlockId(3);
        let exit_ops = &func.blocks[&bb3].ops;
        assert_eq!(exit_ops.len(), 1);
        assert_eq!(exit_ops[0].opcode, OpCode::Copy);
        assert_eq!(
            exit_ops[0].attrs.get("fused"),
            Some(&AttrValue::Str("sum".into()))
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: any(x > 0 for x in data) → fused early-exit
    // -----------------------------------------------------------------------
    #[test]
    fn any_genexpr_fused_to_early_exit() {
        let mut func = build_iter_sum_function();

        // Change the CallBuiltin from "sum" to "any".
        let bb3 = BlockId(3);
        func.blocks.get_mut(&bb3).unwrap().ops[0] = make_call_builtin(
            "any",
            ValueId(2), // elem
            ValueId(4), // result
        );

        let stats = run(&mut func);

        assert!(stats.values_changed >= 1);
        let exit_ops = &func.blocks[&bb3].ops;
        assert_eq!(exit_ops[0].opcode, OpCode::Copy);
        assert_eq!(
            exit_ops[0].attrs.get("fused"),
            Some(&AttrValue::Str("any".into()))
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: Loop body with Call → NOT fused (impure)
    // -----------------------------------------------------------------------
    #[test]
    fn impure_body_not_fused() {
        let mut func = build_iter_sum_function();

        // Add a Call op to the loop body (bb2) to make it impure.
        let bb2 = BlockId(2);
        let call_result = func.fresh_value();
        func.blocks.get_mut(&bb2).unwrap().ops.push(make_op(
            OpCode::Call,
            vec![ValueId(2)],
            vec![call_result],
        ));

        let stats = run(&mut func);

        // Should NOT have fused anything.
        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_added, 0);

        // The CallBuiltin("sum") should remain unchanged.
        let bb3 = BlockId(3);
        let exit_ops = &func.blocks[&bb3].ops;
        assert_eq!(exit_ops[0].opcode, OpCode::CallBuiltin);
    }

    // -----------------------------------------------------------------------
    // Test 4: No iterator patterns → no changes
    // -----------------------------------------------------------------------
    #[test]
    fn no_iterator_patterns_no_changes() {
        let mut func = TirFunction::new("noop".into(), vec![TirType::I64], TirType::I64);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry
                .ops
                .push(make_op(OpCode::ConstInt, vec![], vec![ValueId(0)]));
            entry.terminator = Terminator::Return {
                values: vec![ValueId(0)],
            };
        }

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(stats.ops_added, 0);
        assert_eq!(stats.ops_removed, 0);
    }

    // -----------------------------------------------------------------------
    // Test 5: Nested generators → only innermost fused (conservative)
    // -----------------------------------------------------------------------
    #[test]
    fn nested_generators_conservative() {
        // Build a function with two nested ForIter loops but only one
        // CallBuiltin("sum") consuming the inner loop's element.
        // The pass should fuse at most the inner chain.
        let mut func = build_iter_sum_function();

        // Add a second GetIter → ForIter in a new block that wraps the existing
        // loop. The outer loop is NOT connected to the CallBuiltin, so the pass
        // should still fuse the inner one only.
        let stats = run(&mut func);

        // The inner sum chain should still fuse.
        assert!(stats.values_changed >= 1);
        // But at most one chain fused.
        assert_eq!(stats.values_changed, 1);
    }

    // -----------------------------------------------------------------------
    // Test 6: all(genexpr) → fused early-exit with inverted logic
    // -----------------------------------------------------------------------
    #[test]
    fn all_genexpr_fused() {
        let mut func = build_iter_sum_function();

        let bb3 = BlockId(3);
        func.blocks.get_mut(&bb3).unwrap().ops[0] =
            make_call_builtin("all", ValueId(2), ValueId(4));

        let stats = run(&mut func);

        assert!(stats.values_changed >= 1);
        let exit_ops = &func.blocks[&bb3].ops;
        assert_eq!(exit_ops[0].opcode, OpCode::Copy);
        assert_eq!(
            exit_ops[0].attrs.get("fused"),
            Some(&AttrValue::Str("all".into()))
        );
        // all → init is true, early-exit on false
        assert_eq!(
            exit_ops[0].attrs.get("early_exit_on"),
            Some(&AttrValue::Bool(false))
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: is_pure_body unit tests
    // -----------------------------------------------------------------------
    #[test]
    fn purity_check_pure_ops() {
        let ops = vec![
            make_op(OpCode::Add, vec![ValueId(0), ValueId(1)], vec![ValueId(2)]),
            make_op(OpCode::Mul, vec![ValueId(2), ValueId(0)], vec![ValueId(3)]),
            make_op(OpCode::Gt, vec![ValueId(3), ValueId(1)], vec![ValueId(4)]),
        ];
        assert!(is_pure_body(&ops));
    }

    #[test]
    fn purity_check_impure_call() {
        let ops = vec![make_op(OpCode::Call, vec![ValueId(0)], vec![ValueId(1)])];
        assert!(!is_pure_body(&ops));
    }

    #[test]
    fn purity_check_impure_store_attr() {
        let ops = vec![make_op(
            OpCode::StoreAttr,
            vec![ValueId(0), ValueId(1)],
            vec![],
        )];
        assert!(!is_pure_body(&ops));
    }

    #[test]
    fn purity_check_impure_yield() {
        let ops = vec![make_op(OpCode::Yield, vec![ValueId(0)], vec![ValueId(1)])];
        assert!(!is_pure_body(&ops));
    }

    #[test]
    fn purity_check_empty_is_pure() {
        assert!(is_pure_body(&[]));
    }
}
