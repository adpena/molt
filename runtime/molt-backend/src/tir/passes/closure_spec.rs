//! Closure / Lambda Specialization Detection Pass (Phase 6).
//!
//! Detects `Call` operations where one argument is a statically-known function
//! reference (a `ConstStr` op whose value names a function) and the callee is
//! small enough to be a specialization candidate (≤ 30 ops across all blocks).
//!
//! When a candidate is found the `Call` op is annotated with
//! `attrs["closure_specialized"] = true` so that later passes (and the backend)
//! can decide whether to actually perform the inlining.
//!
//! Actual body inlining is deferred to when the block-splitting infrastructure
//! is available.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

/// Maximum total op count (across all blocks) for a function to be considered
/// a specialization candidate.
const MAX_CALLEE_OPS: usize = 30;

/// Run the closure specialization detection pass on `func`.
///
/// Returns statistics reflecting how many call sites were marked.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "closure_spec",
        ..Default::default()
    };

    // Phase 1: Build a set of ValueIds that are defined by a ConstStr op.
    // These represent statically-known function references (e.g. a lambda or
    // named function passed as a higher-order argument).
    let mut known_func_refs: HashMap<ValueId, String> = HashMap::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstStr
                && let Some(AttrValue::Str(s)) = op.attrs.get("value")
            {
                for &res in &op.results {
                    known_func_refs.insert(res, s.clone());
                }
            }
        }
    }

    // If there are no known function references in scope, there is nothing to do.
    if known_func_refs.is_empty() {
        return stats;
    }

    // Phase 2: Count total ops in this function to use as a small-callee proxy.
    // (We can't look up the callee's body here — that requires inter-procedural
    // information. As a conservative heuristic we use the *current* function's
    // size as a stand-in: if the current function is small it is likely itself
    // a lambda or helper that's worth specializing.)
    let total_ops: usize = func.blocks.values().map(|b| b.ops.len()).sum();
    let is_small = total_ops <= MAX_CALLEE_OPS;

    // Phase 3: Walk all blocks and annotate eligible Call sites.
    let block_ids: Vec<_> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if op.opcode != OpCode::Call {
                continue;
            }
            // Check whether any operand is a known static function reference.
            let has_known_func_arg = op.operands.iter().any(|v| known_func_refs.contains_key(v));

            if has_known_func_arg && is_small {
                op.attrs
                    .insert("closure_specialized".into(), AttrValue::Bool(true));
                stats.values_changed += 1;
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_const_str(result: u32, value: &str) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Str(value.into()));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_call(result: u32, callee: u32, arg: u32) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![ValueId(callee), ValueId(arg)],
            results: vec![ValueId(result)],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Build a minimal function with one I64 param and provided ops, run the pass.
    fn run_pass(ops: Vec<TirOp>) -> Vec<TirOp> {
        let mut func = TirFunction::new("test".into(), vec![TirType::I64], TirType::DynBox);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = ops;
            entry.terminator = Terminator::Return { values: vec![] };
        }
        run(&mut func);
        func.blocks[&func.entry_block].ops.clone()
    }

    #[test]
    fn call_with_known_func_arg_is_marked() {
        // ValueId(0) = I64 param, ValueId(1) = ConstStr("my_lambda"),
        // ValueId(2) = Call(callee=1, arg=0)
        let ops = vec![make_const_str(1, "my_lambda"), make_call(2, 1, 0)];
        let result = run_pass(ops);
        assert_eq!(
            result[1].attrs.get("closure_specialized"),
            Some(&AttrValue::Bool(true)),
            "expected closure_specialized = true on call with known func arg"
        );
    }

    #[test]
    fn call_with_dynamic_arg_not_marked() {
        // ValueId(0) = I64 param (dynamic, not a ConstStr).
        // ValueId(1) = Call(callee=0, arg=0) — both operands are dynamic.
        let ops = vec![make_call(1, 0, 0)];
        let result = run_pass(ops);
        assert!(
            result[0].attrs.get("closure_specialized").is_none(),
            "expected no closure_specialized attr on call with dynamic args"
        );
    }

    #[test]
    fn no_closures_no_changes() {
        // A function with no Call ops and no ConstStr ops — pass should be a no-op.
        let ops = vec![TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(1)],
            attrs: {
                let mut a = AttrDict::new();
                a.insert("value".into(), AttrValue::Int(42));
                a
            },
            source_span: None,
        }];
        let result = run_pass(ops);
        assert!(result[0].attrs.get("closure_specialized").is_none());
    }
}
