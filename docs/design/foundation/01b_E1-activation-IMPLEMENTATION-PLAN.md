<!-- E1 activation — VERIFIED implementation plan (recon swarm wf_02066ea6-d95 + analysis, 2026-06-04). Supersedes the line numbers + restructure in 01_E1-activation.md where they conflict. -->

# E1 inliner activation — verified, line-precise implementation plan (native phase e-1)

Recon (3 code-explorer agents, verified against live code post-`f9afd99d3`) + analysis.
This corrects three things `01_E1-activation.md` got wrong or missed. Full recon artifacts:
`tmp/e1_recon/{nativePath,contracts,testSurface}.md` (regenerate from the swarm if pruned).

## Corrections to `01_E1-activation.md`

1. **Superseded by the no-rollback compiler-pass policy.** The earlier plan
   proposed adding an environment guard to `run_inliner`; that guard is now
   intentionally absent. Inliner activation is controlled by legality,
   profitability, and external-linkage facts, not by an ambient process-global
   no-op switch.
2. **The native per-function TIR pipeline is CONTENT-HASH CACHED** (`simple_backend.rs`
   Phase 1 = 2383-2454 cache hit/miss; Phase 2 parallel = 2504-2587; Phase 3 writeback =
   2590-2594). Cached functions NEVER enter the parallel loop — they load straight into
   `ir.functions[idx].ops` from `tir_cache`. So the blueprint's "restructure the loop to
   return `TirFunction`" CANNOT see cached functions' bodies. The module inliner needs the
   WHOLE module's TIR. → Do NOT restructure the loop; insert a **separate module phase**
   where `inline_functions` is called (below), re-lifting all functions from their
   post-pipeline SimpleIR.
