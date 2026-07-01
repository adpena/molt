# 008 — pact ⇄ molt: v2 witness-decoder consolidation — the geometric decode CHAIN as molt targets + rule-118 rate-half + contest-runtime contracts (2026-06-29)

**Builds on 001–007. Not re-litigating them.** Report `006` already handed you
the exact kernels + determinism gates (Kernel A field-solve, Kernel B INR
forward, the parity oracle), and `007` greened the NumPy/SciPy C-API
scan/missing-symbol layer (447+592 source files, zero missing symbols — thank
you, that was the big unblock). The shared exit criterion is unchanged:

```
python collab/pact/pact_witness_kernel/check_parity.py candidate_outputs.npz   # PASS, Kernel A first
```

This addendum is a **consolidation + extension**, adding four things the channel
has not yet captured in one place:

1. the **full deterministic geometric decode CHAIN** (SE(3) screw-warp + EON
   ground-plane homography, per-class) framed as molt compile targets — the
   architectural layer *above* the 006 viz/INR kernels;
2. the **rule-118 rate-half value prop** (why a faster molt decoder is a real,
   if indirect, score lever);
3. the explicit **contest-runtime hard contracts** (30-min budget on T4 *or*
   CPU; CPU and CUDA as separate axes);
4. the **runtime-rs Rust native sister-backend** relationship (one oracle, two
   backends).

---

## 1. The decode CHAIN (architectural context above Kernel A/B)

The witness has evolved from "store a learned representation and decode it" to
**store a tiny video-derived statistic and REGENERATE the task-space partition
from physics.** The contest source is one rigid ego-trajectory through a
mostly-static world observed by two frozen networks; the v2 decoder exploits
that directly. The decode chain, in order:

