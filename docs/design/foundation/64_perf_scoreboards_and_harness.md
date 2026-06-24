<!--
  Foundation blueprint — Arc: PERFORMANCE MEASUREMENT INFRASTRUCTURE
  ("speed is a release-gating correctness property" — CLAUDE.md Performance Constitution;
   doc 51 §3 "the four+1 scoreboards, kept green, CI-gated").
  Author: portfolio-architect
  Date: 2026-06-23
  Status: DESIGN ONLY / EXECUTABLE PLAN (no code in this change; the lead integrates).
  Assigned path: docs/design/foundation/64_perf_scoreboards_and_harness.md in the
  active 54-67 portfolio route cluster.
-->

# 64 — The Perf Measurement Plane: Five CI-Gated Scoreboards + the pyperf Harness

> **One-line thesis.** Today molt *can* measure speed (`tools/perf_scoreboard.py`,
> 4903 lines, is excellent) but speed is **not gated**: no CI job runs the gate, the
> "five scoreboards" are *columns on one board* rather than five independently-gated
> machine-readable artifacts, PyPy/Codon are wired-but-host-absent with no structured
> "missing-mechanism" fact, Luau lives in a *separate* tool, and "name the missing
> fact" is a human typing a guess into `suspected_missing_fact`. This arc makes "what
> did this commit do to speed?" an **automatic, machine-answered, merge-blocking**
> question — by turning the existing single board into a **measurement PLANE** with one
> evidence schema, five gated projections, a board-level history (so *regressions* gate,
> not just absolute reds), and a derived (not typed-by-hand) fact-attribution path.

---

## 0. The end-state outcome (stated crisply)

**In five years, no molt commit can change performance without the system saying so,
and no human types a perf number into a doc.** Concretely, the steady state is:

1. **Every PR** gets an automatic comment / status check: *"warm perf matrix GREEN; 0
   CPython-red benchmarks; 0 previously-green regressions; PyPy deltas: 3 named-missing-
   mechanism cells; Codon deltas: 2 (1 non-equivalent); binary/RSS/cold/compile within
   budget."* — or it **fails to merge**. This is the "Landing report format" of CLAUDE.md
   produced **by a machine**, not asserted by an agent.
2. **Five machine-readable scoreboards exist as durable, separately-gated artifacts**
   (doc 51 §3): CPython, PyPy, Codon, Backend (native/LLVM/WASM/Luau each its own
   table), Profile (dev-fast/release-fast/release-output each its own table). A native
   win **cannot** hide a WASM regression; a release-output win **cannot** hide a
   release-fast regression — because they are *different artifacts with their own gate
   exit codes*, not cells the eye skims past.
3. **Every perf claim is the full methodology row, automatically**: `benchmark →
   target → backend → profile → CPython ratio → PyPy ratio → Codon ratio → binary
   size → peak RSS → compile time → artifact`, with cold AND warm, classification
   (`GREEN_STABLE / RED_STABLE / RED_NOISY / TIE / DIMENSIONAL_WIN / INFRA`), quiescence
   provenance, and a repeat-pass confidence interval. No "looks faster," no warm-only,
   no cherry-pick — *structurally impossible* because the schema refuses to emit a cell
   that lacks these fields.
