<!-- Foundation blueprint 21b. Architect: crate-graph-architect (Plan agent), 2026-06-23.
Arc: the PRINCIPLED target crate graph for molt-backend decomposition, derived from the
ACTUAL dependency DAG (grep-verified `use`-edges), refining doc 21's coarser groupings.
"Split at logical, necessary, and correct levels" — split where there is a genuine seam
AND a real benefit; do not over-split for size; do not under-split across a real layering
boundary. Companion to 21 (the program), 21a (the function_compiler function-split),
08 (build-speed). Design only — no code refactored in the session that produced it. -->

# 21b — molt-backend Decomposition: Principled Crate-Graph Blueprint

## The measured DAG (what the `use`-edges actually say)

Two load-bearing structural discoveries that REFINE doc 21:

1. **Inside molt-tir there is a clean 3-tier acyclic stack** `vocabulary ← passes ← lowering`.
   The IR vocabulary has ZERO edges to passes or lowering. Passes→lowering edges exist but
   are **100% test-only** (`drop_insertion.rs:6223` under `mod tests`; `loop_unroll.rs`
   1535/1621/1884 under `mod tests`). Lowering→passes is a **hard production edge**
   (`lower_to_simple.rs:305-319` + `lower_from_simple.rs:90-96` consume
   `passes::drop_insertion::*_ATTR`; `lower_to_lir.rs:63-312`, `lower_to_wasm.rs:374`,
   `representation_plan.rs:1240-1332` take `passes::value_range::ValueRangeResult` as a
   production parameter type).

2. **The backends are NOT all mutually independent.** `native_backend → llvm_backend` is a
   REAL edge (`simple_backend.rs:3315-3547`, `#[cfg(feature="llvm")]`): native's
   `SimpleBackend` dispatches to LLVM as an alternate codegen path. `llvm_backend →
   native_backend` is ZERO (only a doc comment at `mod.rs:289`). wasm, luau, rust have ZERO
   cross-backend edges. So the backend layer is `{wasm, luau, rust}` independent + `llvm ←
   native` (a 2-node sub-chain), all over a shared NaN-box ABI.

## Target crate graph (each crate: contents · depends-on · why this is a true seam)

### Layer 0 — foundation
**`molt-ir`** *(vocabulary — the zero-dependency data model)*
- Contents: `tir/{types, ops, op_kinds_generated, values, blocks, function, cfg, dominators,
  ssa, serialize, printer, verify}.rs` + SimpleIR transport `ir.rs`, `ir_schema.rs`,
  `json_boundary.rs` + the `Repr` enum (`representation_plan.rs:838`) + std-only leaves
  `intrinsic_symbols.rs`, `process_diagnostics.rs`, `stdlib_module_symbols.rs`.
- Depends-on: nothing in-workspace (serde + std). ~15K LOC.
- Seam: grep-proven zero back-edges to passes or lowering — the fixpoint of the DAG; it
  never recompiles when a pass or backend changes.

### Layer 1 — optimizer
**`molt-passes`** *(the ~40 transforms + analyses + orchestration)*
- Contents: all `tir/passes/*` (~57K), `passes.rs`, `tir/pass_manager.rs`,
  `tir/module_phase.rs`, `tir/analysis/`, `tir/{call_facts, call_graph, type_refine, deopt,
  exception_regions, drop_phase, parallel, cache, bolt}.rs`.
- Depends-on: `molt-ir`. ~76K LOC.
- Seam: passes depend only on the vocabulary (the lowering edges are test-only, cut behind
  `test-util`); a pass author recompiles `molt-passes`+downstream but never `molt-ir`.

### Layer 2 — lowering / ABI
**`molt-lower`** *(TIR → {LIR, SimpleIR, WASM-IR} lowering + codegen-facing repr plan)*
- Contents: `tir/{lower_from_simple, lower_to_simple, lower_to_lir, lower_to_wasm, lir,
  verify_lir, verify_lir_repr, target_info, mlir_compat}.rs` +
  `representation_plan.rs` *logic* (`ScalarRepresentationPlan`,
  `LlvmReprFacts`, `value_range_for`) + `ir_rewrites.rs`.
- Retired surface: the old TIR `wasm_component`/`wasm_split`/`wasm_streaming` estimate
  modules are no longer a lowering-layer member. WASM split/component/streaming authority
  belongs in emitted-artifact facts in the WASM backend/linker/optimizer path, not TIR
  name heuristics.
