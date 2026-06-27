//! Bounds Check Elimination (BCE) pass for TIR — Tier-0 substrate **S6** consumer.
//!
//! Annotates `Index` / `StoreIndex` operations that are provably
//! bounds-check-safe by adding a `"bce_safe"` attribute (`AttrValue::Bool(true)`).
//! Backend codegen tests for this attribute and emits a straight-line element
//! access with **no bounds check** (see `native_backend::function_compiler` —
//! the `bce_safe` fast paths). A false `bce_safe` is therefore a *silent
//! out-of-bounds memory access*, not a panic, so the proof obligation is
//! absolute.
//!
//! ## Sole proof source: the value-range analysis (S6)
//!
//! All range/length reasoning lives in the [`ValueRange`](super::value_range)
//! analysis (built on [`ScalarEvolution`](super::scev)). This pass is a thin
//! consumer: for each indexing op it asks
//! [`ValueRangeResult::proves_index_in_bounds`] (numeric `0 <= i < len`) and
//! [`ValueRangeResult::proves_index_lt_len_symbolically`] (the
//! `while i < len(c): c[i]` shape where the length is a non-constant SSA value).
//! Both queries are **conservative over-approximations**: they return `true`
//! only when safety is *proven*, and `false` on any uncertainty.
//!
//! This replaces the former in-pass `RangeFact` / `GuardFact` / `KnownLength` /
//! `AddConst` lattices (deleted): range facts, induction-variable ranges,
//! container lengths and guard narrowing are now the value-range analysis's
//! single responsibility, shared with every other range consumer.
//!
//! The loop structure still comes from the S1 [`LoopForest`] analysis (used
//! transitively inside the value-range computation), so this pass — like LICM —
//! reasons over the one sound natural-loop definition (structurally hardening
//! the old ad-hoc loop-body scan, gap-analysis item C1).

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};

use super::PassStats;
use super::value_range::ValueRange;

