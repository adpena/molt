<!-- Parity recon (wf_971517d5-6b2, 2026-06-04). -->

# E1 Inliner Activation + module_slot_promotion: Target × Profile Parity Audit

## 1. LLVM Target

### Does LLVM consume post-module-phase `ir.functions`?

**Yes, but only partially — and this is the critical gap.**

The execution path in `simple_backend.rs` is:

```
SimpleBackend::compile(ir)
  │
  ├─ [!skip_ir_passes] Per-function TIR pipeline (parallel, ~2400–2593)
  │    loop over ir.functions → lower_to_tir → refine_types → run_pipeline → refine_types → lower_to_simple_ir
  │    → ir.functions[idx].ops mutated in place
  │
  ├─ [!skip_ir_passes] eliminate_dead_ops (2597–2598)
  │
  ├─ [!skip_ir_passes] MODULE PHASE (2624–2660)
  │    lower_functions_to_tir_module(&ir.functions)        ← POST per-fn pipeline bodies
  │    run_module_pipeline(&mut tir_module, &native_tti)   ← inliner + slot promotion
  │    for changed functions: lower_to_simple_ir → ir.functions[orig_idx].ops = ops
  │    → ir.functions MUTATED IN PLACE for changed functions only
  │
  ├─ [!skip_ir_passes] eliminate_dead_functions (2665–2667)
  │
  ├─ split_megafunctions, rewrite_copy_aliases, canonicalize_direct_raise_edges (2671–2683)
  │
  ├─ externalize_shared_stdlib_partition (2725–2727)
  │
  └─ #[cfg(feature = "llvm")] if use_llvm {   ← LINE 2737–2738
         for each func in ir.functions:            ← READS THESE MUTATED ir.functions
             lower_to_tir(func)                    ← SECOND lift from the ALREADY-inlined SimpleIR
             refine_types → run_pipeline → refine_types   ← per-function pipeline AGAIN
```

**The LLVM branch at line 2738 reads `ir.functions` AFTER the module phase has mutated them.** Inlined and slot-promoted functions have their post-inline SimpleIR bodies already in `ir.functions[orig_idx].ops` before the `if use_llvm` block is reached.

**The double-pipeline problem**: The LLVM path (lines 2772–2784) calls `lower_to_tir(func)` and `run_pipeline` a second time on the already-pipeline-optimized SimpleIR. This is not a double-run of the module phase — `run_module_pipeline` fires exactly once (lines 2639–2659) — but it is a second per-function TIR lift+pipeline. The comment at line 2777 explicitly acknowledges this: *"Run the full TIR optimization pipeline — same as Cranelift/WASM. Without this, all values stay DynBox."*

**The structural gap**: The native/Cranelift path uses the per-function pipeline output already sitting in `ir.functions[idx].ops` (applied at ~2540–2586) and the module phase mutates only changed functions on top of that. The LLVM path ignores the pre-existing per-function pipeline output and re-lifts from the inlined SimpleIR, re-running the full 25-pass pipeline. This means:

- The LLVM path DOES consume the inliner's and slot-promoter's changed bodies (since those are in `ir.functions`).
- The LLVM path does NOT consume the Cranelift per-function pipeline output stored in `ir.functions[idx].ops` from the earlier parallel pass at ~2540–2586. Instead it re-derives equivalent TIR from scratch from the mutated SimpleIR.
- The second per-function `run_pipeline` call for LLVM is on post-module-phase SimpleIR. The pipelines' outputs will differ from what native Cranelift used because LLVM re-applies passes to bodies that native already had cached. This is not unsound but is an asymmetry.

**No double-run of the module phase.** `run_module_pipeline` fires once, unconditionally, in the `if !self.skip_ir_passes` block at lines 2624–2660. The `#[cfg(feature = "llvm")] if use_llvm` block at 2737 has no call to `run_module_pipeline`.

**Exact control flow quote** (lines 2737–2784):

```rust
#[cfg(feature = "llvm")]
if use_llvm {
    // ...
    let tir_funcs: Vec<_> = ir
        .functions          // <- post-module-phase ir.functions (inlined + promoted)
        .iter()
        .map(|func| {
            let mut tir_func = lower_to_tir(func);  // second lift from inlined SimpleIR
            crate::tir::type_refine::refine_types(&mut tir_func);
            let _stats = crate::tir::passes::run_pipeline(&mut tir_func, &llvm_tti);
            crate::tir::type_refine::refine_types(&mut tir_func);
            (func.is_extern, tir_func)
        })
        .collect();
```

## 2. Luau Target

### Does Luau go through `SimpleBackend::compile`?

**No. Luau is a completely separate code path in `main.rs` that never calls `SimpleBackend::compile` and never invokes `run_module_pipeline`.**

The dispatch in `main.rs` is:

- Line 1975: `let is_luau = args.contains(&"--target".to_string()) && args.contains(&"luau".to_string());`
- Lines 2171–2211: If `is_luau`, a hand-rolled per-function TIR loop runs in `main.rs` directly (not through `SimpleBackend`).
- Lines 2232–2265: If `is_luau`, `LuauBackend::compile_via_ir` or `compile_checked` is called on the (per-function-optimized) `ir`.