3. **Re-roundtripping every function is a broad risk.** A separate module phase that
   `lower_to_simple_ir`s ALL functions (even those the inliner didn't touch) puts every
   native function through a second TIR roundtrip — any non-idempotence changes codegen
   program-wide. → **Back-convert ONLY the functions the inliner changed**; leave every
   unchanged function's SimpleIR byte-identical.

## Verified contracts (no drift)

- `run_module_pipeline(&mut TirModule, &TargetInfo) -> ModuleAnalysis` (`module_phase.rs:110`)
  mutates bodies (inliner wired at 115), builds the call graph TWICE (post-inline at 120),
  `ModuleAnalysis::leaf_functions() -> BTreeSet<String>` is the POST-inline leaf set.
- `run_inliner` triple-refines every changed caller (`inliner.rs:1169-1172`:
  refine→pipeline→refine) → back-conversion sees fully-refined TIR.
- `lower_to_simple_ir(&TirFunction) -> Vec<OpIR>` (`lower_to_simple.rs:159`) is PURE
  per-function (thread-local name bridge, no inter-fn state) — safe on inlined callers.
- `lower_to_tir(&FunctionIR) -> TirFunction` (`lower_from_simple.rs:24`) pure per-function.
  Externs: `is_extern` empty-ops; `compute_leaf_functions_via_call_graph` filters them with
  `.filter(|f| !f.is_extern)` (`simple_backend.rs:350-352`) — the helper must do the same.
- Repr/bigint safety preserved by construction: `repr_by_value_for` floors every value to
  `Repr::default_for(TirType)` (`int`→`MaybeBigInt`) from the POST-inline `value_types`,
  promoting to `RawI64Safe` only via a fresh `value_range_for` on the merged body
  (`representation_plan.rs:411-438`). `apply(mul, 1<<60, 7)` cannot regress.
- Leaf-set consumer: `function_compiler.rs:24029` `leaf_functions.contains(target_name)` →
  recursion-guard skip. Flows from `effective_leaf_functions` (`simple_backend.rs:2942`).
  **Inlining is monotone on leaves** (it only REMOVES calls → only ADDS leaves), so the
  existing post-split `compute_leaf_functions_via_call_graph` on the inlined SimpleIR is
  correct, and the `module_context` pre-inline path is conservative-safe (a stale-subset
  leaf set keeps extra recursion guards = correct, just slower). **Leaf-set change is NOT
  required for correctness of the first cut** — defer threading `module_analysis.leaf_functions()`.

## The first-cut implementation (native e-1) — bounded + safe

### Step 1 — `run_inliner` exposes the changed-function set
`inliner.rs`:
- Change `InlinerStats` to carry `changed_functions: Vec<String>` (or return it alongside);
  push the caller name when `changed_this_fn` (the existing flag at the splice loop).
  Thread it out through `run_module_pipeline` → `ModuleAnalysis` (add `changed_functions`).

### Step 2 — `lower_functions_to_tir_module` helper
`tir/lower_from_simple.rs`:
```rust
/// Lift every NON-extern FunctionIR to TIR and assemble a TirModule. Returns the
/// module + the aligned original `ir.functions` index for each module position
/// (externs are skipped, so positions != indices).
pub fn lower_functions_to_tir_module(
    functions: &[FunctionIR],
) -> (TirModule, Vec<usize>) {
    let mut tir = Vec::new();
    let mut idx_map = Vec::new();
    for (i, f) in functions.iter().enumerate() {
        if f.is_extern { continue; }
        tir.push(lower_to_tir(f));
        idx_map.push(i);
    }
    (TirModule { name: "native_module".into(), functions: tir }, idx_map)
}
```
Unit-test it (round-trips a 2-fn module; externs skipped; idx_map aligned).

### Step 3 — replace the native `inline_functions` block (`simple_backend.rs:2617-2632`)
Hoist `let native_tti = TargetInfo::native_from_simd_caps(SimdCaps::detect_host());` before
the block. Replace the `if analysis.needs_inlining { inline_functions(...) }` body with:
```rust
if analysis.needs_inlining && !self.skip_ir_passes {
    let (mut tir_module, idx_map) = lower_functions_to_tir_module(&ir.functions);
    let module_analysis = run_module_pipeline(&mut tir_module, &native_tti);
    // Back-convert ONLY the inliner-changed functions; leave the rest byte-identical.
    let changed: std::collections::HashSet<&str> =
        module_analysis.changed_functions.iter().map(|s| s.as_str()).collect();
    for (pos, &orig_idx) in idx_map.iter().enumerate() {
        let f = &tir_module.functions[pos];
        if changed.contains(f.name.as_str()) {
            ir.functions[orig_idx].ops = lower_to_simple_ir(f);
        }
    }
}
```
Keep `passes::inline_functions` (deleted in e-4 after WASM/LLVM migrate). The leaf set is
unchanged (the post-split analysis runs on the inlined SimpleIR). `split_megafunctions`
(2642) already runs after, sees merged sizes (blueprint §9.5 — correct, no change).

### Step 4 — validate (the gate, in order)
- `cargo build --profile release-fast -p molt-backend --features native-backend` 0 warn.
- `cargo test ... --lib` ≥ 889.
- **Bigint oracle (the critical non-regression):** build + run
  `def apply(f,x,y):return f(x,y)` / `def mul(a,b):return a*b` / `print(apply(mul,1<<60,7))`
  → `8070450532247928832`... (verify vs CPython) — MUST be byte-identical (the inline path
  must not trusted-unbox the bigint).
- Differential (the inlining shapes from blueprint §7.2): basic add, comprehension-with-helper,
  `safe_div` (observation callee raises, caught), recursive `fib` (NOT inlined), the label-collision
  `g(f(...))` shape, the leaf-set `caller(leaf(...))`. All byte-identical CPython 3.12/3.13/3.14.
- A broad `tests/differential/basic` sweep (catches any back-conversion divergence).
- Perf: `bench_sum` ≥ CPython + the helper-heavy benches (expect 10-25% on small-helper code).
- Compile-time: `MOLT_BACKEND_TIMING=1` — the full re-lift is the cost; measure, and if >5%
  baton the "lift only inlining-participant functions" optimization.
- Regression policy: inliner regressions are fixed through pass predicates,
  representation facts, backend consumers, or a code revert. There is no
  ambient env short-circuit for the production pass.

### Then e-2 (WASM, `wasm.rs:2087-2162` — same pattern + the LIR-fast-path cache-hit fix the
recon flagged at wasm.rs:2102-2114), e-3 (LLVM held patch), e-4 (delete `inline_functions` +
`is_inlineable_with_limit` + `compute_leaf_functions_via_call_graph`). Each a complete piece;
e-4 is the dual-path-deletion completion gate.

## Why this is teed up but not rushed
The inliner LOGIC is proven (phase-c, 889 tests, differential). The activation RISK is the
plumbing (cache, roundtrip, leaf-set, back-conversion) — now fully mapped + designed. But it
is a production-codegen change touching every native binary; the bigint-oracle + differential
+ perf gates above are mandatory and must not be rushed. Implement as a focused arc.
