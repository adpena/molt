# CPython Floor-Scoreboard — the release-blocking performance gate

`tools/perf_scoreboard.py` operationalizes the **Performance Constitution**
(`CLAUDE.md`, commit `538f4386e`) and the **council two-dimensional gating
ruling** (`project_council_decisions_20260608`, section A). It is the
machine-readable artifact the constitution mandates: a single scoreboard keyed
**benchmark × target × backend × profile** reporting the molt-vs-CPython speedup
split into a **warm (execution-engine) axis** and a **cold (startup-tax) axis**,
with a CI-gateable nonzero exit and full **provenance** so a board is never
silently measured against a stale tree.

This tool **measures and surfaces**; it does not fix slow benchmarks. Fixing a
RED benchmark is a separate optimization arc — and per the constitution's
*"fix the representation, not the pass"* posture, the first question for each RED
is never *"which peephole recovers it"* but *"which FACT is missing from IR?"*.

## Two-dimensional gating (council ruling A) — warm ≠ cold

The retired `red` bool blended two structurally different failures. The board
now emits one **verdict** per cell:

| verdict | condition | meaning / lane |
|---------|-----------|----------------|
| `GREEN` | warm fast, cold fast, tax within budget | won |
| `FAIL_ENGINE` | `warm_speedup = cpython_warm/molt_warm ≤ 1.00` | **execution-engine red — RELEASE BLOCKER.** Needs an IR FACT. |
| `FAIL_COLD_BUDGET` | `startup_tax_ms > budget_ms` | startup regression. Routes to the cold-start/runtime/artifact lane. |
| `WARN_COLD_FLOOR` | `cold_speedup ≤ 1.00` BUT `warm_speedup > 1.00` and the loss is the fixed startup tax (within budget) | **not a hard red.** Warns; fails the gate only with `--strict-cold`. |
| `FAIL_STALE` | board tree ≠ fresh origin/main (non-authoritative) | overrides all. Fails the gate unless `--allow-nonauthoritative`. |
| `UNSTABLE` | CV above threshold | untrustworthy in either direction — gated. |
| `BUILD_FAILED` / `RUN_ERROR` | molt build failed / CPython ran but molt did not | gated. |
| `RUN_BLOCKED` / `CPY_INCOMPATIBLE` | wasm build/link-only / no CPython floor | not gated. |

Where:
- `warm_speedup = cpython_warm / molt_warm` (steady-state — the engine axis).
- `cold_speedup = cpython_cold / molt_cold` (end-to-end cold path).
- `startup_tax_ms = (molt_cold_total − molt_warm_total) × 1000` — the **fixed
  startup cost** molt pays cold that the warm steady state does not. This (NOT
  `cold_speedup`) is what the cold-start budget gates against.

**The gate fails (nonzero exit) iff any** `FAIL_ENGINE`, `FAIL_COLD_BUDGET`,
`BUILD_FAILED`, `RUN_ERROR`, or `UNSTABLE`. `WARN_COLD_FLOOR` does **not** fail
the gate (it is a fixed tax within budget) unless `--strict-cold`. `FAIL_STALE`
fails unless `--allow-nonauthoritative` (local-debug opt-out). Never blend
cold+warm into one bucket — warm reds need IR facts; cold reds need
startup/runtime/artifact work.