The Luau TIR loop (lines 2182–2211):

```rust
if is_luau {
    for func in &mut ir.functions {
        // skips func.ops.len() < 4 and __annotate__ stubs
        let mut tir_func = lower_to_tir(func);
        refine_types(&mut tir_func);
        run_pipeline(&mut tir_func, &TargetInfo::native_from_simd_caps(SimdCaps::detect_host()));
        refine_types(&mut tir_func);
        let ops = lower_to_simple_ir(&tir_func);
        if validate_labels(&ops) { func.ops = ops; }
    }
    eliminate_dead_ops(&mut ir);
}
```

**`run_module_pipeline` is never called for Luau.** The inliner and slot-promotion pass are completely absent from the Luau path.

**Exact insertion point for parity**: The module phase would need to run after the per-function loop (after line 2210, after `eliminate_dead_ops`) and before line 2213 (`let output_kind = if is_luau`). The block would mirror the native pattern: `lower_functions_to_tir_module(&ir.functions)` → `run_module_pipeline(&mut tir_module, &luau_tti)` → back-convert changed functions into `ir.functions`. The appropriate `TargetInfo` would be `TargetInfo::native_from_simd_caps(SimdCaps::detect_host())` (matching what the per-function loop uses above it), or a new `TargetInfo::luau_release_fast()` constructor if Luau-specific thresholds are needed.

**Additional Luau asymmetries**:

- The per-function loop at line 2187 skips functions with `ops.len() < 4` or `__annotate__` in the name. This heuristic has no parallel in the native/WASM paths and means small functions get no optimization at all on Luau.
- There is no TIR cache for Luau (the native path has `CompilationCache`, WASM has `CompilationCache` at line 2090–2091; Luau has neither).
- `LuauBackend` is called directly in `main.rs` as `LuauBackend::new()` + `backend.compile_via_ir(&ir)` (line 2235–2239), not through any `SimpleBackend` routing.

## 3. WASM Target

**WASM correctly runs the module phase. No double-run.**

The WASM path in `wasm.rs` (`WasmBackend::compile`) runs:

1. **Per-function TIR pipeline** with `CompilationCache` (lines ~2090–2157): for each function, `lower_to_tir` → `refine_types` → `run_pipeline` (with `TargetInfo::wasm_release_fast()`) → `refine_types` → `lower_to_simple_ir`. Mutates `func_ir.ops` in-place and caches.

2. **Module phase** (lines 2159–2212): the `run_module_pipeline` block, explicitly marked "E1 ACTIVATION (WASM)". Lifts `ir.functions` (post per-function pipeline) to TIR module, calls `run_module_pipeline(&mut tir_module, &wasm_tti)`, back-converts only changed functions. Also recomputes LIR fast-path outputs for inlined functions (lines 2200–2210), handling the case where a pre-computed fast-path entry is now stale because the body changed.

3. `run_module_pipeline` appears exactly once in `wasm.rs` (line 2174). There is no second invocation.

The WASM per-function `TargetInfo` is `wasm_release_fast()` and so is the module phase's `wasm_tti` (line 2171). Consistent.

## 4. Profile Gating: dev-fast vs release-fast

**The Cargo profiles affect the backend daemon binary's optimization level only, not which TIR phases run.**

From `Cargo.toml`:

- `[profile.dev-fast]` inherits `dev`, with `molt-backend` at `opt-level = 1`, `debug = 0`.
- `[profile.release-fast]` inherits `release` with `opt-level = 3`, `lto = "fat"`.
- No Cargo feature flag or `cfg` conditional gates the module phase or per-function pipeline based on profile.

**There is no `skip_ir_passes` gating on dev-fast.** `skip_ir_passes` is set to `true` only in three cases (all explicit caller-set, not profile-conditional):

- `compile_stdlib_cache_object` single-batch path (line 347): stdlib cache objects skip IR passes because the per-function TIR pipeline was already run on the full IR above.
- `compile_stdlib_cache_object` multi-batch path (line 377): same reason.
- Batched user-program compilation (line 2535): batches skip IR passes because the full-program module phase + per-function pipeline already ran on `ir` in the non-batched first pass before splitting.

In all three cases the semantics are correct: the passes already ran before the split, and `skip_ir_passes = true` prevents double-execution.

**Implication**: a dev-fast build of the backend daemon runs the same TIR passes as release-fast. The difference is the daemon binary's own opt-level (1 vs 3), which affects how fast the daemon compiles user programs, not what it emits. The emitted user code is identical. The `TargetInfo` constructors have a `BuildProfile` field but none of the constructors used in production dispatch (`native_from_simd_caps`, `wasm_release_fast`, `from_llvm_feature_string`) set `profile: BuildProfile::DevFast` — they all resolve to `ReleaseFast` (inherited from `native_release_fast()`). So the compilation decisions (inline budgets, unroll thresholds) are identical under both cargo profiles.

## 5. Summary of Parity Status

