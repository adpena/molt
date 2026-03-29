//! Interprocedural optimization passes for TIR.
//!
//! This module provides whole-program analyses that operate across function
//! boundaries, as opposed to the intraprocedural passes that process a single
//! function at a time.
//!
//! # Passes
//!
//! 1. **Call graph construction** — scans all functions for call sites and
//!    builds a directed graph of caller → callee relationships.
//!
//! 2. **Dead function elimination** — removes functions that are not reachable
//!    from any entry point (BFS through the call graph).
//!
//! 3. **Inline candidate identification** — identifies small, loop-free
//!    functions that are good candidates for inlining (stub for Phase 4).

use std::collections::{HashMap, HashSet, VecDeque};

use super::PassStats;
use crate::tir::function::TirModule;
use crate::tir::ops::{AttrValue, OpCode};

// ---------------------------------------------------------------------------
// Call Graph
// ---------------------------------------------------------------------------

/// Whole-program call graph for a TIR module.
///
/// The graph is represented as two complementary adjacency maps:
///   - `callers_to_callees`: for each function, the set of functions it calls.
///   - `callees_to_callers`: for each function, the set of functions that call it.
///
/// Only statically-resolvable call targets appear in the graph.  Dynamic
/// dispatch (unresolved `CallMethod`) and higher-order calls whose target is
/// unknown at compile time are not represented.
#[derive(Debug, Default)]
pub struct CallGraph {
    /// function_name → set of callee names called from this function
    pub callers_to_callees: HashMap<String, HashSet<String>>,
    /// function_name → set of caller names that call this function
    pub callees_to_callers: HashMap<String, HashSet<String>>,
}

impl CallGraph {
    /// Return the set of functions directly called by `caller`.
    pub fn callees_of(&self, caller: &str) -> &HashSet<String> {
        static EMPTY: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
        self.callers_to_callees
            .get(caller)
            .unwrap_or_else(|| EMPTY.get_or_init(HashSet::new))
    }

    /// Return the set of functions that directly call `callee`.
    pub fn callers_of(&self, callee: &str) -> &HashSet<String> {
        static EMPTY: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
        self.callees_to_callers
            .get(callee)
            .unwrap_or_else(|| EMPTY.get_or_init(HashSet::new))
    }

    /// Record a caller → callee edge.
    fn add_edge(&mut self, caller: &str, callee: &str) {
        self.callers_to_callees
            .entry(caller.to_string())
            .or_default()
            .insert(callee.to_string());
        self.callees_to_callers
            .entry(callee.to_string())
            .or_default()
            .insert(caller.to_string());
    }
}

/// Extract the static callee name from an op's attrs, if present.
///
/// Attr-key conventions used across TIR passes:
///   - `Call`        → `attrs["callee"]` (set by CHA devirtualization) or
///     `attrs["s_value"]` (set by the SSA builder for
///     statically-known call targets).
///   - `CallMethod`  → `attrs["method"]` (virtual dispatch target name).
///   - `CallBuiltin` → `attrs["name"]`   (builtin function name).
fn callee_name_from_op(opcode: OpCode, attrs: &HashMap<String, AttrValue>) -> Option<String> {
    let attr_str = |key: &str| -> Option<String> {
        match attrs.get(key) {
            Some(AttrValue::Str(s)) => Some(s.clone()),
            _ => None,
        }
    };

    match opcode {
        OpCode::Call => {
            // Prefer the explicit "callee" attr written by CHA; fall back to
            // "s_value" which the SSA builder records for direct-call ops.
            attr_str("callee").or_else(|| attr_str("s_value"))
        }
        OpCode::CallMethod => attr_str("method"),
        OpCode::CallBuiltin => attr_str("name"),
        _ => None,
    }
}

