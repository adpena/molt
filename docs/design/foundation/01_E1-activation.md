# E1 inliner activation and module ownership

The inliner is part of the shared module pipeline in
`runtime/molt-passes/src/tir/module_phase.rs`. It transforms the optimized TIR
that backend consumers actually lower. The current IR contract is
[0100_MOLT_IR.md](../../spec/areas/compiler/0100_MOLT_IR.md).

## Pipeline and change authority

The cached per-function runner owns the optimized TIR bodies. The module wrapper
consumes that custody, excludes extern declarations from transformations, and
runs inlining, generator fusion, slot promotion, changed-body optimization, and
terminal drop insertion. The initial call graph and bottom-up summaries remain
inputs to the inliner and fusion passes.

`run_module_pipeline` returns a `BTreeSet<String>` containing every function
whose body or ownership markers changed. Each pass contributes to that one set.
SimpleIR consumers back-convert only those functions at their original indices,
retire each consumed TIR body, and retain byte-identical SimpleIR for unchanged
functions. Return ABI and index-map checks run before replacement. Externs keep
their authored signatures and do not enter body passes.

LLVM consumes owned post-module TIR directly and rebuilds value representation
facts from those exact bodies. Its per-function cache runner still produces a
SimpleIR projection; this is separate from the module phase. The LLVM path does
not run the Cranelift module phase as well. Luau uses the shared SimpleIR wrapper
with its own target information and receives the changed-function count.

## Native leaf and linkage metadata

Native recursion-guard decisions use the graph of the final lifetime-processed,
split bodies. Inlining can remove calls; lifetime finalization and splitting can
add or move calls. A pre-finalization graph cannot authorize the emitted leaf
set. `native_backend/simple_backend/module_metadata.rs` owns the final graph and
freezes whole-program leaf and linkage metadata before object partitioning.
Batches reuse this context rather than interpreting missing callees as local
body owners.

Extern declarations carry canonical target-neutral signatures. Every batch
checks arity, closure role, value/void result, and execution-context policy
against the frozen linkage row before LLVM or Cranelift registers it. Shared
stdlib cache admission includes the complete versioned FunctionIR contract and
exact linkage row; module optimization does not replace this ABI authority.

## Demanded diagnostics and verification

FactGraph explicitly analyzes the final local module and builds precise call
facts only for the requested function. This diagnostic work does not run for
ordinary module codegen. See [CallFacts](47_call_facts_leaf_coverage.md).

Module tests verify that an eligible static call disappears, SSA stays valid,
and overlapping inliner/drop changes name a function once. Shared cache tests
verify selective conversion, original index mapping around externs, authored
metadata, ABI refusal, and source-file custody. Backend tests exercise the real
wrapper consumers. These source contracts do not substitute for the release
acceptance matrix or target execution evidence.
