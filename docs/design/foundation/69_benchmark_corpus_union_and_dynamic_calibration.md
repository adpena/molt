<!-- Foundation blueprint 69. The benchmark-corpus union + dynamic calibration arc:
make molt's perf gate measure a union of ALL external suites + CPython's own
regression suite + real-world code (tougher than any one), beat CPython on EVERY
backend/target/profile/arch/OS/version and approach-or-beat Codon/PyPy, and calibrate
+ measure everything (time AND memory AND footprint) dynamically per-host.
Authored 2026-06-24. Governed by DESIGN_DOCTRINE.md + the Performance Constitution
(CLAUDE.md). Feeds doc 64 (scoreboards), doc 65 (compression ladder), doc 66 (parity
oracle); stdlib sourcing/collaboration via doc 70 (Monty). -->

# 69 — Benchmark Corpus Union + Dynamic Calibration

> **Faster than CPython on EVERY benchmark × backend × target × arch × OS × profile ×
> Python version; approaching or beating Codon and PyPy where their models apply.**
> Measure a UNION of every available suite + CPython's own regression suite +
> real-world code (tougher than any single one), each engine on ITS OWN suite with ITS
> OWN methodology, across all backends/profiles — and measure time AND memory AND
> footprint, calibrated dynamically per host. Cross-platform, cross-arch, and
> cross-version are the DEFAULT posture (version-gated), not opt-in lanes.

## 0. Posture — extend the mature subsystem, do NOT rebuild it

This is a *calibration + coverage + dimensioning* arc on top of substantial existing
tooling. The foundation to build on (verified 2026-06-24):

| Existing | Role | State / gap |
|---|---|---|
| `tools/bench.py` | warm multi-engine throughput (molt vs CPython/PyPy/Codon/Nuitka/Pyodide auto-probed); mean+min+variance | works; corpus = molt-authored micros |
| `tools/bench_friends.py` + `bench/friends/manifest.toml` | the "beat them on their OWN suite" harness; per-suite × per-runner lanes; git source-custody; `semantic_mode` | **framework exists; external suites STUBBED** |
| `tools/pyperformance_adapter.py` | pyperformance lane | **smoke subset only (`nbody,fannkuch`)** |
| `tools/perf_scoreboard.py` | warm/cold verdict board (benchmark×target×backend×profile) | swarm's ACTIVE lane (coordinate hooks) |
| `tools/perf_inner_repeat.py` | inner-loop repeat | stability primitive to extend (adaptive CI) |
| `tools/output_startup_size_audit.py` | cold-start matrix (same_path / page_cache_cold / cold_first_sighting) | sophisticated cold model already |
| `bench/scoreboard/cold_start_budget.json` | per (backend,profile) cold tax budget | **STATIC, macOS-seeded (380ms) — the Windows false-red root** |
| `tools/suite_honesty/` | down-only ratchet; `semantic_mode`; **per-(backend × CPython-version) dimensioning** | reuse the tier + version dimensioning for cross-engine + cross-version fairness |
| `safe_run.py` RSS poll | per-run peak RSS | **reads 0 on Windows — memory dimension broken there** |

Gaps this arc closes: corpus is molt micros not the standard suites; friend suites
disabled/unpinned/under-adapted; calibration is a static macOS constant not
host-dynamic; quiescence is Unix-only (false `UNSTABLE` on Windows); memory (RSS) is
unmeasured on Windows; boards are native/release-fast only, not the full
backend×target×profile×arch×OS×version matrix; no regrtest / real-world corpus.

## 1. The union corpus ("tougher than all")

**Canonical-benchmark registry** (`bench/corpus/registry.toml`, NEW): one entry per
*canonical* benchmark → `{id, source_suites[], category, parametrized sizes,
semantic_tier_by_engine, version_gates}`. Dedup across suites (nbody / fannkuch-redux
/ spectral-norm / mandelbrot recur in pyperformance + Codon + PyPy + Benchmarks Game →
ONE canonical entry tagged with every source). The union = **every suite's benchmarks
∪ molt-stress cases** (programs that broke molt — `bench_sum` large-int, resurrection,
exception-heavy, megamorphic dispatch). "Tougher than all" = superset coverage + the
hardest case per category + size parametrization (a win must hold across scales).

**Speed-suite sources** (each a `bench_friends` adapter): pyperformance (full ~60),
`pypy/benchmarks`, Codon numeric kernels (exaloop/codon), the Computer Language
Benchmarks Game (nbody, fannkuch-redux, spectral-norm, mandelbrot, fasta,
k-nucleotide, regex-redux, binary-trees, reverse-complement, pidigits), pyston, and
the existing molt micros. **Full external catalog: appendix A (corpus-research).**

**Correctness + stress corpora (first-class, beyond speed micros):**
- **CPython's own regression suite** — `Lib/test` run through `libregrtest`/`regrtest`.
  Simultaneously the ultimate **parity oracle** (the whole language + stdlib at scale,
  the exact tests CPython holds itself to) and a **stress corpus** (long-running,
  allocation-heavy, adversarial). molt must run it GREEN per Python version
  (version-gated) and, where timed, faster than CPython. Feeds the differential oracle
  (doc 66) + the memory/stress dimensions (§3a).
