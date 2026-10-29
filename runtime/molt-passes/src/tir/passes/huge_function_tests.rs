//! HF-549: the pass pipeline scales to huge functions. Every CFG and
//! dominator-tree walk uses an explicit stack, dominator trees come from
//! Semi-NCA, and dominance queries compare pre/post intervals, so native stack
//! depth and per-query cost do not grow with the function.

use crate::ir::{FunctionIR, OpIR};
use crate::tir::analysis::Analysis;
use crate::tir::blocks::BlockId;
use crate::tir::lower_from_simple::lower_to_tir;
use crate::tir::lower_to_simple::lower_to_simple_ir;
use crate::tir::passes::alias_analysis::AliasAnalysis;
use crate::tir::passes::memory_ssa::{self, MemAccess};
use crate::tir::target_info::TargetInfo;

const STATEMENTS: i64 = 20_000;
const HANDLER: i64 = 1_000_000;
/// Rust's default stack for a spawned thread.
const DEFAULT_THREAD_STACK: usize = 2 << 20;

fn marked(kind: &str, value: i64) -> OpIR {
    OpIR {
        kind: kind.into(),
        value: Some(value),
        ..OpIR::default()
    }
}

fn op(kind: &str, args: &[&str], out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.into(),
        args: Some(args.iter().map(|arg| arg.to_string()).collect()),
        out,
        ..OpIR::default()
    }
}

/// Every statement stores and reloads `obj[key]`, then observes a pending
/// exception that enters one shared handler: a dominator chain 20,000 blocks
/// deep above a join with 20,000 predecessors. A recursive walk overflowed a
/// 2 MiB thread on this shape; per-query idom walks took minutes.
fn huge_function() -> FunctionIR {
    let mut ops = vec![marked("label", 0)];
    for line in 1..=STATEMENTS {
        ops.push(marked("line", line));
        ops.push(op("store_index", &["obj", "key", "obj"], None));
        ops.push(op("index", &["obj", "key"], Some(format!("r{line}"))));
        ops.push(marked("check_exception", HANDLER));
    }
    ops.extend([
        op("ret_void", &[], None),
        marked("label", HANDLER),
        marked("line", HANDLER),
        op("index", &["obj", "key"], Some("handled".into())),
        op("ret_void", &[], None),
    ]);
    FunctionIR {
        name: "huge".into(),
        params: vec!["obj".into(), "key".into()],
        return_abi: molt_ir::FunctionReturnAbi::Value,
        ops,
        ..FunctionIR::default()
    }
}

#[test]
fn twenty_thousand_block_function_compiles_on_a_default_thread() {
    std::thread::Builder::new()
        .stack_size(DEFAULT_THREAD_STACK)
        .spawn(|| {
            let mut tir = lower_to_tir(&huge_function());
            assert!(tir.blocks.len() > 20_000, "{} blocks", tir.blocks.len());
            let handler = tir
                .label_id_map
                .iter()
                .find_map(|(&block, &label)| (label == HANDLER).then_some(BlockId(block)))
                .expect("handler block");

            // The handler merges the memory state of every statement that can
            // raise into it: one phi with one distinct incoming version each.
            let alias = AliasAnalysis::compute(&tir);
            let memory = memory_ssa::compute_standalone(&tir, &alias);
            let phi = memory.block_phis[&handler];
            let Some(MemAccess::Phi { incoming, .. }) = memory.defs.get(&phi) else {
                panic!("handler phi {phi:?} is not a phi");
            };
            assert_eq!(incoming.len(), STATEMENTS as usize);
            let versions: std::collections::HashSet<_> = incoming.iter().map(|(_, v)| v).collect();
            assert_eq!(versions.len(), STATEMENTS as usize);

            let target = TargetInfo::wasm_release_fast();
            super::run_pipeline(&mut tir, &target);
            super::run_drop_phase(&mut tir, &target);
            let lowered = lower_to_simple_ir(&tir);
            let checks = lowered
                .iter()
                .filter(|op| op.kind == "check_exception")
                .count();
            assert_eq!(
                checks, STATEMENTS as usize,
                "every statement keeps its check"
            );
        })
        .expect("spawn a default-stack thread")
        .join()
        .expect("the pipeline completes within a default thread stack");
}