/// Build a whole-program call graph by scanning every op in every function.
///
/// Each function is seeded in `callers_to_callees` even if it makes no calls,
/// so the map can be used as a complete function-name index.
pub fn build_call_graph(module: &TirModule) -> CallGraph {
    let mut graph = CallGraph::default();

    for func in &module.functions {
        // Ensure every function has an entry even if it makes no calls.
        graph
            .callers_to_callees
            .entry(func.name.clone())
            .or_default();
        graph
            .callees_to_callers
            .entry(func.name.clone())
            .or_default();

        for block in func.blocks.values() {
            for op in &block.ops {
                if let Some(callee) = callee_name_from_op(op.opcode, &op.attrs) {
                    graph.add_edge(&func.name, &callee);
                }
            }
        }
    }

    graph
}

// ---------------------------------------------------------------------------
// Dead Function Elimination
// ---------------------------------------------------------------------------

/// Remove functions not reachable from module entry points.
///
/// Entry points are functions whose name is `"__main__"`, `"main"`, or —
/// when neither of those is present — the very first function in the module.
///
/// Reachability is computed via BFS through the call graph.  Only functions
/// whose names appear in the module's function list are considered reachable
/// (external callees that are not defined in the module are ignored for the
/// purpose of reachability, though their edges still appear in the graph).
///
/// Returns statistics describing how many functions were removed.
pub fn eliminate_dead_functions(module: &mut TirModule, graph: &CallGraph) -> PassStats {
    let mut stats = PassStats {
        name: "dead_function_elimination",
        ..Default::default()
    };

    if module.functions.is_empty() {
        return stats;
    }

    // Collect the set of function names defined in this module.
    let defined: HashSet<String> = module.functions.iter().map(|f| f.name.clone()).collect();

    // Determine entry points.
    let mut entry_points: Vec<String> = Vec::new();
    for ep in &["__main__", "main"] {
        if defined.contains(*ep) {
            entry_points.push(ep.to_string());
        }
    }
    // Fallback: use the first function when no conventional entry point exists.
    if entry_points.is_empty() {
        entry_points.push(module.functions[0].name.clone());
    }

    // BFS from every entry point.
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    for ep in entry_points {
        if reachable.insert(ep.clone()) {
            queue.push_back(ep);
        }
    }

    while let Some(caller) = queue.pop_front() {
        for callee in graph.callees_of(&caller) {
            // Only follow edges to functions defined in this module.
            if defined.contains(callee) && reachable.insert(callee.clone()) {
                queue.push_back(callee.clone());
            }
        }
    }

    // Remove unreachable functions.
    let before = module.functions.len();
    module.functions.retain(|f| reachable.contains(&f.name));
    let after = module.functions.len();

    stats.ops_removed = before - after;
    stats
}

// ---------------------------------------------------------------------------
// Inline Candidate Identification (stub — actual inlining deferred to Phase 4)
// ---------------------------------------------------------------------------

/// Maximum number of ops a function may contain to be considered a candidate
/// for inlining.
const INLINE_OP_LIMIT: usize = 30;

/// Returns `true` if the function contains a back-edge (loop), making it
/// unsuitable for simple inlining.
///
/// A back-edge exists whenever a block's terminator targets a block whose
/// `BlockId` is ≤ the source block's `BlockId`.  This is a conservative
/// approximation: block IDs are allocated in topological order during
/// construction, so a lower-numbered target strongly implies a loop.
fn function_has_loop(func: &crate::tir::function::TirFunction) -> bool {
    use crate::tir::blocks::Terminator;

    for (bid, block) in &func.blocks {
        let targets: Vec<_> = match &block.terminator {
            Terminator::Branch { target, .. } => vec![*target],
            Terminator::CondBranch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch { cases, default, .. } => {
                let mut ts: Vec<_> = cases.iter().map(|(_, t, _)| *t).collect();
                ts.push(*default);
                ts
            }
            Terminator::Return { .. } | Terminator::Unreachable => vec![],
        };
        for target in targets {
            if target.0 <= bid.0 {
                return true;
            }
        }
    }
    false
}

