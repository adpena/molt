# CPython Floor-Scoreboard — the release-blocking performance gate

`tools/perf_scoreboard.py` operationalizes the **Performance Constitution**
(`CLAUDE.md`, commit `538f4386e`). It is the machine-readable artifact the
constitution mandates: a single scoreboard keyed **benchmark × target × backend
× profile** reporting the absolute molt-vs-CPython speedup, with any cell below
`1.00×` flagged **RED** (a contract violation) and a CI-gateable nonzero exit.

This tool **measures and surfaces**; it does not fix slow benchmarks. Fixing a
RED benchmark is a separate optimization arc — and per the constitution's
*"fix the representation, not the pass"* posture, the first question for each RED
is never *"which peephole recovers it"* but *"which FACT is missing from IR?"*.

## What it reuses (no new timing loop)

- **Build**: `tools/bench.py` — the canonical molt-vs-CPython harness. It owns
  the daemon batch-build server (`_BenchBatchBuildServer`), the
  `harness_memory_guard` (RSS-cap + wall-clock guard on every build), and the
  binary-size + compile-time capture (`prepare_molt_binary`).
- **Suite**: `tools/bench_suites.py` — `BENCHMARKS` (the curated 56-benchmark
  "core" verified subset), `SMOKE_BENCHMARKS`, and the per-benchmark molt build
  args (`MOLT_ARGS_BY_BENCH`, e.g. `--type-hints trust`).
- **Run / time**: `tools/safe_run.py --json` — wraps **every** molt binary AND
  the CPython baseline run with an RSS cap + timeout (the project's mandatory
  guard for raw-binary execution) and returns `peak_rss_mib` + `elapsed_s` for
  both runtimes, satisfying the constitution's peak-RSS column for free.

The scoreboard is the *new* artifact: none of the existing tools emitted the
constitution's absolute-vs-CPython-floor board. `tools/perf_regression.py`
detects regression-vs-self (a stored baseline); this tool measures
absolute-vs-CPython-floor and gates on it.

## How to run

```bash
export MOLT_SESSION_ID=perfscore
export CARGO_TARGET_DIR="$PWD/target/sessions/perfscore"

# Self-test: 1 benchmark × 1 backend, proves the pipeline + schema.
uv run --python 3.12 python3 tools/perf_scoreboard.py --self-test

# Full baseline run (the gate): core suite × native+llvm × release-fast.
uv run --python 3.12 python3 tools/perf_scoreboard.py \
    --set core --backend native --backend llvm --profile release-fast

# Diff a fresh run against the last stored scoreboard (newly-red / regressed).
uv run --python 3.12 python3 tools/perf_scoreboard.py --set core --backend native --baseline

# Measure-only (do not fail CI on RED): add --no-gate.
```

- The **CPython oracle is the system `python3` (3.14)**, resolved explicitly via
  `--cpython` (default: `/opt/homebrew/bin/python3`), **never** the `.venv`
  interpreter the tool itself is launched under. The constitution pins the floor
  to the system interpreter; letting the 3.12 venv leak in would silently move
  the floor.
