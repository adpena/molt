//! Parallel compilation support for TIR modules.
//!
//! Uses rayon's work-stealing thread pool to compile all functions in a module
//! concurrently. Each function's type refinement and optimization pipeline are
//! fully independent, so there are no data races.

use rayon::prelude::*;

use super::function::TirModule;
use super::passes::PassStats;

/// Compile all functions in a module in parallel using rayon work-stealing.
///
/// For each function the following steps are performed in sequence:
/// 1. Type refinement (`type_refine::refine_types`)
/// 2. Full optimization pipeline (`passes::run_pipeline`)
///
/// Returns a flat `Vec<PassStats>` — all stats from all functions, in the
/// order: [func0_pass0, func0_pass1, …, func1_pass0, func1_pass1, …].
/// The order across functions is non-deterministic (rayon schedules freely),
/// but within a single function the pass order is preserved.
pub fn compile_module_parallel(module: &mut TirModule) -> Vec<PassStats> {
    module
        .functions
        .par_iter_mut()
        .flat_map(|func| {
            super::type_refine::refine_types(func);
            super::passes::run_pipeline(func)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_const_int_func(name: &str, value: i64) -> TirFunction {
        let entry_id = BlockId(0);
        let v0 = ValueId(0);
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));

        let block = TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![v0],
                attrs,
                source_span: None,
            }],
            terminator: Terminator::Return { values: vec![v0] },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        TirFunction {
            name: name.into(),
            param_types: vec![],
            return_type: TirType::I64,
            blocks,
            entry_block: entry_id,
            next_value: 1,
            next_block: 1,
            attrs: AttrDict::new(),
        has_exception_handling: false,
            label_id_map: HashMap::new(),
        }
    }

    fn make_add_func(name: &str) -> TirFunction {
        let entry_id = BlockId(0);
        let p0 = ValueId(0);
        let p1 = ValueId(1);
        let sum = ValueId(2);

        let block = TirBlock {
            id: entry_id,
            args: vec![
                TirValue { id: p0, ty: TirType::I64 },
                TirValue { id: p1, ty: TirType::I64 },
            ],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![p0, p1],
                results: vec![sum],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::Return { values: vec![sum] },
        };
        let mut blocks = HashMap::new();
        blocks.insert(entry_id, block);
        TirFunction {
            name: name.into(),
            param_types: vec![TirType::I64, TirType::I64],
            return_type: TirType::I64,
            blocks,
            entry_block: entry_id,
            next_value: 3,
            next_block: 1,
            attrs: AttrDict::new(),
        has_exception_handling: false,
            label_id_map: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Module with 3 functions — all optimized in parallel, no panic.
    // -----------------------------------------------------------------------
    #[test]
    fn three_functions_parallel_no_data_races() {
        let mut module = TirModule {
            name: "three_func_module".into(),
            functions: vec![
                make_const_int_func("f0", 1),
                make_const_int_func("f1", 2),
                make_add_func("f2"),
            ],
            class_hierarchy: None,
        };

        let stats = compile_module_parallel(&mut module);

        // Each function goes through run_pipeline which emits 8 pass stats.
        // 3 functions × 8 passes = 24 stats total.
        assert_eq!(stats.len(), 3 * 8, "expected 24 stats (3 funcs × 8 passes)");

        // Verify all functions still have their entry blocks intact.
        for func in &module.functions {
            assert!(func.blocks.contains_key(&func.entry_block));
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: Empty module — no crash, zero stats returned.
    // -----------------------------------------------------------------------
    #[test]
    fn empty_module_no_crash() {
        let mut module = TirModule {
            name: "empty".into(),
            functions: vec![],
            class_hierarchy: None,
        };

        let stats = compile_module_parallel(&mut module);
        assert!(stats.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 3: Single function — same result as sequential run_pipeline.
    // -----------------------------------------------------------------------
    #[test]
    fn single_function_matches_sequential() {
        // Build two identical add functions independently (TirFunction doesn't
        // implement Clone, so we construct them separately).
        let mut seq_func = make_add_func("add_seq");
        let par_func = make_add_func("add_par");

        // Sequential path.
        super::super::type_refine::refine_types(&mut seq_func);
        let seq_stats = super::super::passes::run_pipeline(&mut seq_func);

        // Parallel path (single function in module).
        let mut module = TirModule {
            name: "single".into(),
            functions: vec![par_func],
            class_hierarchy: None,
        };
        let par_stats = compile_module_parallel(&mut module);

        // Same number of passes and same names/stats.
        assert_eq!(par_stats.len(), seq_stats.len());
        for (p, s) in par_stats.iter().zip(seq_stats.iter()) {
            assert_eq!(p.name, s.name);
            assert_eq!(p.ops_removed, s.ops_removed);
            assert_eq!(p.ops_added, s.ops_added);
        }
    }
}
