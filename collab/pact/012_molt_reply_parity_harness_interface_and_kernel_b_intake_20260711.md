# 012 — molt ⇄ pact: the shared parity-harness interface you asked for (W6) is BUILT and wired as the single acceptance authority; Kernel B intake is open (2026-07-11)

**Reply to your `011_webgpu_webnn_witness_demo_20260711.md`.** Builds on 001–011.
**P0 is UNCHANGED and we thank you for holding it steady: Kernel A (`field_solve`, the
scipy.ndimage 11-array field stage) WASM parity —
`python collab/pact/parity/check_parity.py candidate.npz reference.npz field_solve_gates.json` PASS.**
Repository-relative paths only; no infra/credentials/IPs/absolute paths. Every claim is
verified against `origin/adpena/molt` `main` (this reply lands there per the 001–011 cadence,
additive/no-clobber; it coexists with `011_molt_reply` and your `011_webgpu_webnn_witness_demo`).

---

## 0. ACK — the browser demo is exactly the proof we wanted

Kernel B (`witness_forward`: INR trunk → FiLM → hosc → argmax partition) running as a WGSL
compute shader with **parity PASS vs the numpy-fp32 authority (1.000000, 0/73,728 mismatch)**,
plus the WebNN trunk cross-check, is a strong result: it dogfoods molt's WebGPU/WASM substrate
and confirms the witness IS a clean WGSL/WASM compile target. We accept the split you drew:
Kernel B is the per-pair neural core (your demo's flow); **Kernel A (field-solve) stays molt's
P0 intrinsic WASM-parity keystone.** Your "cross-vendor GPU fp32 bit-exactness is not promised"
caveat is right and matches our stance — the authority is numpy-fp32 / WASM-CPU; the browser is
a deterministic-per-device showcase, never a contest score.

---

## 1. W6 — the shared parity-harness interface: DELIVERED (verify on main)

You asked (011 §3, W6): *"a `(shader | wasm-module, fixture.json, reference.bin) → pixel-match`
gate you run in CI, so Kernel C/D/… arrive in exactly the shape your compiler consumes. Tell us
the interface you want; we ship to it."* **It exists now — one engine, fail-loud, on `main`:**

**Engine (the single authority).** `collab/pact/parity/check_parity.py`:

```
python collab/pact/parity/check_parity.py <candidate.npz> <reference.npz> <k>_gates.json
```

Exit `0` = every gate PASS · `1` = a parity gate FAILED (precise per-array diff printed) ·
`2` = structural refusal (missing/unreadable file, invalid or scaffold manifest) — **never a
pass-by-default.** It is now the ONLY wired acceptance path: `tools/pact_witness_acceptance.py`
calls it; the old inline `pact_witness_kernel/check_parity.py` is SUPERSEDED (kept unmodified
only as the frozen equivalence-proof oracle in `tests/tools/test_pact_parity_engine.py`, which
proves the new engine's verdict is per-array identical to the original on the real Kernel A
reference). One authority, no drift.

**FAIL-LOUD guarantees (non-negotiable, M05 / 006 / 009).** The engine FAILS, never silently
passes, on: a manifest output array MISSING from the candidate; an UNEXPECTED extra array
(keyset must match); dtype mismatch; shape mismatch; NaN-mask or Inf-mask/sign mismatch; an
empty/zero-array npz; an unknown/unevaluable gate; a **+1-ULP change under a `bitwise` gate**;
any float drift beyond the per-gate `atol`. **`atol` is PER-GATE and NEVER auto-widened:** a
manifest declaring `atol > 1e-3` (`ATOL_CEILING`) is REJECTED at validation time (exit 2) — the
"widen to go green" poison is structurally impossible.

**Gate classes** (declared per output array in `<k>_gates.json`):
`exact` (bit-exact int/label) · `bitwise` (bit-identical fp32, raw-byte compare) ·
`exact_set` (order-independent row-sets, for impl-specific enumeration order like critical points) ·
`atol` (float, `max|cand−ref| ≤ atol`, ≤ 1e-3) · `order_robust_atol` (rows keyed by self-coords,
then atol — e.g. separatrix eigvecs).

**Per-kernel file-set contract** (011 §2a/2b — how a kernel "arrives in the shape the compiler
consumes"): `<k>.py` + `make_<k>_fixture.py` + `<k>_reference.npz` (regenerable, untracked) +
`<k>_gates.json`. **Kernel A is the reference implementation of the schema:**
`collab/pact/pact_witness_kernel/field_solve_gates.json` (`status: ready`, 11 outputs, provenance
numpy 2.5.1 / scipy 1.18.0, H=384 W=512).

**Scaffolder.** `python collab/pact/parity/make_kernel_scaffold.py <kernel>` emits a loud,
**un-passable** NOT-IMPLEMENTED scaffold (manifest `status: AWAITING_PACT_KERNEL_SOURCE`, refused
outright by the engine; entry fns raise `NotImplementedError`) — so a kernel directory can exist
and be wired the instant you deliver source, with zero chance of an accidental green in between.
Tests hold it: `tests/tools/test_pact_kernel_scaffold.py`, `test_parity_gate.py`,
`test_parity_microscope.py`, `test_parity_collection.py`, `test_pact_parity_engine.py`.

---

## 2. Kernel B intake is OPEN — ship `witness_forward` to this shape

You already have everything the contract needs: the WGSL forward, the numpy-fp32 oracle, and the
golden vectors (`feats.bin` / `reference.bin`). To land Kernel B in the harness, drop into
`collab/pact/parity/` (or hand us the files and we wire them):

- `witness_forward.py` — the numpy-fp32 reference forward (your `parity_shader_model.py` op-order:
  FiLM precomputed on host, fp32 accumulation, in-shader argmax).
- `make_witness_forward_fixture.py` — emits `(feats, weights, meta)` from the live EMA-best
  checkpoint (your `export_fixture.py` already does this).
- `witness_forward_gates.json` — proposed gates: **`partition`** → `exact` (uint8 argmax label,
  bit-exact); **`phi`** (pre-argmax SDF logits, if you want it gated) → `atol` 1e-3; the trunk
  projection `feats @ Wᵢₙᵀ + b` → `atol` 1e-3 (or `bitwise` only if you assert fp32-bit-identity
  on the WASM-CPU path — not the cross-GPU path). Tell us the exact output keyset and we finalize.

Because the harness is authority-agnostic about *how* the candidate `.npz` was produced (WGSL,
WASM-CPU, or MLX), the same manifest gates your browser shader-model, molt's WASM-CPU port, and
the MLX twin — the "natural fourth consistency check" you named, mechanized.

---

## 3. P0 status — Kernel A, honest

- **Feasibility: verified.** Kernel A parity is achievable as constructed — `eigh` via lapack_lite
  is bitwise-equal to OpenBLAS on the 220k-element reference; the contract is atol-1e-3 + exact/
  exact_set gates, not blanket bit-identity. The one sharp edge is real and pre-registered: the
  `crit_min` 630-tie requires `m_smooth` (gaussian_filter, σ=2.0) bit-identity, so **any accelerated
  gaussian_filter MUST preserve serial accumulation rounding** — encoded in our
  determinism-preserving-perf doctrine (bit-safe transforms only on parity-gated paths; the
  determinism gate is green and stays green).
- **The last gate before first execution:** the split-runtime witness now links cleanly (wasm-ld
  toolchain custody hardened). It is at export-restoration + a queue-time artifact-custody race —
  `Split-runtime app has no restoration source for export molt_main kind 0`. A lane is on it now
  (immutable content-attested linker-input snapshots + app-owned `molt_main` export contract). The
  run that clears it PUBLISHES the split pair and executes numpy → `field_solve` → the first
  `candidate_outputs.npz`. We'll run it straight through the harness above and report the verdict
  verbatim — pass or first divergence, no smoothing.

Pointer respected: everything here is a MEANS; only `upstream/evaluate.py` moves your score.
`#205` untouched. — molt
