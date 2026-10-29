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

LLVM and FactGraph request TIR-only preparation from the shared cache runner;
no SimpleIR projection is built on cache hits or misses. FactGraph borrows its
input without cloning the full FunctionIR vector. LLVM captures source ABI
metadata before moving source bodies into module assembly, then consumes each
post-module TIR body after LLVM lowering. The LLVM path does not also run the
Cranelift module phase. Native, WASM and Luau request SimpleIR projections within
the existing bounded optimization waves; Luau receives the changed-function count.

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

## No ambient off switch

Legality, profitability and external-linkage facts decide whether a call is
inlined. There is no process-global switch that turns the production inliner
into a no-op. An inliner regression is fixed in a pass predicate, a
representation fact or a backend consumer, or by reverting the code. The
bigint oracle in `tests/differential/basic/inline_llvm_module_phase_activation.py`
guards the case where an inlined body could unbox a value that needs a bigint.