- `MOLT_SESSION_ID=perfscore` + `CARGO_TARGET_DIR=target/sessions/perfscore`
  isolate the build cache (the constitution's concurrent-dev contract).
- The LLVM lane forces `MOLT_BACKEND=llvm` and `LLVM_SYS_211_PREFIX`
  (`/opt/homebrew/opt/llvm@21` — the brew default `llvm@22` is the WRONG version
  for llvm-sys 211). Its first build recompiles the backend with the `llvm`
  feature (~5 min).

### Exit code (the gate)

Exit is **nonzero iff any cell is RED or unstable-unmeasurable**. A
build-failed or error cell is also gated as RED. `run-blocked` (WASM) is *not*
red (it is a documented target limitation). `--no-gate` always exits 0.

### Recoverability

The sweep is long (56 × 2 backends, each with cold + warm + warmup runs for two
runtimes). It checkpoints the full board to `…<gitrev>.partial.json` after every
cell via an atomic temp-rename, so a death mid-sweep is recoverable — the
partial is a valid scoreboard document.

## Methodology (pyperf discipline)

- **≥ 5 measured samples** per (benchmark, runtime) warm phase (`--samples`),
  preceded by `--warmup` discarded runs (default 2).
- **median + mean + stdev + coefficient of variation** per phase. A phase whose
  CV exceeds `0.20` is flagged **unstable**; an unstable cell cannot be trusted
  GREEN and is gated like a RED.
- **COLD and WARM both captured.** COLD = first cold-cache run for each runtime;
  WARM = steady-state median after warmups. The constitution forbids warm-only
  wins, so a cell is RED if **either** the warm OR the cold speedup is `< 1.00×`.
- Per-run RSS-cap (`--rss-mb`, default 4096) + wall-clock timeout (`--timeout`,
  default 120s) via `safe_run.py`. Tight RSS poll (0.01s) so short benchmarks
  still capture a representative peak.

## The scoreboard JSON schema

Written to `bench/scoreboard/cpython_<gitrev>.json`. Per-cell logs in
`bench/scoreboard/logs_<gitrev>/`.

```jsonc
{
  "schema_version": 2,
  "kind": "cpython_floor_scoreboard",
  "generated_at": "<iso8601 utc>",
  "git_rev": "<full sha>",
  "host": {
    "platform": "darwin",
    "python_runner": "3.12.x",        // interpreter the TOOL ran under
    "cpython_baseline": "3.14.5"      // the CPython ORACLE (system python3)
  },
  "direction": "speedup = cpython_time / molt_time; >1.0 = molt faster; <1.0 = RED",
  "red_threshold": 1.00,
  "unstable_cv_threshold": 0.20,
  "methodology": { "samples_per_phase": 5, "warmup_runs": 2,
                   "cold_and_warm": true, "run_guard": "…", "build": "…" },
  "reserved_columns": {                // present-but-nullable; follow-up arc
    "pypy_ratio":  "nullable — PyPy not installed",
    "codon_ratio": "nullable — Codon not installed"
  },
  "summary": { "cells_total", "cells_green", "cells_red", "cells_unstable",
               "cells_build_failed", "cells_run_blocked", "cells_error",
               "any_red" },
  "benchmarks_run":      [ "tests/benchmarks/…", … ],
  "benchmarks_deferred": [ { "benchmark": "…", "reason": "…" }, … ],

  "scoreboard": {
    "<benchmark>": { "<target>": { "<backend>": { "<profile>": {
      // --- build facts ---
      "build_ok": true, "binary_size_kib": 4256.3, "compile_time_s": 3.08,
      // --- run facts ---
      "run_blocked": false, "run_blocked_reason": null,
      "molt_ok": true, "cpython_ok": true,
      // COLD (first cold-cache run) ---
      "cold_molt_s": 0.253, "cold_cpython_s": 0.068, "cold_ratio": 0.27,
      // WARM (steady-state median) ---
      "warm_molt_s": 0.069, "warm_cpython_s": 0.070, "warm_ratio": 1.01,
      // HEADLINE speedup = warm cpython / warm molt ---
      "cpython_ratio": 1.01,
      // peak RSS (worse of cold/warm) per runtime ---
      "molt_peak_rss_mib": 8.0, "cpython_peak_rss_mib": 15.0,
      // stability + status ---
      "stable": true, "red": true,
      "status": "red",   // green | red | unstable | run-blocked | build-failed | error
      "note": null,
      // reserved (follow-up toolchain arc) ---
      "pypy_ratio": null, "codon_ratio": null,
      // provenance ---
      "output_parity": true,
      "molt_stats":    { "samples_s", "median_s", "mean_s", "stdev_s", "cv",
                         "min_s", "max_s", "peak_rss_mib", "stable", "n" },
      "cpython_stats": { … same shape … },
      "log_artifact": "bench/scoreboard/logs_<gitrev>/<bench>__<backend>__<profile>.log"
    } } } }
  }
}
```

**Direction is labelled unambiguously**: `speedup = cpython_time / molt_time`.
`> 1.0` ⇒ molt faster (GREEN); `< 1.0` ⇒ molt slower (RED contract violation).

## Baseline red-list (this host, native + llvm / release-fast)

> Filled from the baseline sweep at the committed `<gitrev>` (git rev
> `79903045f5…`). See the committed `bench/scoreboard/cpython_<gitrev>.json` for
> the authoritative machine-readable board (112 cells = 56 native + 56 llvm);
> this section is the human summary.

**Merged board — 112 cells: 2 GREEN, 80 RED, 11 UNSTABLE, 8 BUILD-FAIL, 7 ERROR,
4 CPython-incompatible (deferred).** Per backend:

| backend | cells | GREEN | RED | UNSTABLE | BUILD-FAIL | ERROR | cpy-incompat |
|---------|-------|-------|-----|----------|------------|-------|--------------|
| native (Cranelift) | 56 | 1 | 46 | 6 | 0 | 0 | 3 |
| llvm (inkwell)     | 56 | 1 | 34 | 5 | **8** | **7** | 1 |

The two GREEN cells are both `bench_class_hierarchy` (native warm **6.09×** / cold
1.16×; llvm warm **3.95×** / cold 1.36×) — both phases beat CPython on both
backends. **LLVM is materially weaker than native**: 8 build-failures + 7 run
errors (0 on native) and 8 warm-reds (vs 3 on native) — a backend divergence the
per-backend table exists to surface (a native win does not excuse an LLVM
regression). The LLVM build-fail/error set clusters on
bytes/regex/generator/async/memoryview codegen (`bench_bytes_*`,
`bench_bytearray_*`, `bench_generator_iter`, `bench_async_await`,
`bench_memoryview_tobytes`, `bench_counter_words`, `bench_gc_pressure`,
`bench_json_roundtrip`, `bench_import_time`).

### NATIVE / release-fast

**56 cells: 1 GREEN, 46 RED, 6 UNSTABLE, 3 CPython-incompatible (deferred).** The
dominant RED class is
**cold-start overhead**, not slow steady-state. molt binaries pay a fixed
~0.15–0.25 s cold cost (binary load + dyld + runtime init) that makes short
benchmarks RED on the **cold** path while they are multiple-× faster **warm**.
The constitution's "no warm-only wins" rule correctly flags these. The RED list
splits into two families (the board's `summary.red_breakdown` keys them):

1. **`warm_red` — genuinely slow steady-state (the real representation gaps).**
   Only **3** on this host:

   | benchmark               | warm speedup | cold speedup | one-line missing-fact hypothesis |
   |-------------------------|--------------|--------------|----------------------------------|
   | `bench_etl_orders`      | **0.60×**    | 0.14×        | dict-of-records build + attribute writes box per field → missing *Repr-precise record/shape + borrow on the dict value slot* (dispatch/class-identity + boxing) |
   | `bench_csv_parse_wide`  | **0.68×**    | 0.14×        | per-cell `str` split/alloc dominates → missing *Repr-precise `str` slicing without per-field heap alloc + memchr-class field scan* (boxing/Repr + RC of the row buffer) |
   | `bench_exception_heavy` | **0.69×**    | 0.32×        | raise/except in a hot loop → missing *zero-cost-happy-path exception lowering + handler-region ownership* (generator/frame + RC, exception-CFG) |

2. **`cold_only_red` — startup tax (warm ≥ 1.0×, cold < 1.0×).** **43** cells.
   The workload is so short that cold-start dominates wall-time. Missing fact =
   **cold-start / binary-init cost** — the constitution's separate
   *binary-size / cold-start / RSS* column, tracked structurally by
   `tools/output_startup_size_audit.py`. Fix = defer module-init via
   `MODULE_IMPORT` + shrink the startup runtime surface; do **not** "optimize the
   benchmark loop." (e.g. `bench_sum` warm **8.61×** / cold 0.82×; `bench_dict_ops`
   warm 1.00× / cold 0.08×.)

### Stale memory-note hypotheses — REFUTED by measurement

The 5-yr-arc / memory notes named suspected reds. The baseline sweep **refutes
all three** — these benchmarks were optimized since the note was written:

| benchmark              | memory note (stale) | measured warm | measured cold | verdict |
|------------------------|---------------------|---------------|---------------|---------|
| `bench_class_hierarchy`| ~0.01× (100× slower)| **6.09× faster** | **1.16× faster** | **GREEN** (fully refuted — both phases beat CPython) |
| `bench_struct`         | ~0.05× (20× slower) | **4.83× faster** | 0.46× (cold) | warm-green; RED only on cold-start (startup tax) |
| `bench_bytes_find`     | ~0.06× (16× slower) | **8.72× faster** | 0.70× (cold) | warm-green; RED only on cold-start (startup tax) |

The lesson: **memory-note perf hypotheses must be confirmed by a real
measurement before being treated as reds.** The board is the source of truth.

### LLVM / release-fast — the divergence lane

**56 cells: 1 GREEN, 34 RED (8 warm-red + 26 cold-only-red), 5 UNSTABLE,
8 BUILD-FAIL, 7 ERROR, 1 CPython-incompatible.** LLVM diverges from native in
three ways the per-backend table is built to catch:

1. **8 BUILD-FAIL — LLVM-backend miscompiles** (0 on native). The clearest is a
   genuine LLVM module-verification failure on the regex path: *"Incorrect number
   of arguments passed to called function"* on `re__error___init__` (called with
   4 args vs its declared signature) — the **frontend closure-ABI `__init__`
   arity bug class** (cf. MEMORY.md re-import LLVM P0). Others cluster on
   generator/async/bytes/json codegen. The cell records `status="build-failed"`
   (gated RED) and the sweep continues rather than crashing.