- **Real-world Python applications** — real packages/apps and their own test suites
  (web frameworks, data/scientific stacks, CLIs, async services). The workloads micros
  miss: large imports, real allocation patterns, breadth of stdlib, sustained load.

**Semantic tiers** — reuse `suite_honesty`'s vocabulary per engine: `runs_unmodified`
/ `requires_adapter` / `unsupported_by_molt`. CPython = universal oracle (all); Codon =
statically-compilable subset, semantics **non-drop-in** (mark, never a win/loss on
divergent cases); PyPy = pure-Python dynamic. A comparison is a win/loss ONLY where
both engines are semantically equivalent on that benchmark — otherwise "non-equivalent."

## 2. The adapter model (suite-agnostic) — extend `bench_friends`, don't fork it

Each external suite is ALREADY a `manifest.toml` entry with per-runner lanes. Make each
real: (1) **pin** an immutable `repo_ref` (pyperformance `1.14.0`; `pypy/benchmarks` +
`codon` need pinned commits — appendix A); (2) **adapt** — complete the thin adapter so
it maps the suite's benchmarks to molt-runnable + reference-runnable form and runs molt
AND the reference engine with the SUITE'S OWN methodology (pyperf for pyperformance; the
suite's own driver for pypy/codon), emitting cells into the unified scoreboard;
(3) **enable** with source custody (already enforced: pinned ref, clean-tree, no
runner-created artifacts). `pyperformance_adapter` exists (extend past smoke);
pypy/codon adapters follow the tinygrad/numpy adapter pattern already in tree.

**"Beat them on their own suite"** = molt's number > the reference engine's number on
the reference's OWN benchmark, with the reference's OWN methodology — the honest,
unimpeachable claim — only where semantics are equivalent.

## 3. Dynamic + sophisticated calibration (a first-class subsystem)

**NEW `tools/perf_calibration.py`** — calibration is its own module that
`perf_scoreboard.py` *consumes* (a thin hook), so the logic is not buried in the swarm's
active board file.
- **Host fingerprint** keys ALL calibration: `{os, arch, cpu_model, physical/logical
  cores, ram, freq governor/turbo, python_version}`. Persisted to
  `bench/scoreboard/host_calibration/<fingerprint>.json`. A budget from a different host
  is never silently applied.
- **Cold-start budget — per-host dynamic.** Replace the static macOS 380ms seed with a
  calibration step: measure the host's irreducible cold-start (minimal program, N cold
  runs via the `cold_first_sighting` model) → `budget_ms = host_floor + workload_margin`.
  v0 stays "current measured baseline" (council ruling A) — but PER HOST. Recalibrate on
  `--recalibrate` or fingerprint change. (Resolves the 6 Windows `FAIL_COLD_BUDGET` false
  reds: ~1000ms Windows first-run page-in is the host floor, not a regression; the
  footprint lever (doc 62) attacks the floor, the budget bounds regression from it.)
- **Cross-platform quiescence (sophisticated).** Replace `pgrep`/loadavg (Unix-only;
  fail-closed → false `UNSTABLE` on Windows) with **psutil**: system + per-core CPU%,
  competing-process detection, thermal-throttle state — Windows/macOS/Linux uniformly.
  Not binary fail-closed: report the measured contention; gate a *promotion to RED* on
  quiescence, never a measured WIN (load only slows molt — a win under load is
  conservative).
- **Adaptive sampling + CI (pyperf-grade).** Auto-calibrate inner-loop count to a target
  per-sample duration (extend `perf_inner_repeat`); adaptive repeats until the CI
  half-width < threshold (or a max cap); warmup detect+discard; outlier handling; report
  **median + CI + CV**. Resolve `UNSTABLE` by sampling more, not flagging. (No optimizing
  from a noisy red; classify GREEN/RED only when the CI clears the threshold.)

## 3a. The full matrix — every backend × target × arch × OS × profile × version × dimension

**The bar (non-negotiable).** molt must be **faster than CPython on EVERY benchmark in
the union, on EVERY backend (native / LLVM / WASM / Luau), EVERY target + arch (x86_64,
aarch64, wasm32), EVERY OS (Windows / macOS / Linux), and EVERY profile (dev-fast,
release-fast, release-output)** — and **approaching or beating Codon** (AOT static-subset
north star) and **PyPy** (dynamic reference) on the classes their models fit. No cell is
exempt; a single sub-1.00× CPython cell on any axis is a contract violation, not "later."

**Cross-platform, cross-arch, cross-version — BY DEFAULT (version-gated).** Matrix axes
explicitly include OS, arch, and Python version (3.12 / 3.13 / 3.14). Parity AND perf
expectations are **dimensioned and gated by Python version** — molt targets exact CPython
semantics per version; a 3.13/3.14-only behavior (or a removed 3.12 one) is an explicit
**version gate**, never a silent mismatch. Reuse `suite_honesty`'s per-version
dimensioning + the doc-66 oracle's version axis. A board cell is valid only for the
`(os, arch, python_version)` recorded in its provenance + host fingerprint (§3). Default
posture, not an opt-in lane.

