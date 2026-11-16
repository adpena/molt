# Module pipeline ownership across targets and profiles

The shared authority is
[the IR contract](../../spec/areas/compiler/0100_MOLT_IR.md), implemented by
`molt-passes::tir::module_phase::run_module_pipeline` and the wrappers in
`molt-tir::tir::pipeline_cache`. The table describes source wiring, not a claim
that every target/profile cell has passed release qualification.

| Consumer | Module result consumed | Final analysis owned by consumer |
| --- | --- | --- |
| Native / Cranelift | Selectively replaced SimpleIR at original indices | Final native leaf and linkage context after lifetime processing and splitting |
| LLVM (native and wasm32) | Owned transformed TIR; terminal-drops-only mode when IR passes are skipped | LLVM representation facts over those exact TIR bodies |
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
drop gates, optimization budgets, or target capability decisions. Native
requests a SimpleIR projection in each bounded optimization wave. LLVM
and FactGraph request only TIR and leave preparation input unchanged. LLVM moves
its source bodies into module assembly and retires each TIR body after lowering;
FactGraph borrows source functions without cloning the full input vector.

`skip_ir_passes` and preoptimized batch entrypoints retain their existing
semantics; they must not perform inlining twice. Build profile alone does not
create an alternate module-analysis owner. Backend/profile support and release
acceptance remain governed by the canonical matrices, not by this source-wiring
inventory.

Focused verification belongs to the module transformation, shared custody and
backconversion, native leaf metadata, LLVM feature, and FactGraph export test
owners. FactGraph JSON equivalence preserves facts,
ordering, and source sites; its precise producer label is
`CallFactsTable::build_for_function` under schema version 3.

## Target identity in the TIR cache key

An LLVM wasm32 job and a host LLVM job share `TargetKind::Llvm`, and
`target_platform_fingerprint` labels both with the host OS, architecture and
pointer width. The key misses no input, because TIR output depends on no
target fact outside the key:

- The key holds the semantic epoch, the cache flavor, every `TargetInfo` field
  (`tir_pipeline_target_fingerprint` destructures the struct, so a new field
  fails to compile until the key covers it) and the full FunctionIR contract.
  Whatever the frontend emits per target (platform constants, externalized
  stdlib bodies) is in that contract.
- No TIR pass reads a target triple, a pointer width or a host `cfg`. The one
  width-shaped proof, the `no_signed_wrap` iterator index in
  `iter_devirt/transform.rs`, rests on `isize::MAX < i64::MAX`, which holds on
  32- and 64-bit targets alike. Pointer width enters only at LLVM lowering,
  after the cache.
- A wasm32 job builds its `TargetInfo` from an empty feature string
  (`compile_driver.rs`), so it differs from an x86-64 AVX host job in vector
  width. On an aarch64 host the two keys are equal, and so is the TIR.

So the host platform label is redundant for LLVM, not wrong. If a pass ever
reads a target fact, it must become a `TargetInfo` field: the destructuring
fingerprint then forces it into the key.