2. **7 ERROR — LLVM binaries that built but failed to run** while CPython ran
   (e.g. `bench_memoryview_tobytes`). These are LLVM-runtime regressions, gated
   RED.
3. **8 warm-red (vs 3 on native)** — LLVM codegen is genuinely slower than
   Cranelift at steady state on: `bench_fib`, `bench_tuple_index`,
   `bench_tuple_pack`, `bench_dict_comprehension`, `bench_set_ops` (warm 0.51×!),
   `bench_csv_parse`, `bench_csv_parse_wide`, `bench_exception_heavy`.

A native win does not excuse any of these; each is its own LLVM RED. The
authoritative per-cell detail is in the committed JSON under each benchmark's
`llvm` key. (Binary sizes are notably smaller on LLVM — ~3547 KiB vs native's
~4256 KiB — but compile time is ~20× slower per build.)

### Per-red missing-fact hypothesis (representation, not passes)

For each *steady-state* (warm) RED the board surfaces, the one-line hypothesis
of the missing IR fact (per the constitution's representation lens):

- **Cold-start reds (warm-green)** → missing fact = *binary-init / dyld / runtime
  bootstrap cost*. Lane: cold-start + binary-size (`output_startup_size_audit`),
  not the hot path. Fix = defer module-init via `MODULE_IMPORT`, shrink the
  startup runtime surface; do **not** "optimize the benchmark loop."
- **str/bytes find/replace/count reds** (if warm-red) → missing fact = *SIMD /
  memchr-class byte-search lowering + Repr-precise `bytes`/`str` storage* (no
  boxing on the scan). RC/ownership of the haystack across the scan loop.
- **dict/set-ops reds** (if warm-red) → missing fact = *class-identity / shape
  guard on the key type + inline-cache tiering for `__hash__`/`__eq__`* so the
  probe loop devirtualizes.
- **generator/async reds** (if warm-red) → missing fact = *resumable-frame
  ownership + generator-fusion eligibility* (per the genleak/fusion arcs).
- **loop/numeric reds** (if warm-red) → missing fact = *induction-variable /
  range / overflow / lane-stability* so the loop stays unboxed I64.

> The authoritative, per-benchmark red-list with exact ratios is the committed
> JSON. Regenerate + diff with `--baseline` on every perf-relevant landing.

## What was measured vs deferred (no silent truncation)

**Measured (baseline run):**
- Backends: **native** (Cranelift) and **llvm** (inkwell, `MOLT_BACKEND=llvm`) —
  both lanes fully swept (56 cells each, 112 total).
- Profile: **release-fast** (the daily-contract profile; CLI `--build-profile
  release` → cargo `release-fast` for the backend).
- Set: **core** = the 56-benchmark curated verified subset in
  `bench_suites.BENCHMARKS`.

**Deferred / excluded (explicitly, not silently):**
- **CPython-incompatible benchmarks** (`status="cpython-incompatible"`, in
  `benchmarks_deferred`): `bench_parse_msgpack` (imports `molt_msgpack`),
  `bench_ptr_registry` + `bench_channel_throughput` (import `molt.intrinsics`
  `molt_chan_*`). These are **molt-internal** benchmarks the system CPython 3.14
  oracle cannot run, so there is no valid floor to compare against — they are
  excluded from the gate, NOT scored RED.
- **WASM run-path** — build/link only, recorded `run-blocked`
  (`run_blocked_reason` = socket-import instantiation gap). Re-enable the run
  column once the WASM instantiation gap is closed; the build-facts (size,
  compile-time, links-ok) are still captured.
- **Luau** — has its own harness (`tools/benchmark_luau_vs_cpython.py`); fold
  into the board as a 4th backend lane in a follow-up.
- **release-output / dev-fast profiles** — release-fast first per the
  constitution; add as additional `--profile` columns (the tool already accepts
  them) in the incremental next fill.
- **The other ~47 benchmark files** in `tests/benchmarks/` outside the core
  suite — the core suite is the repo's canonical perf set; widening to all 103
  is a deliberate next step, not a silent drop.

## PyPy / Codon follow-up toolchain plan

The schema already carries nullable `pypy_ratio` / `codon_ratio` per cell.
Neither runtime is installed on this host; the follow-up arc:

1. **Install + pin the reference runtimes** (their own toolchain step, isolated
   from the molt build env):
   - **PyPy**: `pypy3.11` (or latest), pinned by version; record
     `pypy_version` in `host`. PyPy is the *dynamic-runtime reference* (~3× over
     CPython 3.11 via JIT) for pure-Python dynamic workloads.
   - **Codon**: `codon` from exaloop, pinned; record `codon_version`. Codon is
     the *AOT/native north star* (10–100×+, C/C++-class) for the statically
     compilable, **non-drop-in-semantics** subset.
2. **Add two runtime lanes** mirroring the CPython path: time PyPy and Codon
   through `safe_run.py --json` with the *same* sample/warmup/cold+warm
   discipline. Compute `pypy_ratio = pypy_time / molt_time` and
   `codon_ratio = codon_time / molt_time` (same direction: > 1.0 = molt faster).
3. **Semantic-equivalence gate for Codon**: Codon is *not* drop-in CPython.
   Mark any benchmark whose Codon semantics diverge as `"non-equivalent"` rather
   than a win/loss (the constitution forbids scoring non-equivalent models).
   Maintain a per-benchmark `codon_equivalent: bool` allow-list; only
   equivalent benchmarks contribute a `codon_ratio`.
4. **Two more constitution scoreboards** (separate gates, kept green):
   - **PyPy board**: where PyPy wins, the cell must *name the missing molt
     compiler fact* (IC tiering, class-version guard, borrow inference, generator
     fusion, shape propagation, trace-like loop specialization) — not just a
     number. This is a measure-and-name board, not a hard CI gate (PyPy parity
     is an aspiration, not the floor).
   - **Codon board**: approach/exceed on numeric/loop/data-structure/NumPy-like
     kernels where semantics match; non-equivalent rows excluded.
5. **Backend × Profile boards** (constitution scoreboards 4 + 5): the per-cell
   data already supports slicing native/LLVM/WASM/Luau and
   dev/release-fast/release-output into their own tables — add the report views
   once the LLVM + WASM + Luau lanes and the second profile are all populated.
