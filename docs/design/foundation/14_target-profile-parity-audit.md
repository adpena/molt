# Module pipeline ownership across targets and profiles

The shared authority is
[the IR contract](../../spec/areas/compiler/0100_MOLT_IR.md), implemented by
`molt-passes::tir::module_phase::run_module_pipeline` and the wrappers in
`molt-tir::tir::pipeline_cache`. The table describes source wiring, not a claim
that every target/profile cell has passed release qualification.

| Consumer | Module result consumed | Final analysis owned by consumer |
| --- | --- | --- |
| Native / Cranelift | Selectively replaced SimpleIR at original indices | Final native leaf and linkage context after lifetime processing and splitting |
| WASM | Selectively replaced SimpleIR at original indices | WASM lowering plans over the emitted bodies |
| LLVM | Owned transformed TIR; terminal-drops-only mode when IR passes are skipped | LLVM representation facts over those exact TIR bodies |
| Luau | Selectively replaced SimpleIR and changed-function count | Luau lowering over the replaced bodies |
| FactGraph | Owned transformed local TIR | Final call graph, summaries, and precise facts for the selected function |

All module-phase consumers use the same transformation sequence with their
chosen `TargetInfo`. The initial graph and summaries feed inlining and generator
fusion; the phase returns one deterministic set of changed names. Ordinary
module codegen does not rebuild a post-transform graph, summaries, or call-fact
tables without a consumer. FactGraph requests them explicitly. Native's final
leaf graph remains necessary because later lifetime/splitting passes change
which calls codegen emits.

Cache hits and misses transfer the same optimized TIR custody into the module
wrappers. The change does not alter cache identities, pass skip policy, terminal
drop gates, optimization budgets, or target capability decisions. LLVM's cached
per-function runner still produces a SimpleIR projection even though its module
consumer lowers owned TIR. That distinct conversion is outside this contract.

`skip_ir_passes` and preoptimized batch entrypoints retain their existing
semantics; they must not perform inlining twice. Build profile alone does not
create an alternate module-analysis owner. Backend/profile support and release
acceptance remain governed by the canonical matrices, not by this source-wiring
inventory.

Focused verification belongs to the module transformation, shared custody and
backconversion, native leaf metadata, WASM pipeline, Luau pipeline, LLVM feature,
and FactGraph export test owners. FactGraph JSON equivalence preserves facts,
ordering, and source sites; its precise producer label is
`CallFactsTable::build_for_function` under schema version 3.
