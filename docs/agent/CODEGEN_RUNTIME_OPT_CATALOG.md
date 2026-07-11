# Codegen & Runtime Optimization Catalog (Agner-grounded)

Source of technique-level truth: Agner Fog, *Optimizing software in C++* (agner.org,
v1.7, 179pp; the operator supplied it 2026-07-11). This maps the manual's techniques
onto molt's actual surfaces, tagging each lever **LANDED / OPEN / BLOCKED / N/A** with a
**determinism class** and a measurement path. It exists so the opt-matrix loop
(`docs/agent/OPT_MATRIX_STATE.md`) selects real, unsettled codegen/runtime rungs and does
NOT re-chase work already landed (M46/M47/M52–M55). Complements
`tools/PERF_AUTHORITY.md` (build-time + publication) and is bounded by
`docs/agent/DETERMINISM_PRESERVING_PERF.md`.

**Determinism classes** (M72; the witness Kernel-A parity bar is bit-exact on gated arrays):
- **BIT-SAFE** — preserves per-element results exactly; legal on parity-gated paths.
  (Reassociating *independent* outputs, exact-integer identities, layout/alloc changes.)
- **BIT-UNSAFE** — reorders/rounds a reduction or relaxes fp; legal ONLY off parity-gated
  paths, and MUST be gated by `determinism_perf_gate`. (FP accumulator split, reciprocal
  multiply, FMA contraction, relaxed-simd.) **gaussian_filter σ=2.0 → `m_smooth` must keep
  serial accumulation rounding** (the `crit_min` 630-tie); never vectorize its reduction.

Every claim below is verifiable against the manual page cited; every LANDED tag must be
re-verified against the tree before relying on it (M05 — a tag is a hypothesis).

---

## A. Native codegen (Cranelift → native exe)

| Lever | Agner ref | molt status | Determinism | Measure |
|---|---|---|---|---|
| Loop-carried FP accumulator split (N accumulators, 3–4 optimal) | §11 p113–114 | **LANDED** regular int/float for/while raw-lane green (M46, dcc00a506) | BIT-UNSAFE (reduction reorder) — raw-lane only, gated | `molt_diff.py --jobs 1`; perf_scoreboard |
| **Do NOT unroll when no loop-carried dep** — OOO + register renaming overlap iterations for free | §11 p114 | **doctrine** — validates M46 "don't re-chase regular loops" | n/a | — |
| Integer mul by constant → shifts/LEA (×2ⁿ, ×3/5/9) | §14.4 p149 | Cranelift lowers; **OPEN (verify)** the nan-box-tagged int `*` path emits it | BIT-SAFE | disasm probe; `probe_int` |
| Integer div/mod by constant → magic multiply-shift; ÷2ⁿ→shift; unsigned faster | §14.5 p150–151 | **OPEN (verify)** Python `//` and `%` by const lower to magic (esp. tagged-int lane) | BIT-SAFE (exact) | disasm; microbench div-heavy loop |
| Int-mul CheckedMul peel / `smulhi` overflow-into-bignum | §14.4 | **LANDED** M47 (261efc7b2), 1.65× CPython | BIT-SAFE | done |
| FP div by constant → reciprocal multiply; common-denominator; div-elimination | §14.6 p152 | **OPEN** — only when divisor exactly representable, else gated | BIT-UNSAFE (reciprocal rounds) | ulp-diff before landing |
| Induction-variable strength reduction; LICM; CSE; devirtualization | §8.1 p73–76 | mostly Cranelift/aegraph; **OPEN (verify)** molt TIR hoists loop-invariant Repr moves | BIT-SAFE | `molt-check` TIR validator (M50) |

## B. WASM backend (simd128 / relaxed-simd — M03)

| Lever | Agner ref | molt status | Determinism | Measure |
|---|---|---|---|---|
| Vectorize element-wise array ops across **independent** outputs | §12.3/12.6 p118,129 | **OPEN — highest runtime prize** for numpy ufuncs; the demo trunk (matmul/FiLM/hosc) is a clean target (pact 011) | **BIT-SAFE** iff each lane is an independent output pixel (M72) | WGSL/WASM parity harness `check_parity.py` |
| Vectorize a **reduction** (sum/dot/gaussian) | §12.6 p129 | **BLOCKED on gated paths** — reorders summation | BIT-UNSAFE — off-parity only, gated | determinism_perf_gate |
| CPU dispatch: simd128 vs scalar fallback; relaxed-simd (FMA) variant | §13 p135–140 | **OPEN** — one authority path, no `--export-all`-style duplication | relaxed = BIT-UNSAFE (gated); simd128 fp = BIT-SAFE if op-order preserved | per-target scoreboard |
| Aligned load/store for vectors | §12.4/12.8 p124,133 | **OPEN (verify)** array data 16-byte aligned before simd | BIT-SAFE | — |

