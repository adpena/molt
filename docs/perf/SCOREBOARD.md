# CPython Floor-Scoreboard — the release-blocking performance gate

`tools/perf_scoreboard.py` operationalizes the **Performance Constitution**
(`CLAUDE.md`, commit `d0d32caf6`) and the **council two-dimensional gating
ruling** (`project_council_decisions_20260608`, section A). It is the
machine-readable artifact the constitution mandates: a single scoreboard keyed
**benchmark × target × backend × profile** reporting the molt-vs-CPython speedup
split into **repeated fresh-process** and **first-invocation** comparisons,
with a CI-gateable nonzero exit and full **provenance** so a board is never
silently measured against a stale tree.

This tool **measures and surfaces**; it does not fix slow benchmarks. Fixing a
RED benchmark is a separate optimization arc. First attribute the measured
whole-process cost; a timing ratio alone cannot identify a missing compiler fact.

## Two-dimensional gating (council ruling A) — warm ≠ cold

The retired `red` bool blended two structurally different failures. The board
now emits one **verdict** per cell:

| verdict | condition | meaning / lane |
|---------|-----------|----------------|
| `GREEN` | repeated and first invocations beat CPython, delta within budget | observed process wins; release qualification has additional requirements |
| `FAIL_ENGINE` | `warm_speedup = cpython_warm/molt_warm ≤ 1.00` | historical identifier for a repeated fresh-process loss; profile to attribute cost |
| `FAIL_COLD_BUDGET` | `startup_tax_ms > budget_ms` | first-minus-repeat delta exceeds its recorded ceiling; startup cause unproven |
| `WARN_COLD_FLOOR` | `cold_speedup ≤ 1.00` but `warm_speedup > 1.00`, delta within budget | first invocation loses while repeated processes win |
| `FAIL_STALE` | board provenance is non-authoritative: origin mismatch, dirty tree, modified tool, or required quiescence failed | overrides all. Fails the gate unless `--allow-nonauthoritative`. |
| `UNSTABLE` | CV above threshold | untrustworthy in either direction — gated. |
| `BUILD_FAILED` / `RUN_ERROR` | molt build failed / CPython ran but molt did not | gated. |
| `RUN_BLOCKED` / `CPY_INCOMPATIBLE` | wasm build/link-only / no CPython floor | not gated. |

Molt-only intrinsic workloads have fixed semantic outputs. The runner checks
each guest observation and records `CPY_INCOMPATIBLE` only after successful
execution. CPython failures on comparable workloads remain `RUN_ERROR`.

Where:
- `warm_speedup = cpython_warm / molt_warm` compares repeated fresh-process
  medians after discarded launches. Every sample includes process startup.
  Warmup and measured runs execute in Molt/CPython pairs and alternate which
  runtime starts each pair, reducing sensitivity to drift during a cell.
- `cold_speedup = cpython_cold / molt_cold` compares each runtime's first
  invocation. The harness does not evict or certify cold filesystem caches.
- `startup_tax_ms = (molt_cold_total − molt_warm_total) × 1000` retains its
  historical wire name for an **observational first-minus-repeat delta**.
  Both phases include startup; their difference also includes cache effects,
  scheduler variation and noise. It does not isolate startup or body execution.
  The historical budget continues to gate this same arithmetic.

Schema v8 records this scope in `methodology.process_timing_scope`. Historical
boards retain their recorded schema, fields, classifications and methodology;
summary regeneration cannot add missing scope evidence or qualify old timings.

The release gate also requires the canonical selection, complete attempted and
qualified coverage, output parity, repeated comparison evidence, valid identity
and authority. A green cell or a warning within budget does not establish that
the selected suite completed. `--no-gate` is an exploratory opt-out.