**Multi-dimensional measurement (time AND memory AND everything).** Every cell records,
beyond wall-time:
- **time:** warm (steady-state engine) + cold (end-to-end), median + CI (§3).
- **memory:** **peak RSS** + **allocation count/bytes** + live-object high-water (the
  runtime's `MOLT_PROFILE_JSON` dealloc/alloc counters). Memory is a first-class **gated
  dimension** — a regression here is a `DIMENSIONAL` loss reported honestly, never hidden
  behind a warm-time win.
- **footprint:** binary size, compile time, cold-start tax (doc 61/62 levers).
- **stress:** sustained / large-input / long-running / memory-pressure variants (regrtest
  + real-world + sized stress cases) — molt must hold its lead under load, not just warm.

**Harden memory measurement cross-platform (a found gap).** The native board reported
`RSS = 0` on Windows — `safe_run.py`'s poll does not capture peak there. Fix in the
calibration subsystem (C4): uniform peak-RSS via **psutil** (peak working set on Windows,
`ru_maxrss` on macOS, `/proc/self/status` `VmHWM` on Linux) + the runtime's allocation
counters. Memory measured identically on every OS, or it is not a gate.

## 4. Integration into the five scoreboards + CI

The union feeds all five boards (doc 64) across the full matrix (§3a). Dynamic
calibration (§3) makes them cross-platform-trustworthy (no false reds; CI-backed
verdicts). CI (`tools/ci_gate.py`, swarm's lane) gates on the union with host-calibrated
budgets. Every cell carries: `benchmark → canonical id → source suite → backend → target
→ arch → os → profile → python_version → CPython/PyPy/Codon ratio (cold AND warm, median
+ CI) → peak RSS → allocs → size → compile → cold tax → semantic tier`.

## 5. Decomposition (executable, parallel, each a green measured landing)

**Calibration substrate (new module + thin coordinated hooks):**
- **C1** `perf_calibration.py`: host fingerprint + per-host cold-budget calibration; migrate `cold_start_budget.json` to host-keyed. *(coordinate the `perf_scoreboard.py` hook with the swarm.)*
- **C2** cross-platform quiescence via psutil (replace the Unix-only probe).
- **C3** adaptive sampling + CI (extend `perf_inner_repeat`); board reports median+CI+CV.
- **C4** multi-dimensional measurement: uniform cross-platform peak-RSS (psutil) + alloc counters + size/compile/cold per cell — fixes the Windows `RSS=0` gap; memory becomes a gate.
- **C5** the cross-`(os, arch, python_version)` matrix runner + version gating across native/LLVM/WASM/Luau × profiles (reuse `suite_honesty` version dims + the doc-66 oracle).

**Corpus union (extend `bench_friends`; mostly new adapters — non-colliding):**
- **R** the canonical-benchmark registry (`bench/corpus/registry.toml`) + dedup + semantic tiers + version gates.
- **S1** pyperformance: full adapter (past `nbody,fannkuch` smoke) + enable.
- **S2** `pypy/benchmarks`: pin + adapter + enable.
- **S3** Codon numeric kernels: pin + adapter + enable (mark non-equivalent semantics).
- **S4** Benchmarks Game kernels: vendor (permissive) + register.
- **S5** real-world application workloads + their test suites.
- **S6** regrtest corpus: CPython `Lib/test` via `libregrtest`, version-gated — parity-at-scale + stress (memory, long-running).

**Stdlib sourcing + collaboration:** the Monty (Pydantic) Rust-interpreter stdlib/Rust
reuse + bidirectional contribution is its own arc — see **doc 70** (research in progress).

Sequencing: C1–C3 (calibration substrate) → C4 (memory/dimensions) → C5 (full matrix +
version gating) → S1 (pyperformance, CPython reference, already pinned) → S6 (regrtest —
the parity+stress backbone) → S2/S3 (PyPy/Codon references) → S4/S5. R grows as each
lands. **Every landing runs its cells across ALL backends × targets × profiles ×
(os,arch,version) — a native-only green is not a landing.**

## 6. Invariants (constitution + doctrine)

Quiescent, repeated, attributed, classified — every claim. Cold AND warm. Time AND
memory AND footprint. CI, not vibes. Faster than CPython on every cell of the full matrix;
non-equivalent semantic models marked non-equivalent, never a win/loss. One canonical
registry (no duplicate benchmark truth). Host-fingerprinted + version-gated (no cross-host
or cross-version bleed). A suite is "integrated" only when molt is measured against the
reference engine on the reference's own benchmarks with the reference's own methodology,
across the full matrix — and a "win" is claimed only where semantics are equivalent.

## Appendix A — corpus-research catalog
*(Enumerated by the corpus-research sweep — the complete external-suite landscape:
per-suite benchmark lists, methodology, acquisition, license, semantic tier, cross-suite
overlap, and pinned commits. Appended when the research lands; seeds R + S1–S6.)*
