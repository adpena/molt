# E1 Parity Feasibility — Kernel A (`field_solve`) WASM-vs-oracle, op by op

**Verdict: parity is ACHIEVABLE AS CONSTRUCTED.** No contract change is needed.
The acceptance (`collab/pact/parity/check_parity.py` +
`collab/pact/pact_witness_kernel/field_solve_gates.json`) is *not* blanket
bit-identical fp32 — it is `exact` on the two integer rasters, `exact_set` on
the three critical-point lists, and `atol<=1e-3` on the six float fields
(engine `ATOL_CEILING`, check_parity.py:68; the `bitwise` gate class exists at
check_parity.py:333 but **no Kernel A output uses it**). The ledger's
FP32-BAR row (docs/agent/PACT_CONTRACT_LEDGER.md:16-17, :80) defines the bar
identically; "bit-identical fp32" in lane summaries is shorthand for this.

Every hazard funnels through ONE quantity: **`m_smooth` must be bit-identical**
(the 006 contract already says so: 006 §gates line 99 — "the compiled filter
must return the bit-exact same float at the extremum or the critical-point set
changes"). Everything below is evidence that it is, with measured headroom,
plus the tooling (`tools/parity_microscope.py`) to localize any surprise in
minutes.

All experiments: numpy 2.5.1 / scipy 1.18.0 (the witness stack pinned by
`resolve_scientific_stack`), fixture `make_fixture.py` (384x512, pure exact
geometry — no libm, bit-identical everywhere). Experiment scripts + artifacts:
`C:\Molt\parity-lab\` (host-local lab, not tracked).

## 1. Op-by-op determinism classification (with empirical evidence)

| op | class | why / evidence |
|---|---|---|
| `distance_transform_edt` | **EXACT-SAFE** | scipy `_morphology.py:2584-2602`: int32 Maurer feature transform, then f64 `sqrt(sum((x-b)^2))`. Squared offsets < 2^19 are f64-exact; sqrt is IEEE correctly-rounded. Measured bit-identical across OpenBLAS/no-BLAS x MSVC/GCC x Win/Linux x SIMD-on/off. Drives `phi`, `m12`, `gap13`, `dist`, `sdf_argmax`. |
| `sort/argsort/lexsort/argmax` | **EXACT-SAFE** | comparison-based on bit-identical inputs; kernel canonicalizes all tie/enumeration order in-kernel (field_solve.py:99,:108,:126,:144). |
| `boundary` (neighbor compares) | **EXACT-SAFE** | integer compares only. |
| `gaussian_filter` weights (`np.exp`) | **LIBM-HAZARD, measured SAFE with headroom** | scipy `_filters.py:656-666` computes weights at runtime via `np.exp` on f64. Measured: native np.exp is **correctly rounded on all 16 exact weight inputs** (vs mpmath, 200-bit); **wasi-libc exp (wasm32-wasip1, sysroot 33) returns bit-identical results on all 16** (clang-compiled probe under wasmtime). On a broad 100k sweep the two exps DO differ — the agreement is a property of these inputs, so re-verify per kernel (microscope does). |
| weight normalization (`phi_x.sum()`) | **ACCUMULATION-HAZARD, absorbed** | the exact sum sits 0.4993 ulp from a rounding boundary for sigma=1.5 (worst case) and plausible reduction orders yield 2-3 distinct f64 sums. BUT: measured end-to-end, **±1..4-ulp perturbation of every weight leaves all 11 final outputs bit-identical** (f64-accumulate → f32-round absorbs it). Multi-ulp headroom, not knife-edge. |
| `correlate1d` accumulation | **EXACT-SAFE (structural + measured)** | serial fixed-order f64 loop in scipy C; FP reassociation is illegal at default flags, and wasm32 core has no scalar FMA so contraction cannot materialize. Measured bit-identical MSVC-wheel vs GCC-manylinux-wheel (`-ffp-contract=fast` build) — the loop provably does not contract differently. |
| `maximum_filter`/`minimum_filter` | **EXACT-SAFE** | comparison-based. |
| `percentile` | **EXACT-SAFE derived** | sort + linear interpolation, deterministic IEEE arithmetic on identical inputs. |
| `label` + component means | **EXACT-SAFE** | integer connectivity; enumeration order canonicalized in-kernel (field_solve.py:126); `int(mean)` = truncation of correctly-rounded exact-int division. |
| `np.gradient` | **EXACT-SAFE** | (f[i+1]-f[i-1])/2 ufunc arithmetic. |
| `np.linalg.eigh` (2x2) | **EXACT-SAFE (measured, structural)** | Kernel A eigh inputs are only 2x2 symmetric Hessians. Measured: **220,000 random + near-degenerate 2x2 matrices give bit-identical eigenvalues AND eigenvectors** between scipy-openblas LAPACK and the no-BLAS `lapack_lite` build (numpy 2.5.1 source-built, `-Dallow-noblas=true`, MSVC). Structural: dsyevd at n=2 reduces to the dlaev2 closed form — only +,-,*,/,sqrt (all correctly rounded; no libm, no BLAS), so any IEEE-754 stack incl. wasm32 reproduces it. Eigvec sign is canonicalized in-kernel; fixture eigengaps are huge (min relative gap 0.212). |
| `**1.5` in curvature (`np.power`) | **LIBM-HAZARD, absorbed by atol** | measured Windows-UCRT vs Linux-glibc wheels: 78/196608 f64 elements differ by 1-2 ulp in `kappa_raw`. Feeds only `curvature` (atol 1e-3, ~1e11x headroom) — and even those diffs vanish at the f32 output cast (final outputs measured bit-identical cross-OS). |
| `clip/where/bincount/stack` | **EXACT-SAFE** | comparisons/integer/layout ops. |

SIMD-dispatch (oracle side): numpy 2.5.1 wheels carry exactly ONE
above-baseline dispatch tier (`__cpu_dispatch__ == ['X86_V3']`; baseline
X86_V2). Measured: X86_V3 disabled vs enabled = **bit-identical across all 26
pipeline stages**. There is no AVX512/V4 tier in this wheel, so oracle
host-variance is nil for this pin — and now structurally pinned anyway (§3).

## 2. What the acceptance ACTUALLY accepts (contract-as-implemented)

* Engine: `collab/pact/parity/check_parity.py` — gate vocabulary at :72,
  `ATOL_CEILING=1e-3` at :68 (manifest rejected if wider), evaluators at
  :329-352. Fail-loud on missing/extra keys, dtype/shape drift, NaN/Inf
  mask mismatch.
* Manifest: `field_solve_gates.json` — `exact`: sdf_argmax, boundary;
  `exact_set` (row-set, order-free): crit_max_rc, crit_min_rc,
  crit_saddle_rc; `order_robust_atol` 1e-3 keyed (c,r): crit_saddle_eigvec;
  `atol` 1e-3: m12, gap13, m_smooth, curvature, dist.
* Oracle generation: `tools/pact_witness_acceptance.py:388`
  `_prepare_reference_oracle` (pip wheels via
  `proof_queue._pact_witness_acceptance_spec`, tools/proof_queue.py:3230).

The sharp edge (measured, matches 006 §gates "630/672 tied"): the
`crit_min_rc` keep-120 lexsort cut lands INSIDE a 630-pixel exact-tie group
of `m_smooth` values. The kernel's (row,col) tie-break makes selection
enumeration-independent but NOT value-perturbation-independent:
**±1 f32-ulp perturbation of m_smooth flips crit_min_rc's exact_set gate**
(measured; every other gate still passes). Hence: crit_min parity <=>
m_smooth bit-identity. Margins for the other gates are enormous
(crit_max: nearest window-max pixel is 5.8M f32-ulps from the pct90
threshold; crit_saddle: exact-safe chain end-to-end).

## 3. Alignment landed (no contract semantics touched)

1. **Oracle dispatch pin** — `tools/pact_witness_acceptance.py`
   `_prepare_reference_oracle` and `tools/pact_witness_oracle.py` now set
   `NPY_DISABLE_CPU_FEATURES=X86_V3` (setdefault; operator override wins) so
   the reference is generated on the wheel's portable baseline tier.
   MASK-PROOF: measured bitwise NO-OP on this host (all 26 stages identical
   with the tier on/off) — the pin cannot absorb a candidate divergence, it
   only removes oracle host-variance. Tests capture the generation env on
   both lanes (tests/tools/test_pact_witness_acceptance.py).
2. **First-divergence microscope** — `tools/parity_microscope.py` (§4).
3. NOT landed, deliberately: PYTHONHASHSEED pinning (kernel has no
   hash-ordered numerics — pure numpy/scipy dataflow); any gate/manifest
   change (none needed).

Cross-OS note: a Linux-generated oracle is measured bit-identical to the
Windows one on ALL 11 final outputs (the 1-2-ulp glibc-vs-UCRT `pow`
divergence in f64 `kappa_raw` dies at the f32 cast). Oracle platform is
therefore NOT currently load-bearing — but keep generating it on the
acceptance host anyway (one attested path).

## 4. The first-divergence microscope

`tools/parity_microscope.py` — four subcommands:

* `run --fixture lstar.npz --out stages.npz [--final-out cand.npz]
  [--perturb STAGE=ULPS[@i,j]]` — staged execution persisting all 26
  intermediates (drift-gated bit-identical to `field_solve()` by
  `tests/tools/test_parity_microscope.py`); deterministic k-ulp injection
  models libm/accumulation drift at any stage.
* `compare cand_stages.npz ref_stages.npz` — FIRST diverging stage in
  pipeline order + producing op + hazard class + element indices + exact
  int64-ordinal ulp histogram + accumulation-vs-algorithmic classification.
* `final candidate_outputs.npz reference.npz` — the wasm-candidate surface
  (no intermediates needed): maps diverging output keys onto the pipeline
  DAG, reports the earliest frontier op, then prints the shared-engine gate
  verdict (one authority, reused not duplicated).
* `margins stages.npz` — the feasibility certificate: distance of every
  value to every decision threshold the exact/exact_set gates depend on
  (percentile cuts, window-max ties, keep-40/120 cut structure, eigh
  eigengaps).

Teeth (all in `tests/tools/test_parity_microscope.py`, uv child pinned to the
witness stack): staged==kernel bit-identity; clean self-compare; injected
single-ulp at m_smooth[200,256] localized to exactly that stage AND index
with a 1-ulp histogram; injected weight-ulp caught at `w_gauss_s2` while all
finals stay bit-identical; final-mode frontier localization with crit_min_rc
as the only failing gate; margins reproduce the documented 630-tie structure.

## 5. Feasibility verdict + the one real dependency

**Achievable as constructed.** The candidate must satisfy exactly one
non-trivial numeric property: m_smooth bit-identity, which decomposes into
(a) gaussian weights bit-identical — PROVEN available (wasi exp correctly
rounds all 16 inputs) with ≥4-ulp measured headroom even if it didn't; and
(b) fixed-order, non-contracted correlate1d accumulation — structural on
wasm32 (no scalar FMA exists) and measured stable across MSVC/GCC-fast-contract
natively. Everything else has huge margins or is comparison/integer-exact.

Sharp constraint for future molt optimization work: any accelerated/rewritten
`gaussian_filter` path (SIMD lowering, WebGPU, fusion) MUST preserve the
serial per-pixel accumulation ROUNDING of scipy's correlate1d, or crit_min_rc
breaks — re-run `parity_microscope margins` + the perturbation stress before
swapping that op. JFA-style approximate EDT stays out of the authority path
(006 already mandates exact EDT).

If the first real wasm candidate still fails a gate, the microscope turns it
into a stage+index+ulp report in one native run + one `final` call.

## 6. Kernel B outlook (next acceptance, flagged early — NOT Kernel A scope)

Kernel B (`witness_forward.py`: matmul + sin/cos/tanh/exp over wide ranges,
gate = exact uint8 argmax) does NOT inherit Kernel A's luck: measured, wasi
exp != native np.exp on a 100k-value sweep (checksums differ), and oracle
matmul is OpenBLAS sgemm (blocked/SIMD accumulation order) vs whatever the
wasm build does — f32 last-ulp drift is EXPECTED there. Feasibility of the
exact-argmax gate depends on the argmax margins of real φ, exactly as the
ledger already anticipates (FP32-BAR: "argmax-margin tolerance gate if
exact-uint8 is too strict"). Recommendation: before compiling Kernel B, run
this same microscope pattern (stage the pipeline, measure argmax margins vs
plausible ulp drift) — the tooling generalizes; that decision is pact-owned
and pre-authorized by the ledger row, not a contract change.
