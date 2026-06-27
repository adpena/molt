<!-- Foundation blueprint 21f. Architect: portfolio-architect (Plan agent), 2026-06-24. Arc:
the per-S-move EXECUTABLE specs for the crate-graph decomposition (21b's S1-S8), detailed to
the 21a/21d/21e execution level so the swarm can run each move precisely. Verified against the
live tree at HEAD 13dde78b7 (T1 landed; M1 fc/ families landed; crate decomposition mid-flight).
Move-only / zero-logic-change / minimal-cross-crate-surface. Design only -- no code refactored
in the session that produced it. Governed by DESIGN_DOCTRINE.md (god-files-are-killers: the
crate split is THE incremental-build killer; pythonista-rustacean). -->

# 21f -- Crate-Graph Decomposition: Per-S-Move Executable Specs (S1-S8)

This is the executable detailing of [21b](21b_crate_graph_blueprint.md)'s ranked sequence S1-S8.
21b proved the target graph (`molt-ir <- molt-passes <- molt-lower`, per-backend crates +
`molt-codegen-abi`, thin `molt-backend` driver) from the measured `use`-edge DAG. This doc gives
each move the same fidelity 21a gave the `fc/` function-split and 21e gave the satellite dedup:
the exact file partition, the `Cargo.toml` dep+feature wiring, the precise visibility-widening
method, the per-commit gate set, and the parallelization map. Read 21b first for the *why*; this
doc is the *how*, mechanical enough to hand to N agents.

## 0. PREFACE -- live-state delta from 21b (verified at HEAD; absorb before executing)

21b was authored against "molt-tir extracted, M1 in flight." Eight facts about the LIVE tree
change the mechanics. Absorb all eight.

1. **T1 moved the ENTIRE TIR layer into `molt-tir`, not just the vocabulary.** `runtime/molt-tir/`
   is a live crate (`runtime/molt-tir/Cargo.toml`, in the **root** `Cargo.toml` members list).
   Its `src/` holds `passes.rs` (6,534), `representation_plan.rs` (6,452), `ir.rs`/`ir_schema.rs`/
   `json_boundary.rs`, the leaves (`intrinsic_symbols`/`process_diagnostics`/`debug_artifacts`),
   AND the whole `tir/` subtree (vocab + `tir/passes/*` 56,360 LOC + lowering). So S1 and S2 do
   NOT operate on the old monolith -- **they split the EXISTING `molt-tir` crate three ways**
   (`molt-ir` <- `molt-passes` <- `molt-lower`). 21b's S1 prose ("Split molt-tir -> molt-ir ...
   molt-tir keeps passes+lowering") maps onto: S1 lifts vocab OUT of `molt-tir` into a new
   `molt-ir`; S2 renames the residual `molt-tir` into `molt-passes` + `molt-lower`. The molt-tir
   crate name is RETIRED by end of S2 (or kept as a deprecated re-export shell -- see S2.6).

2. **The real workspace root is `<repo-root>/Cargo.toml`**, NOT `runtime/Cargo.toml` (the latter
   is a stale secondary manifest with a different, shorter members list -- do not edit it; every
   new crate is added to the ROOT members list). New crates live under `runtime/`
   (`runtime/molt-ir`, `runtime/molt-passes`, ...).

3. **The precise-visibility + `test-util` lesson is ALREADY APPLIED at the molt-backend->molt-tir
   seam -- it is the template, not a new invention.** `molt-tir/Cargo.toml` already has
   `test-util = []` (exposes `#[cfg(any(test, feature = "test-util"))]` accessor seams across the
   boundary) and per-feature gates (`native-backend`, `llvm`, `wasm-backend`) that molt-backend
   activates via `molt-tir/<feature>`; `molt-backend/[dev-dependencies]` re-imports
   `molt-tir = { features = ["test-util"] }`. **Every S-move replicates THIS exact pattern**
   (feature passthrough + a `test-util` for cross-crate `#[cfg(test)]` accessors), it does not
   design a new one.

4. **The matches!-oracle hazard 21b flags for S1 is ALREADY STRUCTURALLY KILLED.** The op-effect
   oracles in `molt-tir/src/tir/passes/effects.rs` (`opcode_may_throw` :100, `opcode_is_side_effecting`
   :135, `opcode_is_pure_may_throw` :294) now delegate to
   `crate::tir::op_kinds_generated::*_table(opcode)` -- a GENERATED EXHAUSTIVE `match` over the
   `OpCode` enum with **no wildcard arm** (doc 25 op-kind registry; `tir/op_kinds.toml` is the
   single authority). A new `OpCode` fails to compile until classified. So the S1 "audit as ops
   cross the boundary" is NOT a manual grep-hunt for `matches!(...)` default-false bugs -- it is a
   **gate confirming `op_kinds_generated` + its consuming oracles travel together into `molt-ir`
   and the exhaustive match still compiles** (G-oracle below). This is the doctrine's
   "drift-uncompilable" invariant already in force; S1 must not regress it.

5. **`representation_plan.rs` is ONE 6,452-line file that straddles the molt-ir / molt-lower
   boundary** and must be PHYSICALLY PARTITIONED (T1 only re-exported `Repr` via `crate::Repr`;
   the file never split). Vocab-level enums are interleaved with plan logic: `ScalarKind` (:780),
   `ContainerKind` (:790), `ContainerStorageKind` (:800), `ContainerStorageFact` (:806), **`Repr`
   (:838)** are vocab; the heavy logic (`ScalarRepresentationPlan` :1018, `ScalarPrimaryNameSets`
   :1051, `LlvmReprFacts` :1073, `repr_by_value_for`, `value_range_for`,
   and value-keyed raw-int projection into native `repr_by_name`) consumes
   passes. Resolution (refines 21b flag #5): the 5 vocab enums -> `molt-ir/src/repr.rs`; the
   residual `representation_plan.rs` (plan logic) -> **molt-lower** in S2.

6. **M1's `fc/` family extraction is live and complete for the planned families** (39 handlers in
   `native_backend/function_compiler/fc/`; `compile_func_inner` ~3,314 lines). S7 (native
   extraction) inherits this tree intact -- it moves `native_backend/` *as a subtree* and does
   NOT re-open any `fc/` move. The `use super::*` -> explicit-import rewrite (S7) operates at the
   `native_backend/mod.rs` ancestry root, above `fc/`'s own `use super::super::*` chain.

7. **The NaN-box ABI is split TWO ways by type, refining 21b flag #3 / S3.** ABI-portable
   (scalar, no Cranelift types) -> `molt-codegen-abi`: the raw consts in
   `native_backend_consts.rs` (`QNAN`, `TAG_*`, `INT_*`, `POINTER_MASK`, `CANONICAL_NAN_BITS`,
   header/layout offsets), the scalar `box_int(i64)->i64` (`simple_backend.rs:814`), the
   `NanBoxConsts` *struct* (a plain field-bag of `i64`s; `NanBoxConsts::new` takes `_builder` but
   ignores it -- de-Cranelift it to `NanBoxConsts::new()`), and `pending_bits()->i64` +
   `stable_ic_site_id(...)->i64` (loose in `lib.rs:76`/:81). **Cranelift-typed helpers STAY in
   molt-backend-native**: `unbox_int` (`simple_backend.rs:844`), `unbox_int_or_bool` (:873),
   `box_int_value` (:1100) take `&mut FunctionBuilder` / `Value` and MUST NOT enter a
   molt-ir-only ABI crate (would drag `cranelift` into it). `wasm.rs:19-28` duplicates the const
   subset (plus its own extras like `INT_MIN_INLINE` at :30 that are NOT shared) -- S3 dedups
   ONLY the shared subset.

8. **In-flight git work is the perf/docs lane (disjoint).** `git status` shows only
   `tools/perf_*`, `docs/perf/`, `64_*.md` modified -- no Rust crate files dirty. The swarm is
   *between* crate moves: S1 may start on a clean backend tree. (Coordinate only with the LLVM
   lane for S4/S7, per 21b -- verify no `llvm_backend/*` editor is active at S4 start.)

9. **Per-function cached TIR optimization has ONE live authority.** `runtime/molt-tir/src/tir/pipeline_cache.rs`
   owns SimpleIR->TIR cache keying, batching, warm-hit restoration, cold-miss optimization,
   artifact encoding, LIR verification policy, and index persistence for both native and WASM.
   Backends pass target policy plus optional pre-lowering hooks and consume optimized TIR custody;
   they must not open `CompilationCache` directly for this pipeline. Native, LLVM, and WASM cache
   flavors use explicit schema salts and include target kind, optimization profile, target-cost
   fields, OS, OS family, architecture, pointer width, and endianness in the hash body before they
   store serialized optimized `TirFunction`s. WASM LIR fast outputs are derived from final
   surviving SimpleIR functions, not from a pre-final cached side channel.

10. **Backend-neutral SimpleIR rewrite policy is upstream of backend extraction.** `runtime/molt-tir/src/ir_rewrites.rs`
    owns phi-store lowering, try/except elision, copy-alias rewriting, and annotation-stub
    rewriting. `molt-backend` imports these passes from `molt-tir`; it must not grow a replacement
    `ir_rewrites` module or re-export shim when native/LLVM move into backend leaf crates.

### 0.1 Doctrine binding (the checklist this doc answers, per DESIGN_DOCTRINE.md)
- **Killer retired:** the incremental-build killer (a TIR-pass edit rebuilding all 5 backends; a
  185K-line monolith CU). After S8: a TIR-pass edit recompiles `molt-passes` + relinks; a backend
  edit recompiles ONE backend; N agents own disjoint crates. (Doctrine #1.)
- **Pythonista-rustacean:** every move is byte-identical artifact-preserving (G3) -> exact CPython
  semantics across every backend are untouched (the parity contract is invariant under a
  move-only refactor). The Rustacean win is structural: the layering DAG becomes a compiler-
  enforced crate boundary (a cycle is uncompilable), and the matches!-oracle exhaustiveness
  (fact #4) is preserved as a gate -- drift stays uncompilable. (Doctrine #2, both lenses.)
- **One authority per invariant:** `op_kinds.toml` -> `op_kinds_generated` stays the single
  op-effect authority inside `molt-ir`; the NaN-box ABI gets ONE home (`molt-codegen-abi`),
  killing the `wasm.rs` duplicate copy (a dual-maintenance god-seam).

### 0.2 The universal move-only contract (every S-move, every commit)
**PURE RENAME for moved files** (the crate-extraction-precise-visibility lesson): move each file
with `git mv old new` (or, when lifting a subset, `git show HEAD:old > new` then trim the source
-- preserving git blame/line-history). Widen visibility for ONLY the items the dependent crate's
build actually names as private (read the `error[E0603]: ... is private` / `E0432 unresolved
import` list; widen exactly those, minimal scope: prefer `pub(crate)` -> the narrowest `pub` that
satisfies the *named* consumer, NOT a blanket `pub(crate)->pub` sweep). Add a `test-util` feature
to each new crate for cross-crate `#[cfg(test)]` accessors (mirroring molt-tir, fact #3). Each
S-move is staged as its own move-only commit, G1-G7 green before the next starts.

---

## 1. TARGET vs LIVE crate map (what each move produces)

```
LIVE (post-T1, mid-decomposition):            TARGET (post-S8):
  molt-tir  (vocab+passes+lowering+repr_plan)   molt-ir            (vocabulary, 0 workspace deps)
  molt-backend (5 backends + driver + daemon)     ^- molt-passes   (transforms+analyses+orchestration)
                                                       ^- molt-lower (lowering + repr-plan logic + ir_rewrites)
                                                            ^- molt-codegen-abi (NaN-box ABI; deps molt-ir only)
                                                            ^- molt-backend-llvm
                                                                  ^- molt-backend-native (deps llvm, opt)
                                                            ^- molt-backend-wasm
                                                            ^- molt-backend-luau
                                                            ^- molt-backend-rust
                                                                  ^- molt-backend (thin driver + daemon bin)
```
Build order (topological): `molt-ir -> molt-passes -> {molt-lower, molt-codegen-abi (needs only
molt-ir)} -> {molt-backend-llvm -> molt-backend-native, molt-backend-wasm, molt-backend-luau,
molt-backend-rust} -> molt-backend`.

---

## 2. The gate set (G1-G7 + G-oracle) -- applied to EVERY S-move commit

Inherits the 34e3bddbf / 21a section-5 / 21 section-3 methodology (isolated `CARGO_TARGET_DIR`,
CI-exact: NO `--lib` on the clippy gate so tests compile too -- the "build != test for warnings"
lesson). Three gates are NEW for the crate-graph arc (G6 cross-crate-surface snapshot, G7
cargo-tree feature audit, G-oracle the matches!-exhaustiveness gate).

```
export MOLT_SESSION_ID="<unique>" && export CARGO_TARGET_DIR="$PWD/target-<smove>"
```
- **G1 -- 0-warning build, EVERY feature config the move touches.** For S1/S2 (the IR/pass/lower
  split, feature-gated TIR code): build the new crate under each of `--no-default-features`,
  `--features native-backend`, `--features "native-backend llvm"`, `--features wasm-backend`
  (these gate conditional TIR code; `wasm-encoder` is owned by `molt-backend-wasm`). For
  S3-S8: `cargo clippy -p <crate> --features <each> -- -D warnings` AND `cargo clippy -p
  molt-backend --features native-backend -- -D warnings` + `--features "native-backend llvm"
  --lib` (the llvm gate) + `--features wasm-backend`. NO new warnings vs the pre-move set.
- **G2 -- lib tests, every config.** `cargo test -p <new-crate> --lib` and `cargo test -p
  molt-backend --features native-backend --lib` (baseline ~983 + the M1 in-file tests) +
  `--features "native-backend llvm" --lib llvm_backend::lowering`. Tests that reach moved
  crate-private seams compile via the new crate's `test-util` dev-dep (fact #3).
- **G3 -- byte-identical artifacts (THE move-only proof).** Before & after, compile a fixed `.py`
  corpus to each affected target via the guarded harness (`python -m molt build --target {native,
  wasm,luau,rust} --rebuild`) + capture stderr diagnostics; `diff` the `.o`/`.wasm`/`.luau`/`.rs`
  + diagnostics -> MUST be byte-identical. Any diff => a body changed (or a crate boundary
  perturbed inlining/ordering) => reject and find the leak.
- **G4 -- differential parity e2e.** `python -m molt test` (guarded; never a raw binary) on the
  fib/bigint/generator/exception/dict/list/async differential subset vs CPython -- identical
  output. For backend moves (S4-S7), run the subset on THAT backend.
- **G5 -- symbol/diagnostic identity.** `nm`/`llvm-nm` the rlib before/after the move -- no NEW
  exported symbols beyond the minimal widened surface (G6 owns the budget); the C-ABI / no_mangle
  surface molt-runtime links is unchanged. Embedded panic/diagnostic message literals move
  verbatim.
- **G6 -- cross-crate-surface snapshot (NEW; the precise-visibility ratchet).** After the move,
  record the new crate's `pub` surface (`cargo public-api -p <crate>` if available, else
  `nm`-grep `pub` items in `lib.rs` + count `pub ` items under `src/`). The count must equal
  exactly the enumerated consumed-surface list in this move's spec -- no more. A `pub` not named
  by a downstream consumer is pub-creep => reject. (21 risk register "pub-boundary creep" made a
  gate.)
- **G7 -- cargo-tree feature audit (NEW).** `cargo tree -e features -p molt-backend --features
  <set>` before/after the move -- no transitive feature silently flips (the feature-unification
  trap). Especially: confirm `molt-codegen-abi` does NOT pull `cranelift`/`inkwell`/`wasm-encoder`
  (it must depend on `molt-ir` ONLY), and that a `wasm-backend`-only build does not link
  `molt-backend-native`/`-llvm`.
- **G-oracle -- matches!-exhaustiveness (NEW; S1 + any move that touches `OpCode`/`op_kinds`).**
  Confirm `op_kinds_generated` + `effects.rs`'s `opcode_*_table` consumers compile with the
  generated EXHAUSTIVE match intact (no wildcard arm introduced to paper over a cross-crate path
  break). Grep the moved crate for any NEW `matches!(op.kind` / `matches!(opcode` introduced to
  dodge a privacy error -- there must be none. (Fact #4; doctrine drift-uncompilable.)

**A commit is not done until G1-G7 + G-oracle (where applicable) pass.**

---

## 3. S1 -- Lift the vocabulary: `molt-tir` -> new `molt-ir`

**Goal:** create `molt-ir` (the zero-workspace-dep data model = the DAG fixpoint), lift the
vocabulary + SimpleIR transport + `Repr`(+vocab enums) + std-leaves out of `molt-tir`; `molt-tir`
keeps passes+lowering+repr-plan-logic and gains `molt-ir = { path = "../molt-ir" }`.

### 3.1 Exact file partition (which files move to `runtime/molt-ir/src/`)
Pure renames via `git mv` (these are whole-file moves -- highest fidelity):
- **From `molt-tir/src/tir/` -> `molt-ir/src/tir/`** (the vocabulary; 21b Layer-0 list):
  `types.rs` (650), `ops.rs` (347), `op_kinds_generated.rs` (3,906), `op_kinds.toml` (the
  generator input -- moves with it), `effect_proof.rs` (the validated effect-proof vocabulary
  consumed by SimpleIR schema and TIR passes), `values.rs` (20), `blocks.rs` (176), `function.rs`
  (336), `cfg.rs` (1,453), `dominators.rs` (721), `ssa.rs` (3,676), `serialize.rs`, `printer.rs`
  (879), `verify.rs` (1,210). Plus the `tir/mod.rs` `is_structural` helper (:52) + the primary-
  type re-exports (:82-99) -- but mod.rs is SHARED with S2's modules, so S1 moves the vocab
  `pub mod` lines + re-exports and LEAVES the passes/lowering `pub mod` lines as
  `pub use molt_ir::tir::{...}` shims until S2 (see 3.4).
- **From `molt-tir/src/` -> `molt-ir/src/`** (SimpleIR transport + leaves): `ir.rs` (679),
  `ir_schema.rs` (266), `json_boundary.rs` (145), `intrinsic_symbols.rs` (70),
  `process_diagnostics.rs` (44), `stdlib_module_symbols.rs` **(currently in molt-backend/src --
  21b lists it as a molt-ir std-leaf; move it here in S1; molt-backend's `pub use
  crate::stdlib_module_symbols::*` at lib.rs:52 becomes `pub use molt_ir::stdlib_module_symbols::*`)**.
  `debug_artifacts.rs` (98) -- keep in molt-ir (consumed by passes + backends; zero deps).
- **The `representation_plan.rs` vocab subset -> `molt-ir/src/repr.rs` (NOT a whole-file move;
  a `git show HEAD:representation_plan.rs > ...` extract of the 5 vocab items, fact #5):**
  `ScalarKind` (:780), `ContainerKind` (:790), `ContainerStorageKind` (:800),
  `ContainerStorageFact` (:806), `Repr` (:838). `molt-ir/src/lib.rs` re-exports `pub use
  crate::repr::Repr;` to mirror the current `crate::Repr` path (molt-tir/lib.rs:30). The residual
  `representation_plan.rs` (the plan LOGIC) stays in `molt-tir` for now and imports `use
  molt_ir::repr::{Repr, ScalarKind, ContainerKind, ...}`; it migrates to molt-lower in S2.

`molt-ir/src/lib.rs` = a pure rename of the vocab-relevant lines of the current
`molt-tir/src/lib.rs` (the `#![allow]`s, `pub mod {tir, ir, ir_schema, json_boundary,
intrinsic_symbols, process_diagnostics, debug_artifacts}`, `pub use crate::ir::{...}`, `pub use
crate::repr::Repr`, `MOLT_CLOSURE_PARAM_NAME`). New `pub mod repr;`. New `pub mod
stdlib_module_symbols;`.

**Landed pre-S1 cleanup:** `effect_proof.rs` now owns `EffectProof`,
`simple_ir_effect_proof`, and the TIR proof recognizers outside `tir::passes`. This removes the
old upward dependency from `ir_schema.rs` into the pass module; S1 must move `effect_proof.rs`
with the vocabulary and make only the downstream-consumed proof API public across the new crate
boundary. `printer.rs` is now TIR-only; LIR formatting lives in `lir_printer.rs` and must stay
with the residual lower/LIR layer until S2 moves it. `repr.rs` now owns the representation
vocabulary (`ScalarKind`, `ContainerKind`, `ContainerStorageKind`, `ContainerStorageFact`, and
`Repr`); `representation_plan.rs` is planner logic only and consumes that vocabulary from the
leaf module until S2 moves the residual planner into `molt-lower`.

### 3.2 Cargo.toml + feature wiring
- New `runtime/molt-ir/Cargo.toml`: `[dependencies]` = `serde`, `serde_json`, `rmp-serde`, `libc`,
  `rayon` (mirror molt-tir's current deps MINUS `wasm-encoder` UNLESS a moved vocab file needs it
  -- `serialize.rs`/`ir.rs` do not; verify with G1 `--no-default-features`). `[features]`:
  `default=[]`, `native-backend=[]`, `llvm=[]`, `wasm-backend=[]` (passthrough gates, mirroring
  molt-tir fact #3 -- needed because moved vocab has `#[cfg(feature=...)]` lines), `test-util=[]`.
- `molt-tir/Cargo.toml`: add `molt-ir = { path = "../molt-ir" }`; in `[features]` forward each
  gate: `native-backend = ["molt-ir/native-backend"]`, etc.; `test-util = ["molt-ir/test-util"]`.
- ROOT `Cargo.toml`: add `"runtime/molt-ir"` to `members`.

### 3.3 Visibility-widening method (minimal cross-crate surface)
Within molt-ir, `tir/mod.rs` items used cross-file were `pub(crate)` (e.g. `is_structural` :52);
they stay `pub(crate)` (same crate now = molt-ir). The cross-crate widening is ONLY what
`molt-tir`'s build (passes+lowering+repr-plan) now names from molt-ir. Method: do the `git mv`,
build `molt-tir`, read the `E0603 is private`/`E0432` list, widen EXACTLY those. Expected set
(grep-derived from current `crate::tir::*` consumers in passes/lowering): the `tir/mod.rs`
re-exports (`BlockId/Terminator/TirBlock`, `TirFunction/TirModule`, `OpCode/TirOp/AttrDict/
AttrValue/Dialect`, `FuncSignature/TirType`, `TirValue/ValueId`) -- NOTE several mod.rs re-exports
(`ExceptionRegions`, `CallFacts`, `call_graph`, `module_phase`) are PASSES per 21b and move in S2,
so their re-exports stay in molt-tir; in S1 only the *vocab* names widen -- the
`op_kinds_generated::*_table` fns (consumed by `effects.rs`, a pass -> cross-crate in S1: widen
`pub(super)` -> `pub`), `serialize::{serialize_ops, deserialize_ops}`, `printer::print_function`,
`dominators::executable_reachable_blocks`. **G6 budgets this list (~25-30 items).**

### 3.4 The mod.rs straddle (S1<->S2 handoff)
`molt-tir/src/tir/mod.rs` declares both vocab modules (move to molt-ir) and passes/lowering
modules (stay for S2). S1 leaves a `molt-tir/src/tir/mod.rs` that does `pub use
molt_ir::tir::{<vocab modules + their re-exports>};` (so `crate::tir::types::*` etc. resolve
unchanged for the still-resident passes/lowering) and keeps `pub mod {passes, lower_*, lir, ...}`
for the residents. S2 dissolves this shim.

### 3.5 Gates / parallelization
Full G1-G7 + **G-oracle is load-bearing here** (op_kinds_generated + effects oracle cross the
boundary). S1 is the **foundation gate** -- nothing else can start (S2 needs molt-ir; S3 needs
molt-ir). Single-agent, serial.

---

## 4. S2 -- Split the residual: `molt-tir` -> `molt-passes` + `molt-lower`

**Goal:** dissolve `molt-tir` into `molt-passes` (the ~40 transforms + analyses + orchestration,
deps molt-ir) and `molt-lower` (TIR->{LIR,SimpleIR,WASM-IR} lowering + repr-plan logic +
`ir_rewrites`, deps molt-passes). `ir_rewrites.rs` MIGRATES IN from molt-backend (21b flag #4).

### 4.1 Exact file partition
**-> `runtime/molt-passes/src/` (deps `molt-ir`; 21b Layer-1):**
- `passes.rs` (6,534) [the SimpleIR passes] -> `molt-passes/src/passes.rs` (or `lib.rs` root).
- The whole `tir/passes/` dir (56,360 LOC, ~45 files) -> `molt-passes/src/passes_tir/` (rename
  the dir to avoid colliding with `passes.rs`; internal `crate::tir::passes::X` paths -> module
  paths within molt-passes).
- `tir/analysis/` (`mod.rs`) -> `molt-passes/src/analysis/`.
- `tir/pass_manager.rs` (1,040), `tir/module_phase.rs` (725) [orchestration].
- The analyses/facts 21b assigns to passes: `tir/call_facts.rs` (1,050), `tir/call_graph.rs`
  (924), `tir/type_refine.rs` (3,650 -- it consumes passes (6 refs) and is consumed by passes +
  lowering; it is an analysis -> passes layer, fact verified),
  `tir/exception_regions.rs` (2,249), `tir/drop_phase.rs` (631), `tir/parallel.rs` (249),
  `tir/cache.rs` (1,001 -- 0 passes/lower refs but assigned to passes by 21b; it is the
  compilation cache the pass pipeline drives), `tir/bolt.rs` (203).

**-> `runtime/molt-lower/src/` (deps `molt-passes`, transitively molt-ir; 21b Layer-2):**
- `tir/lower_from_simple.rs` (1,408), `tir/lower_to_simple.rs` (8,187), `tir/lower_to_lir.rs`
  (1,095), `tir/lower_to_wasm.rs` (2,469), `tir/lir.rs` (143), `tir/verify_lir.rs` (1,489),
  `tir/verify_lir_repr.rs` (184), `tir/target_info.rs` (609 -- 0 passes/lower refs; 21b assigns
  to lower as the target/profile descriptor lowering consumes), `tir/mlir_compat.rs` (464),
  `tir/wasm_component.rs` (104), `tir/wasm_split.rs` (146), `tir/wasm_streaming.rs` (112).
- **`representation_plan.rs` (residual ~6,000 lines after S1's vocab extract) -> `molt-lower/src/
  representation_plan.rs`** -- the plan LOGIC (`ScalarRepresentationPlan`, `LlvmReprFacts`,
  `repr_by_value_for`, `value_range_for` which calls `passes::{value_range,scev}`,
  and native `repr_by_name` projection from those value-keyed facts). This is WHY
  it sits above passes (hard production dep).
- **`ir_rewrites.rs` (597) MIGRATES IN from `molt-backend/src/`** (21b flag #4: it deps only
  `ir`+`representation_plan`+`passes::SimpleIrScalarPurityFacts`). molt-backend's `mod ir_rewrites;`
  + `pub use crate::ir_rewrites::{...}` (lib.rs:19-23) become `pub use
  molt_lower::ir_rewrites::{elide_useless_try_blocks, elide_useless_try_blocks_for_function,
  rewrite_annotate_stubs, rewrite_copy_aliases, rewrite_phi_to_store_load};`.

### 4.2 The test-only passes->lowering refs (the 21b/S1 mechanical cost -- DO NOT re-introduce a cycle)
21b's central S1/S2 mechanical task: passes->lowering edges are 100% test-only and must not
become a production molt-passes->molt-lower back-edge (that would make the DAG cyclic). Verified
live refs (all under `#[cfg(test)] mod tests`):
- `molt-tir/src/tir/passes/drop_insertion.rs:6223` `use crate::tir::lower_from_simple::lower_to_tir;`
- `molt-tir/src/tir/passes/loop_unroll.rs:1535,1621,1884` `crate::tir::lower_to_simple::lower_to_simple_ir(...)`.
**Resolution:** a `[dev-dependencies] molt-lower` on molt-passes is a CYCLE (lower deps passes).
So **relocate these specific round-trip tests OUT of molt-passes INTO molt-lower's test suite**
(they test the passes<->lowering round-trip, which is properly an integration test at the
molt-lower layer -- `molt-lower/tests/` or the `#[cfg(test)]` of `lower_to_simple.rs`/`lower_from_simple.rs`).
The pass file keeps its pure-pass unit tests. This keeps molt-passes free of ANY lowering dep
(production OR dev) and the DAG strictly acyclic -- the precise-visibility lesson's "test-util for
cross-crate `#[cfg(test)]` accessors" applied to the direction that would otherwise re-create a
cycle. (If a handful of assertions genuinely need a pass-private accessor from the relocated test,
expose it via molt-passes `test-util` and import it from molt-lower's dev-dep on molt-passes.)

### 4.3 Cargo.toml + feature wiring
- `runtime/molt-passes/Cargo.toml`: deps `molt-ir = { path = "../molt-ir" }` + `serde`,
  `serde_json`, `rmp-serde`, `rayon`, `sha2` (cache.rs hashing), `libc`. Features: passthrough
  (`native-backend=["molt-ir/native-backend"]`, etc.), `test-util=["molt-ir/test-util"]`.
- `runtime/molt-lower/Cargo.toml` was superseded by the landed
  `molt-tir`/`molt-backend-wasm` split: TIR/LIR lowering remains in
  `molt-tir`, while `lower_to_wasm` and `wasm-encoder` live in
  `molt-backend-wasm`. Features pass through conditional TIR code without
  pulling backend instruction encoders into `molt-tir`.
- `molt-backend/Cargo.toml`: replace `molt-tir = {...}` with `molt-lower = { path =
  "../molt-lower" }` (it transitively re-exports molt-passes + molt-ir). Forward features to
  `molt-lower/<feature>`. `[dev-dependencies]`: `molt-lower = { features = ["test-util"] }`.
  Remove `mod ir_rewrites;` (migrated).
- `molt-backend/src/lib.rs`: the big re-export block (`pub use molt_tir::{...}` :11-14, `pub use
  molt_tir::passes::{...}` :43-51, `pub use molt_tir::repr::Repr` :61, `pub use
  molt_tir::MOLT_CLOSURE_PARAM_NAME` :55) re-points to `molt_lower::` / `molt_passes::` /
  `molt_ir::` as appropriate. To MINIMIZE churn in main.rs/wasm.rs (which reach the layer via
  `crate::tir::*`/`crate::passes::*` re-exports), have `molt-lower/src/lib.rs` re-export
  molt-passes + molt-ir at its root (`pub use molt_passes::*; pub use molt_ir::*;` selectively),
  so a single `molt_lower::{...}` path mostly suffices. ROOT members: add `"runtime/molt-passes"`,
  `"runtime/molt-lower"`; the `molt-tir` entry is removed (or kept as a shell -- 4.6).

### 4.4 Visibility-widening method
Two boundaries open: molt-passes->molt-ir (already widened in S1) and molt-lower->molt-passes
(NEW). Build molt-lower, read the `E0603`/`E0432` list, widen exactly what molt-lower names from
molt-passes. The production edges are known: `passes::value_range::ValueRangeResult` (consumed by
`lower_to_lir.rs:63,175,198,210,312`, `lower_to_wasm.rs:374`, `representation_plan.rs:1240,1278,
1303,1332`), `passes::value_range::{compute_value_range, copy_value_source}`,
`passes::scev::compute_scev`, `passes::drop_insertion::{DROP_INSERTED_ATTR,
EXCEPTION_REGION_DROPS_INSERTED_ATTR}` (consumed by `lower_to_simple.rs:305-319,3664-3683` +
`lower_from_simple.rs:90-133`), `passes::SimpleIrScalarPurityFacts` (consumed by ir_rewrites).
Widen `pub(super)`/`pub(crate)` on EXACTLY these to `pub`. G6 budgets it.

### 4.5 Gates / parallelization
Full G1-G7. **G3 is the highest-risk gate** here (lowering is where artifact bytes are decided --
a crate boundary that perturbs inlining of a hot lowering fn could shift `.o` bytes; if G3 diffs,
inspect for an `#[inline]` that needs `#[inline(always)]` across the new boundary, and document
any proven-identical exception). Serial after S1. Single logical move but stage as TWO commits:
**S2a** create molt-passes (move passes/analyses/orchestration; molt-tir temporarily keeps
lowering+repr-plan and deps molt-passes), **S2b** create molt-lower (move lowering + repr-plan
logic + ir_rewrites; dissolve molt-tir). Each commit independently green.

### 4.6 molt-tir disposition
After S2b, `molt-tir` has no files. RECOMMENDED: remove the crate + its root members entry +
molt-backend dep (cleanest; the build order has no molt-tir node). ALTERNATIVE (only if some
out-of-tree tooling pins `molt_tir::`): a 3-line `molt-tir/src/lib.rs` `pub use molt_lower::*; pub
use molt_passes::*; pub use molt_ir::*;` deprecation shell. Decision: REMOVE it -- grep confirms
the only `molt_tir`/`molt-tir` references are molt-backend (re-pointed in S2) + the stale
runtime/Cargo.toml (not the build root) -- no external pins.

---

## 5. S3 -- Extract `molt-codegen-abi` (the shared NaN-box ABI)

**Goal:** a tiny crate (deps `molt-ir` ONLY) holding the ABI-portable NaN-box vocabulary that 3
backends share (native 48x, llvm 29x, wasm 34x per 21b), killing the `wasm.rs` duplicate copy.

### 5.1 Exact partition (fact #7 -- by TYPE, not by file)
**-> `runtime/molt-codegen-abi/src/lib.rs`:**
- All consts from `molt-backend/src/native_backend_consts.rs` (pure rename of the file body;
  re-export at `pub` -- they were `pub(super)`). The header offsets pin
  `molt-runtime/src/object/layout.rs` -- keep that doc comment verbatim.
- The scalar `box_int(val: i64) -> i64` (`simple_backend.rs:814-819`) -- moves verbatim; it uses
  only `QNAN|TAG_INT|INT_MASK` (now local to this crate).
- The `NanBoxConsts` struct (`simple_backend.rs:23-53`) + its `new` (de-Cranelift: change `fn
  new(_builder: &mut FunctionBuilder)` -> `fn new()`, dropping the unused `_builder`; ALL call
  sites `NanBoxConsts::new(&mut builder)` -> `NanBoxConsts::new()` -- a mechanical sweep,
  G3-checked). Its fields are all `i64` derived from the consts.
- `pending_bits() -> i64` (`lib.rs:76-78`) and `stable_ic_site_id(func, op_idx, lane) -> i64`
  (`lib.rs:81-99`) -- pure scalar/FNV; move verbatim.

**STAYS in molt-backend-native (Cranelift-typed -- do NOT move; would pull cranelift into the ABI
crate, violating G7):** `unbox_int` (:844), `unbox_int_or_bool` (:873), `box_int_value` (:1100),
and every `*_value` helper taking `&mut FunctionBuilder`/`Value`. These import the consts from
`molt-codegen-abi` post-S7.

### 5.2 The wasm de-dup (21b G3 byte-identical, the duplication-kill)
`wasm.rs:19-28` redefines the const subset (`QNAN, CANONICAL_NAN_BITS, TAG_INT, TAG_BOOL,
TAG_NONE, TAG_PTR, TAG_PENDING, TAG_MASK, POINTER_MASK`). Replace the block with `use
molt_codegen_abi::{QNAN, CANONICAL_NAN_BITS, TAG_INT, TAG_BOOL, TAG_NONE, TAG_PTR, TAG_PENDING,
TAG_MASK, POINTER_MASK};`. **KEEP wasm.rs's NON-shared consts in place** (`QNAN_TAG_MASK_I64` :26,
`QNAN_TAG_PTR_I64` :27 -- derived; verify `INT_MASK`/`INT_SHIFT` :28-29 against
`native_backend_consts` -- they ARE there, so import them too; `INT_MIN_INLINE` :30 is wasm-only
-> keep local). **Gate: the deduped consts must be byte-identical to the originals** (they are --
both define `QNAN = 0x7ff8_..`); G3 proves the emitted `.wasm` is unchanged.

### 5.3 Cargo.toml / wiring
- `runtime/molt-codegen-abi/Cargo.toml`: deps `molt-ir = { path = "../molt-ir" }` ONLY (~300 LOC
  crate; NO cranelift/inkwell/wasm-encoder -- G7 enforces). Features: none needed (consts are
  unconditional); add `test-util=[]` for symmetry.
- `molt-backend/Cargo.toml`: add `molt-codegen-abi = { path = "../molt-codegen-abi" }`. Delete
  `mod native_backend_consts;` + `use native_backend_consts::*;` (lib.rs:37-40); `pending_bits`
  moves out so lib.rs:75-78 is deleted (callers import `molt_codegen_abi::pending_bits`); replace
  crate-root NaN-box references with `use molt_codegen_abi::*;`. ROOT members: add
  `"runtime/molt-codegen-abi"`.

### 5.4 Gates / parallelization
Full G1-G7; **G7 critical** (prove molt-codegen-abi pulls only molt-ir; prove no cranelift leak).
**[parallel] with S2** -- it only needs `molt-ir` (S1), touches `native_backend_consts.rs` +
`lib.rs` + `wasm.rs` + `simple_backend.rs` (the const/helper defs), which S2 does not move. BUT it
edits `molt-backend/lib.rs` + `Cargo.toml` (shared with S2/S8) -> serialize the `lib.rs`/`Cargo.toml`
dep-line edits against whichever of S2/S3 commits first (rebase the dep-line edits; the file MOVES
are independent). Single agent.

---

## 6. S4 -- Extract `molt-backend-llvm`

**Goal:** the LLVM codegen leaf (zero edges INTO it; `llvm->native` is zero, verified -- only a
doc comment). Deps `molt-lower` + `molt-codegen-abi` + `inkwell`(opt). BEFORE native.

### 6.1 Exact partition
`git mv` `molt-backend/src/llvm_backend/` (whole dir: `mod.rs`, `lowering.rs` 10,656, `types.rs`,
`pgo.rs`, `runtime_imports.rs`) -> `runtime/molt-backend-llvm/src/` (the dir files become the
crate modules; `llvm_backend/mod.rs` -> `molt-backend-llvm/src/lib.rs`, or keep `mod.rs` + a thin
`lib.rs`). Internal `crate::tir::*`/`crate::representation_plan::*`/`crate::passes::*` paths ->
`molt_lower::{...}` / `molt_passes::{...}` / `molt_ir::{...}`; the NaN-box consumers
(`stable_ic_site_id`, `pending_bits`, `QNAN`, consts) -> `molt_codegen_abi::{...}`; the repr-plan
facts (`Repr`, `LlvmReprFacts`, `ContainerKind`) -> `Repr`/`ContainerKind` from `molt_ir` (vocab)
+ `LlvmReprFacts` from `molt_lower::representation_plan` (plan logic). (38 NaN-box refs in
`lowering.rs`.)

### 6.2 Cargo.toml / wiring
- `runtime/molt-backend-llvm/Cargo.toml`: deps `molt-lower`, `molt-codegen-abi`, `inkwell =
  { version = "0.8", features = ["llvm21-1"], optional = true }`, `serde`/`serde_json` as needed.
  Features: `llvm = ["dep:inkwell", "molt-lower/llvm"]`, `polly=["llvm"]`,
  `test-util=["molt-lower/test-util"]`.
- `molt-backend/Cargo.toml`: `molt-backend-llvm = { path = "../molt-backend-llvm", optional =
  true }`; `llvm = ["dep:molt-backend-llvm", "molt-backend-llvm/llvm", "molt-lower/llvm"]`.
- `molt-backend/src/lib.rs`: `#[cfg(feature="llvm")] pub mod llvm_backend;` (:24-25) -> `#[cfg(
  feature="llvm")] pub use molt_backend_llvm as llvm_backend;` (preserves the `crate::llvm_backend::*`
  path `simple_backend.rs:3317,3324,3531,3547` uses for the native->llvm dispatch).

### 6.3 Visibility / gates / parallelization
Widen only what `simple_backend.rs`'s `#[cfg(feature="llvm")]` block names: `LlvmBackend`,
`MoltOptLevel`, `runtime_imports::declare_runtime_functions`, `lowering::declare_tir_function`,
`lowering::try_lower_tir_to_llvm` (`simple_backend.rs:3317-3547`). G6 budgets these ~5 items.
Full G1-G7; **G5 (symbol identity) load-bearing** (LLVM emits an `.ll`/object surface). **[seq:
S2,S3]; [parallel] with S5/S6.** BEFORE S7 (native deps llvm). **Coordinate the active LLVM lane**
(21b/21 section-0.3): verify no `llvm_backend/*` editor is live at S4 start; if active,
freeze-window or sequence after their arc.

---

## 7. S5 -- Extract `molt-backend-wasm`

**Goal:** the WASM encoder leaf (independent of every backend; TIR->WASM lowering already lives in
molt-lower, so this crate is a clean consumer of molt-lower output). Deps `molt-lower` +
`molt-codegen-abi` + `wasm-encoder`/`wasmparser`.

### 7.1 Exact partition
The live partition is the full WASM authority cluster, not the older
`wasm.rs`/`wasm_imports.rs` slice: `runtime/molt-backend-wasm/src/{wasm.rs,
wasm_abi.rs,wasm_abi_generated.rs,wasm_abi_manifest.toml,wasm_binary.rs,
wasm_data.rs,wasm_dispatch.rs,wasm_import_tracking.rs,wasm_imports.rs,
wasm_options.rs,wasm_plan.rs,wasm_values.rs,wasm/**}`. Shared SimpleIR debug
and trampoline metadata live in `molt-tir`; `molt-backend` keeps no private WASM
encoder or ABI modules.

### 7.2 Cargo.toml / wiring
- `runtime/molt-backend-wasm/Cargo.toml`: deps `molt-ir`, `molt-tir`,
  `wasm-encoder`(opt), and `wasmparser`(opt). Features:
  `wasm-backend = ["dep:wasm-encoder", "dep:wasmparser",
  "molt-ir/wasm-backend", "molt-tir/wasm-backend"]`,
  `test-util=["molt-ir/test-util", "molt-tir/test-util"]`.
- `molt-backend/Cargo.toml`: `molt-backend-wasm = { path = "...", optional = true }`;
  `wasm-backend = ["dep:molt-backend-wasm", "molt-backend-wasm/wasm-backend"]`. Delete the inline
  `wasm-encoder`/`wasmparser` deps if the driver no longer uses them.
- `molt-backend/src/lib.rs`: `#[cfg(feature="wasm-backend")] pub use
  molt_backend_wasm::wasm;` (preserves
  `molt_backend::wasm::{WasmBackend, WasmCompileOptions}` that main.rs:16 imports).

### 7.3 Visibility / gates / parallelization
Widen what main.rs names (`wasm::{WasmBackend, WasmCompileOptions}`) + what the wasm `[[test]]`
integration tests reach. The wasm `[[test]]` entries (molt-backend/Cargo.toml:79-117:
`wasm_compilation`, `wasm_import_registry`, `wasm_import_filtering`, `wasm_data_segments`,
`wasm_type_section`, `wasm_fastcall_lowering`, `jumpful_malformed_control`) MOVE to
`molt-backend-wasm/tests/` (they test the encoder). Full G1-G7; **G3 on `--target wasm`.** **[seq:
S2,S3]; [parallel] with S4/S6/S7** (disjoint files:
runtime/molt-backend-wasm/src/** vs llvm_backend/ vs native_backend/ vs
luau*/rust.rs). Touches molt-backend/lib.rs+Cargo.toml -> serialize those edits
(S8 join).

---

## 8. S6 -- Extract `molt-backend-luau` + `molt-backend-rust`

**Goal:** the two lowest-coupling leaves (luau touches a handful of molt-lower fns; rust touches
only `representation_plan::ScalarRepresentationPlan`). 21b: "easiest; can precede S3/S4 (no ABI
touch)."

### 8.1 Exact partition
- **`molt-backend-luau`:** `git mv` `molt-backend/src/{luau.rs (~677 KB), luau_ir.rs (~36 KB),
  luau_lower.rs (~37 KB), luau_json_prelude.luau}` -> `runtime/molt-backend-luau/src/`. Paths ->
  `molt_lower::{tir::type_refine::refine_types, tir::target_info::*, tir::passes::*,
  tir::lower_to_simple::*, tir::lower_from_simple::*, tir::drop_phase::*}` (the verified luau
  surface -- ~7 items; NO NaN-box -> no molt-codegen-abi dep).
- **`molt-backend-rust`:** `git mv` `molt-backend/src/rust.rs` (~217 KB) ->
  `runtime/molt-backend-rust/src/`. Path -> `molt_lower::representation_plan::ScalarRepresentationPlan`
  (the SOLE molt-lower item it names).
- **`egraph_simplify.rs`** is referenced ONLY by `molt-backend/src/lib.rs`
  (`#[cfg(feature="egraphs")] pub mod egraph_simplify;`) -- NOT by rust.rs (verified). 21b
  parenthesizes it "(+ opt egraph_simplify.rs)" with molt-backend-rust, but the current consumer
  is the driver. **Decision: KEEP `egraph_simplify.rs` in molt-backend (driver) for now; do NOT
  fabricate a molt-backend-rust dep on it.** Flag a follow-up to move it into molt-backend-rust
  under an `egraphs` feature IF/WHEN rust.rs consumes it.

### 8.2 Cargo.toml / wiring
- `molt-backend-luau/Cargo.toml`: deps `molt-lower`; features `luau-backend=["molt-lower/..."]`
  (luau-backend is currently a no-op `[]` at molt-backend/Cargo.toml:65 -- becomes
  `dep:molt-backend-luau`), `test-util`.
- `molt-backend-rust/Cargo.toml`: deps `molt-lower`; features `rust-backend` ->
  `dep:molt-backend-rust`, `test-util`. (No `egraphs` -- egraph_simplify stays in the driver.)
- `molt-backend/src/lib.rs`: `#[cfg(feature="luau-backend")] pub mod luau;` (:63-64) -> `pub use
  molt_backend_luau::luau;` (preserves `molt_backend::luau::LuauBackend` main.rs:12); same for
  `rust` (:65-66 -> `pub use molt_backend_rust::rust;`, preserves `molt_backend::rust::RustBackend`
  main.rs:14). ROOT members: add both.

### 8.3 Gates / parallelization
Widen `luau::LuauBackend`, `rust::RustBackend` (+ whatever the `rust_transpiler_preview` + luau
`[[test]]` reach; move those `[[test]]` entries to the new crates). Full G1-G7; G3 on `--target
luau` + `--target rust`. **[seq: S2]; [parallel] with S4/S5** -- luau/rust files are disjoint from
llvm/wasm/native. Two crates but ONE move (commit each crate separately, both green). Lowest risk
-> good first-after-S2 parallel lane (may even precede S3/S4 since it touches no ABI).

---

## 9. S7 -- Extract `molt-backend-native` (deps molt-lower + abi + opt llvm)

**Goal:** the largest codegen crate + the ONLY backend that depends on another (the real
`native->llvm` edge, feature-gated). Riskiest (symbol-identity G5 + the `use super::*` ->
explicit-import rewrite). LAST backend, after llvm (S4).

### 9.1 Exact partition
`git mv` `molt-backend/src/native_backend/` (whole subtree: `mod.rs`, `simple_backend.rs` ~6,268,
`vec_layout.rs`, `function_compiler.rs` ~9,476, AND the entire `function_compiler/fc/` tree of 39
M1 handlers) -> `runtime/molt-backend-native/src/`. **The fc/ tree moves intact -- S7 does NOT
re-open any M1 move** (fact #6). `native_backend/mod.rs` -> the crate `lib.rs` (or keep mod.rs +
thin lib.rs). Remaining `crate::*` paths -> `molt_lower::` / `molt_passes::` / `molt_ir::`; NaN-box
-> `molt_codegen_abi::` (consts + scalar `box_int` + `NanBoxConsts` + `pending_bits` +
`stable_ic_site_id`); the Cranelift-typed helpers (`unbox_int`/`unbox_int_or_bool`/`box_int_value`)
STAY in this crate (they were always here -- simple_backend.rs).

### 9.2 The `use super::*` -> explicit-import rewrite (the main mechanical cost; 21b/21 section-1.3)
`native_backend/mod.rs:1` is `use super::*;` -- it currently re-exports lib.rs's crate-root items
(NaN-box consts via `use native_backend_consts::*`, the lib.rs helpers, plus the shared Cranelift
imports it ALSO declares at mod.rs:11-26) down to `simple_backend`/`function_compiler` (and `fc/`
via their own `use super::super::*` chain). After extraction there is no crate-root `super` to
glob. Rewrite: replace `use super::*;` at mod.rs:1 with the EXPLICIT set the subtree needs --
`use molt_lower::{...}`, `use molt_passes::{...}`, `use molt_ir::{tir::{...}, Repr, ...}`, `use
molt_codegen_abi::{QNAN, TAG_*, NanBoxConsts, box_int, pending_bits, stable_ic_site_id, ...}`. The
Cranelift imports ALREADY explicitly declared at mod.rs:11-26 stay (they were never via super).
The `fc/` files keep `use super::super::*` (now resolving to `native_backend/mod.rs`'s explicit
imports -- the ancestry-privacy chain is intact WITHIN the crate; only the crate-root super edge
is rewritten). **Do NOT widen everything to `pub` to dodge this** (pub-creep, G6) -- the glob is
replaced by NAMED IMPORTS, not by widening the source.

### 9.3 Cargo.toml / wiring
- `molt-backend-native/Cargo.toml`: deps `molt-lower`, `molt-codegen-abi`, `cranelift-*`(opt, copy
  the per-arch cranelift blocks from molt-backend/Cargo.toml:12-16,53-57), `molt-backend-llvm =
  { path = "../molt-backend-llvm", optional = true }` (THE native->llvm edge), `libc`,
  `windows-sys`(target-gated, copy :42-51), `serde`/`serde_json`/`sha2`/`rayon`,
  `tikv-jemallocator`(opt). Features: `native-backend = ["dep:cranelift-codegen", ...,
  "molt-lower/native-backend"]`, `llvm = ["dep:molt-backend-llvm", "molt-backend-llvm/llvm",
  "molt-lower/llvm"]`, `jemalloc`, `test-util=["molt-lower/test-util"]`.
- `molt-backend/Cargo.toml`: `molt-backend-native = { path = "...", optional = true }`;
  `native-backend = ["dep:molt-backend-native", "molt-backend-native/native-backend"]`; `llvm =
  ["molt-backend-native/llvm"]` (llvm now under native, per 21 section-2.1 wiring). Move the
  cranelift + windows-sys + jemalloc deps OUT of molt-backend (into molt-backend-native) unless
  the driver still needs them (it does not -- they were native-only).
- `molt-backend/src/lib.rs`: `#[cfg(feature="native-backend")] mod native_backend;` (:28-29) + the
  `pub use crate::native_backend::{CompileOutput, NativeBackendModuleContext, SimpleBackend}`
  (:30-31) + the `pub(crate) use crate::native_backend::{DeferredDefine, NanBoxConsts, VarValue,
  block_has_terminator, extend_unique_tracked, switch_to_block_tracking, unbox_int}` (:32-36) ->
  `pub use molt_backend_native::{CompileOutput, NativeBackendModuleContext, SimpleBackend};`. The
  `pub(crate)` re-exports served the now-moved native code; audit which the driver/main.rs still
  names (`SimpleBackend` yes, main.rs:10; `NanBoxConsts`/`unbox_int`/etc. were used by moved code,
  so they DROP from molt-backend's lib.rs -- G6 confirms the surface shrinks).

### 9.4 Gates / parallelization
Widen ONLY what the driver + main.rs name: `SimpleBackend`, `CompileOutput`,
`NativeBackendModuleContext` (main.rs:10 + lib.rs:30-31). **G5 (symbol identity) is THE
load-bearing gate** -- `nm` the native rlib before/after; the C-ABI/no_mangle surface molt-runtime
links must be byte-identical (native emits the object file). **G3 on `--target native`** + the
`loop_continue`/`native_batch_worker_spawn` `[[test]]` (move to molt-backend-native/tests/). **[seq:
S2,S3,S4]; LAST backend.** Single agent (the rewrite is touch-heavy + risky). Follow the in-flight
21a fc/ (already landed) and the LLVM lane (S4).

---

## 10. S8 -- Reduce `molt-backend` to the thin driver + daemon

**Goal:** the final fan-in. molt-backend becomes the lib facade + `main.rs` (CLI/daemon) -- the
ONLY crate that knows all backends; per-backend features are `dep:` activations.

### 10.1 Exact partition / what remains
After S1-S7, `molt-backend/src/` retains: `lib.rs` (the facade -- now mostly `pub use
molt_*::{...}` re-exports preserving the public API main.rs/frontend consume + the small driver
helpers `should_dump_ir`/`dump_ir_ops`/`externalize_function_with_signature`/`TrampolineKind`/
`function_requires_value_return`/`env_setting`; AUDIT each -- those that are lowering-level (e.g.
`externalize_function_with_signature` rewrites a `FunctionIR`) may relocate to molt-lower; the
daemon-only ones stay), `main.rs` (~239 KB -- the daemon: `run_daemon`,
`partition_functions_for_batches` :290, batch/health/cache, the `[[bin]]`),
`bin/typed_repr_report.rs` (a separate diagnostic binary reading TIR -- stays, or moves to
molt-lower's bin). `egraph_simplify.rs` (per 8.1 decision, stays under `egraphs`).

### 10.2 Cargo.toml -- the feature fan-in (21 section-2.1 target wiring, verified-applicable)
```
# molt-backend/Cargo.toml (driver, after S8)
[dependencies]
molt-ir = { path = "../molt-ir" }
molt-passes = { path = "../molt-passes" }
molt-lower = { path = "../molt-lower" }
molt-codegen-abi = { path = "../molt-codegen-abi" }
molt-backend-native = { path = "../molt-backend-native", optional = true }
molt-backend-llvm   = { path = "../molt-backend-llvm",   optional = true }
molt-backend-wasm   = { path = "../molt-backend-wasm",   optional = true }
molt-backend-luau   = { path = "../molt-backend-luau",   optional = true }
molt-backend-rust   = { path = "../molt-backend-rust",   optional = true }
# (cranelift/inkwell/wasm-encoder deps now live in their backend crates, NOT here)
[features]
default        = ["native-backend"]
native-backend = ["dep:molt-backend-native", "molt-backend-native/native-backend"]
llvm           = ["molt-backend-native/llvm"]      # llvm under native (the real edge)
wasm-backend   = ["dep:molt-backend-wasm", "molt-backend-wasm/wasm-backend"]
luau-backend   = ["dep:molt-backend-luau"]
rust-backend   = ["dep:molt-backend-rust"]
egraphs        = ["egg"]                            # egraph_simplify stays in the driver
```
ROOT `Cargo.toml` members now lists `molt-ir, molt-passes, molt-lower, molt-codegen-abi,
molt-backend-{native,llvm,wasm,luau,rust}, molt-backend` (molt-tir removed in S2).

### 10.3 Gates / parallelization
Full G1-G7 across EVERY feature permutation (this is the integration commit): `--no-default-features`,
`native-backend`, `native-backend llvm`, `wasm-backend`, `luau-backend`, `rust-backend`, and the
all-on combo. **G7 is the headline gate** -- prove the incremental-build win: edit a
`molt-passes/passes_tir/*.rs` file, `cargo build` -> recompiles `molt-passes` + relinks dependents
(NOT molt-ir, NOT the backends' source); edit `wasm.rs` -> recompiles `molt-backend-wasm` +
relinks molt-backend ONLY. **[seq: S4-S7]** -- the join; serial, single agent. Mostly a
Cargo.toml + lib.rs cleanup commit (the file moves all happened in S1-S7).

---

## 11. Parallelization map (the swarm schedule)

```
S1 (molt-ir)            [FOUNDATION GATE -- serial, single agent; nothing starts until green]
  |
  +--> S2 (molt-passes, molt-lower)   [serial after S1; 2 commits S2a/S2b; single agent]
  +--> S3 (molt-codegen-abi)          [PARALLEL with S2 -- needs only molt-ir]
            |                               (serialize the molt-backend/lib.rs+Cargo.toml dep-line
            |                                edits vs S2; the file MOVES are disjoint)
  after S2 + S3 -- THREE INDEPENDENT LANES (3 agents at once, disjoint crates):
       Lane A (native chain, serial within):  S4 (molt-backend-llvm) --> S7 (molt-backend-native)
       Lane B:                                S5 (molt-backend-wasm)
       Lane C:                                S6 (molt-backend-luau + molt-backend-rust)
  |
  S8 (thin driver)       [JOIN -- serial after S4-S7; single agent]
```

**Independence:** S4/S5/S6/S7 each `git mv` a DISJOINT source subtree (`llvm_backend/` /
`runtime/molt-backend-wasm/src/**` / `luau*`+`rust.rs` / `native_backend/`) and create a DISJOINT new
crate -> new-crate creation is fully independent. **Serialization points (shared files):** every
backend move ALSO edits `molt-backend/src/lib.rs` (the `pub mod X` -> `pub use molt_backend_X`
line), `molt-backend/Cargo.toml` (the dep+feature line), and ROOT `Cargo.toml` members. These
three files are the contention surface -> each backend agent edits ONLY its own lines; rebase/merge
in commit order; S8 reconciles. (Same shape 21e flags for its shared `lib.rs`/baseline files.)
S6 may even precede S3/S4 (no ABI/llvm touch) if an agent is free earlier. The native->llvm
ordering (S4 before S7) is the only HARD intra-lane serialization. Each S-move is staged as its
own move-only commit, G1-G7 gated.

---

## 12. Risk register (S-move-specific; extends 21 section-4)

| Risk | Move | Mitigation |
|---|---|---|
| G3 artifact bytes shift from a crate boundary perturbing inlining of a hot lowering/codegen fn | S2, S7 | If G3 diffs, locate the cross-boundary `#[inline]` candidate; promote to `#[inline(always)]` or document a proven-semantically-identical exception. Reject silent diffs. |
| Re-introducing a passes<->lowering CYCLE via the test-only refs | S2 | Relocate the 4 round-trip tests (drop_insertion:6223, loop_unroll:1535/1621/1884) to molt-lower's test suite; molt-passes gains NO lowering dep (prod or dev). |
| `representation_plan.rs` split fractures a vocab enum from its plan logic | S1/S2 | Move ONLY the 5 named vocab enums to molt-ir/repr.rs (:780-:838 items); the rest stays one file -> molt-lower. The plan-logic deps (value_range/scev) all sit above passes (verified). |
| matches!-oracle exhaustiveness regresses (a wildcard arm added to dodge a path break) | S1 | G-oracle: op_kinds_generated + effects.rs `*_table` consumers travel together; grep for NEW `matches!(op.kind/opcode` introduced in the move -> must be zero. |
| cranelift/inkwell/wasm-encoder leaks into molt-codegen-abi | S3 | G7 cargo-tree: molt-codegen-abi deps molt-ir ONLY; the Cranelift-typed unbox helpers STAY in native. |
| pub-creep (blanket pub(crate)->pub) | all | G6 surface snapshot = exactly the enumerated consumed list; widen only `E0603`/`E0432`-named items. |
| symbol-identity break (C-ABI/no_mangle) | S7 (native), S4 (llvm) | G5 nm before/after; the object/`.ll` export surface byte-identical. |
| extracting from under the active LLVM editor | S4/S7 | Verify no `llvm_backend/*` editor live; freeze-window or sequence after their arc (21 section-0.3). |
| shared molt-backend/lib.rs+Cargo.toml contention across S4-S7 | S4-S7 | Each agent edits only its own backend's lines; rebase in commit order; S8 reconciles. |
| stale `runtime/Cargo.toml` secondary manifest edited by mistake | all | The build root is the REPO-ROOT Cargo.toml; never touch runtime/Cargo.toml. |
| `NanBoxConsts::new(_builder)` de-Cranelift misses a call site | S3 | Mechanical sweep of all `NanBoxConsts::new(` call sites -> `NanBoxConsts::new()`; G1 compile + G3 byte-identical catch any miss. |

---

## 13. The win (doctrine ledger)

After S8 the incremental-build killer is retired structurally: editing a TIR pass recompiles
`molt-passes` + relinks (not molt-ir, not the 5 backends' source -- they recompile in PARALLEL as
separate CUs, never as one 185K-line monolith unit); editing a backend recompiles exactly ONE
backend crate + relinks the thin driver; N agents own disjoint crates (`molt-ir`, `molt-passes`,
`molt-lower`, `molt-codegen-abi`, and five `molt-backend-*`) with the layering DAG enforced by the
compiler (a back-edge is uncompilable). The NaN-box ABI has ONE authority (`molt-codegen-abi`),
killing the `wasm.rs` duplicate. Every move is byte-identical (G3) so the Pythonista's exact
CPython semantics across every backend are provably unchanged; the matches!-oracle exhaustiveness
(the Rustacean's drift-uncompilable invariant) is preserved as a gate. (DESIGN_DOCTRINE #1 killer
retired; #2 both lenses satisfied; one-authority-per-invariant for op-effects + the ABI.)

## Critical files
- `runtime/molt-tir/src/tir/mod.rs` (S1/S2 module list to partition; the vocab vs passes vs
  lowering boundary + `is_structural` :52 + the primary-type re-exports :82-99)
- `runtime/molt-tir/src/representation_plan.rs` (S1 vocab-enum extract @:780-:838 -> molt-ir/repr.rs;
  residual plan logic -> molt-lower; the value_range/scev production edges @:1240-:1332)
- `runtime/molt-passes/src/tir/passes/effects.rs` + `runtime/molt-ir/src/tir/op_kinds_generated.rs`
  (the matches!-oracle exhaustiveness authority that must travel into molt-ir together -- G-oracle)
- `runtime/molt-backend/src/lib.rs` (the re-export facade S1/S2/S3/S4-S8 all re-point; the loose
  NaN-box helpers @:76-99 -> molt-codegen-abi; the per-backend `pub mod` -> `pub use` lines)
- `runtime/molt-backend/src/native_backend/mod.rs` (S7 `use super::*` @:1 -> explicit-import
  rewrite, the ancestry root above the fc/ tree) + `native_backend_consts.rs` + `simple_backend.rs`
  (NaN-box const/helper defs split by type for S3: scalar -> abi, Cranelift-typed -> stay)
- `Cargo.toml` (REPO ROOT -- the workspace members list every new crate joins; NOT runtime/Cargo.toml)