4. **A RED is auto-attributed to a missing IR FACT, not a guessed string.**
   `suspected_missing_fact` stops being a human hint and becomes a *derived* join:
   cycle-profile hot symbols (#76) × the representation census (`call_fact_coverage.py`)
   × a pass-delta ledger (which pass lost the Repr / added the box / added the generic
   call) → "this RED is the `ShapeFacts`-missing class." This is the compression-ladder
   instrument: it tells you *which class of slowness* a red belongs to, so you retire
   the class, not the benchmark.
5. **The board is the root of trust; lore is subordinate.** A doc/memory claim about
   perf loses to the board; the board loses to quiescence+provenance. (Already the
   SCOREBOARD.md doctrine; this arc makes it *enforced in CI*, not just documented.)

The class this arc retires: **"silent perf drift"** — the entire family of "a
correctness fix quietly slowed a benchmark / a backend / a profile and nobody noticed
until a customer did." After this arc, that family is **unexpressible on main**: the
merge gate refuses it.

---

## 1. What already exists (cite-and-compose; do NOT duplicate)

This arc is **90% composition** of existing, high-quality tooling. The investigation
found the substrate is mostly built — the gap is *gating, projection, and derivation*,
not raw measurement. Authoritative inventory (verified against the tree, 2026-06-23):

| Asset | Path | What it already does | Gap this arc fills |
|---|---|---|---|
| **CPython floor-scoreboard** | `tools/perf_scoreboard.py` (4903 ln) | Single board keyed `benchmark×target×backend×profile`; cold/warm split; 2-D verdict (`FAIL_ENGINE`/`FAIL_COLD_BUDGET`/`WARN_COLD_FLOOR`/…); 5-state `--classify` (`RED_STABLE/RED_NOISY/TIE/GREEN_STABLE/DIMENSIONAL_WIN/INFRA`); quiescence guard (`--require-quiescent`); repeat-CI (`--repeat`); cycle attribution (`--emit-cycle-profile`, `--sample-hot-only` #76 inner-repeat+symbolicate); provenance/anti-stale schema v3; PyPy/Codon comparator columns (`--pypy`/`--codon`); WASM `RUN_BLOCKED`; checkpointing | Not invoked by CI; one board not five; PyPy/Codon as columns not gated boards; Luau not unified; god-file (4904 ln > 2500 ceiling) — decompose per 21x |
| Methodology doctrine | `docs/perf/SCOREBOARD.md` (≈650 ln) | The 5 PERMANENT RULES, 5-state classification table, contamination asymmetry, the authoritative quiet board, the #68/#76 cycle attributions | Describes ONE board; must generalize to the plane + document the gate wiring |
| Build harness | `tools/bench.py` (2349 ln) | Daemon batch build (`_BenchBatchBuildServer`), `harness_memory_guard`, binary-size + compile-time capture | Reused as-is (perf_scoreboard already imports it) |
| Curated suite | `tools/bench_suites.py` (112 ln) | `BENCHMARKS` (56 core), `SMOKE_BENCHMARKS`, `MOLT_ARGS_BY_BENCH`, `canonical_benchmark_key` | Add suite *tiering* + per-benchmark *reference-class tags* (see §4.2) |
| Evidence policy | `tools/bench_evidence.py` (≈110+ ln) | `METRIC_OK_GATES` (incl. `molt_pypy_ratio`, `molt_codon_ratio`, `molt_nuitka_ratio`, `molt_pyodide_ratio`), `comparable_run_metadata_errors`, `validated_runtime_samples` | The shared *fail-closed comparator gate* the plane keys on — already has the PyPy/Codon/Nuitka/Pyodide ok-gates |
| Regression-vs-self | `tools/perf_regression.py` | Bootstrap CI, Mann-Whitney U, Cohen's d, min-detectable-effect, linear trend; thresholds per metric category | The statistical core for the **board-level history gate** (§Phase 4) — currently file-vs-file, must become board-vs-board-history |
| Safe execution | `tools/safe_run.py` | RSS-cap + wall-timeout watchdog; `--json` returns `peak_rss_mib`+`elapsed_s` | Reused (mandatory per CLAUDE.md Safe Execution) |
| Inner-repeat transform | `tools/perf_inner_repeat.py` | AST-based semantics-preserving `for _ in range(N): main()` wrap; refuses non-canonical shapes | Reused by `--sample-hot-only`; extended to be the warm-attribution substrate for ALL backends |
| Representation census | `tools/call_fact_coverage.py` | Per-call CallFacts coverage / MISSING-FACT census | One of the two join inputs for *derived* fact-attribution (§Phase 5) |
| Luau bench | `tools/benchmark_luau_vs_cpython.py` | Molt-Luau (via Lune) vs CPython, `--all`, `--report` | **Fold into the Backend board** as the `luau` backend lane (one schema, one gate) |
| pyperformance adapter | `tools/pyperformance_adapter.py` | Parses pyperformance MANIFEST; `nbody`/`fannkuch` smoke | The bridge to the *canonical external* suite (PyPy/Codon publish numbers on it) |
| Existing boards (data) | `bench/scoreboard/{quiet_native,quiet_llvm,hot_profile_native,cold_start_budget,cold_start_decomposition}.json` + two historical `cpython_*.json` | Real measured data, schema v3; the first authoritative quiet board | The seed for the history index (§Phase 4); the budget files are the cold/size/RSS ceilings |
| CI workflows | `.github/workflows/perf-validation.yml`, `perf_demo.yml`, `pr_trust_gate.yml`, `nightly.yml`, `ci.yml` | `perf-validation.yml` runs **`bench.py`** (not the gate-capable scoreboard), is **`workflow_dispatch`-only**, uploads an artifact with **no gate step** | **The single biggest gap: there is no CI job that runs the gate and blocks on it.** |
| Unified CI driver | `tools/ci_gate.py` (1088 ln) | Tiered (Tier 1/2/3) correctness pipeline, memory-guarded, JSON | **Zero references to perf** — perf is not a tier. Add a perf tier (§Phase 3). |
| Structural audit | `docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md` | Names `tools/perf_causality.py` and `tools/pass_delta_dashboard.py` as **MISSING**; names `fact_graph.rs`+`fact_graph_dump.py` as missing | Phase 5 builds `perf_causality.py` + `pass_delta_dashboard.py` (confirmed absent on disk) |

**North-star alignment.** Doc 51 ("10-year roadmap", NORTH STAR) §3 *names this exact
deliverable*: "The four+1 scoreboards (kept green, CI-gated): 1 CPython, 2 PyPy, 3
Codon, 4 Backend, 5 Profile." Doc 51 §1 names the method ("retire one CLASS per month…
keep the four+1 scoreboards green; every fact gets an Alive2-style validator") and §2
the matrix (4 targets × 4 profiles × 5 dimensions: warm, cold #62, RSS, size <2MB,
compile). **This arc is the executable realization of doc 51 §3.** It does not invent a
new direction; it builds the instrument doc 51 declares mandatory.

> **Refusal recorded (deletes a bad plan).** The naive plan — "write five new scoreboard
> scripts" — is REJECTED. It would create five duplicate measurement loops (five build
> paths, five quiescence guards, five provenance blocks) → five sources of truth → the
> exact compound-interest-of-bugs trap CLAUDE.md forbids. The structurally correct design
> is **one measurement core that emits one canonical evidence stream, and five
> PROJECTIONS (views) over that stream, each with its own gate predicate.** Five boards,
> one truth. This is the load-bearing architectural decision (§3).

---

## 2. Time-traveler derivation (end-state → required structural facts)

Work backward from the §0 end-state to the mechanisms that make it inevitable.

- **END:** "no commit changes perf without the system saying so."
  → **requires** a *merge-blocking* gate that runs on every PR and on main.
  → **requires** the gate to be *fast and deterministic enough* to run per-PR (a 56×4×4
     full sweep is not; so the gate needs a **tiered suite**: a small per-PR smoke set +
     the full nightly sweep — mirroring `ci_gate.py`'s Tier 1/2/3).
  → **requires** *quiescence + provenance* to be a **hard precondition of authority** in
     CI (a noisy CI runner must downgrade to advisory, never a false RED that blocks a
     good PR — Rule 3). **FACT NEEDED:** `authoritative: bool` is already in schema v3;
     the gate must *consume* it (CI runners are noisy → the per-PR gate keys on the
     **regression** axis with statistical CI, not raw warm point estimates; see §3.3).

- **END:** "five boards; a native win can't hide a WASM regression."
  → **requires** the five boards to be **separate artifacts with separate exit codes**,
     not columns. **FACT NEEDED:** a `BoardProjection` abstraction — `(name, cell_filter,
     gate_predicate, summary_shape)` — that reduces the canonical cell stream to one
     board + one verdict. (§3.2)

- **END:** "every claim is the full methodology row, automatically; no warm-only."
  → already structurally enforced by schema v3 (a cell without cold+warm+stability is
     malformed). **FACT NEEDED:** make the schema a *validated* contract — a
     `tools/perf_schema.py` (or a `jsonschema`) that `--self-test` and CI both check, so
     a cell missing a column **fails to write**. (§Phase 1)

- **END:** "a RED is attributed to a missing IR FACT, not a guessed string."
  → **requires** `suspected_missing_fact` to be *derived*, joining three signals:
     (a) **cycle profile** (#76, exists) — which symbol burns cycles;
     (b) **representation census** (`call_fact_coverage.py`, exists) — which calls/values
         are un-specialized (boxed / generic-dispatch / missing CallFacts);
     (c) **pass-delta ledger** (`pass_delta_dashboard.py`, MISSING) — which pass
         *introduced* the box / lost the Repr / added the generic call between baseline
         and current.
  → **FACT NEEDED:** a **fact-class taxonomy** (the doc-51 fact families: `op_kinds ·
     operand-ownership · FinalizerSensitive · CallFacts · Typed CallableTarget ·
     ShapeFacts · ownership lattice · ExceptionRegion · Repr/TirType lattice · class
     identity/version`) and a deterministic mapping `hot-symbol-pattern → fact-class`.
     This is `tools/perf_causality.py` (MISSING). (§Phase 5)

- **END:** "regressions gate, not just absolute reds."
  → **requires** a **durable board history** keyed by content-addressed identity so
     "previously-green" is a *queryable fact*, and `perf_regression.py`'s statistics run
     board-vs-history. **FACT NEEDED:** a `bench/scoreboard/history/` index + a
     `board_identity` (git_rev + tool blob + suite hash + host class). (§Phase 4)

- **END:** "PyPy names the missing mechanism."
  → **requires** the PyPy lane to emit a *structured* `pypy_advantage_class` enum (IC
     tiering / class-version guard / borrow inference / generator fusion / shape
     propagation / trace-like loop specialization — the exact list from CLAUDE.md and
     doc 51 §3), not a free-text note. **FACT NEEDED:** the same taxonomy as above,
     applied to the PyPy-wins case. (§Phase 6)

- **END:** "Codon marks non-equivalent semantics honestly, never as win/loss."
  → **requires** a per-benchmark `codon_semantics: equivalent | non_equivalent | n/a`
     tag *declared in the suite* (not inferred), so the Codon board can only compare on
     matched semantics. **FACT NEEDED:** `bench_suites.py` reference-class tags. (§4.2)

The dependency spine (what must exist before what):

```
Phase 0  inventory + schema contract pinned (this doc + perf_schema.py)
   │
Phase 1  ONE canonical evidence core (refactor perf_scoreboard → measurement core
   │       + BoardProjection) — the keystone; everything else is a projection
   │
   ├── Phase 2  the 5 board projections (CPython / Backend / Profile from existing
   │             data; PyPy / Codon gated when host present, advisory when absent)
   │
   ├── Phase 3  CI WIRING — perf tier in ci_gate.py + a real perf-gate workflow
   │             (per-PR smoke gate + nightly full sweep). DEPENDS on Phase 2.
   │
   ├── Phase 4  board HISTORY + regression gate (board-vs-history via perf_regression
   │             statistics). DEPENDS on Phase 1 (board_identity).
   │
   ├── Phase 5  DERIVED fact-attribution (perf_causality.py + pass_delta_dashboard.py).
   │             DEPENDS on Phase 1 (cell stream) + existing #76 + call_fact_coverage.
   │
   └── Phase 6  PyPy/Codon mechanism taxonomy + reference-class suite tags + the
                 canonical pyperformance bridge. DEPENDS on Phase 2.

Phase 7  decompose perf_scoreboard.py (4904 ln) under the 21x god-file ratchet.
         INTERLEAVED with Phase 1 (the refactor IS the decomposition; see §8).
```

Phases 2–6 are **parallelizable across agents** once Phase 1 lands (non-overlapping
files; see §7). Phase 1 is the single serialization point.

---

## 3. The structural facts / mechanisms to build (each tied to the class it retires)

### 3.1 FACT: one canonical evidence stream (`PerfCell`) — retires "five sources of truth"

A single, validated record type is the source of truth. Everything (the five boards, the
history, the causality join, the PR comment) is a *pure function of a list of these*.

`PerfCell` (the schema-v3 cell, promoted to a *named, validated* contract — proposed home
`tools/perf_schema.py`, a small leaf module both the scoreboard and the gate import):

```
PerfCell:
  # identity / matrix coordinates
  benchmark: str                 # canonical key (bench_suites.canonical_benchmark_key)
  target: str                    # "native" | "wasm"  (CLI --target)
  backend: str                   # "native" | "llvm" | "wasm" | "luau"
  profile: str                   # "dev-fast" | "release-fast" | "release-output"
  reference_class: str           # "dynamic" | "static_equiv" | "numeric" | "io" ...  (§4.2)
  codon_semantics: str           # "equivalent" | "non_equivalent" | "n/a"            (§4.2)
  # build facts
  build_ok: bool; binary_size_kib: float|None; compile_time_s: float|None
  # run facts (cold + warm; the constitution forbids warm-only)
  molt_ok: bool; cpython_ok: bool
  warm_molt_s; warm_cpython_s; cold_molt_s; cold_cpython_s
  warm_speedup; cold_speedup; startup_tax_ms; cold_budget_ms
  molt_peak_rss_mib; cpython_peak_rss_mib
  # comparator lanes (null unless lane ran + bench_evidence ok-gate passed)
  pypy_warm_s; pypy_ratio; pypy_advantage_class: str|None     # §6 taxonomy
  codon_warm_s; codon_ratio; codon_note: str|None
  # statistics / hygiene
  samples: int; warmup: int; cv: float; repeat_ci: [lo, hi]|None
  stable: bool; quiescent: bool
  # verdict / classification
  verdict: str                   # the 2-D verdict (GREEN/FAIL_ENGINE/…)
  classification: str|None       # the 5-state (RED_STABLE/…); set under --classify
  # attribution (Phase 5 fills these from a DERIVED join, not a guess)
  cycle_profile: {top_symbols:[...], launch_fraction, available, refused}|None
  suspected_missing_fact: str|None       # DERIVED in Phase 5
  fact_class: str|None                   # the doc-51 fact family enum (Phase 5)
  attribution_confidence: str|None       # "cycle+census+delta" | "cycle-only" | "hint"
  # provenance / artifact
  output_parity: bool; log_artifact: str
```

**Validation contract (fail-closed):** `perf_schema.validate_cell(cell)` rejects any cell
that (a) is GREEN/RED without both cold AND warm, (b) claims a comparator ratio whose
`*_ok` gate (`bench_evidence.METRIC_OK_GATES`) is false, (c) claims `RED_STABLE` without
`quiescent=true` and a `repeat_ci` clearing below 1.0 (Rule 1+3 made structural), or (d)
omits `log_artifact`. **The class retired:** "a perf claim that skipped the methodology"
— it cannot be serialized.

### 3.2 FACT: `BoardProjection` — retires "a win in one column hides a loss in another"

```
BoardProjection:
  name: str                       # "cpython" | "pypy" | "codon" | "backend" | "profile"
  kind: str                       # the JSON "kind" tag, e.g. "cpython_floor_scoreboard"
  cell_filter: Callable[[PerfCell], bool]    # which cells this board owns
  group_by: tuple[str, ...]       # the table axes (e.g. backend board groups by backend)
  gate_predicate: Callable[[PerfCell], GateVerdict]  # PASS/FAIL/ADVISORY/SKIP per cell
  required_lanes: tuple[str, ...] # lanes whose ABSENCE is INFRA-skip vs FAIL
  out_path: Callable[[Identity], Path]
```

The five projections (one definition each, all over the same `list[PerfCell]`):

| Board | `cell_filter` | `group_by` | `gate_predicate` (FAIL iff) | absence policy |
|---|---|---|---|---|
| **CPython** | all cells with a CPython floor | (benchmark, backend, profile) | `warm_speedup < 1.00` stable+quiescent **OR** cold-budget exceeded **OR** UNSTABLE/BUILD_FAILED/RUN_ERROR | the existing board's gate, unchanged |
| **Backend** | all cells | **backend** → (benchmark, profile) | any backend lane has a stable warm RED that another lane doesn't (a *cross-backend* divergence) OR any single lane FAILs its own CPython floor | a backend with no run-path (WASM today) → `RUN_BLOCKED`/INFRA, **not** FAIL |
| **Profile** | all cells | **profile** → (benchmark, backend) | release-fast/release-output warm RED (dev-fast is compile-latency-optimized → its warm reds are `WARN`, not FAIL, per doc 51 profiles table) | dev-fast warm = advisory |
| **PyPy** | cells with `reference_class == "dynamic"` AND pypy lane ok | (benchmark, backend) | `pypy_ratio < 1.00` AND `pypy_advantage_class is None` (a loss with no *named* missing mechanism is the failure — losing is allowed, *un-attributed* losing is not) | pypy host absent → whole board `ADVISORY` (skip-gate), recorded |
| **Codon** | cells with `codon_semantics == "equivalent"` AND codon lane ok | (benchmark, backend) | (no hard FAIL — Codon is a *ceiling*, not a floor) → emits `RED`/`approaching`/`exceeds` advisory; FAIL only if a cell tagged `equivalent` regressed vs its own Codon-ratio history | non_equivalent cells excluded by construction; codon absent → ADVISORY |

**The class retired:** "asymmetric perf coverage" — the doc-51 §3 invariant "a native win
never excuses a WASM regression; release-output never hides a release-fast regression" is
now *enforced by separate exit codes*, not by a reader's diligence.

> **Why Codon is not a hard floor.** CLAUDE.md: Codon is the AOT *reference ceiling* on
> matched semantics, "mark non-equivalent semantic models as non-equivalent, never as a
> win/loss." So the Codon board *advises* (approach/match/exceed) and only *gates on
> regression vs itself* for equivalent-tagged cells — never "you lost to Codon = RED."
> CPython is the only absolute floor (the Performance Constitution).

### 3.3 FACT: tiered gate authority — retires "false RED blocks a good PR on a noisy runner"

The per-PR gate cannot trust a raw warm point estimate on a shared CI runner (Rule 2/3:
contamination manufactures false reds). So the gate has **two authority tiers**:

- **Per-PR (`--tier smoke`, blocking, fast):** runs the SMOKE suite (≈6 benchmarks ×
  native+llvm × release-fast). It gates on (a) **BUILD_FAILED / RUN_ERROR / output-parity
  break** (always trustworthy regardless of load), and (b) **regression vs the merge-base
  board** using `perf_regression.py`'s Mann-Whitney+bootstrap CI (a *relative* delta is
  far more contamination-robust than an absolute warm point, because both sides ran on the
  same noisy runner in the same job). It does **NOT** hard-fail on an absolute warm RED
  under contamination — it downgrades that to `RED_NOISY` advisory (Rule 3) and posts it.
- **Nightly (`--tier full`, authoritative, on a quiescent self-hosted runner if
  available):** the full 56×4×4 sweep with `--require-quiescent --repeat 5 --classify
  --emit-cycle-profile`, producing the authoritative boards that *seed* the merge-base
  baseline the per-PR tier diffs against. This is where `RED_STABLE` is allowed to be a
  real release-blocking target.

**The class retired:** "the measurement system blocks good work" — the gate is *trustable
on noisy hardware* because it gates on the contamination-robust axes (build/parity/
regression-CI) per-PR and reserves absolute-warm-RED authority for quiescent nightly.

### 3.4 FACT: board history + identity — retires "previously-green silently regressed"

`bench/scoreboard/history/<board>/<board_identity>.json` + an `index.json`. `board_identity
= sha256(git_rev || benchmark_tool_blob || suite_hash || host_class)`. The history lets the
gate answer "was this cell green before?" as a *fact*, so the doc-51 §3 / CLAUDE.md triage
priority #2 ("any previously-green benchmark that regressed") is gateable, not anecdotal.

### 3.5 FACT: derived fact-attribution — retires "perf work optimizes blind"

`suspected_missing_fact` becomes a *deterministic derivation* (Phase 5), not a human guess.
This is the **compression-ladder instrument**: it classifies a RED into one of the doc-51
fact families, so the fix is "add the missing fact (retire the class)" not "peephole this
benchmark." **The class retired:** "optimizing a symptom instead of a representation" —
the board now *tells you the representation*.

---

## 4. Concrete phases (dependency order; each independently landable with green gates)

> Build/test discipline for every phase (CLAUDE.md): `export MOLT_SESSION_ID=perf-<phase>`
> before any build; route any raw-binary run through `tools/safe_run.py --rss-mb <cap>
> --timeout <s>`; never `cargo clean`; max 2 build-triggering agents. All new tooling is
> **Python** (the perf plane is host tooling) — *no Rust rebuild is on the critical path
> for Phases 0,2,3,4,6,7*; only Phase 5's `pass_delta_dashboard.py` consumes Rust-emitted
> pass-delta JSON (which the backend already can dump via `MOLT_TIR_DUMP`-class hooks; if a
> machine-readable pass-delta emit does not yet exist it is added behind an env flag in the
> backend as a *additive diagnostic*, never changing product output).

### Phase 0 — Pin the contract (this doc + `tools/perf_schema.py`)

**Deliverable:** `tools/perf_schema.py` — the `PerfCell` dataclass + `validate_cell()` +
`SCHEMA_VERSION` / `RED_THRESHOLD` / `UNSTABLE_CV` constants + the `fact_class` enum (the
doc-51 fact families) + the `pypy_advantage_class` enum + the
`reference_class`/`codon_semantics` enums. Extract the *currently-inline* schema constants
from `perf_scoreboard.py` (the `VERDICT_*`, `CLASS_*`, `CLASSIFY_STATES`, thresholds at
lines ~104–172) into this leaf module and have `perf_scoreboard.py` import them. The
contract is not just vocabulary: `validate_cell()` must reject measured verdicts that lack
the full cold/warm methodology row, and `RED_STABLE` must prove quiescence plus a repeat CI
that clears below the CPython floor.

**Gates:** `uv run --python 3.12 python tools/perf_scoreboard.py --self-test` passes
unchanged (proves the extraction is behavior-preserving); a new
`tests/tools/test_perf_schema.py` round-trips every cell in the committed
`bench/scoreboard/quiet_native.json` through `validate_cell()` (proves the contract
accepts real data and rejects a column-dropped mutant). `python tools/structural_audit.py
--check` does not regress (new file is small/leaf).

**Independently valuable:** yes — a validated schema catches malformed boards immediately.

### Phase 1 — Extract the measurement CORE (keystone; one serialization point)

The single structurally-correct refactor: split `perf_scoreboard.py` into
`perf_measure.py` (the CORE: build one cell — quiescence, build via `bench.py`, run via
`safe_run`, stats, cycle profile) and `perf_board.py` (PROJECTIONS: reduce `list[PerfCell]`
→ boards). `perf_scoreboard.py` becomes a thin CLI facade over both (preserving every
existing flag — `--set`, `--backend`, `--profile`, `--classify`, `--require-quiescent`,
`--repeat`, `--emit-cycle-profile`, `--sample-hot-only`, `--pypy`, `--codon`, `--baseline`,
`--no-gate`, `--self-test`, …). This is **pure code motion + the `BoardProjection`
abstraction**; the measurement loop is byte-for-byte the same logic, just callable as
`measure_cells(specs) -> list[PerfCell]`.

**Why this is the keystone:** every other phase consumes `list[PerfCell]` from
`perf_measure.measure_cells(...)` or projects via `perf_board.project(...)`. Landing it
once means Phases 2–6 touch *different files* and parallelize.

**Gates:** the existing full self-test + a **golden-board equivalence test**: run
`perf_scoreboard.py --set smoke --backend native --no-gate` before and after the refactor;
assert the emitted `cpython_*.json` is *semantically identical* (same cells, same verdicts;
timing fields tolerance-compared). `cargo`-free. `structural_audit --check`:
`max_god_file_lines` for `perf_scoreboard.py` must **go down** (ratchet: the file shrinks).

### Phase 2 — The five board PROJECTIONS

Define the five `BoardProjection`s of §3.2 in `perf_board.py`. Each `project()` writes its
own artifact (`bench/scoreboard/<board>_<identity>.json`) with its own `kind` and its own
summary + gate verdict. CPython/Backend/Profile project from the *same* `list[PerfCell]`
already produced by a native+llvm+(wasm build-only)+luau sweep. PyPy/Codon project the
comparator columns (gated when the host has the binary, `ADVISORY` when absent — recorded,
never faked).

**Sub-phase 2a (CPython/Backend/Profile):** these need *no new measurement* — they are
pure re-projections of data the core already gathers. Land first.

**Sub-phase 2b (Luau into the Backend board):** fold `benchmark_luau_vs_cpython.py`'s
Lune-driven run into `perf_measure` as the `backend="luau"` lane (a `BackendSpec("luau",
…)` alongside the existing `NATIVE`/`LLVM`/`WASM` specs at `perf_scoreboard.py:309-314`),
so Luau is a lane in the one schema, not a separate tool. Keep
`benchmark_luau_vs_cpython.py` as a thin wrapper that calls the core (no duplicate loop;
delete its private timing loop — asymmetric-coverage rule).

**Gates:** `--self-test --all-boards` emits five well-formed artifacts that
`perf_schema.validate_board()` accepts; a unit test feeds a hand-built `list[PerfCell]`
with one WASM-only RED and asserts the **Backend** board FAILs while the **CPython** board
(native green) PASSes — proving the asymmetry invariant. Luau lane: a smoke
(`bench_sum.py`) produces a `luau` cell with parity-checked output.

### Phase 3 — CI WIRING (the biggest gap; makes speed actually gate)

1. Add a **perf tier** to `tools/ci_gate.py` (a new `Check` with `tier=` and the smoke
   gate command), so the unified driver knows about perf. The smoke gate command:
   `python3 tools/perf_scoreboard.py --set smoke --backend native --backend llvm
   --profile release-fast --gate-mode regression --baseline-from-merge-base`.
2. Rewrite `.github/workflows/perf-validation.yml`: change the trigger from
   `workflow_dispatch` only to **`pull_request` + `push: [main]`**, replace the
   `bench.py` step with the **gate** step (the smoke tier), and add a **failing exit =
   failing check** (remove the "upload artifact and pass" no-op). Add a PR-comment step
   (post the methodology summary table). The full sweep stays in `perf_demo.yml`/
   `nightly.yml` (nightly cron) and *writes the authoritative baseline* to
   `bench/scoreboard/history/`.
3. Register the perf check in `pr_trust_gate.yml` so it is a required status for merge.

**Quiescence on CI (Rule 2/3 honored):** the per-PR runner is shared/noisy → the workflow
passes `--gate-mode regression` (contamination-robust delta gate, §3.3) and stamps the
board `authoritative=false` for warm verdicts; it **never** hard-fails on an absolute warm
RED in PR context (that authority lives in the nightly quiescent job). A self-hosted
quiescent runner, if registered, runs the nightly `--require-quiescent` job.

**Gates:** a dry-run (`ci_gate.py --tier <perf> --dry-run`) lists the perf check; a
deliberate regression fixture (a benchmark patched to be 2× slower behind a test-only flag)
makes the gate exit nonzero in a local invocation; the workflow YAML passes `actionlint`
(if available) / a `python -c "import yaml; yaml.safe_load(open(...))"` smoke.

### Phase 4 — Board history + the regression gate

`bench/scoreboard/history/` + `index.json`; `perf_history.py` (record/query) computing
`board_identity` (§3.4). Wire `perf_regression.py` to run **board-vs-history** (it already
has the statistics; today it is file-vs-file — generalize the input to "latest authoritative
history entry for this `board_identity`'s suite+host class"). The gate's "previously-green
regressed" predicate reads history.

**Seed:** import the committed `quiet_native.json`/`quiet_llvm.json` as the first history
entries (they are already authoritative quiet boards).

**Gates:** record two synthetic boards (green, then one cell regressed) and assert the
regression gate flags exactly the regressed cell with `severity=error`; assert a *noise-
only* delta (within `perf_regression` thresholds + CI) is NOT flagged (no false positive).

### Phase 5 — DERIVED fact-attribution (`perf_causality.py` + `pass_delta_dashboard.py`)

Build the two MISSING tools the STRUCTURAL_AUDIT_BOARD names:

- `tools/pass_delta_dashboard.py`: consumes a per-pass IR-fact delta (which pass lost a
  `Repr`, added a box, added a generic call, added an RC event) for a given benchmark.
  Source of the delta: the backend's existing pass-instrumentation (`MOLT_TIR_DUMP` /
  analysis-verify hooks). If a *machine-readable* per-pass delta emit does not exist, add
  an **additive** `MOLT_EMIT_PASS_DELTA=1` JSON dump in the TIR pass manager
  (`runtime/molt-tir/src/tir/passes/…` pass-manager seam) — diagnostic-only, never alters
  product output (Bootstrap/zero-workaround safe: it is a pure observer).
- `tools/perf_causality.py`: the **join**. For each `RED_STABLE` cell it takes (a) the #76
  cycle profile (`in_binary_top`), (b) the `call_fact_coverage.py` census for that
  benchmark, (c) the `pass_delta_dashboard.py` delta vs baseline, and a deterministic
  `hot-symbol-pattern → fact_class` table (e.g. `molt_inc_ref_obj`/`molt_dec_ref_obj` →
  `ownership-lattice/Repr`; `*generic_call*`/`*bind*` → `CallFacts/Typed CallableTarget`;
  `*box*`/`to_bigint` → `Repr/TirType lattice`; `split_field_bounds`/`Utf8Chunks` →
  `ShapeFacts/string-repr`; `record_exception*`/`exception_stack_*` → `ExceptionRegion`).
  Output: the cell's `fact_class` + `suspected_missing_fact` + `attribution_confidence`.

Wire the result back into `PerfCell.suspected_missing_fact`/`fact_class` so the board's
triage hint is *derived evidence*, not a human guess. This realizes CLAUDE.md "Perf work's
deliverable is a NEW IR FACT that makes a class of slow programs unexpressible."

**Gates:** feed the committed `hot_profile_native.json` (the real #68/#76 attributions) and
assert `perf_causality.py` classifies `bench_exception_heavy` → `ExceptionRegion/ownership`
(matches the documented "~22% refcount + ~12% exception bookkeeping" finding in
SCOREBOARD.md) and `bench_etl_orders` → `ShapeFacts/string-repr` (matches the documented
"per-row split + UTF-8 decode + dataclass construction"). This is a **falsifiable** gate:
the tool must reproduce the human attributions the council already verified.

### Phase 6 — PyPy/Codon mechanism taxonomy + reference-class suite tags + canonical bridge

1. **Reference-class tags** in `bench_suites.py` (§4.2): each benchmark gets
   `reference_class` and `codon_semantics`. Dynamic-feature benchmarks (attr access,
   class hierarchy, exception heavy, generators) → PyPy board; numeric/loop/data with
   matched semantics (sum, matrix_math, sieve) → `codon_semantics=equivalent`.
2. **`pypy_advantage_class`**: when `pypy_ratio < 1.00` (PyPy wins), `perf_causality.py`'s
   taxonomy assigns *which JIT mechanism* PyPy has that molt lacks (the exact CLAUDE.md
   list: IC tiering, class-version guard, borrow inference, generator fusion, shape
   propagation, trace-like loop specialization). A PyPy loss with `pypy_advantage_class
   == None` FAILs the PyPy board (un-attributed loss is the failure, per §3.2).
3. **Canonical pyperformance bridge:** extend `pyperformance_adapter.py` beyond
   `nbody`/`fannkuch` to the subset PyPy/Codon publish numbers on, so molt's ratios sit on
   the *same* benchmark definitions the references use (apples-to-apples), feeding the
   PyPy/Codon boards.

**Gates:** with PyPy/Codon absent (the current host), the boards emit `ADVISORY` + record
"host absent" (no fake numbers — verified by a test that asserts no `pypy_ratio` is written
when `--pypy` is off). With a *mock* comparator (a fixture binary), assert a PyPy-loss cell
without a mechanism class FAILs and the same cell *with* a class PASSes.

### Phase 7 — Decompose `perf_scoreboard.py` under the 21x ratchet (interleaved)

The file is 4904 ln (ceiling 2500) — a high-severity god-file on the audit board. Phases
0/1 already cut it (schema → `perf_schema.py`, core → `perf_measure.py`, projections →
`perf_board.py`, history → `perf_history.py`, causality → `perf_causality.py`). Phase 7 is
the *finishing* pass: ensure the residual `perf_scoreboard.py` is a <2500-ln CLI facade and
that `structural_audit.py --check` shows `max_god_file_lines`/`god_files` strictly
decreasing. This composes with the **doc 21** decomposition program (21a function-split,
21b crate-graph, 21d CLI-package) — same method (extract cohesive modules along legible
seams), same ratchet gate.

---

## 4.2 The reference-class taxonomy (suite tags — load-bearing for PyPy/Codon boards)

A benchmark's *class* decides which boards gate it. Declared in `bench_suites.py`
(authoritative, not inferred):

| `reference_class` | meaning | gated by | example benchmarks |
|---|---|---|---|
| `dynamic` | dynamic-dispatch / duck-typed / exception / generator heavy | CPython floor + **PyPy** | `bench_class_hierarchy`, `bench_exception_heavy`, `bench_generator_iter`, `bench_attr_access` |
| `static_equiv` | statically compilable, semantics match Codon | CPython floor + **Codon** | `bench_sum`, `bench_matrix_math`, `bench_deeply_nested_loop` |
| `numeric` | numeric kernels (overlaps static_equiv) | CPython + Codon | `bench_fib`, `bench_prod_list`, `bench_min_list` |
| `io` / `parse` | I/O, parsing, serialization | CPython floor only | `bench_csv_parse`, `bench_json_roundtrip`, `bench_parse_msgpack` |
| `string` | string/bytes algorithms | CPython floor (+ PyPy where JIT helps) | `bench_bytes_find`, `bench_str_split`, `bench_str_count_unicode` |

`codon_semantics ∈ {equivalent, non_equivalent, n/a}` is **independent** of
`reference_class` (a `numeric` benchmark using bigint promotion molt has but Codon's fixed-
width ints don't → `non_equivalent`, excluded from the Codon *win/loss* gate, recorded as
a note). This honors CLAUDE.md "mark non-equivalent semantic models as non-equivalent."

---

## 5. Verification / gates per phase (measurement discipline, parity oracle)

The plane's *own* tests must obey the same discipline the plane enforces on the compiler.

- **Schema-contract gate (every phase):** `perf_schema.validate_cell/validate_board` runs
  in `--self-test` and in `tests/tools/test_perf_schema.py`. A board that drops a required
  column fails to write. This is the plane's Alive2-style "checkable obligation" (doc 51
  §1 discipline) — the schema is a *checkable contract*, not a convention.
- **Golden-equivalence gate (Phase 1/7):** the refactor must not change a single verdict.
  Before/after `--no-gate` boards are diffed for semantic identity (the anti-regression for
  the keystone refactor itself).
- **Parity oracle (always):** every cell carries `output_parity` (molt stdout == CPython
  stdout). A perf number for a wrong-answer run is *invalid* and the cell is `RUN_ERROR`,
  never GREEN. (The plane cannot reward a fast wrong answer — correctness is the floor.)
- **Quiescence + provenance gate (authoritative boards):** nightly job runs
  `--require-quiescent`; a non-quiescent board is `authoritative=false` and may not seed
  the baseline (Rule 2). Per-PR boards are explicitly non-authoritative-for-warm and gate
  only on the contamination-robust axes (§3.3).
- **Falsifiable causality gate (Phase 5):** `perf_causality.py` must reproduce the
  *already-verified* human attributions for `bench_exception_heavy` and `bench_etl_orders`
  from the committed `hot_profile_native.json`. If it can't, the taxonomy is wrong — fix
  the taxonomy, don't special-case the benchmark (per-test-special-case rule).
- **No-false-positive gate (Phase 4):** a noise-only board delta within statistical CI
  must NOT trip the regression gate (a flaky gate that blocks good PRs is the Rule-3
  measurement-system bug). Tested with synthetic within-threshold deltas.
- **No-fake-number gate (Phase 6):** with `--pypy`/`--codon` off, no `pypy_ratio`/
  `codon_ratio` is ever written (the `bench_evidence.METRIC_OK_GATES` already enforce the
  `*_ok` precondition; a test asserts the absence).
- **Ratchet gate (Phase 1/7):** `tools/structural_audit.py --check` —
  `max_god_file_lines` and `god_files` strictly decrease as `perf_scoreboard.py` is split.

Every PR that touches the perf plane runs: `--self-test --all-boards`, the schema tests,
the projection-asymmetry test, and `structural_audit --check`. No Rust rebuild required for
the plane PRs (host tooling) — so these are *fast* gates that run in Tier 1.

---

## 6. How it composes with the decomposition (21a–e) and the multi-agent model

### Composition with the 21x decomposition program

- **Method match:** Phase 1/7 *is* a 21-style decomposition (extract cohesive modules
  along legible seams; ratchet the god-file metric down). `perf_scoreboard.py` (4904 ln) is
  on the same `STRUCTURAL_AUDIT_BOARD` god-file list as `cli.py`/`function_compiler.rs`; this
  arc clears one entry the way 21a–d clear theirs. No conflict — *different files*.
- **21d (CLI package):** the perf tools are invoked by the CLI/CI but are standalone
  scripts under `tools/`; the new leaf modules (`perf_schema/measure/board/history/
  causality`) follow 21b's "keep moved files as pure renames + widen pub precisely" spirit
  (here: pure Python extraction, import the constants, no behavior change).
- **Dependency direction:** the perf plane is a *consumer* of the compiler (it builds and
  times artifacts) and of two analysis tools (`call_fact_coverage.py`, the pass-delta
  emit). It introduces **no cycle** — it sits strictly downstream of build + analysis, like
  `dx_baseline.md`'s measurement sits downstream of the build.

### Composition with the parallel multi-agent execution model

Maps onto the council **three-lane model** (CLAUDE.md): this arc is squarely **Lane C**
("infra/scoreboards/decomposition that makes A&B faster… C is never decorative"). It is the
instrument Lane B (perf frontier) *steers by* and Lane A (P0 safety) *reports regressions
to*. The Phase plan is explicitly parallel-friendly:

- **Serialization point:** Phase 1 (the core extraction) is the *only* phase that must land
  before others; it is one agent, one PR.
- **After Phase 1, fan out (non-overlapping files):**
  - Agent-1 (`perf_board.py`): Phase 2 projections.
  - Agent-2 (`.github/workflows/*`, `ci_gate.py`): Phase 3 CI wiring.
  - Agent-3 (`perf_history.py`, extend `perf_regression.py`): Phase 4 history gate.
  - Agent-4 (`perf_causality.py`, `pass_delta_dashboard.py`, the TIR pass-delta emit):
    Phase 5 attribution. (This one may touch Rust for the *additive* diagnostic emit →
    serialize through the daemon socket per CLAUDE.md "max 2 build-triggering agents.")
  - Agent-5 (`bench_suites.py`, `pyperformance_adapter.py`): Phase 6 taxonomy + bridge.
- **Build discipline:** only Agent-4 triggers a Rust build (the diagnostic emit); the other
  four are pure-Python and need no `cargo` — so they don't count against the "2 concurrent
  build agents" cap and can all run at once. Each exports its own `MOLT_SESSION_ID`.
- **Tranche/evidence standard (CLAUDE.md):** each phase CHANGES PROJECT STATE (a landed
  tool/board/gate/test), reports the PERF/SPEED STATUS block, and runs the relevant gates;
  Phase 0's refusal of "five separate scripts" is the durable deleted-plan artifact.

**Cross-arc dependencies (what this arc enables / needs):**
- **Enables (downstream consumers):** *every* perf and correctness arc. Doc 51 §4–8
  (finalizer/ownership/async correctness → then optimizer turn-up) all require this plane
  to prove "what did this do to speed?". The doc-00 §1 "PERF/SPEED STATUS block" every
  batch must report *is produced by this plane*. The `bench_struct 0.04×` /
  `bench_exception_heavy 0.55×` reds named in doc-00 are the first targets the plane
  attributes.
- **Needs (upstream producers it consumes):** the **semantic fact plane** (doc 46
  control plane; doc 51 §1 fact families) — Phase 5's `fact_class` taxonomy *names* those
  facts, so as the fact families land (CallFacts #47, ownership lattice #58, ShapeFacts,
  Repr lattice), the attribution gets sharper. The plane and the fact-plane co-evolve:
  the plane *measures* which fact is missing; the fact-plane *adds* it; the plane confirms
  the class is retired. This is the compression-ladder feedback loop made mechanical.
- **Independent of:** the RC-1/finalizer correctness front (it *measures* their perf cost
  but does not block on them); the LTO/DX build-speed arc (doc 08 — orthogonal; that arc
  speeds the plane's *own* iteration but is not a dependency).

---

## 7. Parallel execution map (file ownership, no overlaps)

| Phase | Owner files (new unless noted) | Touches Rust? | Blocks / blocked-by |
|---|---|---|---|
| 0 | `tools/perf_schema.py`; `tests/tools/test_perf_schema.py`; (extract consts from `perf_scoreboard.py`) | no | blocks all |
| 1 | `tools/perf_measure.py`, `tools/perf_board.py`; `perf_scoreboard.py` → facade | no | blocked-by 0; blocks 2–6 |
| 2 | `tools/perf_board.py` (projections); fold `benchmark_luau_vs_cpython.py` | no | blocked-by 1 |
| 3 | `.github/workflows/perf-validation.yml`, `pr_trust_gate.yml`; `tools/ci_gate.py` | no | blocked-by 2 |
| 4 | `tools/perf_history.py`; `tools/perf_regression.py` (extend) | no | blocked-by 1 |
| 5 | `tools/perf_causality.py`, `tools/pass_delta_dashboard.py`; `runtime/molt-tir/src/tir/passes/` (additive emit) | **yes (additive diag only)** | blocked-by 1; serialize Rust build |
| 6 | `tools/bench_suites.py`, `tools/pyperformance_adapter.py`; taxonomy in `perf_schema.py` | no | blocked-by 2 |
| 7 | `tools/perf_scoreboard.py` (residual shrink); `docs/perf/SCOREBOARD.md` (plane update) | no | interleaved with 1 |

Five of seven phases never trigger a Rust build → maximal parallelism. The doctrine "make
all independent tool calls in one batch" applies: Phases 2,4,6 can be three simultaneous
pure-Python agents the moment Phase 1 merges.

---

## 8. Risks + structural (not band-aid) treatment

### Risk 1: CI runner noise manufactures a false RED that blocks a good PR (Rule 3)
**Band-aid (rejected):** loosen the RED threshold globally → hides real reds.
**Structural fix:** the **tiered authority** of §3.3 — per-PR gates on the
contamination-robust axes (build/parity/**regression-CI**), absolute-warm-RED authority is
reserved for the quiescent nightly job; per-PR boards are `authoritative=false` for warm by
construction. The regression delta is robust because both sides ran in the *same* noisy job.

### Risk 2: PyPy/Codon are not installed on the dev/CI host → the boards are vapor
**Band-aid (rejected):** invent ratios / skip silently.
**Structural fix:** `ADVISORY` board state + `bench_evidence.METRIC_OK_GATES` fail-closed
(no `*_ratio` without `*_ok`). The board *exists and is recorded as host-absent*, so when a
PyPy/Codon-equipped runner (self-hosted or a dedicated nightly image) appears, the lane
lights up with zero code change. The pyperformance bridge (Phase 6) means molt's numbers
are on the *same* definitions PyPy/Codon publish, so even an *external* reference number is
comparable.

### Risk 3: `suspected_missing_fact` stays a human guess (the audit-board "MISSING" tools)
**Band-aid (rejected):** keep typing guesses into the field.
**Structural fix:** Phase 5 *derives* it from the cycle×census×pass-delta join, with a
falsifiable gate (must reproduce the council-verified #68/#76 attributions). The field
becomes evidence with `attribution_confidence`, downgrading gracefully to `cycle-only` or
`hint` when a join input is unavailable — never claiming more than it knows.

### Risk 4: The full 56×4×4 sweep is too slow to run per-PR
**Band-aid (rejected):** run the full sweep per-PR and time-out / flake.
**Structural fix:** the **tiered suite** (smoke per-PR, full nightly), mirroring
`ci_gate.py` Tier 1/2/3. The per-PR smoke set is the high-signal subset; the nightly full
sweep seeds the baseline. This is the same discipline the correctness CI already uses.

### Risk 5: The pass-delta emit changes product codegen (Bootstrap/zero-workaround risk)
**Band-aid (rejected):** thread perf instrumentation through the hot codegen path.
**Structural fix:** the emit is a **pure observer** behind `MOLT_EMIT_PASS_DELTA=1`,
additive like the existing `MOLT_KEEP_SYMBOLS=1`/`MOLT_TIR_DUMP` hatches — byte-identical
product output, only a diagnostic JSON side-channel (the same pattern SCOREBOARD.md §#76
established and proved code-identical).

### Risk 6: The refactor (Phase 1) silently changes a verdict (the keystone is risky)
**Band-aid (rejected):** "trust the move was clean."
**Structural fix:** the golden-equivalence gate (§5) diffs before/after boards for semantic
identity; the existing 983-passing self-tests + the new schema tests bound it. Pure code
motion, verified, not asserted.

### Risk 7: The plane becomes its own god-file / second source of perf truth
**Band-aid (rejected):** keep piling onto `perf_scoreboard.py`.
**Structural fix:** Phase 7 ratchets the file down; the `BoardProjection` abstraction means
there is exactly ONE cell stream and N *views* — adding a board is a `BoardProjection`
value, not a new measurement loop. The schema contract (`perf_schema.py`) is the single
authority; the structural-audit `duplicate_authorities` metric (currently 0) must stay 0.

### Risk 8: Cold-start / size / RSS dimensions get conflated with warm speed (#62 lesson)
**Band-aid (rejected):** one number per cell.
**Structural fix:** already in schema v3 (warm ≠ cold; `startup_tax_ms` gated against
`cold_start_budget.json`, not blended into `warm_speedup`). The plane *preserves* this:
each of the five doc-51 §2 dimensions (warm, cold, RSS, size, compile) is a distinct gated
field, and `DIMENSIONAL_WIN` (Rule 4) is honestly reported as dimensional, never a warm
heal. The Profile board specifically prevents a release-output size/speed win from masking
a release-fast warm regression.

---

## 9. The landing-report this arc makes automatic

When this arc is complete, every PR's perf check emits — *mechanically* — the CLAUDE.md
"Landing report format":

> **tests green; perf matrix green** (CPython board: 0 stable warm reds; Backend board: 0
> cross-backend divergences; Profile board: 0 release-fast/release-output regressions);
> **no CPython-red benchmarks**; **PyPy deltas known** (N cells lose to PyPy, each with a
> named `pypy_advantage_class`); **Codon deltas known** (M equivalent-tagged cells:
> approach/match/exceed; K non-equivalent excluded); **regressions zero or explicitly
> tracked** (board-vs-history: 0 previously-green regressed); binary size / peak RSS /
> cold-start / compile-time within budget. Artifacts: `bench/scoreboard/{cpython,backend,
> profile,pypy,codon}_<id>.json`.

That sentence is the product of this arc. Today a human writes it (or forgets to). After
this arc, the machine writes it on every commit, and the merge gate enforces it. **Speed
becomes a release-gating correctness property in fact, not just in the constitution.**

---

## Appendix A — Exact file/line anchors (for the implementing agents)

- Schema constants to extract in Phase 0: `tools/perf_scoreboard.py` lines ~90
  (`SCHEMA_VERSION`), ~104–119 (`VERDICT_*`), ~121–149 (`CLASS_*`, `CLASSIFY_STATES`),
  ~151–172 (quiescence thresholds, `DIMENSIONAL_WIN_MIN_FRACTION`).
- BackendSpec table to extend with Luau (Phase 2b): `tools/perf_scoreboard.py` lines
  ~298–314 (`BackendSpec`, `NATIVE`/`LLVM`/`WASM`, the spec dict).
- Board JSON `kind` tags + writers (Phase 1/2): lines ~1626 (`hot_only_cycle_profile`),
  ~3233 (`cpython_floor_scoreboard`), ~4042/4128/4634/4805 (board writers).
- Comparator ok-gates the plane keys on (Phase 2/6): `tools/bench_evidence.py`
  `METRIC_OK_GATES` (already has `molt_pypy_ratio`, `molt_codon_ratio`,
  `molt_nuitka_ratio`, `molt_pyodide_ratio`).
- Statistics to reuse for the history gate (Phase 4): `tools/perf_regression.py`
  `DEFAULT_THRESHOLDS` (lines ~66–71), bootstrap/Mann-Whitney/Cohen's-d machinery.
- CI driver to add a perf tier to (Phase 3): `tools/ci_gate.py` `Check` dataclass (lines
  ~95–110) + the tier registry.
- Workflow to rewrite (Phase 3): `.github/workflows/perf-validation.yml` (currently
  `workflow_dispatch`, runs `bench.py`, no gate) → `pull_request`+`push:[main]`, runs the
  smoke gate, blocks on nonzero exit.
- MISSING tools to build (Phase 5): `tools/perf_causality.py`, `tools/pass_delta_dashboard.py`
  (both confirmed absent on disk; named MISSING in `STRUCTURAL_AUDIT_BOARD.md` "TOP
  TOOLING GAPS").
- Census join input (Phase 5): `tools/call_fact_coverage.py` (exists).
- Cycle-profile join input (Phase 5): `bench/scoreboard/hot_profile_native.json` (the
  committed #68/#76 attributions — the Phase-5 falsification fixture).
- Suite to tag (Phase 6): `tools/bench_suites.py` `BENCHMARKS`/`SMOKE_BENCHMARKS`/
  `MOLT_ARGS_BY_BENCH`.
- Doctrine to update (Phase 7): `docs/perf/SCOREBOARD.md` (generalize "one board" →
  "the plane: one truth, five projections").

## Appendix B — Why this is the compression-ladder unit, not "a benchmark dashboard"

A dashboard shows numbers. This plane makes a *class* of failure unexpressible: after it
lands, "a commit silently changed perf on some backend/profile and merged anyway" cannot
happen on main — the gate refuses it. That is the doc-51 §1 method ("retire one CLASS per
month by adding a first-class fact that makes a whole class of bad programs unexpressible")
applied to the *process* layer: the missing fact here is "the system knows, automatically,
what every commit did to speed, across the full matrix, with attribution." Once that fact
exists and gates, the entire family of silent-perf-drift bugs is gone — and every
*downstream* perf arc inherits a measurement plane sharp enough to name the missing IR fact
it must add next.