The cold-start budget lives in `bench/scoreboard/cold_start_budget.json`, keyed
`<backend>/<profile>` → `budget_ms`. **v0 = the measured baseline** per the
council ladder ("v0 = current measured baseline; near-term = no regression from
baseline; release-output native Y1 = startup_tax < 100ms"). A missing budget
means `FAIL_COLD_BUDGET` cannot fire (we never invent a budget); the measured
delta is still recorded so the ceiling can be seeded. Existing numeric budgets
remain ceilings on the recorded delta; they are not newly calibrated isolated
startup limits.

## Regressions on main: the instruction signal

The five-board projection (`perf_board.py`) and the board-vs-history gate
(`perf_history.py`, `perf_regression.py`, `check_perf_plane_gate.py`) are
retired: the weekly CI board never finished inside its runner budget and the
history gate never armed (HF-222). "Any previously-green benchmark that
regressed" is answered on main by the instruction signal,
`tools/perf_instructions.py`: a farm run compares Molt's instructions retired
with the nearest measured ancestor and names the suspect landings
(`tools/PERF_AUTHORITY.md`, "Instruction signal on main"). This scoreboard stays
the manual, quiet-host release measurement of the absolute CPython floor.

## Provenance — the anti-stale-lore enforcement (council ruling A + B)

The top-level `provenance` block records source revision, methodology, and
selected or observed tool/artifact identities. These observations do not prove
which compiler/runtime bytes were loaded or provide authenticated release
custody; the existing used-byte and release-admission holds remain in force.
[The methodology contract and lock](../../config/perf_methodology.toml) own the
current version. Historical records retain the version they measured:


```jsonc
"provenance": {
  "origin_sha":              "<origin/main tip>",
  "local_head_sha":          "<HEAD the board was measured at>",
  "merge_base_sha":          "<merge-base(HEAD, origin/main)>",
  "dirty_tree":              false,
  "diverges_from_origin":    false,
  "methodology_schema":      "molt-perf-methodology-v1",
  "methodology_version":     3,                // illustrative historical value; use this run's recorded lock
  "methodology_sha256":      "<digest of the methodology files>",
  "methodology_locked":      true,             // content matches its version's lock
  "backend_binary_identity": { "native/release-fast": "<digest of observed selected compiler bytes>" },
  "runtime_identity":        { "native/release-fast": "<digest of the linked runtime library>" },
  "authoritative":           true,
  "authoritative_reason":    "revision is on origin/main, clean, methodology locked"
}
```

`authoritative` is **false** whenever the local HEAD is not on origin/main (it
must be origin/main or one of its ancestors; main may move on during a run),
OR the tree is dirty, OR the scoreboard tool itself is modified-vs-HEAD, OR
`--require-quiescent` cannot certify the machine as quiet. A non-authoritative
board PRINTS *"WARNING: scoreboard provenance is non-authoritative; benchmark is
exploratory unless explicitly requested"* and stamps every cell `FAIL_STALE`.
`backend_binary_identity` is `molt.backend_binary_identity`'s digest of the
compiler bytes, the same identity the stdlib/TIR caches salt with, so a reader
can detect the stale-cache confound class directly. Each measured build replaces
the pre-build probe with the compiler it observed, and records the runtime
library it linked as `runtime_identity` (`null` for a lane that links none).

Tooling changes can alter selection, compilation, timing or classification.
`--allow-nonauthoritative` retains exploratory observations at their recorded
source; it does not prove equivalence to origin/main or qualify performance.

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

### Measurement classification (`--classify`)

The single process ratio cannot establish measurement quality or locate a
compiler bottleneck. With `--classify`, each cell gets a classification. Its inputs are
**machine QUIESCENCE** and the **repeat-pass CONFIDENCE INTERVAL** (`--repeat
N`): a warm-red is a real target only if it is stable AND quiescent AND its CI
sits clear below 1.00.

These labels describe recorded cell classification. Optimization claims also
require admitted source, output parity and complete measurement evidence; the
signal projection checks those facts before recommending work.

| classification | condition | meaning |
|----------------|-----------|---------|
| `RED_STABLE` | `warm < 1.00`, repeat CI entirely below 1.00, machine quiescent, cell robustly stable | confirmed repeated fresh-process loss; profile to attribute cost |
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

**Quiescence applies in both directions.** Competing work can delay CPython,
Molt, or both members of an interleaved pair. Stable ratios and narrow repeat
intervals do not remove that bias. A non-quiescent apparent win is `RED_NOISY`
(green unconfirmed), as is a non-quiescent loss. The historical LLVM board's
22 non-quiescent `GREEN_STABLE` classifications remain recorded historical
outputs; the current classifier no longer issues that claim.

### The quiescence guard (`--require-quiescent`) — checks + thresholds

BEFORE any number is taken, `gather_quiescence()` detects contamination. The run
still produces EXPLORATORY numbers on a noisy machine, but the board is stamped
`authoritative=false` and stdout prints *"NON-AUTHORITATIVE: machine not quiet;
do not optimize from this red list"* naming WHICH check failed:

| check | signal | threshold / rule |
|-------|--------|------------------|
| active build processes | POSIX: `pgrep -fl 'cargo\|rustc\|molt-backend\|molt build'` as broad discovery, then the shared image/argv classifier. Windows: headless `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden` + `Get-CimInstance Win32_Process`, then the same classifier. | **any real build worker** (other than this tool's own tree) ⇒ not quiet. Proof-log observers such as `tail`/`grep` are not counted merely because their command line contains a cargo/rustc log path. **Claude/Codex host-control processes are excluded by name** — never counted, never killed (project policy). |
| 1-min load average | Python `os.getloadavg()` + `os.cpu_count()` (macOS `sysctl` fallback). Windows uses headless CIM `Win32_Processor.LoadPercentage`, normalized onto the same load scale as `cpu_percent / 100 * ncpu`. | `load > ncpu × 0.5` ⇒ not quiet (18-core host ⇒ load > 9.0). Permissive enough that a quiet desktop still measures; an active build always trips this AND the process check. |
| runnable-thread storm | runnable thread count | `> max(2, ncpu × 0.5)` ⇒ contended (catches a build storm before the 1-min EWMA does). |
| thermal / frequency throttle | `pmset -g therm` (best-effort) | throttle active ⇒ not quiet. Skipped if the probe is unavailable (never invents a result). |
| probe failure | process/load probe unavailable (`pgrep`, CIM, or the platform fallback) | **fail-closed**: if we cannot probe, we cannot certify quiet ⇒ not quiet. |
| stale tree / dirty / tool-modified | git + tool blob | folded into `provenance.authoritative` (the existing stale-lore guard). |

`--print-provenance` dumps the full block including the new fields
`active_molt_processes`, `active_cargo_or_rustc_processes`, `loadavg_1m`,
`ncpu`, `runnable_signal` alongside the origin/candidate SHAs,
`backend_binary_identity`, and `runtime_identity`.

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

One-shot profiles can be dominated by loader, import or initialization work.
The shares in the [retained hot-only capture](../../bench/scoreboard/hot_profile_native.json)
describe that source, invocation and sampling window, not a universal startup
fraction or current optimization order.

`--sample-hot-only` uses `--inner-repeat N` to wrap an eligible `main()` inside
one process through `tools/perf_inner_repeat.py`, and `MOLT_KEEP_SYMBOLS=1` to
retain symbols for attribution. Ineligible AST shapes are refused. The repeated
variant has its own source/invocation identity: AST eligibility alone does not
prove equivalent output, state or allocation across repeated calls. Qualify the
variant's observations independently; it does not replace the original program's
semantic or performance evidence.

Symbol retention helps resolve sampled frames. Do not assume the resulting
artifact bytes, layout or timing match a shipping stripped product; acceptance
uses the appropriate uninstrumented product and declared measurement protocol.

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
runtime is too short to carve a steady window (→ raise N); or the sizing run exceeds its RSS budget. An OOM is a retained resource
failure, not proof of a per-iteration leak. Distinguish peak live state, retained
inputs and leaking allocations with matched evidence; lowering N can permit a
bounded profile but does not repair the cause.

**The `inner_loops` provenance field.** Every hot-only profile records
`inner_loops` (the wrap factor N), `symbolicated` + `symbolicate_mechanism`
(`MOLT_KEEP_SYMBOLS=1`), `launch_refusal_fraction` (0.40), and per-cell
`launch_breakdown` (`{total, launch_samples, launch_fraction, in_binary_*,
launch_dominates}`) + `in_binary_top` (the named cycle facts with their
`leaderboard_pct`). These are observations for matched attribution, not a causal diagnosis.

Use qualified snapshot signals and the [V1 ledger](../agent/V1_HANDOFF_FINDINGS.md)
to choose current work. Historical profile interpretations remain linked under
[retained evidence](#retained-historical-evidence); they do not prescribe a
current optimization target.

## What it reuses (no new timing loop)

- **Build**: `tools/bench.py` — the canonical molt-vs-CPython harness. It owns
  the daemon batch-build server (`_BenchBatchBuildServer`), the
  `harness_memory_guard` (RSS-cap + wall-clock guard on every build), and the
  binary-size + compile-time capture (`prepare_molt_binary`).
- **Suite**: `tools/bench_suites.py` — `BENCHMARKS` (the curated 56-benchmark
  "core" verified subset), `SMOKE_BENCHMARKS`, and the per-benchmark molt build
  args (`MOLT_ARGS_BY_BENCH`, e.g. `--type-hints trust`).
- **Run / time**: `tools/safe_run.py` — the scoreboard calls its shared typed guard API
  directly for Molt and CPython. `elapsed_s` is direct-child launch-to-exit
  time; guard cleanup has its own interval. Peak process-tree RSS and complete
  descendant closure come from the same guard result. Guest text is never
  parsed as measurement metadata. The CLI remains a diagnostic adapter.

The scoreboard is the *new* artifact: none of the existing tools emitted the
constitution's absolute-vs-CPython-floor board. The instruction signal
(`tools/perf_instructions.py`) detects regression-vs-self on main; this tool
measures absolute-vs-CPython-floor and gates on it.

## How to run

```bash
export MOLT_SESSION_ID=perfscore
eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"

# Self-test: 1 benchmark × 1 backend, proves the pipeline + schema.
uv run python3 tools/perf_scoreboard.py --self-test

# Full baseline run (the gate): core suite × native+llvm × release-fast.
uv run python3 tools/perf_scoreboard.py \
    --set core --backend native --backend llvm --profile release-fast \
    --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent

# Diff a fresh run against the last stored scoreboard (newly-red / regressed).
uv run python3 tools/perf_scoreboard.py --set core --backend native --baseline

# Measure-only (do not fail CI on RED): add --no-gate.

# WARM-HOT cycle attribution (#76): looped + symbolicated sample of the warm
# hot path (NOT a speedup gate). One backend; writes hot_profile_<backend>_<rev>.json.
# Exits 1 if ANY benchmark is REFUSED (launch still dominated, or not loopable).
uv run python3 tools/perf_scoreboard.py --sample-hot-only \
    --backend native --inner-repeat 40 \
    --benchmark tests/benchmarks/bench_exception_heavy.py \
    --benchmark tests/benchmarks/bench_etl_orders.py
```

The `--sample-hot-only` path (#76) is documented in full under
[Warm-hot cycle attribution](#warm-hot-cycle-attribution---sample-hot-only-76-the-warm-prerequisite).
`--inner-repeat N` sets the in-process repeat factor (default 40 — a multi-second
process for the curated benchmarks within a sane RSS budget); raise it if the tool
REFUSES with launch-still-dominates; lower it when repeated live state or
allocation exceeds the RSS cap, then investigate the cause. `--profile-build` (implied by
`--sample-hot-only`) builds with `MOLT_KEEP_SYMBOLS=1` so molt user-fn symbols are
retained. Treat that as a diagnostic build variant; do not assume its bytes or
timing equal an ordinary build.

- The **CPython oracle is a probed host-native CPython 3.12+ interpreter**,
  resolved explicitly via `--cpython` or by OS-aware candidate discovery on
  Windows, macOS, and Linux. The resolver rejects non-CPython, wrong-OS,
  wrong-arch, wrong-pointer-width, too-old, broken launcher, and project
  `.venv`/session interpreters before any benchmark cell runs. The accepted
  executable, OS, normalized architecture, and pointer width are recorded in
  `host.cpython_oracle`.
- `MOLT_SESSION_ID=perfscore` isolates the build cache (the constitution's
  concurrent-dev contract): the DX authority places its `CARGO_TARGET_DIR` at
  `<artifact root>/target/sessions/perfscore`, outside the checkout.
- The LLVM lane builds with `--backend llvm` and resolves the required
  `LLVM_SYS_<ver>_PREFIX` from the same `molt.llvm_toolchain` authority used by
  CI and setup diagnostics. For the current `llvm22-1` inkwell pin, that is
  `LLVM_SYS_221_PREFIX` and the Homebrew fallback is `/opt/homebrew/opt/llvm@22`.
  Its first build recompiles the backend with the `llvm` feature (~5 min).

### Exit code (the gate)

Release exit is nonzero unless the selected canonical matrix is complete and
eligible under the release authority. The separate per-cell verdicts retain the
historical two-dimensional arithmetic. Historical `RUN_BLOCKED` and `CPY_INCOMPATIBLE`
remain readable report vocabulary. A failed CPython sample is `RUN_ERROR`,
including missing guard metrics, and cannot create a compatibility exclusion.
`--no-gate` skips release qualification; input, execution and publication errors
can still fail the command.

```bash
# Add the PyPy + Codon comparator lanes (auto-detect, or pass an explicit path).
uv run python3 tools/perf_scoreboard.py --set core --backend native \
    --pypy --codon

# Local debugging on a divergent tree (e.g. an in-flight scoreboard-tool commit):
#   classifies real numbers, board stays authoritative=false, gate won't FAIL_STALE.
uv run python3 tools/perf_scoreboard.py --set core --allow-nonauthoritative

# Make cold-floor warnings hard-fail too:
uv run python3 tools/perf_scoreboard.py --set core --strict-cold
```

### Recoverability

The selected matrix comes from `--plan`. Before the first build, the runner
publishes all selected coordinates to `…<gitrev>.partial.json`; every completed
cell replaces that checkpoint through an atomic temp-rename. An interruption
before the first result therefore retains zero attempts and the full `not_run`
list. Each observed cell retains the captured quiescence before final
classification. Checkpoints are valid current-schema documents with open
source custody and `authoritative=false`.

Unsharded partial observations may be merged only with both `--no-gate` and
`--allow-nonauthoritative`; even a fully observed merged selection retains
`authoritative=false`. This recovers diagnostics, never release admission.
Shard merges retain their stricter existing rule: every declared shard and
member must be present exactly once. An incomplete shard remains an inspectable
checkpoint rather than a mergeable release board.

## Methodology (pyperf discipline)

- **≥ 5 measured samples** per (benchmark, runtime) warm phase (`--samples`),
  preceded by `--warmup` discarded runs (default 2).
- **median + mean + stdev + coefficient of variation** per phase. A phase whose
  CV exceeds `0.20` is flagged **unstable**; an unstable cell cannot be trusted
  GREEN and is gated like a RED.
- **First invocation and repeated fresh processes both captured.** Historical
  `cold_*` fields hold the first invocation; `warm_*` fields hold the repeated
  process median after discarded launches. Cache state is uncontrolled and no
  process-local warmup state survives between launches. The verdict table above
  defines the preserved arithmetic; release coverage is a separate obligation.
- Per-run RSS-cap (`--rss-mb`, default 4096) + wall-clock timeout (`--timeout`,
  default 120s) via the shared typed guard API. Tight RSS poll (0.01s) so short benchmarks
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
measured runnable cell must also carry affirmative schema-v7 `output_parity`
evidence from `tools.compat.comparison`: exact stdout, exact user stderr, exact
exit status, and stable output across the cold observation plus every existing
warmup and timed warm sample. The receipt stores hashes and the first mismatching sample
identity, never raw output. Missing, legacy-boolean, mismatching, or unstable
evidence is `RUN_ERROR`, never a performance verdict. Earlier schema boards remain
historical; `--rebuild-summary` and `--merge` refuse to promote them without a
fresh measurement.
 A
`RED_STABLE` classification must also carry `measured_quiescent=true` plus a
numeric repeat CI clearing below the CPython floor. When a board carries modern
host metadata, `validate_board()` also verifies that `host.cpython_oracle` is
CPython, matches the board OS/architecture/pointer width, matches
`cpython_baseline`, and uses the resolved executable as `cmd[0]` rather than a
launcher wrapper. `tools/perf_scoreboard.py` validates every CPython scoreboard
document before writing it, including checkpoint partials, rebuild-summary
rewrites, merge outputs, and final run artifacts; a schema violation raises
`ScoreboardSchemaError` and leaves the target artifact untouched.
Schema v5 additionally binds WASM execution to a `host.execution_host` observation
and each WASM cell's `execution_host`: `kind: "node"`, the absolute selected
`executable`, observed `version`, and the canonical executable `identity`
(`entrypoint`, `content_filename`, `size`, `sha256`). Native cells carry null.
The CLI admits one typed Node plan per invocation before any benchmark build,
and passes that plan through every WASM cell and its facts through every
checkpoint. Standalone cell measurement uses the same admission owner.

Comparability uses engine kind, version, size and SHA-256. An identical image at
a different install path or basename remains comparable; a changed image at the
same version, or a changed version, does not. Merge reconciles native and WASM
lanes and rejects conflicting engines. Board projections retain exact cell paths
and source schema; history revalidates stored observations instead of trusting
an index's claimed comparability class. Missing legacy facts cannot establish a
comparison, regression baseline or release result. Baseline diff and dimensional
improvement likewise refuse to compare different execution engines.

The plan performs one content hash and one version probe at admission. Metadata
fences surround samples and precede builds/artifact binding (one POSIX `lstat`
per fence; a Windows handle ChangeTime query). Replacement, including observed
replace-and-restore, refuses the cell without selecting another Node. These
checks do not hold the image open through OS execution, do not attest loaded
bytes, and do not close the release receipt gap. Ordinary WASM runner calls keep
the existing resolver/probe path without benchmark hashing or fencing.

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
calls, RC events, exception events, guards, and heap allocations. Schema v2 also
records `before.value_reprs` and `after.value_reprs`, the value-keyed Repr facts
sampled from `representation_facts.rs`, plus a Rust-owned
`translation_validation` verdict. `tools/translation_validator.py
--pass-delta-jsonl` consumes that verdict as the R3a proof-order gate so
surviving `ValueId`s cannot silently lose raw-carrier proof or drift across
scalar families. By default the diagnostic feed is written under
`<lane scratch>/molt-backend/tir/pass_delta.jsonl` (`molt.dx.scratch_root`); set `MOLT_PASS_DELTA_PATH` to direct it
to a specific artifact path.

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
  "schema_version": 8,
  "kind": "cpython_floor_scoreboard",
  "generated_at": "<iso8601 utc>",
  "git_rev": "<full sha>",
  "run_state": "complete",
  "planned_cells": [
    {"benchmark": "<benchmark>", "target": "native", "backend": "native", "profile": "release-fast"}
  ],
  "coverage": {
    "selected": 1, "attempted": 1, "parity_qualified": 1,
    "measured": 1, "release_qualified": 0, "not_run": []
  },
  "provenance": { /* see the Provenance section above */ },
  "host": {
    "execution_host": null, // native-only; WASM carries the admitted Node record
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
                   "process_timing_scope": {
                     "sampling_unit": "fresh_process",
                     "first_sample": "first_invocation",
                     "repeated_sample": "fresh_process_after_discarded_launches",
                     "cache_state": "uncontrolled",
                     "delta": "first_minus_repeated_process_latency"
                   },
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
      "workload": {
        "schema": "molt.benchmark-workload/2",
        "entry": "tests/benchmarks/<name>.py",
        "entry_source": "tests/benchmarks/<name>.py",
        "scope": "local-python-source-closure",
        "sources": [{"path": "tests/benchmarks/<name>.py", "size": 123, "sha256": "<exact sha256>"}],
        "topology_sha256": "<closure topology sha256 or empty>",
        "run_args": [], "molt_args": [],
        "sealed_inputs": null
      },
      // --- build facts ---
      "build_ok": true, "binary_size_kib": 4256.3, "compile_time_s": 3.08,
      // --- run facts ---
      "run_blocked": false, "molt_ok": true, "cpython_ok": true,
      // COLD / WARM ---
      "cold_molt_s": 0.253, "cold_cpython_s": 0.068,
      "warm_molt_s": 0.069, "warm_cpython_s": 0.070,
      // --- TWO-DIMENSIONAL verdict (council ruling A) ---
      "warm_speedup": 1.01,            // repeated fresh-process ratio, startup included
      "cold_speedup": 0.27,            // = cpython_cold / molt_cold
      "startup_tax_ms": 184.0,         // = (cold_molt - warm_molt) * 1000
      "cold_budget_ms": 250.0,         // the budget this cell was gated against
      "verdict": "WARN_COLD_FLOOR",    // GREEN|FAIL_ENGINE|FAIL_COLD_BUDGET|WARN_COLD_FLOOR|…
      "fact_class": "shape_facts",     // attribution hypothesis family
      "suspected_missing_fact": "…",   // hypothesis requiring independent cost evidence
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
      "output_parity": {
        "checked": true, "ok": true, "reference_runtime": "cpython",
        "comparison_law_version": "molt.compat-comparison.v1",
        "mode": "exact", "stderr_mode": "exact", "exit_law": "exact",
        "reason": "match", "detail": "",
        "stdout_match": true, "stderr_match": true, "exit_match": true,
        "reference_stable": true, "molt_stable": true,
        "reference_observation_count": 36, "molt_observation_count": 36,
        "mismatch_observation": null, "mismatch_observation_sha256": null,
        "reference_stdout_sha256": "…", "molt_stdout_sha256": "…",
        "reference_stderr_sha256": "…", "molt_stderr_sha256": "…",
        "reference_returncode": 0, "molt_returncode": 0
      },
      "molt_stats": { … }, "cpython_stats": { … },
      "log_artifact": "bench/scoreboard/logs_<gitrev>/<bench>__<backend>__<profile>.log"
    } } } }
  }
}
```

**Direction is labelled unambiguously**: `speedup = cpython_time / molt_time`.
`> 1.0` ⇒ molt faster for the recorded process scope. `warm_speedup` compares
repeated fresh processes, `cold_speedup` compares first invocations, and the
historically named startup budget bounds their Molt latency difference.

### The hot-only cycle-profile board (`--sample-hot-only`, #76)

A **separate** JSON document (`kind: "hot_only_cycle_profile"`, written to
`bench/scoreboard/hot_profile_<backend>_<gitrev>.json`) — it is a cycle-attribution
artifact, **not** a speedup board, and never feeds the release gate.

```jsonc
{
  "schema_version": 8,
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
      "leak_suspected": false,       // diagnostic flag; OOM alone does not prove a leak
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
- First invocation and first-minus-repeat observations are also sensitive to
  host load and uncontrolled cache state. They do not isolate page-in,
  codesigning or another cause. Treat non-quiescent comparisons as exploratory,
  regardless of their point values.
- Your OWN benchmark builds do not contaminate (they run serially BEFORE timing);
  a *parallel session's* build does. If a parallel session is compiling, WAIT in
  an until-loop for quiescence, or stamp the board non-authoritative and say so.

## Retained historical evidence

These files retain their original observations, schemas and classifications.
They do not establish current-main performance or release acceptance. Current
qualification and remaining obligations belong to [V1-07 and its dependencies
in the V1 ledger](../agent/V1_HANDOFF_FINDINGS.md); the current measurement
contract belongs to [PERF_AUTHORITY](../../tools/PERF_AUTHORITY.md).

| Retained artifact | Original schema and recorded `git_rev` | Recorded limitations |
| --- | --- | --- |
| [Earlier scoreboard](../../bench/scoreboard/cpython_79903045f5013f866358b2dcb8889300b69454dd.json) | v2; `cfc6dd47cd6bb6735ead7dc2a55e8cb3f8806d33` | No `provenance` block; marked a stale, non-authoritative performance snapshot. |
| [Prior native/LLVM scoreboard](../../bench/scoreboard/cpython_b54dd9b896b4fb6d5ad8196902a75b34cb5094ae.json) | v3; `b2216555553a5e2d8cf72689f3f08c1a3b49f4d4` | Dirty, diverged tree; `authoritative=false`; no quiescence certification recorded in provenance. |
| [Native quiet capture](../../bench/scoreboard/quiet_native.json) | v3; `441eb2a4d3ed20b530589264038cbc00e7985fa6` | `quiescent=true`, but dirty, diverged and tool-modified; `authoritative=false`. |
| [LLVM quiet capture](../../bench/scoreboard/quiet_llvm.json) | v3; `f33caead157c287a2bf49910102ad3f7d495cec1` | Dirty, diverged tree; active build processes recorded; `quiescent=false` and `authoritative=false`. |
| [Native hot-only profile](../../bench/scoreboard/hot_profile_native.json) | v3; `74bd61801c1072bc9d080d8011fbbe185c2fb554` | Separate `hot_only_cycle_profile` document; `quiescence.quiet=false`; no release speedup qualification. |

Filenames, `git_rev`, `provenance.local_head_sha` and `provenance.origin_sha`
are distinct recorded identities. A filename or historical narrative cannot
substitute for those fields or prove the compiler/runtime bytes consumed.
Retain the raw samples, failures, build facts and symbol leaderboards in these
files without promoting them to a newer schema or current qualification.
Historical `GREEN_STABLE` or `RED_STABLE` labels do not override their recorded
source, quiescence and methodology limitations. Contention can bias either
comparator; a non-quiescent win has no special exemption.

The hot-only capture records inner repetition, retained symbols, delayed
sampling and a launch-fraction refusal threshold. It is evidence about that
profiled execution, separate from fresh-process timing and memory measurements.
The hot-only protocol above documents its fields. Repeated-call RSS growth or
an OOM remains a failure to
retain and investigate; bounding a profiler's loop count does not repair it.
Historical symbol shares and suspected missing facts are leads for matched
cost evidence, not a current causal diagnosis or a permanent optimization order.

## Selected and observed coverage

Use the existing metadata plan to inspect a selected source's inventory without
acquisition, oracle or Node probes, or compilation:

```sh
uv run --no-project python -B tools/venv_exec.py python tools/perf_scoreboard.py --plan
```

`--set`, `--benchmark`, `--backend`, `--profile` and `--shard` select the same
coordinates used by measurement; `--plan-format json` emits the full plan.
[bench_suites](../../tools/bench_suites.py) owns primary suites, the smoke
subset, application recipes and typed exclusions.
[Build-profile binding](../../tools/perf_scoreboard_build_profiles.py) owns
the scoreboard's requested/observed profile checks; the
[release matrix](../../config/release_acceptance_matrix.toml) declares the
required release lanes. Planning these coordinates does not qualify them.

For a retained current-schema board, read `planned_cells`, `run_state` and
derived `coverage`: selected, attempted, parity-qualified, measured,
release-qualified and `not_run` are distinct. Old boards without those fields
retain their original scope. Do not infer present suite size or completion from
historical totals, and do not regenerate a summary to supply missing evidence.

Molt-only workloads require their declared semantic output rather than a
CPython timing floor. Comparable workload failures remain failures. Linked WASM
deployments execute through the declared manifest and public Node runner; first,
warmup and timed observations all require guest-output parity, with host
diagnostics retained separately. Fresh Node timings include engine startup,
compilation and instantiation; they do not measure persistent JIT state.

## Optional PyPy and Codon comparisons

`--pypy` and `--codon` select optional comparator lanes. Resolve the executable
for the actual run and retain its observed version and provenance; this document
does not certify a host installation. Unavailable engines, compile failures and
unsupported comparisons remain missing comparison evidence.

`pypy_ratio = pypy_warm / molt_warm` and
`codon_ratio = codon_warm / molt_warm`; values above 1 mean Molt is faster for
the recorded scope. These columns provide comparator diagnostics rather than
replacing the CPython floor. Both lanes discard warmup launches and measure
repeated fresh processes through the shared typed measurement API. Their nullable
warm columns include startup and provide no independent first-invocation
comparison; separate launches do not preserve PyPy's JIT state.

Codon comparisons are restricted to `CODON_EQUIVALENT_BENCHMARKS` in the
[scoreboard](../../tools/perf_scoreboard.py). Non-equivalent workloads carry
`codon_equivalent=false` and are never scored as wins or losses; a failed
allowlisted build likewise supplies no win. Admit any equivalence expansion
against the workload's semantics before scoring it. Ratios and attribution
fields suggest questions for profiling; they do not prove a missing compiler
fact or prescribe a current optimization target.

Source admission, comparison and final publication follow the [workload-generation contract](../design/foundation/64_perf_scoreboards_and_harness.md#admitted-benchmark-source-generations). Source changes after the final pre-commit admission boundary do not rewrite historical identity. Missing old workload evidence remains unqualified; summary regeneration never supplies it.

For a declared application the same workload-v2 record uses `scope: sealed-project`.
`sealed_inputs` supplies `seal_sha256`, ordered portable relative `python_roots`
and an `oracle` path; `topology_sha256` equals that seal digest. The existing
package-seal authority binds complete declared package/resource, data,
configuration, provenance and oracle bytes. Comparisons bind the full record,
while actual invocations use its sealed execution paths/environment/cwd.
See the [input and publication contract](../design/foundation/64_perf_scoreboards_and_harness.md#admitted-benchmark-source-generations).
`--set applications` is diagnostic coverage, not an expansion of release
acceptance. Application timing remains unqualified until its actual guest and
reference observations satisfy the full contract.