| Target | module phase runs | inliner active | slot-promotion active | double module-phase | notes |
|--------|-------------------|---------------|----------------------|---------------------|-------|
| Native/Cranelift | yes (simple_backend.rs:2624–2660) | yes | yes | no | correct |
| WASM | yes (wasm.rs:2159–2212) | yes | yes | no | correct; LIR fast-path recomputed |
| LLVM | inherited via `ir.functions` mutation | partial — changed bodies arrive pre-inlined | partial — same | no module-phase double-run; second per-fn pipeline | **LLVM re-runs per-fn pipeline from post-inline SimpleIR, not from cached post-pipeline SimpleIR** |
| Luau | **never** | **no** | **no** | n/a | **full parity gap: module phase entirely absent** |

### The two open gaps

**Gap 1 (Luau, blocking parity)**: `run_module_pipeline` is never called on the Luau path. The inliner and `run_module_slot_promotion` produce zero benefit for Luau. The insertion point is `main.rs` after line 2210 (`eliminate_dead_ops`) and before line 2213 (`let output_kind = …`). The per-function loop (2185–2206) must remain as-is (it is the pre-module-phase per-function pipeline); the module phase block goes after it.

**Gap 2 (LLVM, structural asymmetry)**: LLVM re-lifts post-module-phase SimpleIR and re-runs the full 25-pass per-function pipeline (lines 2772–2784), whereas native uses its per-function pipeline output already stored in `ir.functions[idx].ops` from the earlier parallel pass. Both get the inlined bodies, but the LLVM per-function pipeline runs on a structurally different input (the inlined body rebuilt from SimpleIR) than what native's per-function pipeline ran on (the original pre-inline body). This is not wrong — the LLVM per-function pipeline will discover the inlining wins just as well — but it means the two differ in how many passes they run and over what input. In practice, for functions not touched by the inliner, LLVM re-runs the full per-function pipeline redundantly. This is a compile-time waste (two full TIR lifts per non-inlined function on the LLVM path) but not a correctness issue.

## 6. Measurement Commands (dev-fast profile perf matrix)

Build the backend daemon with dev-fast:

```bash
export MOLT_SESSION_ID="perf-matrix"
cargo build --profile dev-fast -p molt-backend --features native-backend
```

**Inliner probe — confirm inliner fires and measure function count:**

```bash
# Native (confirm module phase ran, count inlined sites)
MOLT_INLINE_STATS=1 python3 -m molt build --target native --output /tmp/bench_sum_out tests/benchmarks/bench_sum.py --rebuild 2>&1 | grep '\[E1\]'

# Baseline — compare against the recorded pre-activation artifact or scoreboard
# manifest; the production inliner has no ambient disable switch.

# WASM
MOLT_INLINE_STATS=1 python3 -m molt build --target wasm --output /tmp/bench_sum_wasm.wasm tests/benchmarks/bench_sum.py --rebuild 2>&1 | grep '\[E1\]'
```

**bench_sum perf matrix (time the compiled binary via safe_run.py):**

```bash
# Native release-fast
cargo build --profile release-fast -p molt-backend --features native-backend
python3 -m molt build --target native --output /tmp/bench_sum_native_rf tests/benchmarks/bench_sum.py --rebuild
python3 tools/safe_run.py --rss-mb 512 --timeout 30 -- /tmp/bench_sum_native_rf

# Native dev-fast (daemon at opt-1, emitted passes identical)
cargo build --profile dev-fast -p molt-backend --features native-backend
python3 -m molt build --target native --output /tmp/bench_sum_native_df tests/benchmarks/bench_sum.py --rebuild
python3 tools/safe_run.py --rss-mb 512 --timeout 30 -- /tmp/bench_sum_native_df

# CPython baseline
python3 tests/benchmarks/bench_sum.py

# Regression check: use the scoreboard baseline artifact/provenance instead of
# disabling the production inliner with an environment variable.
```

**Confirm WASM module-phase fires and is not double-run:**

```bash
TIR_OPT_STATS=1 MOLT_INLINE_STATS=1 python3 -m molt build --target wasm --output /tmp/bench_sum.wasm tests/benchmarks/bench_sum.py --rebuild 2>&1 | grep -E '\[E1\]|\[TIR\]'
```

**Luau gap confirmation (zero inlining, baseline only):**

```bash
MOLT_INLINE_STATS=1 python3 -m molt build --target luau --output /tmp/bench_sum.luau tests/benchmarks/bench_sum.py --rebuild 2>&1 | grep '\[E1\]'
# Expected: no [E1] lines — the Luau path never calls run_module_pipeline
```

## Key Files

- `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs` — lines 2597–2667 (module phase + LLVM dispatch)
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs` — lines 2060–2212 (WASM per-fn pipeline + module phase)
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/main.rs` — lines 2171–2265 (Luau path, bypasses SimpleBackend entirely)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/module_phase.rs` — lines 115–179 (`run_module_pipeline`: call graph → inliner → slot-promotion → rebuild)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/target_info.rs` — lines 246–311 (TargetInfo constructors; `BuildProfile` field exists but dev-fast vs release-fast never changes a pass decision)
- `/Users/adpena/Projects/molt/Cargo.toml` — lines 167–175 (dev-fast profile: molt-backend at opt-level=1 only, no pass gating)