/// Count the total number of ops across all blocks in a function.
fn total_ops(func: &crate::tir::function::TirFunction) -> usize {
    func.blocks.values().map(|b| b.ops.len()).sum()
}

/// Identify small, loop-free functions that are candidates for inlining.
///
/// A function qualifies when:
///   1. It has at most [`INLINE_OP_LIMIT`] total ops.
///   2. It contains no back-edges (no loops).
///
/// **Phase 3 stub**: this function only *identifies* candidates and returns
/// their names.  Actual inlining (which requires block splitting and SSA
/// renaming) is deferred to Phase 4.
pub fn identify_inline_candidates(module: &TirModule, _graph: &CallGraph) -> Vec<String> {
    module
        .functions
        .iter()
        .filter(|f| total_ops(f) <= INLINE_OP_LIMIT && !function_has_loop(f))
        .map(|f| f.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator};
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a minimal function with a single entry block containing the given ops.
    fn func_with_ops(name: &str, ops: Vec<TirOp>) -> TirFunction {
        let mut func = TirFunction::new(name.to_string(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return { values: vec![] };
        func
    }

    /// Build a `Call` op with a statically-known callee name in `attrs["callee"]`.
    fn call_op(callee: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("callee".into(), AttrValue::Str(callee.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    /// Build a `CallBuiltin` op with the given builtin name.
    fn call_builtin_op(name: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str(name.to_string()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        }
    }

    /// Build a simple module with the given functions.
    fn make_module(functions: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "test".to_string(),
            functions,
            class_hierarchy: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Call graph construction — A calls B, B calls C
    // -----------------------------------------------------------------------
    #[test]
    fn call_graph_three_functions() {
        // A calls B; B calls C; C calls nothing.
        let func_a = func_with_ops("A", vec![call_op("B")]);
        let func_b = func_with_ops("B", vec![call_op("C")]);
        let func_c = func_with_ops("C", vec![]);

        let module = make_module(vec![func_a, func_b, func_c]);
        let graph = build_call_graph(&module);

        // A → B
        assert!(graph.callees_of("A").contains("B"));
        assert!(!graph.callees_of("A").contains("C"));

        // B → C
        assert!(graph.callees_of("B").contains("C"));
        assert!(!graph.callees_of("B").contains("A"));

        // C → nothing
        assert!(graph.callees_of("C").is_empty());

        // Reverse edges
        assert!(graph.callers_of("B").contains("A"));
        assert!(graph.callers_of("C").contains("B"));
        assert!(graph.callers_of("A").is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: Dead function elimination — D is unreachable
    // -----------------------------------------------------------------------
    #[test]
    fn dead_function_eliminated() {
        // __main__ calls B; D is never called.
        let func_main = func_with_ops("__main__", vec![call_op("B")]);
        let func_b = func_with_ops("B", vec![]);
        let func_d = func_with_ops("D", vec![]); // dead

        let mut module = make_module(vec![func_main, func_b, func_d]);
        let graph = build_call_graph(&module);
        let stats = eliminate_dead_functions(&mut module, &graph);

        // D must be removed.
        assert_eq!(stats.ops_removed, 1);
        let names: HashSet<_> = module.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains("__main__"));
        assert!(names.contains("B"));
        assert!(!names.contains("D"));
    }

    // -----------------------------------------------------------------------
    // Test 3: Inline candidate identification
    //   - Small function (≤30 ops, no loop) → candidate
    //   - Large function (>30 ops) → not a candidate
    // -----------------------------------------------------------------------
    #[test]
    fn inline_candidates_identified() {
        // Small function: 3 ops, no loops.
        let small = func_with_ops(
            "small",
            vec![
                call_builtin_op("len"),
                call_builtin_op("str"),
                call_builtin_op("int"),
            ],
        );

        // Large function: 31 ops.
        let large_ops: Vec<TirOp> = (0..31).map(|_| call_builtin_op("len")).collect();
        let large = func_with_ops("large", large_ops);

        let module = make_module(vec![small, large]);
        let graph = build_call_graph(&module);
        let candidates = identify_inline_candidates(&module, &graph);

        assert!(candidates.contains(&"small".to_string()));
        assert!(!candidates.contains(&"large".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test 3b: Function with a loop is not an inline candidate
    // -----------------------------------------------------------------------
    #[test]
    fn function_with_loop_not_inlined() {
        // Build a function with an explicit back-edge: entry (BlockId(0))
        // branches back to itself, creating a loop.
        let mut func = TirFunction::new("loopy".to_string(), vec![], TirType::None);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            // Self-loop: branch to block 0 (back-edge since target.0 <= source.0).
            entry.terminator = Terminator::Branch {
                target: BlockId(0),
                args: vec![],
            };
        }

        let module = make_module(vec![func]);
        let graph = build_call_graph(&module);
        let candidates = identify_inline_candidates(&module, &graph);

        assert!(!candidates.contains(&"loopy".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test 4: Empty module — no crash
    // -----------------------------------------------------------------------
    #[test]
    fn empty_module_no_crash() {
        let mut module = make_module(vec![]);
        let graph = build_call_graph(&module);

        // Call graph on empty module should be empty.
        assert!(graph.callers_to_callees.is_empty());
        assert!(graph.callees_to_callers.is_empty());

        // DFE on empty module should return zero removals.
        let stats = eliminate_dead_functions(&mut module, &graph);
        assert_eq!(stats.ops_removed, 0);

        // Inline identification on empty module should return empty list.
        let candidates = identify_inline_candidates(&module, &graph);
        assert!(candidates.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 5: main (not __main__) as entry point
    // -----------------------------------------------------------------------
    #[test]
    fn main_entry_point_respected() {
        let func_main = func_with_ops("main", vec![call_op("helper")]);
        let func_helper = func_with_ops("helper", vec![]);
        let func_orphan = func_with_ops("orphan", vec![]);

        let mut module = make_module(vec![func_main, func_helper, func_orphan]);
        let graph = build_call_graph(&module);
        let stats = eliminate_dead_functions(&mut module, &graph);

        assert_eq!(stats.ops_removed, 1);
        let names: HashSet<_> = module.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains("main"));
        assert!(names.contains("helper"));
        assert!(!names.contains("orphan"));
    }

    // -----------------------------------------------------------------------
    // Test 6: Transitive reachability preserved
    // -----------------------------------------------------------------------
    #[test]
    fn transitive_reachability_preserved() {
        // __main__ → A → B → C; D is unreachable
        let func_main = func_with_ops("__main__", vec![call_op("A")]);
        let func_a = func_with_ops("A", vec![call_op("B")]);
        let func_b = func_with_ops("B", vec![call_op("C")]);
        let func_c = func_with_ops("C", vec![]);
        let func_d = func_with_ops("D", vec![]);

        let mut module = make_module(vec![func_main, func_a, func_b, func_c, func_d]);
        let graph = build_call_graph(&module);
        let stats = eliminate_dead_functions(&mut module, &graph);

        assert_eq!(stats.ops_removed, 1);
        let names: Vec<_> = module.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"__main__"));
        assert!(names.contains(&"A"));
        assert!(names.contains(&"B"));
        assert!(names.contains(&"C"));
        assert!(!names.contains(&"D"));
    }

    // -----------------------------------------------------------------------
    // Test 7: CallBuiltin edges recorded in call graph
    // -----------------------------------------------------------------------
    #[test]
    fn call_builtin_edge_recorded() {
        let func_a = func_with_ops("A", vec![call_builtin_op("print")]);
        let module = make_module(vec![func_a]);
        let graph = build_call_graph(&module);

        assert!(graph.callees_of("A").contains("print"));
        assert!(graph.callers_of("print").contains("A"));
    }
}