/// Bounds Check Elimination pass.
///
/// Marks `Index` / `StoreIndex` ops whose index is provably in
/// `[0, len(container))` via the value-range analysis. Returns [`PassStats`]
/// counting how many ops were annotated (`values_changed`).
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "bce",
        ..Default::default()
    };

    // The value-range analysis owns all range/length reasoning (constants,
    // induction-variable ranges from SCEV, container lengths, and edge-sensitive
    // guard narrowing). Clone it so we can take `&mut func.blocks` below — the
    // analysis is a pure function of the (here unchanged) function.
    let vr = am.get::<ValueRange>(func).clone();

    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

    for bid in &block_ids {
        let Some(block) = func.blocks.get_mut(bid) else {
            continue;
        };
        for op in block.ops.iter_mut() {
            if op.opcode != OpCode::Index && op.opcode != OpCode::StoreIndex {
                continue;
            }
            // Idempotent: never re-mark (and never *un*-mark) — a previously
            // proven-safe op stays safe.
            if op.attrs.contains_key("bce_safe") {
                continue;
            }
            let Some(&container) = op.operands.first() else {
                continue;
            };
            let Some(&index) = op.operands.get(1) else {
                continue;
            };

            // BCE-only conservative proof: carrier facts are deliberately
            // excluded so a full-range raw int can never elide bounds checks.
            let proven = vr.proves_index_in_bounds_conservatively(*bid, container, index);

            if proven {
                op.attrs
                    .insert("bce_safe".to_string(), AttrValue::Bool(true));
                stats.values_changed += 1;
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::analysis::AnalysisManager;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

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

    fn make_nsw_add(operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut o = make_op(OpCode::Add, operands, results);
        o.attrs
            .insert("no_signed_wrap".into(), AttrValue::Bool(true));
        o
    }

    fn make_const_int(result: ValueId, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    fn make_call_builtin(name: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str(name.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    /// Build a list of length `len` via `BuildList` with `len` const-0 elements.
    fn build_list_of_len(func: &mut TirFunction, len: usize) -> (Vec<TirOp>, ValueId) {
        let mut ops = Vec::new();
        let mut elems = Vec::new();
        for _ in 0..len {
            let e = func.fresh_value();
            ops.push(make_const_int(e, 0));
            elems.push(e);
        }
        let container = func.fresh_value();
        ops.push(make_op(OpCode::BuildList, elems, vec![container]));
        (ops, container)
    }

    fn run_bce(func: &mut TirFunction) -> PassStats {
        run(func, &mut AnalysisManager::new())
    }

    // ------------------------------------------------------------------
    // Test 1: constant index 0 into a non-empty list → marked bce_safe.
    //
    // (The container length must EXCEED the index — a const index into an empty
    // list is genuinely out of bounds; the value-range proof correctly refuses
    // it. The old pass marked any non-negative const regardless of length,
    // which was unsound — a false bce_safe is a silent OOB access.)
    // ------------------------------------------------------------------
    #[test]
    fn constant_zero_index_into_nonempty_marked_safe() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let (mut ops, container) = build_list_of_len(&mut func, 1);
        let idx = func.fresh_value();
        let result = func.fresh_value();
        ops.push(make_const_int(idx, 0));
        ops.push(make_op(OpCode::Index, vec![container, idx], vec![result]));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run_bce(&mut func);
        assert_eq!(stats.values_changed, 1);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "const 0 index into a length-1 list must be bce_safe"
        );
    }

    // ------------------------------------------------------------------
    // Test 2: positive constant index within bounds → marked bce_safe.
    // ------------------------------------------------------------------
    #[test]
    fn positive_constant_index_in_bounds_marked_safe() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        // length 50 list, index 42 → in bounds.
        let (mut ops, container) = build_list_of_len(&mut func, 50);
        let idx = func.fresh_value();
        let result = func.fresh_value();
        ops.push(make_const_int(idx, 42));
        ops.push(make_op(OpCode::Index, vec![container, idx], vec![result]));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run_bce(&mut func);
        assert_eq!(stats.values_changed, 1);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert_eq!(index_op.attrs.get("bce_safe"), Some(&AttrValue::Bool(true)));
    }

    // ------------------------------------------------------------------
    // Test 2b: constant index OUT of bounds (>= len) → NOT marked.
    //
    // This is the soundness improvement over the old pass, which marked it.
    // ------------------------------------------------------------------
    #[test]
    fn constant_index_out_of_bounds_not_marked() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        // length 3 list, index 5 → OUT of bounds.
        let (mut ops, container) = build_list_of_len(&mut func, 3);
        let idx = func.fresh_value();
        let result = func.fresh_value();
        ops.push(make_const_int(idx, 5));
        ops.push(make_op(OpCode::Index, vec![container, idx], vec![result]));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run_bce(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "OOB const index must NOT be marked"
        );
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "an out-of-bounds const index must keep its runtime bounds check"
        );
    }

    // ------------------------------------------------------------------
    // Test 3: negative constant index → NOT marked (Python wraparound).
    // ------------------------------------------------------------------
    #[test]
    fn negative_constant_index_not_marked() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let (mut ops, container) = build_list_of_len(&mut func, 5);
        let idx = func.fresh_value();
        let result = func.fresh_value();
        ops.push(make_const_int(idx, -1));
        ops.push(make_op(OpCode::Index, vec![container, idx], vec![result]));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run_bce(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "Negative constant must not be marked bce_safe"
        );
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(!index_op.attrs.contains_key("bce_safe"));
    }

    // ------------------------------------------------------------------
    // Test 4: non-constant index → NOT marked.
    // ------------------------------------------------------------------
    #[test]
    fn non_constant_index_not_marked() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::None);
        let (mut ops, container) = build_list_of_len(&mut func, 5);
        let result = func.fresh_value();
        let param_idx = ValueId(0); // function parameter
        ops.push(make_op(
            OpCode::Index,
            vec![container, param_idx],
            vec![result],
        ));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run_bce(&mut func);
        assert_eq!(stats.values_changed, 0);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(!index_op.attrs.contains_key("bce_safe"));
    }

    // ------------------------------------------------------------------
    // Test 5: no Index ops → no changes, no panic.
    // ------------------------------------------------------------------
    #[test]
    fn no_index_ops_no_changes() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return { values: vec![] };
        let stats = run_bce(&mut func);
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 6: mixed ops — only the in-bounds const index is marked.
    // ------------------------------------------------------------------
    #[test]
    fn mixed_indices_only_safe_ones_marked() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::None);
        let (mut ops, container) = build_list_of_len(&mut func, 10); // len 10
        let const_idx = func.fresh_value(); // ConstInt(5) → in bounds
        let neg_idx = func.fresh_value(); // ConstInt(-2) → unsafe
        let param_idx = ValueId(0); // parameter → unsafe
        let r0 = func.fresh_value();
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();

        ops.push(make_const_int(const_idx, 5));
        ops.push(make_const_int(neg_idx, -2));
        ops.push(make_op(OpCode::Index, vec![container, const_idx], vec![r0]));
        ops.push(make_op(OpCode::Index, vec![container, neg_idx], vec![r1]));
        ops.push(make_op(OpCode::Index, vec![container, param_idx], vec![r2]));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![r0, r1, r2],
        };

        let stats = run_bce(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "Only the in-bounds const index should be marked bce_safe"
        );

        let index_ops: Vec<_> = func.blocks[&func.entry_block]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::Index)
            .collect();
        assert_eq!(index_ops.len(), 3);
        assert_eq!(
            index_ops[0].attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true))
        );
        assert!(!index_ops[1].attrs.contains_key("bce_safe"));
        assert!(!index_ops[2].attrs.contains_key("bce_safe"));
    }

    // ==================================================================
    // Inductive range analysis tests (range loop)
    // ==================================================================

    /// Build the post-range_devirt shape for `for i in range(range_bound)` with
    /// a `list_len`-element list and an `a[i]` access in the body. The IV is a
    /// header block-argument with a no-signed-wrap back-edge increment, which
    /// the SCEV analysis recognizes as an `AddRec {0, +, 1}`.
    fn build_range_loop_func(
        list_len: usize,
        range_bound: i64,
        use_negative_index: bool,
    ) -> (TirFunction, BlockId, BlockId, BlockId) {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);

        // Entry: build the list, set up start/stop/step constants, br header(start).
        let (mut entry_ops, container) = build_list_of_len(&mut func, list_len);
        let start = func.fresh_value();
        let stop = func.fresh_value();
        let step = func.fresh_value();
        entry_ops.push(make_const_int(start, 0));
        entry_ops.push(make_const_int(stop, range_bound));
        entry_ops.push(make_const_int(step, 1));

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        let iv = func.fresh_value();
        let cond = func.fresh_value();
        let next = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = entry_ops;
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![start],
            };
        }

        // Header(iv): cond = Lt(iv, stop); condbr cond -> body, exit.
        func.blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![TirValue {
                    id: iv,
                    ty: TirType::I64,
                }],
                ops: vec![make_op(OpCode::Lt, vec![iv, stop], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        // Body: a[index]; next = Add(iv, step) [nsw]; br header(next).
        let index_operand = if use_negative_index {
            func.fresh_value()
        } else {
            iv
        };
        let mut body_ops = Vec::new();
        if use_negative_index {
            body_ops.push(make_const_int(index_operand, -1));
        }
        let r = func.fresh_value();
        body_ops.push(make_op(
            OpCode::Index,
            vec![container, index_operand],
            vec![r],
        ));
        body_ops.push(make_nsw_add(vec![iv, step], vec![next]));
        func.blocks.insert(
            body_id,
            TirBlock {
                id: body_id,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![next],
                },
            },
        );

        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        (func, header_id, body_id, exit_id)
    }

    // ------------------------------------------------------------------
    // Test 7: a[i] in for i in range(N) where len(a) == N → bce_safe.
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_index_eliminated() {
        let (mut func, _header, body_id, _exit) = build_range_loop_func(5, 5, false);
        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present in loop body");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in range loop with matching container length must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 8: negative index inside range loop → NOT marked.
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_negative_index_preserved() {
        let (mut func, _header, body_id, _exit) = build_range_loop_func(5, 5, true);
        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present in loop body");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "Negative index inside range loop must NOT be marked bce_safe"
        );
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 9: non-range loop index → NOT marked (parameter-driven loop).
    // ------------------------------------------------------------------
    #[test]
    fn non_range_loop_index_preserved() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let param_iter = ValueId(0);
        let elem_val = func.fresh_value();
        let done_val = func.fresh_value();
        let index_result = func.fresh_value();

        let (entry_ops, container) = build_list_of_len(&mut func, 5);

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = entry_ops;
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![],
            };
        }

        // Header: IterNextUnboxed from the parameter iterator (NOT a range IV).
        func.blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::IterNextUnboxed,
                    vec![param_iter],
                    vec![elem_val, done_val],
                )],
                terminator: Terminator::CondBranch {
                    cond: done_val,
                    then_block: exit_id,
                    then_args: vec![],
                    else_block: body_id,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        func.blocks.insert(
            body_id,
            TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![make_op(
                    OpCode::Index,
                    vec![container, elem_val],
                    vec![index_result],
                )],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "Index with non-range iterator must NOT be marked bce_safe"
        );
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 10: range loop where container is too small → NOT marked.
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_container_too_small_not_marked() {
        // a has len 3, for i in range(5): a[i] → i can reach 4 > 2 → unsafe.
        let (mut func, _header, body_id, _exit) = build_range_loop_func(3, 5, false);
        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present in loop body");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "Index in range loop with container smaller than bound must NOT be bce_safe"
        );
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 11: range loop with container larger than bound → marked.
    // ------------------------------------------------------------------
    #[test]
    fn range_loop_container_larger_than_bound() {
        let (mut func, _header, body_id, _exit) = build_range_loop_func(10, 5, false);
        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in range loop with oversized container must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ==================================================================
    // While-loop guard tests
    // ==================================================================

    /// Build a `while i <= n` (Le) or `while i < n` (Lt) loop indexing a
    /// container of length `len_minus`-relative to `n`. The IV is a header
    /// block-arg with a no-signed-wrap +1 back-edge increment starting at 0.
    ///
    /// `is_le`: guard is `Le(i, n)` (vs `Lt(i, n)`).
    /// `container_len`: a closure that, given the entry builder, produces the
    ///   container value and its ops (length-vs-`n` relationship under test).
    fn build_while_guard_func(
        is_le: bool,
        store: bool,
        make_container: impl FnOnce(&mut TirFunction, ValueId) -> (Vec<TirOp>, ValueId),
    ) -> (TirFunction, BlockId, BlockId) {
        let mut func = TirFunction::new("w".into(), vec![], TirType::None);
        let n = func.fresh_value();
        let const_1 = func.fresh_value();
        let i_start = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        let (container_ops, container) = make_container(&mut func, n);

        let i_phi = func.fresh_value();
        let cond = func.fresh_value();
        let i_next = func.fresh_value();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            let mut ops = vec![make_const_int(n, 100), make_const_int(const_1, 1)];
            ops.extend(container_ops);
            ops.push(make_const_int(i_start, 0));
            entry.ops = ops;
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![i_start],
            };
        }

        let cmp = if is_le { OpCode::Le } else { OpCode::Lt };
        func.blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![TirValue {
                    id: i_phi,
                    ty: TirType::I64,
                }],
                ops: vec![make_op(cmp, vec![i_phi, n], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        let access = if store {
            let v = func.fresh_value();
            vec![
                make_const_int(v, 0),
                make_op(OpCode::StoreIndex, vec![container, i_phi, v], vec![]),
            ]
        } else {
            let elem = func.fresh_value();
            vec![make_op(OpCode::Index, vec![container, i_phi], vec![elem])]
        };
        let mut body_ops = access;
        body_ops.push(make_nsw_add(vec![i_phi, const_1], vec![i_next]));
        func.blocks.insert(
            body_id,
            TirBlock {
                id: body_id,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![i_next],
                },
            },
        );

        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        (func, header_id, body_id)
    }

    // ------------------------------------------------------------------
    // Test 12: while(i<=n) StoreIndex into [True]*(n+1) → bce_safe.
    //
    // The container length is the SSA value `n+1`. The IV i ranges over [0, n]
    // (start 0, +1, guard Le(i,n)). We prove via the numeric range: i <= n and
    // len = n+1 = const? No — n is const 100 here, so len = 101 (constant), and
    // i in [0, 100] ⇒ i < 101. Numeric proof discharges it.
    // ------------------------------------------------------------------
    #[test]
    fn while_loop_guard_le_store_index_marked_safe() {
        let (mut func, _header, body_id) = build_while_guard_func(true, true, |func, n| {
            // is_prime = [True] * (n + 1).
            let const_1 = func.fresh_value();
            let n_plus_1 = func.fresh_value();
            let true_val = func.fresh_value();
            let list_1 = func.fresh_value();
            let is_prime = func.fresh_value();
            let ops = vec![
                make_const_int(const_1, 1),
                make_nsw_add(vec![n, const_1], vec![n_plus_1]),
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n_plus_1], vec![is_prime]),
            ];
            (ops, is_prime)
        });

        let stats = run_bce(&mut func);
        let store_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::StoreIndex)
            .expect("StoreIndex op must be present");
        assert_eq!(
            store_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "StoreIndex in while(i<=n) with is_prime=[True]*(n+1) must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 13: while(i<=n) Index into [True]*(n+1) → bce_safe.
    // ------------------------------------------------------------------
    #[test]
    fn while_loop_guard_le_index_marked_safe() {
        let (mut func, _header, body_id) = build_while_guard_func(true, false, |func, n| {
            let const_1 = func.fresh_value();
            let n_plus_1 = func.fresh_value();
            let true_val = func.fresh_value();
            let list_1 = func.fresh_value();
            let is_prime = func.fresh_value();
            let ops = vec![
                make_const_int(const_1, 1),
                make_nsw_add(vec![n, const_1], vec![n_plus_1]),
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n_plus_1], vec![is_prime]),
            ];
            (ops, is_prime)
        });

        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in while(i<=n) with container=[True]*(n+1) must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 14: while(i<n) Index into container of length n → bce_safe.
    // ------------------------------------------------------------------
    #[test]
    fn while_loop_guard_lt_index_marked_safe() {
        let (mut func, _header, body_id) = build_while_guard_func(false, false, |func, n| {
            // container = [True] * n.
            let true_val = func.fresh_value();
            let list_1 = func.fresh_value();
            let container = func.fresh_value();
            let ops = vec![
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n], vec![container]),
            ];
            (ops, container)
        });

        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in while(i<n) with container of length n must be bce_safe"
        );
        assert!(stats.values_changed >= 1);
    }

    // ------------------------------------------------------------------
    // Test 15: while(i < len(lst)) Index lst[i] → bce_safe (symbolic-len).
    // ------------------------------------------------------------------
    #[test]
    fn while_lt_len_container_index_marked_safe() {
        // After iter_devirt: len_val = len(lst); header(i): cond = Lt(i, len_val);
        // body: x = Index(lst, i). The container length is NON-constant (it is
        // the SSA len(lst)), so this exercises the symbolic `i < len(c)` proof.
        let mut func = TirFunction::new("post_devirt".into(), vec![], TirType::None);

        let true_val = func.fresh_value();
        let list_1 = func.fresh_value();
        let n = func.fresh_value();
        let lst = func.fresh_value();
        let len_val = func.fresh_value();
        let const_1 = func.fresh_value();
        let i_start = func.fresh_value();
        let i_phi = func.fresh_value();
        let cond = func.fresh_value();
        let elem = func.fresh_value();
        let i_next = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = vec![
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_const_int(n, 100),
                make_op(OpCode::Mul, vec![list_1, n], vec![lst]),
                make_call_builtin("len", vec![lst], vec![len_val]),
                make_const_int(const_1, 1),
                make_const_int(i_start, 0),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![i_start],
            };
        }

        func.blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![TirValue {
                    id: i_phi,
                    ty: TirType::I64,
                }],
                ops: vec![make_op(OpCode::Lt, vec![i_phi, len_val], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        func.blocks.insert(
            body_id,
            TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![
                    make_op(OpCode::Index, vec![lst, i_phi], vec![elem]),
                    make_nsw_add(vec![i_phi, const_1], vec![i_next]),
                ],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![i_next],
                },
            },
        );
        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index in while(i<len(lst)) must be bce_safe (symbolic-len proof)"
        );
        assert!(stats.values_changed >= 1);
    }

    #[test]
    fn full_deopt_symbolic_len_index_not_marked_safe() {
        let mut func =
            TirFunction::new("full_deopt_index".into(), vec![TirType::I64], TirType::None);

        let n = ValueId(0);
        let true_val = func.fresh_value();
        let list_1 = func.fresh_value();
        let lst = func.fresh_value();
        let len_val = func.fresh_value();
        let const_1 = func.fresh_value();
        let i_start = func.fresh_value();
        let i_phi = func.fresh_value();
        let cond = func.fresh_value();
        let elem = func.fresh_value();
        let i_next = func.fresh_value();
        let overflow = func.fresh_value();

        let entry_block = func.entry_block;
        let header_id = func.fresh_block();
        let body_id = func.fresh_block();
        let exit_id = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&entry_block).unwrap();
            entry.ops = vec![
                make_const_int(true_val, 1),
                make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
                make_op(OpCode::Mul, vec![list_1, n], vec![lst]),
                make_call_builtin("len", vec![lst], vec![len_val]),
                make_const_int(const_1, 1),
                make_const_int(i_start, 0),
            ];
            entry.terminator = Terminator::Branch {
                target: header_id,
                args: vec![i_start],
            };
        }

        func.blocks.insert(
            header_id,
            TirBlock {
                id: header_id,
                args: vec![TirValue {
                    id: i_phi,
                    ty: TirType::I64,
                }],
                ops: vec![make_op(OpCode::Lt, vec![i_phi, len_val], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body_id,
                    then_args: vec![],
                    else_block: exit_id,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header_id, LoopRole::LoopHeader);

        func.blocks.insert(
            body_id,
            TirBlock {
                id: body_id,
                args: vec![],
                ops: vec![
                    make_op(OpCode::Index, vec![lst, i_phi], vec![elem]),
                    make_op(
                        OpCode::CheckedAdd,
                        vec![i_phi, const_1],
                        vec![i_next, overflow],
                    ),
                ],
                terminator: Terminator::Branch {
                    target: header_id,
                    args: vec![i_next],
                },
            },
        );
        func.blocks.insert(
            exit_id,
            TirBlock {
                id: exit_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

        let stats = run_bce(&mut func);
        let index_op = func.blocks[&body_id]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "full-range checked accumulator under symbolic len guard must keep bounds check"
        );
        assert_eq!(stats.values_changed, 0);
    }
}