- Depends-on: `molt-passes` (transitively `molt-ir`). ~24K LOC.
- Seam: hard production dep on passes (`ValueRangeResult`, drop-insertion attrs) → must sit
  above passes; isolating it means a Cranelift/WASM author editing lowering doesn't
  recompile the 40 passes. (`ir_rewrites.rs` belongs HERE, not the orchestrator — flag #4.)

**`molt-codegen-abi`** *(the shared NaN-box / value-encoding ABI)*
- Contents: NaN-box consts `QNAN`/`TAG_*`, header-layout facts, type-id ABI facts,
  `NanBoxConsts`, and helpers `unbox_int`/`box_int`/`pending_bits`/
  `stable_ic_site_id`.
- Depends-on: `molt-ir` only (~300 LOC).
- Seam: THREE backends share it (native 48x, llvm 29x, wasm 34x); WASM formerly
  duplicated `QNAN`, and the shared crate is now the ABI authority each backend imports
  without depending on a sibling backend.

### Layer 3 — per-backend codegen (the fan-out)
**`molt-backend-llvm`** — `llvm_backend/{mod, lowering, types, pgo, runtime_imports}.rs`;
deps `molt-lower` + `molt-codegen-abi` + `inkwell` (opt); ~17K. Seam: zero edges INTO it; a
leaf consumer, so LLVM edits never touch Cranelift/wasm/luau.

**`molt-backend-native`** — `native_backend/` entire subtree (incl. the 21a `fc/` tree);
deps `molt-lower` + `molt-codegen-abi` + **`molt-backend-llvm` (optional, `llvm` feature)`**
— the real `native → llvm` edge; ~50K. Seam: largest codegen unit + the only backend that
depends on another, so it sits one level above llvm (NOT beside/grouped). Biggest cache win.

**`molt-backend-wasm`** — `wasm.rs`, `wasm_imports.rs`; deps `molt-lower` +
`molt-codegen-abi` + `wasm-encoder`/`wasmparser`; ~20K. Seam: independent of every backend;
the TIR→WASM lowering already lives in molt-lower, so the encoder is a clean consumer.

**`molt-backend-luau`** — `luau.rs`, `luau_ir.rs`, `luau_lower.rs`; deps `molt-lower`; ~17K.
Seam: fully self-contained, minimal lowering coupling — easiest clean extraction.

**`molt-backend-rust`** — `rust.rs` (+ opt `egraph_simplify.rs`); deps `molt-lower`; ~5K.
Seam: smallest, zero cross-backend edges; a leaf consumer.

### Layer 4 — driver
**`molt-backend`** *(thin orchestrator + daemon binary — the ONLY crate that knows all backends)*
- Contents: `lib.rs` façade (public API main.rs/frontend consume) + `main.rs` (CLI/daemon:
  `run_daemon`, batch/health/cache).
- Depends-on: `molt-backend-{native, llvm, wasm, luau, rust}` (each behind its feature) +
  `molt-lower` + `molt-passes` + `molt-ir`. Per-backend features become `dep:` activations.
- Seam: top of the DAG — feature-flag fan-in + daemon orchestration; the one place allowed
  to "know about every backend."

### Build order (topological)
```
molt-ir
  └─ molt-passes
       └─ molt-lower ──┬─ molt-codegen-abi (parallel; only needs molt-ir)
                       ├─ molt-backend-llvm
                       │     └─ molt-backend-native  (native depends on llvm)
                       ├─ molt-backend-wasm   ┐
                       ├─ molt-backend-luau   ├─ (these 3 parallel, independent)
                       └─ molt-backend-rust   ┘
                                  └─ molt-backend (driver + daemon bin)
```

## Verdict on the two granularity questions
**Q1 — split molt-tir into `molt-ir` ← `molt-passes` ← `molt-lower`? YES (a true seam).**
The DAG is acyclic in exactly this shape (vocab→{passes,lowering}=0; passes→lowering=test-only
4 refs; lowering→passes=hard production). Layers are ~15K/76K/24K LOC; the benefit is real +
asymmetric (a pass author edits 1 of ~40 files, recompiles `molt-passes`+downstream, never the
15K vocabulary; lowering+backends don't recompile when a pass's internals change but its
signature doesn't). NOT finer (per-pass): the 40 passes co-change through shared analyses
(`call_facts`, `value_range`, `scev`, `effects`) + a shared `pass_manager` — one cohesive
optimizer; agents own individual *files* via the module tree. Mechanical cost: move the 4
test-only passes→lowering refs behind `molt-tir/test-util`.

**Q2 — per-backend crates? YES — and native+llvm are TWO crates, not grouped.**
The real edge is `native → llvm` (one-way, feature-gated, `simple_backend.rs:3315`); llvm→native
is zero. Bundling them (doc 21 #5 / doc 08 Phase 3) defeats the cache benefit for the LLVM lane.
Correct: `molt-backend-llvm` (leaf) + `molt-backend-native` (optionally depends on llvm).
wasm/luau/rust each independent → one crate each. The shared NaN-box ABI → `molt-codegen-abi`
(depends only on molt-ir), de-duping wasm's copy.

## Flags against doc 21 / doc 08 (granularity errors + corrections)
| # | Doc claim | Reality (grep-proven) | Correction |
|---|-----------|----------------------|------------|
| 1 | doc 21: `molt-tir` = ONE crate (vocab+passes+lowering). | Clean 3-tier acyclic stack; passes→lowering test-only. | **Too coarse** → `molt-ir` ← `molt-passes` ← `molt-lower`. |
| 2 | doc 21 #5 / doc 08 Ph3: `molt-backend-native` = native **+ llvm** in one crate. | `native→llvm` one-way; `llvm→native` zero. | **Too coarse** → two crates (`molt-backend-llvm` leaf + `molt-backend-native` deps it). |
| 3 | doc 21: NaN-box consts/helpers stay in orchestrator `lib.rs`. | 3 backends consume (48/29/34×); wasm duplicates `QNAN`. | **Missing crate** → `molt-codegen-abi` (deps molt-ir); de-dup wasm. |
| 4 | doc 21: `ir_rewrites.rs` in orchestrator core. | deps only `ir`+`representation_plan`+`passes::SimpleIrScalarPurityFacts`. | **Wrong layer** → move into `molt-lower`. |
| 5 | doc 21: `representation_plan` logic in orchestrator. | deps `tir::lower_to_simple::SimpleValueNames` + `passes::value_range`/`scev`. | **Wrong layer** → plan logic in `molt-lower` (only `Repr` enum is vocab-level, already split). |
| 6 | doc 21 #1: file-split function_compiler.rs. | ~one 22K-line fn; file-split ≈0 build win. | Already corrected by 21a (function-extraction). Consume 21a. |
| 7 | doc 08 rebuild graph: native deps `molt-backend` core. | After Q1 there is no monolith core; lowers are ir/passes/lower. | **Stale** → backends dep `molt-lower`(+abi); driver deps backends (inverted). |
*(doc 21 otherwise validated: zero-backend-back-edge holds, feature-flag-as-dep wiring correct, per-phase gate methodology unchanged.)*

## Ranked extraction sequence (from current state: molt-tir extracted, M1 in flight)
Key: **[∥]** different crate, parallelizable; **[seq:X]** touches crate X, serialize.

| Order | Move | Crate(s) | Parallel? | Notes |
|------|------|----------|-----------|-------|
| **S1** | Split `molt-tir` → `molt-ir` (lift vocabulary+transport+Repr+std-leaves; molt-tir keeps passes+lowering, deps molt-ir). | molt-tir/molt-ir | foundation | FIRST; matches!-oracle audit (doc 21 §4) as ops cross the boundary; cut 4 test-only refs behind `test-util`. |
| **S2** | Split residual → `molt-passes` ← `molt-lower` (passes+analyses+pass_manager+module_phase → molt-passes; lower_*+lir+repr-plan-logic+`ir_rewrites` → molt-lower). | molt-passes, molt-lower | [seq:S1] | Q1 sub-split; `ir_rewrites` migrates in (flag #4). |
| **S3** | Extract `molt-codegen-abi` (consts from `native_backend_consts.rs` + helpers from lib.rs; rewrite `wasm.rs:17` to import). | new + molt-backend | **[∥]** S2 (only needs molt-ir) | wasm de-dup gated by G3 byte-identical. |
| **S4** | Extract `molt-backend-llvm`. | new + molt-backend | [seq:S2,S3]; ∥ S5/S6/S7 | BEFORE native; coordinate active LLVM lane. |
| **S5** | Extract `molt-backend-wasm`. | new + molt-backend | [seq:S2,S3]; ∥ S4/S6/S7 | encoder consumes molt-lower output. |
| **S6** | Extract `molt-backend-luau` + `molt-backend-rust`. | 2 new + molt-backend | [seq:S2]; ∥ S4/S5 | easiest; can precede S3/S4 (no ABI touch). |
| **S7** | Extract `molt-backend-native` (deps molt-lower+abi+opt llvm); `use super::*`→explicit. | new + molt-backend | [seq:S2,S3,S4]; LAST backend | riskiest (symbol-identity G5); follow llvm + the in-flight 21a fc/. |
| **S8** | Reduce `molt-backend` to thin driver (lib façade + main.rs/daemon; features→`dep:`). | molt-backend | [seq:S4–S7] | final fan-in. |

**Parallelization:** S1 is the gate. After S1: S3 ∥ S2. After S2+S3: two independent lanes —
{S4→S7} (native/llvm chain, serialized internally) and {S5, S6} (wasm + luau/rust, parallel) —
3 agents at once on disjoint crates. S8 is the join. File-removals from the shrinking
`molt-backend` serialize on its `lib.rs`+`Cargo.toml`; each new-crate creation is independent.
Stage each as its own move-only commit, G1–G5 gated (doc 21 §3).
