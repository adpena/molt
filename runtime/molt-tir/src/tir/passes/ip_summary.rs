//! Bottom-up **interprocedural function summaries** (Tier-0 substrate **S4**).
//!
//! A [`FunctionSummary`] is the compact, callee-side fact the interprocedural
//! tier reads at each call site: is the callee a leaf, how big is it (the inline
//! cost input), and what type does it return (the IPSCCP / return-type
//! backpropagation seed). Summaries are computed **bottom-up over the call
//! graph** — every callee's summary is finalized before its callers' — so a
//! later inliner (E1) / IPSCCP (E4) consumes ready facts in a single pass.
//!
//! This is the minimal, sound summary set S4 needs to stand up the module phase
//! and replace the native leaf scan. The richer slots the IPO tier will add
//! (does-not-capture-param[i] for IP-escape E3, is-pure for CSE/LICM of user
//! calls) extend [`FunctionSummary`] when those arcs land; they are intentionally
//! *not* fabricated here, because an unsound `is_pure=true` would miscompile.

use std::collections::BTreeMap;

use super::super::call_graph::CallGraph;
use super::super::function::TirModule;
use super::super::types::TirType;

/// The interprocedural summary of a single function.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSummary {
    /// True iff the function makes no call of any kind (the
    /// [`CallGraph::leaf_functions`] predicate). A leaf cannot recurse.
    pub is_leaf: bool,
    /// Total number of TIR ops across all blocks — the size input a cost model
    /// (S2 [`crate::tir::target_info::TargetInfo::inline_budget`]) compares
    /// against the inline budget. Excludes terminators (which the inline cost
    /// model also excludes).
    pub op_count: usize,
    /// The function's declared return type. The IPSCCP / return-type
    /// backpropagation seed for call sites.
    pub return_type: TirType,
}

/// All function summaries for a module, keyed by function name.
#[derive(Debug, Clone, Default)]
pub struct ModuleSummaries {
    summaries: BTreeMap<String, FunctionSummary>,
}

impl ModuleSummaries {
    /// Compute every function's summary **bottom-up over the call graph**.
    ///
    /// The traversal order is [`CallGraph::bottom_up_order`] (callees before
    /// callers); each function's summary is finalized in that order so that, as
    /// the IPO tier grows summary fields that genuinely depend on callee
    /// summaries (purity, escape), the computation already visits in the correct
    /// dependency order. The fields computed today (leaf / op-count / return
    /// type) are intraprocedural, so the order does not change their values —
    /// but establishing the bottom-up scaffold now is the structural point of
    /// this pass.
    pub fn compute(module: &TirModule, call_graph: &CallGraph) -> ModuleSummaries {
        // Index functions by name for O(1) lookup during the ordered walk.
        let by_name: BTreeMap<&str, &super::super::function::TirFunction> = module
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f))
            .collect();

        let mut summaries: BTreeMap<String, FunctionSummary> = BTreeMap::new();

        for scc in call_graph.bottom_up_order() {
            for name in scc {
                let Some(func) = by_name.get(name.as_str()) else {
                    continue;
                };
                let op_count = func.blocks.values().map(|b| b.ops.len()).sum();
                summaries.insert(
                    name.clone(),
                    FunctionSummary {
                        is_leaf: !call_graph.makes_any_call(&name),
                        op_count,
                        return_type: func.return_type.clone(),
                    },
                );
            }
        }

        ModuleSummaries { summaries }
    }

    /// The summary of `name`, if the function is in the module.
    pub fn get(&self, name: &str) -> Option<&FunctionSummary> {
        self.summaries.get(name)
    }

    /// The set of leaf function names (those whose summary marks `is_leaf`).
    /// Equals [`CallGraph::leaf_functions`]; provided so a consumer holding the
    /// summaries does not need the call graph too.
    pub fn leaf_functions(&self) -> std::collections::BTreeSet<String> {
        self.summaries
            .iter()
            .filter(|(_, s)| s.is_leaf)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Number of summarized functions.
    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    /// True if no functions are summarized.
    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    fn func_calling(name: &str, ret: TirType, callees: &[&str], extra_ops: usize) -> TirFunction {
        let mut func = TirFunction::new(name.into(), vec![], ret);
        let entry = func.entry_block;
        // Allocate value ids for the extra ConstNone ops up front (mutable
        // borrow of `func` for fresh_value must not overlap the block borrow).
        let extra_vals: Vec<_> = (0..extra_ops).map(|_| func.fresh_value()).collect();
        let block = func.blocks.get_mut(&entry).unwrap();
        for callee in callees {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str((*callee).to_string()));
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![],
                attrs,
                source_span: None,
            });
        }
        for v in extra_vals {
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstNone,
                operands: vec![],
                results: vec![v],
                attrs: AttrDict::new(),
                source_span: None,
            });
        }
        block.terminator = Terminator::Return { values: vec![] };
        func
    }

    fn module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "m".into(),
            functions: funcs,
        }
    }

    #[test]
    fn summary_marks_leaf_and_counts_ops_and_return_type() {
        // a (calls b, returns I64, 1 call + 2 ops = 3) → b (leaf, returns Str,
        // 3 ops). Build the module once and derive the call graph from it.
        let funcs = || {
            vec![
                func_calling("a", TirType::I64, &["b"], 2),
                func_calling("b", TirType::Str, &[], 3),
            ]
        };
        let cg = CallGraph::build(&module(funcs()));
        let m = module(funcs());
        let summaries = ModuleSummaries::compute(&m, &cg);

        let sa = summaries.get("a").unwrap();
        assert!(!sa.is_leaf);
        assert_eq!(sa.return_type, TirType::I64);
        assert_eq!(sa.op_count, 3); // 1 Call + 2 ConstNone

        let sb = summaries.get("b").unwrap();
        assert!(sb.is_leaf);
        assert_eq!(sb.return_type, TirType::Str);
        assert_eq!(sb.op_count, 3);
    }

    #[test]
    fn leaf_functions_matches_call_graph() {
        let funcs = || {
            vec![
                func_calling("a", TirType::None, &["b"], 0),
                func_calling("b", TirType::None, &[], 0),
                func_calling("c", TirType::None, &[], 0),
            ]
        };
        let cg = CallGraph::build(&module(funcs()));
        let m = module(funcs());
        let summaries = ModuleSummaries::compute(&m, &cg);
        assert_eq!(summaries.leaf_functions(), cg.leaf_functions());
    }

    #[test]
    fn every_function_is_summarized() {
        let funcs = || {
            vec![
                func_calling("a", TirType::None, &["b"], 0),
                func_calling("b", TirType::None, &["c"], 0),
                func_calling("c", TirType::None, &[], 0),
            ]
        };
        let cg = CallGraph::build(&module(funcs()));
        let m = module(funcs());
        let summaries = ModuleSummaries::compute(&m, &cg);
        assert_eq!(summaries.len(), 3);
        assert!(summaries.get("a").is_some());
        assert!(summaries.get("b").is_some());
        assert!(summaries.get("c").is_some());
    }

    #[test]
    fn empty_module_has_no_summaries() {
        let m = module(vec![]);
        let cg = CallGraph::build(&m);
        let summaries = ModuleSummaries::compute(&m, &cg);
        assert!(summaries.is_empty());
    }
}
