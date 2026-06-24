<!-- Foundation blueprint 69. The benchmark-corpus union + dynamic calibration arc:
make molt's perf gate measure a union of ALL external suites (tougher than any one),
beat each engine on ITS OWN suite, and calibrate everything dynamically per-host.
Authored 2026-06-24. Governed by DESIGN_DOCTRINE.md + the Performance Constitution
(CLAUDE.md). Feeds doc 64 (scoreboards) and doc 65 (compression ladder). -->

# 69 — Benchmark Corpus Union + Dynamic Calibration

> **Beat everyone on their own suite; calibrate everything per-host.** molt's perf
> gate must measure a UNION of every available Python benchmark suite (more
> comprehensive and tougher than any single one), report molt-vs-CPython/PyPy/Codon
> on each engine's OWN suite with that suite's OWN methodology, and do it with
> calibration that is dynamic, host-aware, and cross-platform — never a hardcoded
> constant.

## 0. Posture — extend the mature subsystem, do NOT rebuild it

This is a *calibration + coverage* arc on top of substantial existing tooling. The
foundation to build on (verified 2026-06-24):

| Existing | Role | State / gap |
|---|---|---|
| `tools/bench.py` | warm multi-engine throughput (molt vs CPython/PyPy/Codon/Nuitka/Pyodide auto-probed); mean+min+variance | works; corpus = molt-authored micros |
| `tools/bench_friends.py` + `bench/friends/manifest.toml` | the "beat them on their OWN suite" harness; per-suite × per-runner lanes; git source-custody; semantic_mode | **framework exists; external suites STUBBED** |
| `tools/pyperformance_adapter.py` | pyperformance lane | **smoke subset only (`nbody,fannkuch`)** |
| `tools/perf_scoreboard.py` | warm/cold verdict board (benchmark×target×backend×profile) | swarm's ACTIVE lane (coordinate) |
| `tools/perf_inner_repeat.py` | inner-loop repeat | stability primitive to extend |
| `tools/output_startup_size_audit.py` | cold-start matrix (same_path / page_cache_cold / cold_first_sighting) | sophisticated cold model already |
| `bench/scoreboard/cold_start_budget.json` | per (backend,profile) cold tax budget | **STATIC, macOS-seeded (380ms) — the Windows false-red root** |
| `tools/suite_honesty/` | down-only ratchet; `semantic_mode` vocabulary | reuse the tier vocabulary for cross-engine fairness |

The gaps this arc closes: (1) the corpus is molt micros, not the standard suites;
(2) the friend suites are disabled / unpinned / under-adapted; (3) calibration is a
static macOS constant, not host-dynamic, and quiescence is Unix-only (fail-closed on
Windows → false `UNSTABLE`).

## 1. The union corpus ("tougher than all")

**Canonical-benchmark registry** (`bench/corpus/registry.toml`, NEW): one entry per
*canonical* benchmark → `{id, source_suites[], category, parametrized sizes,
semantic_tier_by_engine}`. Dedup across suites (nbody / fannkuch-redux /
spectral-norm / mandelbrot recur in pyperformance + Codon + PyPy + Benchmarks Game →
ONE canonical entry tagged with every source). The union = **every suite's
benchmarks ∪ molt-stress cases** (the programs that broke molt — `bench_sum`
large-int accumulation, resurrection/finalizer, exception-heavy, megamorphic
dispatch). "Tougher than all" = superset coverage + the hardest case per category +
size parametrization (so a win must hold across scales).

**Sources** (each a `bench_friends` suite adapter): pyperformance (full ~60),
`pypy/benchmarks`, Codon numeric kernels (exaloop/codon), the Computer Language
Benchmarks Game (nbody, fannkuch-redux, spectral-norm, mandelbrot, fasta,
k-nucleotide, regex-redux, binary-trees, reverse-complement, pidigits), pyston,
real-world (web request handling + templating, json/pickle/msgpack/protobuf,
asyncio, ETL/data pipelines, ML inference via the tinygrad/DFlash path), and the
existing molt micros. **Full external catalog: appendix A (corpus-research).**

**Semantic tiers** — reuse `suite_honesty`'s vocabulary per engine: `runs_unmodified`
/ `requires_adapter` / `unsupported_by_molt`. CPython = the universal oracle (all);
Codon = the statically-compilable subset, semantics **non-drop-in** (mark, never a
win/loss on divergent cases); PyPy = pure-Python dynamic. A comparison is only a
win/loss when both engines are semantically equivalent on that benchmark — otherwise
it is reported "non-equivalent," per the constitution.

## 2. The adapter model (suite-agnostic) — extend `bench_friends`, don't fork it

Each external suite is ALREADY a `manifest.toml` entry with per-runner lanes. The
work is to make each one real:
1. **Pin** an immutable `repo_ref` (pyperformance is `1.14.0`; `pypy/benchmarks` +
   `codon` need pinned commits — appendix A).
2. **Adapt**: complete the thin adapter so it (a) maps the suite's benchmarks to a
   molt-runnable + reference-runnable form, (b) runs molt AND the reference engine
   with the SUITE'S OWN methodology (pyperf for pyperformance; the suite's own driver
   for pypy/codon), (c) emits cells into the unified scoreboard. `pyperformance_adapter`
   exists (extend past the smoke subset); pypy/codon adapters are new but follow the
   tinygrad/numpy adapter pattern already in the tree.