## C. Runtime hot paths

| Lever | Agner ref | molt status | Determinism | Measure |
|---|---|---|---|---|
| Integer ops on float bits (sign flip, abs, ×2ⁿ via exponent, compare-as-int) | §14.9 p154–156 | **LANDED as the NaN-box scheme itself** | BIT-SAFE (bit-exact) | — |
| **Caveat: don't round-trip a register-resident value through memory/union; half-double writes → store-forward stall** | §14.9 p157 | **OPEN** = M48 loop-unbox: keep values unboxed in registers across a hot loop body; box once at the boundary | BIT-SAFE | unbox-hoist microbench; M36 correctness note |
| "Variables used together stored together"; align to cache line; power-of-2 object size only for random access | §9.4/9.5 p93–95 | **OPEN (audit)** hot object header layout (refcount + type-tag + payload adjacency) | BIT-SAFE | cache-miss counters (§16.1) |
| Dynamic-alloc cost → pool small objects | §9.6/7.1 p28,95 | **LANDED** buffer-export 1112B box pool (M55); **OPEN** general small-object pool | BIT-SAFE | alloc-rate; perf_scoreboard |
| Inline; avoid nested calls in innermost loop; fastcall/vectorcall; local linkage | §7.14 p48–50 | **OPEN (verify)** call ABI in hot dispatch; trampoline recently canonicalized | BIT-SAFE | call-overhead microbench |
| Tail-call elimination | §7.17 p51 | **OPEN** — wasm tail-call proposal for self/mutual recursion | BIT-SAFE | recursion microbench |
| Replace branch chains with lookup tables; bitwise multi-value tests | §7.12/14.1/14.3 p43,144,148 | **OPEN (verify)** type-tag dispatch is table not branch-chain | BIT-SAFE | branch-mispredict counters |
| Bounds-check elimination on proven-safe indices | §14.2 p147 | **BLOCKED** M47 GAP-3 dynamic-IV BCE (memory-safety) | BIT-SAFE | — |

## D. Build-time / whole-program (host compiler — M09)

| Lever | Agner ref | molt status | Measure |
|---|---|---|---|
| Profile FIRST — hot spots, clock-cycle budget | §3.1/3.2 p15–16 | **doctrine** M10 | — |
| Pure-function attribution enables CSE/DCE/reorder across calls | §8.3 p83 | **LANDED foundation** op_kinds/`writes_heap` (M38); pure ops are CSE-able, may-raise = barrier | table-drift gate |
| Cross-module optimization / whole-program (LTO) for native output | §8.3 p81 | **OPEN (evaluate)** native-exe LTO vs build-time cost (M09) | build wall-clock + runtime scoreboard |
| Pointer-aliasing obstacles block vectorization | §8.3 p81–82 | **OPEN** — molt TIR can assert no-alias on freshly-allocated array outputs | — |

---

## Ranked OPEN levers (gain per validation cost — feeds opt-matrix rung selection)

Same discipline as `tools/PERF_AUTHORITY.md` "Backlog gain per validation cost". Estimates,
release-profile evidence required to land (A12 / `powerplay_acceptance.py`).

| Lever | Surface | Gain | Cost | Determinism |
|---|---|---:|---:|---|
| Vectorize independent-output ufuncs (simd128) | B | 10 | 5 | BIT-SAFE |
| M48 loop-unbox hoist (register-resident nan-box) | C | 8 | 3 | BIT-SAFE |
| Integer div/mod by constant → magic-number lowering (verify+close) | A | 6 | 2 | BIT-SAFE |
| Hot object-layout / cache-line audit | C | 5 | 3 | BIT-SAFE |
| Table-dispatch for type-tag (verify branch-chain→table) | C | 5 | 2 | BIT-SAFE |
| Native-exe LTO evaluation | D | 5 | 4 | BIT-SAFE |
| Tail-call elimination (wasm tail-calls) | C | 4 | 3 | BIT-SAFE |

**Rule:** any BIT-UNSAFE lever (FP accumulator split, reciprocal multiply, relaxed-simd,
reduction vectorization) may land ONLY on a non-parity-gated path with a
`determinism_perf_gate` guard; it is a **regression, not a win**, if it perturbs any gated
witness array. Correctness/determinism first, then distill (M75).