| Stage | Repo-relative reference | Role |
|---|---|---|
| **SE(3) screw-warp** | `src/tac/se3.py` | Matrix-Lie-group exp/log on SE(3)=SO(3)⋉ℝ³ (closed-form Rodrigues + small-angle Taylor fallback). Warps a canonical scene along the rigid ego screw motion. |
| **EON ground-plane homography** | `src/tac/camera.py` | comma-camera intrinsics + extrinsics → inverse-perspective-mapping homography, applied **per class** (ground-homography for road, identity for hood, rotation-only for sky). |
| **SDF rasterizer + level-set decode** | `src/tac/boundary_math/lane_sdf_component.py`, `.../hood_static_component.py` | The class partition is `argmax_k phi_k` over K=5 signed-distance fields; a parametric polynomial+dash lane band rasterizes to an SDF. (This is the structured form of Kernel A's field stage.) |
| **Coordinate-INR forward** | `src/tac/boundary_math/lever_b_generator.py` (and the `lever_b_levelset_generator` Kernel-B oracle from 006) | Deterministic Fourier/curvelet features → FiLM-per-pair modulation → small MLP → 5-class SDF head → argmax. The learned-residual generator for the boundary annulus the geometric prior can't fully place. |
| **range / entropy decode + integer decode** | (decode path) | Decodes the *counted* payload (AR-coded trajectory coords + small residual) and produces the integer witness the frozen scorer reads. |

So Kernel A/B from 006 sit **inside** this chain: SE(3)+homography supply the
warp that places the canonical scene per frame; the SDF/INR stages (your current
Kernel A/B targets) produce the partition. All stages are pure-numeric and
numpy-fp32-referenced — molt's wheelhouse. **No new acceptance target here**;
Kernel A WASM parity remains the milestone. This section just gives the molt
team the map of where the two kernels live in the full decoder.

---

## 2. The rule-118 value prop (why molt speed is a score lever)

Under the contest rules: the **rate term scores ONLY `archive.zip` bytes**;
`inflate.py`/`inflate.sh` are **not** sized; there is **no time term** — the only
runtime constraint is the **30-minute full-eval budget**. External **generic
algorithm / code is FREE**; only **video-derived learned artifacts are COUNTED**.

Therefore a molt-compiled fast deterministic decoder **does not change the byte
count by itself** (the generic generator is free either way). What it buys:

> A faster decoder lets us run a **more sophisticated free generator** inside the
> 30-minute budget, which lets the **counted payload shrink** — if the generator
> can expand a smaller sufficient statistic into the same witness within budget,
> we ship fewer counted bytes.

molt = "run a bigger free generator within budget" = a **direct enabler of the
rate half** of the sub-0.15 goal. **Honesty note (NO-FAKE):** molt is *not* a
rate win on its own — it is a speed/portability/within-budget enabler; the rate
win comes from generator design, and molt makes a more aggressive generator
*affordable*. We keep that distinction crisp.

---

## 3. Contest-runtime hard contracts (additions to the 006 determinism gates)

The 006 determinism gates (bit-exact vs numpy-fp32, the per-op scipy-parity
table) stand unchanged and remain the core contract. Two contest-runtime
contracts to add explicitly:

- **30-minute budget on contest hardware: T4 (16GB VRAM) OR CPU (4-core / 16GB).**
  The full-eval decode must finish inside the budget on the weaker target we
  pick. This is what makes the §2 "bigger free generator" affordable — or not.
- **CPU and CUDA are SEPARATE axes.** We never infer one axis's result from the
  other. A WASM-CPU parity pass does not imply a WebGPU/CUDA parity pass and vice
  versa; each is measured independently. (Sister of 006's "one path must
  reproduce the numpy-fp32 reference as the authority" — the *authority* path is
  per-axis.)

These compose with 006: the authority lane is bit-exact-fp32 **and** within
budget **and** axis-scoped.

---

## 4. OPEN QUESTION (sharpened): is WebGPU/WASM available in the contest's headless `inflate.sh` runner?

007 closed the C-API scan layer; the remaining compat question for the
**contest-legal** path is the runtime environment, not the symbol layer:

- The contest runs `inflate.sh` in a **headless CI-class runner** (GitHub-Actions
  family, T4 GPU or CPU, no browser/display). Is **WebGPU** reachable there, or
  is it browser-only in practice? If WebGPU is not present in that environment,
  the **contest-legal decode target is CPU-WASM (or native)**, and WebGPU is for
  the dogfooding showcase only.
- For the **CPU-WASM** path on a headless runner: is bit-exact fp32 (006 gate)
  achievable, and what is the realistic decode-throughput envelope vs the 30-min
  budget (§3)?

A short matrix — `{WASM-CPU, WebGPU} × {headless CI, browser} → supported /
needs-port / blocked` — lets us pick the contest-legal target with confidence
and reserve WebGPU for the showcase if it is browser-only. (This is downstream of
the *current* Kernel A WASM-parity milestone, not a substitute for it.)

---

## 5. runtime-rs: the Rust native sister-backend (one oracle, two backends)

pact also maintains a Rust native-lowering path (`runtime-rs/`) as a
deterministic-decode backend. **molt (Python → WASM/WebGPU) and runtime-rs
(Rust → native) are SISTER backends:**

- **molt** = the **browser / WASM / portable** sister (and the showcase engine);
- **runtime-rs** = the **native** sister (max-throughput contest runtime).

Both must pass the **same numpy-fp32 parity vectors** (the 006 oracle). The numpy
reference is the single source of truth; both native backends are promotable only
after bit-exact parity against it. This keeps the two paths honest and
interchangeable and prevents either from drifting into a divergent
re-implementation. molt and runtime-rs are not competitors here — they cover
different deployment surfaces behind one contract.

---

## Priority (relative to the live 007 milestone)

- **P0 (unchanged from 007):** Kernel A WASM parity →
  `check_parity.py candidate_outputs.npz` PASS. This addendum does not move P0.
- **P1:** the contest-runtime compat answer (§4) — gates whether the
  contest-legal target is CPU-WASM, native, or WebGPU.
- **P2:** keep the §3 contest-runtime contracts (30-min / per-axis) attached to
  the authority lane as it lands.
- **P3:** the WebGPU showcase (live in-browser re-solve) — highest-observability
  dogfooding, off the contest-legal critical path.

## Dogfooding / mutual-elevation signal (positive)

pact remains all-in on molt and vendors molt's memory-guard / safe-run
primitives as the containment substrate for measured-and-bounded compute runs;
they work well. The witness decoder is molt's flagship scientific-compute compile
target. Both sides gain: pact gets a fast, portable, deterministic witness
runtime + the interactive showcase + a clean dependency-light carrier core; molt
gets a serious numpy/scipy/extension dogfooding customer driving its WASM/WebGPU
numeric-parity maturity. The end on pact's side remains the sub-0.15 exact
contest score; molt is a means that accelerates and ports the decoder, it does
not by itself move the score.

*Disclosure hygiene: shared-repo artifact — no credentials, private-infra URLs,
local absolute paths, provider logs, or account metadata; source references are
repository-relative module paths only.*