3. **Enable** + record source custody (already enforced by `bench_friends`: pinned
   ref, clean-tree, no runner-created artifacts).

**"Beat them on their own suite"** = molt's number > the reference engine's number on
the reference engine's OWN benchmark, measured with the reference's OWN methodology.
This is the honest, unimpeachable claim. Per-suite × per-engine boards feed the five
scoreboards (CPython/PyPy/Codon/Backend/Profile).

## 3. Dynamic + sophisticated calibration (a first-class subsystem)

**NEW `tools/perf_calibration.py`** — calibration is its own module that
`perf_scoreboard.py` *consumes* (a thin hook), so the logic is not buried in the
swarm's active board file.

- **Host fingerprint** keys ALL calibration: `{os, cpu_model, physical_cores,
  logical_cores, ram, freq governor / turbo state}`. Persisted to
  `bench/scoreboard/host_calibration/<fingerprint>.json`. A board records the
  fingerprint it was measured under; a budget from a different host is never silently
  applied.
- **Cold-start budget — per-host dynamic.** Replace the static macOS 380ms seed with
  a calibration step: measure the host's irreducible cold-start (a minimal program,
  N cold runs via `output_startup_size_audit`'s `cold_first_sighting` model) → seed
  `budget_ms = measured_host_floor + workload_margin`. v0 stays "current measured
  baseline" per council ruling A — but **per host**, not one macOS constant.
  Recalibrate when the fingerprint changes or on `--recalibrate`. (Resolves the 6
  Windows `FAIL_COLD_BUDGET` false reds: Windows ~1000ms first-run page-in is the
  host floor, not a regression — the artifact-footprint lever (doc 62) attacks the
  floor; the budget bounds *regression from it*.)
- **Cross-platform quiescence (sophisticated).** Replace `pgrep`/loadavg (Unix-only;
  fail-closed → false `UNSTABLE` on Windows) with **psutil**: system CPU%, per-core
  load, competing-process detection, and (where available) thermal-throttle state —
  on Windows/macOS/Linux uniformly. Not a binary fail-closed: report the measured
  contention level; gate a *promotion to RED* on quiescence, never a measured WIN
  (load can only slow molt — a win under contention is conservative).
- **Adaptive sampling + CI (pyperf-grade).** Auto-calibrate the inner-loop count to a
  target per-sample duration (extend `perf_inner_repeat`); take adaptive repeats until
  the confidence-interval half-width < threshold (or a max-sample cap); detect +
  discard warmup; handle outliers; report **median + CI + CV**, not a bare point
  estimate. Resolve `UNSTABLE` by *sampling more*, not flagging. (No optimizing from a
  noisy red; classify GREEN/RED only when the CI clears the threshold — the
  constitution.)

## 4. Integration into the five scoreboards + CI

The union corpus feeds all five boards (doc 64). The dynamic calibration makes them
trustworthy cross-platform (no Windows false reds; CI-backed verdicts). CI
(`tools/ci_gate.py`, swarm's lane) gates on the union with the host-calibrated
budgets. Every reported cell carries: `benchmark → canonical id → source suite →
target → backend → profile → CPython/PyPy/Codon ratio (cold AND warm) → CI → host
fingerprint → semantic tier`.

## 5. Decomposition (executable, parallel, each a green measured landing)

**Calibration (new module + thin coordinated hooks):**
- **C1** `perf_calibration.py`: host fingerprint + per-host cold-budget calibration; migrate `cold_start_budget.json` to host-keyed. *(coordinate the `perf_scoreboard.py` consume-hook with the swarm.)*
- **C2** cross-platform quiescence via psutil (replace the Unix-only probe). *(coordinate hook.)*
- **C3** adaptive sampling + CI (extend `perf_inner_repeat`); board reports median+CI+CV.

**Corpus union (extend `bench_friends`; mostly new adapters — non-colliding):**
- **R** the canonical-benchmark registry (`bench/corpus/registry.toml`) + dedup + semantic tiers.
- **S1** pyperformance: full adapter (past the `nbody,fannkuch` smoke subset) + enable.
- **S2** `pypy/benchmarks`: pin commit + adapter + enable.
- **S3** Codon numeric kernels: pin commit + adapter + enable (mark non-equivalent semantics).
- **S4** Benchmarks Game kernels: vendor (permissive) + register.
- **S5** real-world workloads: web / serialization / async / ETL / ML.

Sequencing: C1–C3 first (they make EVERY board trustworthy — the measurement
substrate), then S1 (pyperformance is the CPython reference + already pinned), then
S2/S3 (PyPy/Codon references), then S4/S5. R is built incrementally as each S lands.

## 6. Invariants (constitution + doctrine)

Quiescent, repeated, attributed, classified — every claim. Cold AND warm. CI, not
vibes. Non-equivalent semantic models marked non-equivalent, never a win/loss. One
canonical registry (no duplicate benchmark truth). Host-fingerprinted (no
cross-host budget bleed). A suite is "integrated" only when molt is measured against
the reference engine on the reference's own benchmarks with the reference's own
methodology — and a "win" is only claimed where the semantics are equivalent.

## Appendix A — corpus-research catalog
*(Enumerated by the corpus-research sweep — the complete external-suite landscape:
per-suite benchmark lists, methodology, acquisition, license, semantic tier, and
cross-suite overlap. Appended when the research lands; seeds R + S1–S5.)*