The cold-start budget lives in `bench/scoreboard/cold_start_budget.json`, keyed
`<backend>/<profile>` → `budget_ms`. **v0 = the measured baseline** per the
council ladder ("v0 = current measured baseline; near-term = no regression from
baseline; release-output native Y1 = startup_tax < 100ms"). A missing budget
means `FAIL_COLD_BUDGET` cannot fire (we never invent a budget); the measured
tax is still recorded so the ceiling can be seeded.

## Provenance — the anti-stale-lore enforcement (council ruling A + B)

Every board carries the exact tree + tool + artifact identity it was measured
against, under the top-level `provenance` block:

```jsonc
"provenance": {
  "origin_sha":              "<origin/main tip>",
  "local_head_sha":          "<HEAD the board was measured at>",
  "merge_base_sha":          "<merge-base(HEAD, origin/main)>",
  "dirty_tree":              false,
  "diverges_from_origin":    false,
  "benchmark_tool_sha":      "<git blob of perf_scoreboard.py on disk>",
  "benchmark_tool_modified": false,            // tool differs from its committed blob?
  "backend_binary_identity": { "native/release-fast": "<path|mtime_ns|size>" },
  "stdlib_cache_key":        "<runtime/codegen source-tree fingerprint>",
  "authoritative":           true,
  "authoritative_reason":    "tree == origin/main, clean, tool unmodified"
}
```

`authoritative` is **false** whenever the local HEAD diverges from origin/main,
OR the tree is dirty, OR the scoreboard tool itself is modified-vs-HEAD. A
non-authoritative board PRINTS *"WARNING: local tree diverges from origin;
benchmark is non-authoritative unless explicitly requested"* and stamps every
cell `FAIL_STALE`. `backend_binary_identity` reuses `cli.py._backend_binary_identity`
(`path|mtime_ns|size`) — the same signal the stdlib/TIR caches salt with — so a
reader can detect the stale-cache confound class directly.

> Authoring note: a Lane-C run that **adds a commit to the scoreboard tool on top
> of origin/main** is technically non-authoritative by the strict
> `local_head ≠ origin_sha` rule, even though the COMPILER behavior measured is
> exactly origin/main's. Such a run uses `--allow-nonauthoritative`; the board's
> `provenance` records `local_head_sha` (the tooling commit) and `origin_sha`
> (the compiler tree) so a reader sees the only diff is the tool, and the
> compiler perf numbers ARE origin/main's.

## Measurement hygiene (#69) — the standard, and the 5 PERMANENT RULES

The last optimization cycle proved the scoreboard can pick the **wrong**
subsystem under load: a "0.66 red" was a **loaded-machine artifact** (a parallel
multi-agent build stole cycles from the timed process), and an alloc-count was
misattributed to warm time. The council ruling (#69): **measurement hygiene
OUTRANKS all optimization.** A bad measurement that sends an agent to optimize a
phantom red is worse than no measurement. These five rules are PERMANENT.

### The 5 PERMANENT RULES

1. **Alloc-attribution alone cannot justify a warm-time optimization.** A
   warm-red (`warm_speedup < 1.00`) requires **CYCLE attribution** — a CPU
   self-time profile of the running molt binary (`--emit-cycle-profile`,
   `/usr/bin/sample`) that names the hot symbols. An allocation count is a
   *hypothesis generator*, never the justification: the only valid warm-opt
   signal is cycles spent, because the thing being optimized is wall-clock.
2. **No run is authoritative while other molt / cargo / rustc work is active.**
   A board measured while ANY build/test work competes for cores is
   `authoritative=false` for warm verdicts, full stop. Your OWN benchmark builds
   are fine — but do them FIRST, then time with nothing else compiling.
3. **A red that vanishes under quiescence is a measurement-system bug, not a
   compiler target.** If a cell is `RED_*` under load but `TIE`/`GREEN` on a
   quiet machine, the defect is in the MEASUREMENT, not the compiler. Fix the
   measurement (or wait for quiescence) — do NOT open a compiler optimization.
4. **A `DIMENSIONAL_WIN` may land without a warm flip, but MUST be reported as
   dimensional, not a speed heal.** A change that does not move `warm_speedup`
   above 1.00 but materially improves alloc / RSS / binary-size / cold / backend
   vs a baseline is real and may land — but it is recorded as a *dimensional*
   win. It never gets to claim it "fixed" a warm red it did not move.
5. **Stale lore loses to the board; the board loses to quiescence + provenance.**
   A claim in a doc/memory loses to what the board measures. But the board
   itself is only believed when its provenance says the tree was origin/main,
   clean, and the machine was quiescent. Provenance + quiescence are the root of
   trust; everything downstream (lore, then board) is subordinate.

### The 5-state classification (`--classify`)

The single warm verdict (`FAIL_ENGINE` vs `GREEN`) cannot answer the council's
real question: *is this a TRUE compiler target, or a measurement artifact?* With
`--classify`, each cell gets one of five states. The decisive new inputs are
**machine QUIESCENCE** and the **repeat-pass CONFIDENCE INTERVAL** (`--repeat
N`): a warm-red is a real target only if it is stable AND quiescent AND its CI
sits clear below 1.00.

| classification | condition | meaning |
|----------------|-----------|---------|
| `RED_STABLE` | `warm < 1.00`, repeat CI entirely below 1.00, machine quiescent, cell robustly stable | **the TRUE warm-red set** — a real compiler target. Steer the next opt by its CYCLE profile. |
| `RED_NOISY` | warm point below 1.00 BUT contaminated (not quiescent) / unstable / no repeat CI / CI does not clear | **NOT a target yet.** The reason names why (this is exactly the "0.66 under load" artifact). Re-measure quiet with `--repeat`. |
| `TIE` | repeat CI straddles 1.00, OR `warm == 1.00` single-pass | statistically CPython — neither a win nor a loss. Wants facts to cross decisively, but is not a release-blocking red. |
| `GREEN_STABLE` | `warm > 1.00`, stable + quiescent (CI clears above 1.00 when repeated) | a confirmed win. |
| `DIMENSIONAL_WIN` | warm gate flat (tie) BUT a non-warm dimension (alloc/RSS/size/cold) improved ≥ 5% vs `--baseline` | Rule 4: landed without a warm flip, better elsewhere. Needs a baseline to detect. |
| `INFRA` | `BUILD_FAILED` / `RUN_ERROR` / `RUN_BLOCKED` / `CPY_INCOMPATIBLE` / `FAIL_STALE` | no warm number to classify — passes through. |

The repeat CI is **authoritative over a lone point estimate** (the whole reason
for `--repeat`): a 0.98 point sample whose 5-pass CI clears *above* 1.00 is a
`GREEN_STABLE`, not a red; a 1.05 point sample whose CI sits *below* 1.00 is a
red. `FAIL_ENGINE` maps to `RED_STABLE` **only** when quiescent + stable + CI
below; under contamination the same cell is `RED_NOISY`.

**Asymmetry of contamination (why quiescence gates reds but not greens).** A
competing build steals cycles from the *timed* molt process, so it can only make
molt look **slower**, never artificially faster. Quiescence is therefore
**mandatory for a warm-RED** (load can manufacture a false red → a non-quiescent
red is `RED_NOISY`, never `RED_STABLE`) but **not required for a warm-GREEN**: a
cell measured *faster* than CPython under load is a *conservative* `GREEN_STABLE`
(the quiet number can only be better). Only cell **instability** demotes a green.
This is why the non-quiescent LLVM board still shows `bench_sum` 10.56× as GREEN,
not RED_NOISY — calling a 10× cell "red" because an idle daemon was running would
be the measurement-system bug Rule 3 warns against. Board-level *authority* is
still gated by quiescence; the per-cell green/red *direction* is not.

### The quiescence guard (`--require-quiescent`) — checks + thresholds

BEFORE any number is taken, `gather_quiescence()` detects contamination. The run
still produces EXPLORATORY numbers on a noisy machine, but the board is stamped
`authoritative=false` and stdout prints *"NON-AUTHORITATIVE: machine not quiet;
do not optimize from this red list"* naming WHICH check failed:

| check | signal | threshold / rule |
|-------|--------|------------------|
| active build processes | `pgrep -fl 'cargo\|rustc\|molt-backend\|molt build'` | **any** match (other than this tool's own tree) ⇒ not quiet. **Claude/Codex host-control processes are excluded by name** — never counted, never killed (project policy). |
| 1-min load average | `sysctl -n vm.loadavg` vs `sysctl -n hw.ncpu` | `load > ncpu × 0.5` ⇒ not quiet (18-core host ⇒ load > 9.0). Permissive enough that a quiet desktop still measures; an active build always trips this AND the process check. |
| runnable-thread storm | runnable thread count | `> max(2, ncpu × 0.5)` ⇒ contended (catches a build storm before the 1-min EWMA does). |
| thermal / frequency throttle | `pmset -g therm` (best-effort) | throttle active ⇒ not quiet. Skipped if the probe is unavailable (never invents a result). |
| probe failure | `pgrep` unavailable | **fail-closed**: if we cannot probe, we cannot certify quiet ⇒ not quiet. |
| stale tree / dirty / tool-modified | git + tool blob | folded into `provenance.authoritative` (the existing stale-lore guard). |

`--print-provenance` dumps the full block including the new fields
`active_molt_processes`, `active_cargo_or_rustc_processes`, `loadavg_1m`,
`ncpu`, `runnable_signal` alongside the origin/candidate SHAs,
`backend_binary_identity`, and `stdlib_cache_key`.

### The cycle-attribution mechanism (`--emit-cycle-profile`) — Rule 1

For every warm-red (`RED_STABLE`/`RED_NOISY`), the tool launches the molt binary
under the `safe_run` watchdog (a profiled binary is still a raw binary and MUST
be RSS/timeout-guarded), finds the binary's PID, attaches `/usr/bin/sample` for
~3 s, and parses the *"Sort by top of stack"* self-time leaderboard into the
cell's `cycle_profile.top_symbols` (`{symbol, self_samples, lib}`). This is the
attribution signal the NEXT optimization is steered by — **CYCLES, not
alloc-count** (Rule 1). If the sampler is unavailable or the process is too short
to attach, the cell records a documented `available=false` note — never a fake
signal.

### Warm-hot cycle attribution (`--sample-hot-only`) — #76, the WARM prerequisite

**Why the one-shot `--emit-cycle-profile` above is INVALID for *warm* attribution.**
A one-shot benchmark binary spends **~85–92 % of its leaf self-time in
`_dyld_start`** — process launch plus the first-touch page-in of molt's large
static binary. Measured directly on the #69 quiet board:
`bench_exception_heavy` = **91.7 % `_dyld_start`**, `bench_etl_orders` =
**88.5 % `_dyld_start`**, with the in-binary frames showing as `???` (release-fast
strips the molt user-fn symbols). So the steady-state Python hot path *never*
dominates the sample leaderboard, and **Rule 1 ("warm-red optimization requires
cycle attribution") is UNSATISFIABLE** for warm hot paths from a one-shot sample.
The prior path ran the binary N times *back-to-back as separate processes*, so
every run re-paid `_dyld_start` — launch still dominated. `--sample-hot-only`
(#76) is the machinery that makes the warm hot path legible. It defeats the two
root causes structurally:

1. **Launch/page-in domination → `--inner-repeat N` (looped body).** The
   benchmark's `main()` is wrapped in `for _ in range(N): main()` **inside ONE
   process** (`tools/perf_inner_repeat.py`), so `_dyld_start` is paid *once* and
   amortizes over N iterations of the actual hot path (pyperf's `inner_loops`
   model — recorded as `inner_loops` in the profile board). The transform is
   **AST-based and semantics-preserving**: it wraps ONLY the canonical molt-bench
   shape (a single top-level `def main()` with no required args and no
   `global`/`nonlocal`, plus an `if __name__ == "__main__":` guard whose body is
   exactly `main()`), so N iterations produce the one-shot output printed N times
   and nothing else. Any other shape is **REFUSED** with a typed reason — never a
   silently non-equivalent variant (zero-workaround policy). Verified under
   CPython: looped output `== one-shot output × N` for both benchmarks.

2. **Symbol stripping → `--profile-build` (symbolicate).** release-fast strips
   the molt user-fn symbols at the *final link* (`-Wl,-x -Wl,-S` + a post-link
   `strip -x`), so `sample` shows `???` for in-binary frames. The fix REUSES
   molt's existing **`MOLT_KEEP_SYMBOLS=1`** diagnostic build-env hatch
   (`src/molt/cli.py`), which skips BOTH strips and keeps the local symbol names.
   It is additive: it changes **no** default product build and adds **only** a
   symbol table — the CODE is byte-identical to the stripped build, so the
   profiling binary's *timing* is representative. We do NOT add a redundant cargo
   profile: the user-fn symbols come from the Cranelift `output.o` + the final
   link, which a `[profile.*]` would not govern (those symbols are already
   unstripped pre-link; only the final-link strip removes them).

**How it samples.** For each benchmark the tool (a) times one looped run to learn
its lifetime, (b) launches the looped+symbolicated process under `safe_run`,
sleeps a short **warmup** so the first iterations (cold I-cache, first-touch
page-in) are excluded, then (c) attaches `/usr/bin/sample` to the now-running
steady state for a window auto-fitted to the remaining lifetime (so the sampler
always closes before the process exits). `/usr/bin/sample` has no built-in
warmup-delay flag, so the warmup is realized by *delaying the attach*.

**The REFUSAL rule (fail-closed, same discipline as #69's quiescence guard).**
After looping + symbols, the tool classifies the leaderboard into launch
(`_dyld_start` in `dyld`) vs in-binary self-time. If launch/page-in is still
**≥ 40 %** of leaf self-time, the loop factor was too small — the tool prints
`CYCLE-ATTRIBUTION INVALID: launch/page-in dominates; increase --inner-repeat`
and emits **NO** hot-path claim (`available=false, refused=true`). It also
refuses (with a precise reason) when: the benchmark is not loopable; the looped
runtime is too short to carve a steady window (→ raise N); or the inner-repeat
amplified a **per-iteration molt leak** past the RSS cap (the size run OOMs — a
real compiler-RC finding surfaced as a side effect; → LOWER N to profile a
bounded window).

**The `inner_loops` provenance field.** Every hot-only profile records
`inner_loops` (the wrap factor N), `symbolicated` + `symbolicate_mechanism`
(`MOLT_KEEP_SYMBOLS=1`), `launch_refusal_fraction` (0.40), and per-cell
`launch_breakdown` (`{total, launch_samples, launch_fraction, in_binary_*,
launch_dominates}`) + `in_binary_top` (the named cycle facts with their
`leaderboard_pct`). This is the cycle fact that selects the next optimization.

**#76 is the prerequisite for #68 (etl_orders) and exception_heavy optimization.**
Before this, warm cycle attribution for those two `RED_STABLE` cells was
impossible (launch dominated). The attributions this unblocks are recorded in
[the etl_orders / exception_heavy section below](#etl_orders-exception_heavy-cycle-attribution-68-76-now-attributed).

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
    --set core --backend native --backend llvm --profile release-fast \
    --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent

# Diff a fresh run against the last stored scoreboard (newly-red / regressed).
uv run --python 3.12 python3 tools/perf_scoreboard.py --set core --backend native --baseline

# Measure-only (do not fail CI on RED): add --no-gate.

# WARM-HOT cycle attribution (#76): looped + symbolicated sample of the warm
# hot path (NOT a speedup gate). One backend; writes hot_profile_<backend>_<rev>.json.
# Exits 1 if ANY benchmark is REFUSED (launch still dominated, or not loopable).
uv run --python 3.12 python3 tools/perf_scoreboard.py --sample-hot-only \
    --backend native --inner-repeat 40 \
    --benchmark tests/benchmarks/bench_exception_heavy.py \
    --benchmark tests/benchmarks/bench_etl_orders.py
```

The `--sample-hot-only` path (#76) is documented in full under
[Warm-hot cycle attribution](#warm-hot-cycle-attribution---sample-hot-only-76-the-warm-prerequisite).
`--inner-repeat N` sets the in-process repeat factor (default 40 — a multi-second
process for the curated benchmarks within a sane RSS budget); raise it if the tool
REFUSES with launch-still-dominates, LOWER it if a benchmark amplifies a
per-iteration leak past the RSS cap. `--profile-build` (implied by
`--sample-hot-only`) builds with `MOLT_KEEP_SYMBOLS=1` so molt user-fn symbols are
retained — additive, never changes a normal build or any speedup number.

- The **CPython oracle is a probed host-native CPython 3.12+ interpreter**,
  resolved explicitly via `--cpython` or by OS-aware candidate discovery on
  Windows, macOS, and Linux. The resolver rejects non-CPython, wrong-OS,
  wrong-arch, wrong-pointer-width, too-old, broken launcher, and project
  `.venv`/session interpreters before any benchmark cell runs. The accepted
  executable, OS, normalized architecture, and pointer width are recorded in
  `host.cpython_oracle`.
- `MOLT_SESSION_ID=perfscore` + `CARGO_TARGET_DIR=target/sessions/perfscore`
  isolate the build cache (the constitution's concurrent-dev contract).
- The LLVM lane forces `MOLT_BACKEND=llvm` and `LLVM_SYS_211_PREFIX`
  (`/opt/homebrew/opt/llvm@21` — the brew default `llvm@22` is the WRONG version
  for llvm-sys 211). Its first build recompiles the backend with the `llvm`
  feature (~5 min).

### Exit code (the gate)

Exit is **nonzero iff any cell is `FAIL_ENGINE`, `FAIL_COLD_BUDGET`,
`BUILD_FAILED`, `RUN_ERROR`, or `UNSTABLE`**. `WARN_COLD_FLOOR` does NOT fail
the gate (fixed tax within budget) unless `--strict-cold`. `FAIL_STALE` fails
unless `--allow-nonauthoritative`. `RUN_BLOCKED` (WASM) and `CPY_INCOMPATIBLE`
are not gated. `--no-gate` always exits 0.

```bash
# Add the PyPy + Codon comparator lanes (auto-detect, or pass an explicit path).
uv run --python 3.12 python3 tools/perf_scoreboard.py --set core --backend native \
    --pypy --codon

# Local debugging on a divergent tree (e.g. an in-flight scoreboard-tool commit):
#   classifies real numbers, board stays authoritative=false, gate won't FAIL_STALE.
uv run --python 3.12 python3 tools/perf_scoreboard.py --set core --allow-nonauthoritative

# Make cold-floor warnings hard-fail too:
uv run --python 3.12 python3 tools/perf_scoreboard.py --set core --strict-cold
```

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

`tools/perf_schema.py` is the schema authority: it owns the schema version, the
verdict/classification vocabularies, the `RED_THRESHOLD` and `UNSTABLE_CV`
gate constants, required top-level/provenance/host/summary/cell fields, and the
`validate_cell()` / `validate_board()` helpers used by
`tools/perf_scoreboard.py`. For measured verdicts (`GREEN`, `FAIL_ENGINE`,
`FAIL_COLD_BUDGET`, `WARN_COLD_FLOOR`, `UNSTABLE`), `validate_cell()` now
fails closed unless the build/run booleans, binary size, compile time, cold and
warm timings, speedups, startup tax, RSS peaks, and log artifact are present. A
`RED_STABLE` classification must also carry `measured_quiescent=true` plus a
numeric repeat CI clearing below the CPython floor. When a board carries modern
host metadata, `validate_board()` also verifies that `host.cpython_oracle` is
CPython, matches the board OS/architecture/pointer width, matches
`cpython_baseline`, and uses the resolved executable as `cmd[0]` rather than a
launcher wrapper. `tools/perf_scoreboard.py` validates every CPython scoreboard
document before writing it, including checkpoint partials, rebuild-summary
rewrites, merge outputs, and final run artifacts; a schema violation raises
`ScoreboardSchemaError` and leaves the target artifact untouched.
`tools/perf_causality.py` owns the deterministic cycle-symbol/taxonomy join that
fills `fact_class`, `suspected_missing_fact`, `pypy_advantage_class`,
`reference_class`, `codon_semantics`, and `attribution_confidence`; the
scoreboard runner copies those derived fields into red cells instead of keeping
a private name-pattern hint table. When supplied `--pass-delta-dashboard` and
`--call-fact-coverage` JSON evidence, it also records the supporting pass-delta
score/passes/fact classes and call-fact attached/transient counts without letting
those secondary inputs override the primary attribution class.
`MOLT_EMIT_PASS_DELTA=1` adds the machine-readable TIR pass-delta feed consumed
by `tools/pass_delta_dashboard.py`: one JSONL row per pass with explicit host
OS/architecture/pointer-width, target/profile, before/after fact profiles, and
the deltas for lost representation values, added boxes, generic/runtime-helper
calls, RC events, exception events, guards, and heap allocations. By default the
diagnostic feed is written under `tmp/molt-backend/tir/pass_delta.jsonl`; set
`MOLT_PASS_DELTA_PATH` to direct it to a specific artifact path.

The unified CI driver includes this as the required Tier 1
`perf-scoreboard-contract` check: it runs `tests/tools/test_perf_causality.py`,
`tests/tools/test_pass_delta_dashboard.py`, `tests/tools/test_perf_schema.py`,
and `tests/tools/test_perf_scoreboard.py` without a Rust rebuild, so schema,
attribution, pass-delta dashboard, oracle, and emission-contract drift cannot
bypass the default gate.

Written to `bench/scoreboard/cpython_<gitrev>.json`. Per-cell logs in
`bench/scoreboard/logs_<gitrev>/`.

```jsonc
{
  "schema_version": 3,
  "kind": "cpython_floor_scoreboard",
  "generated_at": "<iso8601 utc>",
  "git_rev": "<full sha>",
  "provenance": { /* see the Provenance section above */ },
  "host": {
    "platform": "darwin",
    "machine": "arm64",
    "arch": "aarch64",
    "pointer_bits": 64,
    "python_runner": "3.12.x",        // interpreter the TOOL ran under
    "cpython_baseline": "3.14.5",     // the accepted CPython oracle version
    "cpython_oracle": {
      "cmd": ["/opt/homebrew/bin/python3"],
      "executable": "/opt/homebrew/bin/python3",
      "implementation": "CPython",
      "version": "3.14.5",
      "sys_platform": "darwin",
      "machine": "arm64",
      "arch": "aarch64",
      "pointer_bits": 64
    },
    "pypy": "3.10.14 (… PyPy 7.3.17)",// null if --pypy not used
    "codon": "codon 0.19.6"           // null if --codon not used
  },
  "direction": "speedup = cpython_time / molt_time; >1.0 = molt faster",
  "red_threshold": 1.00,
  "unstable_cv_threshold": 0.20,
  "verdict_legend": { "GREEN": "…", "FAIL_ENGINE": "…", … },
  "methodology": { "samples_per_phase": 5, "warmup_runs": 2,
                   "cold_and_warm": true,
                   "warm_speedup": "cpython_warm / molt_warm",
                   "cold_speedup": "cpython_cold / molt_cold",
                   "startup_tax_ms": "(molt_cold_total - molt_warm_total) * 1000" },
  "reserved_columns": { "pypy_ratio": "…", "codon_ratio": "…" },
  "summary": {
    "cells_total", "cells_green",
    // the two-dimensional verdict counts (the gate axes) ---
    "cells_fail_engine", "cells_fail_cold_budget", "cells_warn_cold_floor",
    "cells_fail_stale", "cells_unstable", "cells_build_failed",
    "cells_run_blocked", "cells_error", "cells_cpython_incompatible",
    "gate_fails",
    // every cell keyed by its verdict (route warm vs cold reds) ---
    "verdict_breakdown": { "FAIL_ENGINE": [...], "FAIL_COLD_BUDGET": [...],
                           "WARN_COLD_FLOOR": [...], "GREEN": [...], … }
  },
  "benchmarks_run":      [ "tests/benchmarks/…", … ],
  "benchmarks_deferred": [ { "benchmark": "…", "reason": "…" }, … ],

  "scoreboard": {
    "<benchmark>": { "<target>": { "<backend>": { "<profile>": {
      // --- build facts ---
      "build_ok": true, "binary_size_kib": 4256.3, "compile_time_s": 3.08,
      // --- run facts ---
      "run_blocked": false, "molt_ok": true, "cpython_ok": true,
      // COLD / WARM ---
      "cold_molt_s": 0.253, "cold_cpython_s": 0.068,
      "warm_molt_s": 0.069, "warm_cpython_s": 0.070,
      // --- TWO-DIMENSIONAL verdict (council ruling A) ---
      "warm_speedup": 1.01,            // = cpython_warm / molt_warm (engine axis)
      "cold_speedup": 0.27,            // = cpython_cold / molt_cold
      "startup_tax_ms": 184.0,         // = (cold_molt - warm_molt) * 1000
      "cold_budget_ms": 250.0,         // the budget this cell was gated against
      "verdict": "WARN_COLD_FLOOR",    // GREEN|FAIL_ENGINE|FAIL_COLD_BUDGET|WARN_COLD_FLOOR|…
      "fact_class": "shape_facts",     // derived fact family for a warm red
      "suspected_missing_fact": "…",   // derived missing-fact attribution
      "pypy_advantage_class": "shape_propagation",
      "reference_class": "static_equiv",
      "codon_semantics": "non_equivalent",
      "attribution_confidence": 0.7,
      "suspected_startup_component": "…", // triage hint for a cold red
      // peak RSS + stability ---
      "molt_peak_rss_mib": 8.0, "cpython_peak_rss_mib": 15.0,
      "stable": true, "note": null,
      // --- PyPy / Codon comparator lanes (null unless --pypy/--codon) ---
      "pypy_ratio": 0.51, "pypy_warm_s": 0.035,       // pypy_warm/molt_warm
      "codon_ratio": 0.30, "codon_warm_s": 0.018,     // codon_warm/molt_warm
      "codon_equivalent": true, "codon_note": "equivalent (codon -release AOT)",
      // provenance ---
      "output_parity": true,
      "molt_stats": { … }, "cpython_stats": { … },
      "log_artifact": "bench/scoreboard/logs_<gitrev>/<bench>__<backend>__<profile>.log"
    } } } }
  }
}
```

**Direction is labelled unambiguously**: `speedup = cpython_time / molt_time`.
`> 1.0` ⇒ molt faster. The **warm** axis (`warm_speedup`) is the
execution-engine contract; the **cold** axis is the startup-tax budget — they
are never blended.

### The hot-only cycle-profile board (`--sample-hot-only`, #76)

A **separate** JSON document (`kind: "hot_only_cycle_profile"`, written to
`bench/scoreboard/hot_profile_<backend>_<gitrev>.json`) — it is a cycle-attribution
artifact, **not** a speedup board, and never feeds the release gate.

```jsonc
{
  "schema_version": 3,
  "kind": "hot_only_cycle_profile",
  "git_rev": "<full sha>",
  "backend": "native", "target": "native", "profile": "release-fast",
  "inner_loops": 40,                 // the in-process repeat factor N (provenance)
  "symbolicated": true,
  "symbolicate_mechanism": "MOLT_KEEP_SYMBOLS=1 (link-strip + post-link strip skipped)",
  "launch_refusal_fraction": 0.40,   // launch >= this => REFUSE a hot-path claim
  "cpython_baseline": "3.14.5",
  "quiescence": { /* the #69 quiescence block, same shape */ },
  "methodology": "inner-repeat main() N times in one process; MOLT_KEEP_SYMBOLS=1; …",
  "cells": [ {
    "benchmark": "tests/benchmarks/bench_exception_heavy.py",
    "backend": "native", "profile": "release-fast",
    "inner_loops": 40,
    "build": { "inner_loops": 40, "symbolicated": true, "looped": true,
               "refused": false, "reason": null, "looped_source_path": "…" },
    "profile_result": {
      "available": true,             // false + refused=true when launch dominated / leaked
      "mode": "hot-only",
      "inner_loops": 40,
      "launch_breakdown": { "total": 2526, "launch_samples": 0,
                            "launch_fraction": 0.0, "in_binary_samples": 2526,
                            "in_binary_fraction": 1.0, "launch_dominates": false },
      "in_binary_top": [             // the NAMED cycle facts (the deliverable)
        { "symbol": "bench_exception_heavy__molt_user_main",
          "self_samples": 543, "leaderboard_pct": 21.5, "lib": "…_molt" },
        { "symbol": "molt_inc_ref_obj", "self_samples": 300,
          "leaderboard_pct": 11.9, "lib": "…_molt" }
        /* … */
      ],
      "top_symbols": [ /* the full leaderboard incl. libsystem/dyld frames */ ],
      "refused": false, "refused_reason": null,
      "leak_suspected": false,       // true when the size run OOM'd (inner-repeat leak)
      "note": "/usr/bin/sample 1.7s steady-state (after 0.6s warmup) of ONE looped(…)…"
    }
  } ]
}
```

## Contamination policy (when a board is believed)

- A board is **authoritative for warm verdicts** only when `provenance` says the
  tree was origin/main + clean + tool-unmodified AND (`--require-quiescent`) the
  machine was quiescent. Anything else is **EXPLORATORY**: the numbers may be
  read for direction, but **no warm-red on a non-authoritative board may send an
  agent to optimize** (Rule 3 — re-measure quiet first).
- `WARN_COLD_FLOOR` and the *cold* axis tolerate mild load better than the warm
  axis (the tax is dominated by page-in/codesign, not stolen cycles), but warm
  classification is the one #69 protects — treat any non-quiescent warm number as
  `RED_NOISY` regardless of its point value.
- Your OWN benchmark builds do not contaminate (they run serially BEFORE timing);
  a *parallel session's* build does. If a parallel session is compiling, WAIT in
  an until-loop for quiescence, or stamp the board non-authoritative and say so.

## Known NON-AUTHORITATIVE historical runs (do NOT optimize warm from these)

| board | what it is | warm-verdict status |
|-------|-----------|---------------------|
| `bench/scoreboard/cpython_b54dd9b896…json` | the prior 112-cell board (56 native + 56 llvm, compiler tree `2c10e20a5…`) | **NON-AUTHORITATIVE for warm verdicts** — measured under **multi-agent load** (a parallel build was active). Its 30 `FAIL_ENGINE` cells are reclassified below as `RED_NOISY` (no repeat-CI + contaminated) / `TIE` (`warm==1.00`); the TRUE warm-red set comes from the QUIET board. Its *build-fact* columns (binary size, compile-time, build-ok, the #47 healed/not-healed analysis) and the *cold/WARN_COLD_FLOOR* axis remain usable. |
| `bench/scoreboard/cpython_79903045…json` | the older "stale" board | superseded; build-failure baseline for the #47 healed-comparison only. |

## Authoritative QUIET board (origin/main compiler, native / release-fast)

> **First authoritative quiet board (#69).** Native measured on the fresh
> origin/main worktree (`origin_sha 1fa7448a2706`) with `--require-quiescent
> --repeat 5 --classify --emit-cycle-profile` on a QUIESCENT machine
> (`load 4.75 < threshold 9.0`, ncpu 18, runnable 3, **zero competing builds**;
> the quiescence guard certified `quiescent=true`). LLVM measured with `--repeat
> 3` (the slower divergence lane). The tooling commit sits on top of origin (so
> the board is `authoritative=false` by the strict `local_head ≠ origin_sha`
> rule and was run with `--allow-nonauthoritative`), but the **machine was
> certified quiet** and the COMPILER measured is exactly origin/main's —
> `provenance` records `origin_sha` vs `local_head_sha`. Boards:
> `bench/scoreboard/quiet_native.json` + `quiet_llvm.json`.

**NATIVE — 56 cells: 42 `GREEN_STABLE`, 2 `RED_STABLE`, 7 `TIE`, 5 `INFRA`
(4 BUILD_FAILED + 1 CPY_INCOMPAT).** The headline: under quiescence + 5-pass CI,
the **TRUE native warm-red set is just 2 cells** — of the prior board's "11
native FAIL_ENGINE", only 2 survive as `RED_STABLE`; the other 9 were
contamination artifacts (5 are actually `GREEN_STABLE` ~1.9×, 4 are `TIE`).

#### The TRUE native warm reds (`RED_STABLE` — the only real compiler targets)

| benchmark | warm | 95% CI | suspected fact | cycle attribution (Rule 1) |
|-----------|------|--------|----------------|----------------------------|
| `bench_exception_heavy` | **0.68×** | `[0.670, 0.699]` | zero-cost happy-path exception-state + handler-region ownership | LAUNCH/PAGE-IN-bound at this scale (see below) |
| `bench_etl_orders` | **0.81×** | `[0.628, 0.777]` | record/dict value-slot shape + borrow/ownership of stable field flow | LAUNCH/PAGE-IN-bound at this scale (see below) |

> Note the contamination correction: `bench_etl_orders` reads **0.81×** quiet,
> not the contaminated board's **0.60×** — load inflated its redness. It is still
> a real `RED_STABLE`, but the magnitude was a load artifact.

#### Protected native greens (confirmed on the quiet board)

The won classes stay green and strengthen under quiescence: `bench_bytes_find`
**10.88×** `[10.26, 11.22]`, `bench_sum` **10.25×**, `bench_class_hierarchy`
**5.66×** `[5.15, 6.53]`, `bench_struct` **4.81×** `[4.47, 5.95]`,
`bench_matrix_math` 2.94×, plus 37 more — 42 `GREEN_STABLE` total. These do NOT
reopen.

#### etl_orders / exception_heavy cycle attribution (#68 + #76 — NOW ATTRIBUTED)

The prior board reported this as **NEITHER confirmed nor refuted** because a
one-shot sample is ~85–92 % `_dyld_start` (launch + page-in) with `???` in-binary
frames — and correctly named the two fixes needed: (a) loop the hot region inside
one warmed process so page-in amortizes, and (b) a symbolicated build so the
in-binary frames are named. **#76 built exactly that machinery
(`--sample-hot-only`), and the warm hot path is now legible.** Looped
(`--inner-repeat`) + symbolicated (`MOLT_KEEP_SYMBOLS=1`), `_dyld_start` collapses
from ~88 % to **0 %** of leaf self-time; the leaderboards are **100 % in-binary**.
Board: `bench/scoreboard/hot_profile_native.json` (native / release-fast).

**`bench_etl_orders` (warm 0.81× `RED_STABLE`) — HOT, launch 0.0 %, 1577 leaf
samples.** Top in-binary frames (share of the whole steady-state leaderboard):

| share | frame | meaning |
|------:|-------|---------|
| 6.7 % | `ops_string::split_field_bounds_at_index` | the `rows[idx].split("|")` field parse |
| 5.5 % | `bench_etl_orders__molt_user_main` | the compiled Python `main` loop body |
| 5.0 % | `core::str::lossy::Utf8Chunks::next` | UTF-8 decode inside the split |
| 4.5 % | `state::runtime_state` | per-op runtime-state access |
| 4.4 % + 2.6 % | `GilGuard::new` + `GilGuard::drop` | per-call GIL acquire/release |
| 4.1 % + 2.7 % | `mi_page_free_list_extend` + `mi_heap_malloc_zero_aligned_at` | allocator (per-row Order + split temps) |
| 1.5 % | `object::ops_slice::dataclass_new_from_value_slice` | **dataclass construction** (`Order(...)`) |
| 1.6 % | `numbers::int_subclass_value_bits_raw` / `to_bigint` | int field arithmetic |

→ **The cost is the per-row `str.split("|")` + UTF-8 decode + dataclass
construction + GIL/alloc churn**, NOT a split-allocation artifact (the falsified
theory is **refuted** — `dataclass_new_from_value_slice` and the split/decode
path are what burn cycles, not a duplicate allocation). The #68
*dataclass-field-read / dict-value-slot* hypothesis is **partially confirmed**:
dataclass construction is a named hot frame, but the dominant single cost is the
field-PARSE (`split` + UTF-8 decode), and the GIL/runtime-state/allocator per-op
overhead is collectively larger than any single attribute access. The dict
`totals.get/[]=` did **not** surface in the top frames — the dict update is cheap
relative to per-row parsing and object construction.

**`bench_exception_heavy` (warm 0.68× `RED_STABLE`) — HOT, launch 0.0 %, 2526 leaf
samples.** Top in-binary frames:

| share | frame | meaning |
|------:|-------|---------|
| 21.5 % | `bench_exception_heavy__molt_user_main` | the compiled try/except loop body |
| 11.9 % + 10.0 % | `molt_inc_ref_obj` + `molt_dec_ref_obj` | **refcount churn on the raised `ValueError(i)` objects** |
| 6.2 % + 4.6 % | `GilGuard::new` + `GilGuard::drop` | per-op GIL |
| 2.9 % | `object::alloc_object` | allocating each exception object |
| 2.8 % | `exceptions::record_exception_with_caller_frame` | exception-state bookkeeping |
| 2.4 % each | `exception_context_set` / `exception_stack_pop` / `exception_stack_push` | exception stack/context machinery |

→ **The happy-path-exception cost is dominated by REFCOUNT churn (~22 %) on the
raised exception objects, then GIL (~11 %), then the exception-state bookkeeping
(push/pop/context/record, ~12 % collectively)** — exactly the
"happy-path-exception-machinery cost" the council expected, with RC as the single
largest lever.

**Which warm red to attack first (from the cycle facts).** **`exception_heavy`** —
it is the more foundational target *and* the data agrees: its hot path is
~22 % refcount + ~11 % GIL + ~12 % exception bookkeeping, all of which are
**shared runtime machinery** (inc/dec_ref, GilGuard, the exception stack) that
every exception-raising program pays. Optimizing the raised-exception-object RC
path (e.g. avoiding the inc/dec round-trip on a transient `ValueError` that is
caught one frame up, or a lighter exception-object representation) attacks the
single largest frame class and generalizes far beyond this benchmark.
etl_orders' top cost (`str.split` + UTF-8 decode + dataclass construction) is
more workload-specific; it is the second target.

**Side finding (a real compiler bug surfaced by #76, not Lane C's to fix):** the
inner-repeat amplified a **per-`main()`-call molt leak** in both benchmarks — each
iteration leaks its working set (a one-shot run hides it). `bench_etl_orders`
leaks ~45 MiB/iter (OOMs at high N); `bench_exception_heavy` grows ~70 MiB/30-iter.
The hot-only profiler bounds N to a safe RSS budget and **refuses with the leak as
the documented reason** when the size run OOMs; the leak itself is a separate
RC-correctness item for the compiler owners (cf. the genleak #46 / iter_next_pair
family).

- The cycle-attribution MECHANISM is verified working end-to-end (inner-repeat
  transform proven semantics-preserving under CPython; `MOLT_KEEP_SYMBOLS=1`
  symbolication proven to keep 7593 named text symbols vs 1 stripped; the
  warmup-then-attach steady-state sampler + the ≥ 40 % launch refusal gate).

### Reclassification of the prior 30 FAIL_ENGINE cells (contaminated → true state)

The prior `cpython_b54dd9b896…` board's 30 `FAIL_ENGINE` cells, re-derived under
quiescence + repeat CI. **Native** (11 prior FAIL_ENGINE) is from the
authoritative quiet board; **LLVM** (19 prior FAIL_ENGINE) is from the
`--repeat 3` quiet LLVM board.

#### Native (the authoritative reclassification)

| benchmark [native] | prior warm | quiet class | quiet warm | quiet 95% CI |
|--------------------|-----------:|-------------|-----------:|--------------|
| `bench_etl_orders` | 0.60× | **RED_STABLE** | 0.81× | `[0.628, 0.777]` |
| `bench_csv_parse_wide` | 0.66× | **TIE** | 1.00× | `[0.963, 1.021]` |
| `bench_exception_heavy` | 0.73× | **RED_STABLE** | 0.68× | `[0.670, 0.699]` |
| `bench_fib` | 1.00× | **GREEN_STABLE** | 1.21× | `[1.051, 1.281]` |
| `bench_dict_ops` | 1.00× | **GREEN_STABLE** | 1.94× | `[1.920, 2.005]` |
| `bench_dict_views` | 1.00× | **GREEN_STABLE** | 1.94× | `[1.886, 2.089]` |
| `bench_list_ops` | 1.00× | **GREEN_STABLE** | 1.94× | `[1.890, 2.013]` |
| `bench_tuple_pack` | 1.00× | **TIE** | 1.03× | `[0.919, 1.414]` |
| `bench_generator_iter` | 1.00× | **TIE** | 1.00× | `[0.973, 1.027]` |
| `bench_memoryview_tobytes` | 1.00× | **GREEN_STABLE** | 1.94× | `[1.225, 2.250]` |
| `bench_startup` | 1.00× | **TIE** | 1.94× | `[0.738, 2.013]` |

**Native verdict: 11 prior FAIL_ENGINE → 2 RED_STABLE + 4 TIE + 5 GREEN_STABLE.**
`bench_csv_parse_wide` is **confirmed a TIE** (the council's suspicion — its
"0.66 red" was contamination). Only `bench_etl_orders` and
`bench_exception_heavy` survive as real warm reds. The 5 GREEN_STABLE cells
(`bench_fib`, `bench_dict_ops`, `bench_dict_views`, `bench_list_ops`,
`bench_memoryview_tobytes`) read `warm==1.00` under load but are ~1.9× wins on a
quiet machine — a textbook demonstration of Rule 3 (a red that vanishes under
quiescence is a measurement-system bug, not a compiler target). The 4 TIEs are
`bench_csv_parse_wide`, `bench_tuple_pack`, `bench_generator_iter`, and
`bench_startup` (whose wide CI `[0.738, 2.013]` — cold-path noise — keeps it a
TIE despite a 1.94× point estimate, not a GREEN).

#### LLVM (EXPLORATORY — measured non-quiescent)

> ⚠️ The LLVM quiet board (`quiet_llvm.json`, `--repeat 3`) was measured while
> two **idle** `molt-backend` daemons (one this session's, one a parallel
> session's `wt_opsem`) were running. Both were at **0.0 % CPU** and host load
> was **2.83** (genuinely quiet by load), but the quiescence guard is fail-closed
> on *any* `molt-backend` process, so it correctly stamped `quiescent=false`.
> Consequence: **no LLVM cell can be `RED_STABLE`** (that state requires a
> certified-quiet machine) — every warm-red is `RED_NOISY` pending a
> fully-daemon-free re-run. This is the guard doing its job, not a tool defect.

| benchmark [llvm] | prior warm | exploratory class | quiet warm | 95% CI |
|------------------|-----------:|-------------------|-----------:|--------|
| `bench_fib` | 0.30× | **RED_NOISY** | 0.35× | `[0.253, 0.394]` |
| `bench_exception_heavy` | 0.47× | **RED_NOISY** | 0.43× | `[0.416, 0.447]` |
| `bench_tuple_pack` | 0.52× | **RED_NOISY** | 0.52× | `[0.508, 0.531]` |
| `bench_csv_parse_wide` | 0.64× | **RED_NOISY** | 0.61× | `[0.585, 0.631]` |
| `bench_tuple_index` | 0.65× | **RED_NOISY** | 0.65× | `[0.621, 0.694]` |
| `bench_csv_parse` | 0.68× | **RED_NOISY** | 0.67× | `[0.674, 0.674]` |
| `bench_str_count` | 0.68× | **RED_NOISY** | 0.68× | `[0.670, 0.712]` |
| `bench_str_endswith` | 0.67× | **RED_NOISY** | 0.67× | `[0.656, 0.692]` |
| `bench_str_find` | 0.67× | **RED_NOISY** | 0.70× | `[0.670, 0.712]` |
| `bench_str_startswith` | 0.65× | **RED_NOISY** | 0.70× | `[0.611, 0.738]` |
| `bench_descriptor_property` | 0.67× | **RED_NOISY** | 0.70× | `[0.632, 0.735]` |
| `bench_dict_comprehension` | 0.66× | **RED_NOISY** | 0.75× | `[0.719, 0.789]` |
| `bench_attr_access` | 0.97× | **TIE** | 1.03× | `[0.964, 1.057]` |
| `bench_str_replace` | 0.97× | **TIE** | 1.00× | `[0.927, 1.118]` |
| `bench_try_except` | 1.00× | **TIE** | 1.03× | `[0.918, 1.103]` |
| `bench_generator_iter` | 1.00× | **TIE** | 1.03× | `[0.993, 1.097]` |
| `bench_str_find_unicode_warm` | 1.00× | **TIE** | 1.88× | `[0.422, 2.838]` |
| `bench_str_count_unicode` | 1.00× | **TIE** | 1.03× | `[0.782, 1.528]` |
| `bench_str_count_unicode_warm` | 1.00× | **TIE** | 0.98× | `[0.633, 1.753]` |

**LLVM verdict: 19 prior FAIL_ENGINE → 12 RED_NOISY + 7 TIE (0 RED_STABLE,
non-quiescent).** The 12 RED_NOISY reds are *consistent* between the contaminated
board and the exploratory-quiet board (fib 0.35×, exception_heavy 0.43×, the
str/tuple/csv family 0.5–0.75×) — they are real LLVM warm gaps (LLVM is the
known-weaker divergence lane: a native win does NOT excuse an LLVM red), but they
need a daemon-free quiescent re-run to be promoted from RED_NOISY to RED_STABLE.
The 7 prior `warm==1.00` ties resolve to TIE (straddling CIs).

> Whole-board LLVM classification (56 cells): **22 GREEN_STABLE, 12 RED_NOISY,
> 9 TIE, 13 INFRA** (10 BUILD_FAILED + 2 RUN_ERROR-class + 1 CPY_INCOMPAT). The
> GREEN_STABLE count reflects the contamination-asymmetry rule: a warm WIN under
> load is a *conservative* green (load can only make molt look slower, never
> faster), so `bench_sum` 10.56×, `bench_class_hierarchy` 3.94×, etc. are GREEN
> even on the non-quiescent board.

### Prior board (CONTAMINATED) human summary — build facts + cold axis only

> The numbers below are from the **non-authoritative** `cpython_b54dd9b896…`
> board. Per #69, treat the **warm** column as EXPLORATORY (reclassified above);
> the build-fact and cold-axis observations stand.

**112 cells: 1 GREEN, 30 FAIL_ENGINE, 0 FAIL_COLD_BUDGET, 56 WARN_COLD_FLOOR,
6 UNSTABLE, 9 BUILD_FAILED, 7 RUN_ERROR, 3 CPY_INCOMPATIBLE.** Per backend:

| backend | cells | GREEN | FAIL_ENGINE | WARN_COLD_FLOOR | UNSTABLE | BUILD_FAIL | RUN_ERROR | CPY-INCOMPAT |
|---------|-------|-------|-------------|-----------------|----------|------------|-----------|--------------|
| native (Cranelift) | 56 | **1** | 11 | 36 | 3 | 4 | 0 | 1 |
| llvm (inkwell)     | 56 | 0 | 19 | 20 | 3 | 5 | 7 | 2 |

The **2-D split is the headline**: the old "80 RED" collapses into **30
FAIL_ENGINE** (warm ≤ CPython — the real release blockers) + **56
WARN_COLD_FLOOR** (warm > CPython, cold loses only to the fixed startup tax,
within budget — NOT a hard red). FAIL_COLD_BUDGET = 0 (no cell exceeds the v0
budget; see `cold_start_budget.json`).

### PROTECTED GREENS (do NOT reopen — confirmed won classes)

These are the **protected greens**: warm-decisive wins on the engine axis that
must not be reopened by any optimization arc. They stay green across the
contaminated and the quiet boards.

| benchmark | backend | warm | note |
|-----------|---------|------|------|
| `bench_class_hierarchy` | native | **8.00×** | method dispatch / class identity — both phases beat CPython. Was spuriously UNSTABLE on the raw board (a single CPython GC outlier `[.417 .427 .424 .415 .637]` cv 0.23 vs molt's rock-stable `[.054…]` cv 0.03); the **robust-stability rule** (trim one CPython outlier each side, check the verdict holds) rescues it. |
| `bench_struct` | native / llvm | **4.78× / 2.53×** | struct field layout / unboxed lane — won on both backends. |
| `bench_bytes_find` | native | **9.85×** | bytes search / borrowed-view — the largest native warm margin. |

Other warm-decisive wins that are WARN_COLD_FLOOR (warm-green, cold-tax only —
NOT reopened): `bench_str_*` family, `bench_sum_list` (2.94×), `bench_set_ops`
(2.06×), `bench_max_list` (2.00×). These are won classes on the engine axis.

### NATIVE warm reds — CONTAMINATED / EXPLORATORY (reclassified above)

> ⚠️ The table below is from the **non-authoritative** contaminated board (#69):
> these warm numbers were measured under multi-agent load and are **EXPLORATORY**.
> The TRUE warm-red set is the `RED_STABLE` rows in the *Reclassification* table
> from the quiet `--repeat 5` board. Do NOT open an optimization from this table
> alone — confirm the cell is `RED_STABLE` on the quiet board first (Rule 3).

The genuine warm reds (warm < 1.0) the contaminated board flagged (council's
named set), shown with their suspected missing IR fact for triage:

| benchmark | warm | cold | pypy | suspected missing IR fact |
|-----------|------|------|------|---------------------------|
| `bench_etl_orders` | **0.60×** | 0.12× | 0.60× | record/dict value-slot shape + borrow/ownership of stable field flow |
| `bench_csv_parse_wide` | **0.66×** | 0.14× | 0.68× | substring/slice repr (alloc-free field extraction) |
| `bench_exception_heavy` | **0.73×** | 0.33× | 0.15× | zero-cost happy-path exception-state + handler-region ownership |

The other 8 native FAIL_ENGINE are **warm == 1.00 ties** (a statistical tie with
CPython at steady state, caught by the `warm_speedup ≤ 1.00` rule per council
ruling A): `bench_fib`, `bench_dict_ops`, `bench_dict_views`, `bench_list_ops`,
`bench_tuple_pack`, `bench_generator_iter`, `bench_memoryview_tobytes`,
`bench_startup`. These are borderline — molt neither beats nor loses to CPython
warm; they want the same facts (Repr precision, hash-slot Repr, frame ownership)
to cross decisively above 1.0.

### LLVM — the divergence lane (materially weaker than native)

**19 FAIL_ENGINE (vs 11 native), 5 BUILD_FAILED, 7 RUN_ERROR (vs 0 native).**
LLVM codegen is genuinely slower at steady state on the str/tuple/csv/dict family
(`bench_fib` llvm **0.30×** vs native 1.00×, `bench_tuple_pack` 0.52×,
`bench_exception_heavy` 0.47×, the whole `bench_str_*` family 0.65–0.68×) — a
native win does NOT excuse the LLVM red. The remaining 7 LLVM RUN_ERRORs are a
bytes/bytearray/memoryview LLVM runtime-codegen gap (`bench_bytes_*`,
`bench_bytearray_*`, `bench_memoryview_tobytes`, `bench_gc_pressure`). Binary
sizes are smaller on LLVM (~3547 KiB vs ~4256 KiB native) but compile time is
far higher.

### LLVM #47 status — PARTIALLY healed on origin (not fully gone)

Comparing the stale board (`79903045…`) to origin/main, of the **8 LLVM
build-failures** the council expected gone:

| benchmark [llvm] | stale | origin/main | healed? |
|------------------|-------|-------------|---------|
| `bench_generator_iter` | build-failed | **FAIL_ENGINE** (builds+runs) | ✓ |
| `bench_import_time` | build-failed | **UNSTABLE** (builds+runs) | ✓ |
| `bench_json_roundtrip` | build-failed | **WARN_COLD_FLOOR** (builds+runs) | ✓ |
| `bench_etl_orders` | build-failed | BUILD_FAILED | ✗ (llvm-only) |
| `bench_async_await` | build-failed | BUILD_FAILED | ✗ (also fails native) |
| `bench_channel_throughput` | build-failed | BUILD_FAILED | ✗ (CPython-incompat + native) |
| `bench_counter_words` | build-failed | BUILD_FAILED | ✗ (also fails native) |
| `bench_ptr_registry` | build-failed | BUILD_FAILED | ✗ (CPython-incompat + native) |

**3 of 8 healed** (the re-import / closure-ABI class that broke
generator/import/json now builds and runs). The 7 LLVM bytes/bytearray
RUN_ERRORs are **unchanged** from the stale board. So #47 is *reduced, not
gone*; the remaining LLVM issues are a **different class** (bytes/bytearray
runtime codegen + a few cross-backend build-fails), and `bench_etl_orders` is
the one LLVM-ONLY build-fail. `bench_counter_words`/`bench_async_await` fail on
**both** backends, so they are not LLVM-specific.

### PyPy / Codon comparator signal (where molt trails a mature compiler)

`bench_fib`: molt warm 1.00× CPython, but **PyPy 0.51×** (PyPy JIT ~2× molt) and
**Codon 0.26×** (Codon AOT ~3.9× molt) — a recursive-int kernel is the clearest
class where mature JIT/AOT compilers lead; the missing molt fact is call-target
devirt + unboxed-int recursion. `bench_exception_heavy` PyPy 0.15× (PyPy excels
at exception-heavy loops). `bench_memoryview_tobytes` PyPy 3.89× and
`bench_generator_iter` PyPy 2.00× — molt LEADS PyPy there. Codon is scored only
on the equivalence allowlist (fib here); everything else is `non-equivalent`,
never scored.

> The authoritative, per-benchmark detail with exact ratios + the 2-D
> verdict_breakdown is the committed JSON. Regenerate + diff with `--baseline`
> on every perf-relevant landing.

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

## PyPy / Codon comparator lanes (council Lane C — INSTALLED)

Both reference runtimes are now installed on this host and wired as opt-in
lanes (`--pypy`, `--codon`; auto-detect or explicit path). Versions are
recorded in `host.pypy` / `host.codon` and the provenance.

- **PyPy 7.3.17 (Python 3.10.14)** — `/opt/homebrew/bin/pypy3.10` (`brew install
  pypy3.10`). The *dynamic-runtime comparator* (JIT). `pypy_ratio =
  pypy_warm / molt_warm` (> 1.0 = molt faster). PyPy is **measure-and-name, NOT
  a hard gate**: where PyPy wins, the cell carries the molt fact to name (IC
  tiering, class-version guard, borrow inference, generator fusion, trace-like
  loop specialization). PyPy's strength is long-running JIT warmup — a different
  operating point than molt's AOT, so a PyPy win on a hot loop is expected and
  informative, not a contract violation.
- **Codon 0.19.6** — `~/.codon/bin/codon` (exaloop release tarball). The
  *AOT/native north star* (C/C++-class). `codon_ratio = codon_warm / molt_warm`.
  Codon is **equivalence-gated**: it is NOT drop-in CPython (no full object
  model, restricted dynamism), so only benchmarks on
  `CODON_EQUIVALENT_BENCHMARKS` (numeric/loop kernels with no CPython-object-model
  dependence) are scored; every other benchmark is recorded `codon_equivalent:
  false` / `"non-equivalent"` and **never scored win/loss**. A Codon *compile
  failure* on an allowlisted benchmark is likewise recorded, never scored (a
  missing comparison ≠ a molt win). Codon-compiled binaries link
  `libomp`/`libcodonrt` via `@loader_path`; the runner sets `DYLD_LIBRARY_PATH`
  (+ `CODON_LIBRARY`) to `~/.codon/lib/codon` so they run under `safe_run`.

Both lanes use the identical ≥5-sample cold+warm discipline through
`safe_run.py --json` as the CPython path.

### First deltas (native / release-fast, this host)

| benchmark | molt warm | warm vs CPython | pypy_ratio | codon_ratio | note |
|-----------|-----------|-----------------|------------|-------------|------|
| `bench_fib` | (recursive int) | 1.23–1.25× | **0.51×** | **0.30×** | PyPy JIT 2× molt; Codon AOT ~3.3× molt — recursive-int is a class where both mature compilers lead; missing molt fact = call-site devirt + unboxed-int recursion. |
| `bench_class_hierarchy` | (method dispatch) | **9.41×** | 0.67× | — (non-equiv) | molt beats CPython AND PyPy; Codon not scored (object-model). |

> The recursive-int gap to Codon/PyPy on `bench_fib` is the first
> measure-and-name signal: molt is *above CPython* but *below the AOT/JIT
> comparators* — a Lane-B representation diagnosis target (call-target devirt +
> unboxed-int recursion), NOT a CPython-floor red.

### Remaining toolchain arc

- **Backend × Profile boards** (constitution scoreboards 4 + 5): the per-cell
  data already supports slicing native/LLVM/WASM/Luau and
  dev/release-fast/release-output into their own tables — add the report views
  once the LLVM + WASM + Luau lanes and the second profile are all populated.
- **Widen `CODON_EQUIVALENT_BENCHMARKS`** as more numeric/loop kernels are
  verified semantically drop-in (conservative by design — a false "equivalent"
  is worse than a missing comparison).
