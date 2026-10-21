<!-- Foundation blueprint 56. Arc: DEVELOPER EXPERIENCE — build speed + daemon +
concurrent-dev + debugging. Author: portfolio-architect. Date: 2026-06-23.
Design only / executable plan. Time-traveler method: start from the 5-to-100-year
DX end-state, work backward to the structural FACTS that make sub-second edits,
N-agent parallelism, and instant feedback INEVITABLE rather than heroic.
Composes with 08_DX-buildspeed.md (LANDED Phase 1a thin-LTO), dx_baseline.md
(the MEASURED baseline + the "module≠crate / function-is-the-codegen-unit" laws),
21/21a/21b/21d (decomposition program), 51 (10-year roadmap), 52 (autonomous
charter). Every factual claim verified against the tree at branch `main`
(HEAD 2f7c40cf7); verification pointers inlined. No code refactored in this
session; this is the executable plan the lead integrates. -->

# 56 — Developer Experience: Build Speed, Daemon, Concurrent Dev, Debugging

## 0. Why DX is a first-class arc (the load-bearing claim)

Every other arc in the portfolio — the fact-plane (51 §1), the ownership lattice
(48/49/50), the perf frontier (06/03/04/05), tinygrad/DFlash fidelity, the
compat verticals — is *throttled by the edit→rebuild→observe loop*. The 10-year
contract (51 §0) is "retire one CLASS of slowness per month." That cadence is a
function of how many *correct experiments per machine-hour* the swarm can run.
DX is the **derivative** on the whole program: a 2× faster, collision-free,
observable loop doubles the rate at which every *other* class gets retired.

This arc therefore obeys the same discipline as a perf arc (51 §1, CLAUDE.md
Performance Constitution): **do not "optimize the build" with peephole tweaks —
fix the REPRESENTATION of the build graph, the coordination state, and the
observability surface so that whole classes of DX failure become UNEXPRESSIBLE.**
The DX compression ladder, stated as the four classes this arc retires:

| # | Class of DX wrongness retired | Made unexpressible by (the FACT/mechanism) |
|---|---|---|
| **DX-1** | "editing file X in crate C rebuilds unrelated code Y" | the **crate-graph cut fact** (21b DAG) + a machine-checked *recompile-blast-radius* ratchet that fails CI when an edit's rebuild set exceeds its declared crate |
| **DX-2** | "two agents collide on a shared build dir / shared source-of-truth / shared daemon and silently corrupt or serialize each other" | the **session-isolation + crate-ownership lattice**: `MOLT_SESSION_ID` target isolation (LANDED) + per-crate ownership records in `coordination.json` + a *collision oracle* that is FAIL-CLOSED on overlapping write-sets, not advisory |
| **DX-3** | "the loop is slow because of stale processes / cold caches / wrong profile / a diagnostic bin nobody needs" | the **canonical build-environment fact**: one `molt dx` authority that resolves profile, target-dir, sccache, daemon, and process hygiene from facts (not per-agent shell ritual), cross-platform (Windows + POSIX), replacing `throughput_env.sh` (POSIX-only) |
| **DX-4** | "I can't see why the compiler did X / which pass changed it / where the time went" | the **unified observability plane**: one `MOLT_DX_DUMP=fn:pass:stage` filter authority threaded through frontend TIR + backend TIR/CLIF + a *pass-delta ledger* (audit board calls this MISSING, STRUCTURAL_AUDIT_BOARD §"TOOLING GAPS") that attributes every Repr/box/RC/dispatch change to the pass that caused it |

The keystone insight from `dx_baseline.md` (§4, §8, MEASURED, binding):
**a module split buys edit-locality; only a CRATE split buys build-cache
isolation; and a function is rustc's atomic codegen unit.** This arc never
re-litigates that — it builds the *machine-checked enforcement* of it (DX-1) so
the decomposition program's wins cannot silently regress.

---

## 1. The END-STATE (state it crisply, then work backward)

**5-year end-state (the inevitable steady state, not an aspiration):**

1. **Sub-second incremental edits for the common case.** Editing a single TIR
   pass, a single `fc/*.rs` opcode handler, a single runtime builtin, or a single
   frontend visitor mixin rebuilds *only* the crate that owns it and relinks the
   daemon — wall-clock dominated by link, not recompile. The 90–170s "edit any
   backend file → full `molt-backend` rebuild" world (dx_baseline §5) is gone,
   replaced by the 21b crate fan-out where a pass edit touches `molt-passes`
   (not `molt-ir`, not the 5 backends) and an `fc/arith.rs` edit touches one
   codegen unit inside `molt-backend-native`.
2. **N agents build the backend in parallel with zero collision.** Each agent
   owns a disjoint *crate* (21b) or a disjoint *file within a crate's module
   tree* (21a/21d); `MOLT_SESSION_ID` isolates the cargo target dir and daemon
   socket (LANDED); the coordination plane refuses overlapping write-sets
   FAIL-CLOSED before the first heavy command; the shared sccache content-cache
   means agent-2's first build hits ~85% on the dependency closure agent-1
   already compiled.
3. **Instant, attributable feedback.** One env-var authority dumps frontend or
   backend IR filtered to `fn:pass:stage`; a pass-delta ledger answers "which
   pass boxed this value / added this RC op / lost this Repr" without bisecting;
   the differential repro flow (`molt debug`) reduces a failing program to a
   minimal byte-identical-divergence case automatically; the build itself streams
   structured progress (it already does — `tools/compile_progress.py`).
4. **No host hazards, ever.** No OOM, no hang, no zombie daemon, no stale lock
   wedges the machine or another agent — the guard/sentinel/custody facilities
   are the *only* execution path and are cross-platform.

**50-to-100-year end-state (the structural invariant that must hold as the system
grows 100×):** the build graph, the coordination state, and the observability
plane are all **generated, checkable FACTS** with CI ratchets — so that as the
codebase 100×'s, "is this edit's blast radius minimal?", "do these two agents
overlap?", and "why did the compiler do X?" remain O(1)-answerable queries
against a fact store, never archaeology. The DX plane scales by *adding facts*,
never by adding ritual. This is the same anti-entropy contract as the semantic
control plane (46) and the autonomous charter's "the verifier is the product"
(52 §0) — applied to the developer loop itself.

---

## 2. Current state (VERIFIED against `main` @ 2f7c40cf7) — what already landed

The stale docs (08, dx_baseline) predate large wins. The authoritative current
state, grep/read-verified this session:

| Surface | Verified state | Evidence |
|---|---|---|
| **`molt-tir` crate** | **EXTRACTED** (21 T1 LANDED). `runtime/molt-tir/Cargo.toml` exists; `tir/` is ~73 files there; `agent_coordination.py:289` routes `runtime/molt-passes/src/tir/type_refine.rs` as its own proof lane. | `runtime/molt-tir/Cargo.toml`; workspace member `Cargo.toml:22` |
| **`function_compiler.rs`** | **28,147 lines** (down from 39,043), `fc/` subtree of 39 handler files; M1.1–M1.16 function-extraction LANDED (arith/compare/calls/loops/indexing/…). | `STRUCTURAL_AUDIT_BOARD.md:26,82`; `21a` §"M1.1-M1.16 now landed" |
| **`cli.py` → `cli/` package** | **DONE through 21d Phase 0+.** `src/molt/cli.py` no longer exists; `src/molt/cli/__init__.py` (~41K lines, the residual engine) + `wasm.py`, `deps.py`, `native_toolchain.py`, `completion.py`, `arg_helpers.py`, `maintenance.py`, `debug_helpers.py`. `cli/__main__.py` present (preserves `-m molt.cli`). | `src/molt/cli/*.py` glob; audit board still flags `src/molt/cli.py` 41641 — **board path is STALE** (the line count moved into `cli/__init__.py`) |
| **Frontend mixin split** | **IN FLIGHT** (21c). `frontend/__init__.py` now 27,940 lines (was 44,620); `frontend/visitors/calls.py` (8,733), `frontend/visitors/classes.py` (3,977), `frontend/lowering/serialization.py` (4,413) extracted. | `STRUCTURAL_AUDIT_BOARD.md:26,32,89,113,115`; git status shows `frontend/visitors/classes.py` + `lowering/serialization.py` modified |
| **Cargo profiles** | **MATURE.** `dev`, `dev-fast` (cgu=256, lto=off), `release-fast` (lto=off), one shipping authority across `release-output`/`release-size`/`wasm-release` (ThinLTO, cgu=16, debug=0, abort), and symbol-bearing `dev-release`. The alternate WASM profile, package-local CGU overrides, combined-build kill switches, and sequential retry lane are deleted. | `Cargo.toml`; `docs/agent/RUNTIME_ARTIFACT_CODEGEN_AUTHORITY_20260727.md` |
| **Session isolation** | **LANDED, opt-in.** Caller-pinned `MOLT_SESSION_ID` / `--session-id` maps to `target/sessions/<component>` for perf, benchmark, test-shard, and proof-isolation lanes. `session_artifact_component` keeps an ID of up to 32 letters, digits, `-` or `_` as it is and gives any other ID a 15-character prefix plus 16 hex digits of its SHA-256, so two sessions never share a target or a backend-daemon label; ordinary generated RunContext custody ids keep Cargo on the persistent selected-root `target` so warm incremental state survives. | `src/molt/dx.py`; `tests/test_dx_run_context.py` |
| **sccache + artifact roots** | Wiring LANDED. `molt dx env` / `RunContext.dx_env` now resolve `SCCACHE_DIR=$MOLT_EXT_ROOT/.sccache` and `SCCACHE_CACHE_SIZE=10G` cross-platform; dev/guarded Cargo builds install the same defaults before enabling `RUSTC_WRAPPER=sccache`. The DX resolver selects checkout-family custody or explicit operator roots independently of drive letters and directory names. This is a developer control-plane rule, not the public compile contract: real users may build in place, use Cargo defaults, or pass target/output flags. Ordinary developer Cargo output is the persistent `$MOLT_EXT_ROOT/target`; caller-pinned isolation lanes use `$MOLT_EXT_ROOT/target/sessions/<session component>`; explicit `CARGO_TARGET_DIR` remains the operator-owned escape hatch. `tools/throughput_env.sh` is a POSIX compatibility wrapper over the Python DX resolver, not the cache-policy authority. | code-verified |
| **DX toolchain check** | `molt dx check` is the cross-platform preflight for developer toolchain facts and reuses the setup-validation authority. It reports the Windows LLVM failure mode where `clang` is installed from `C:\Program Files\LLVM\bin` but `llvm-config.exe` is absent, so `llvm-sys` cannot build even though LLVM itself is on PATH. | code-verified |
| **Coordination facility** | Cross-platform discovery index, not a source lock. `agent_coordination.py` reports live ownership and proof-lane collisions. Differential serialization uses `diff_run.lock` under the [shared build-state authority](../../OPERATIONS.md#build-throughput-multi-agent); receipt or disposable-output placement does not create another lock lane. | `tools/agent_coordination.py`; `docs/ops/MULTI_AGENT_COORDINATION.md`; `src/molt/build_state_layout.py` |
| **Structural audit ratchet** | **LANDED + CI-gated.** `tools/structural_audit.py --check` ratchets `god_files`, `max_god_file_lines`, `duplicate_authorities` (currently 0), `debt_markers_total`. | `STRUCTURAL_AUDIT_BOARD.md:8-19` |
| **Debug/observability tooling** | `src/molt/debug/` package (bisect, diff, ir, perf, reduce, trace, verify, contracts, manifest); `MOLT_TIR_DUMP`/`TIR_DUMP`/`MOLT_DUMP_IR`/`MOLT_DUMP_CLIF*`/`MOLT_VERIFY_ANALYSIS` env vars threaded through the daemon (`main.rs:63-119` `DAEMON_REQUEST_ENV_KEYS`). | read-verified |
| **Process hygiene** | `molt clean --apply --kill-processes`, `tools/process_sentinel.py`, `tools/safe_run.py`, `tools/memory_guard.py`, `src/molt/backend_daemon_custody.py`. | CLAUDE.md; glob-verified |

**What this means for THIS arc:** the *first generation* of DX work (thin LTO,
crate extraction of molt-tir, function-extraction of fc, cli package, session
isolation, the coordination index) is **substantially landed or in flight under
21/08**. This arc's job is NOT to redo it. This arc's job is the **second
generation**: (a) *complete* the 21b crate fan-out that the decomposition program
designed but has not finished (the backend is still one `molt-backend` crate
below `molt-tir`), (b) convert the advisory/ritual parts of the DX loop into
*machine-checked facts with CI ratchets* (blast-radius, collision, build-env),
and (c) build the observability plane (DX-4) the audit board explicitly lists as
MISSING. Each is a structural FACT, not a script.

### 2.1 Live latency evidence: WASM ABI generator and runtime crate proof

The 2026-07-01 Pact recovery exposed two DX facts that should be treated as
regression tests for developer velocity:

- `tools/gen_wasm_abi.py --check` was observed at 52.3s before the generator
  cache move. The structural fix is now in the generator itself: persistent
  render-bundle cache keyed by manifest/generator/runtime inputs, batched
  `rustfmt`, lru-cached manifest scans, and exact-content no-op writes for
  generated files. The current source-rendering no-cache check is 26.8s, and
  the measured hot check is 1.362s with `--check --timings`.
- A one-test `molt-runtime` Cargo proof for the call binder still took 184.5s
  in a session target even though the test body itself ran in 0.00s. That is the
  current live example of why `molt-runtime` remains the highest-impact
  crate-extraction target: runtime call/binder edits should not rebuild the
  whole runtime facade.
- The proof queue must carry latency intent in its notes. A queue row should say
  whether it is using a warmed target/cache, whether the selector is exact, and
  what changed. Otherwise the elapsed time cannot distinguish structural build
  cost from avoidable operator drag.


### 2.2 Repository scanner ownership and bounded work

HF-35 owns repository-check cost. These are development checks; their source
projections, process pools, scan records and regression oracles add no work to
compiled guests.

**Measured cost.** `tools/molt_dev.py integrate` runs the 24 `always` commands of
`tools/proof_plan.toml` serially, one process each. At `bac4c7416` on a Mac arm64
host, warm, the median of three rounds was 62.4 s wall and 99.6 s CPU. Five
per-file scanners took 37.7 s of that wall time: `structural_audit` 13.6 s,
`check_subprocess_guard_coverage` 7.0 s, `check_ratio_direction` 6.8 s,
`encoding_gate` 6.2 s and `check_environment_registry` 4.2 s (19.5 s CPU on eight
workers). `generators.py check` took 16.1 s. Profiles show one root cause in every
scanner: each run re-derives every file's facts, although almost every file is
unchanged. `check_ratio_direction` parses 4,094 files, the subprocess audit 1,947,
the encoding gate 1,287 and the environment scan 3,125 Python and 2,324 Rust files.
Nothing keyed a per-file result by its inputs.

**Scan records.** `tools/scan_records.py` is the one authority for content-keyed
per-file results. A scanner names one pure per-file operation. Its result depends
only on the file's bytes, an identity text (the repository-relative path, or
nothing when the result ignores the path) and the scanner's code. The record key
is SHA-256 over the identity and the bytes. The code generation digests the
interpreter identity, the scanner's own file and every loaded first-party module.
Publication recomputes the generation and publishes nothing when a module was
imported or edited during the run. A record holds JSON facts, never a parse
tree: a few integers and strings per file for most scanners, and the sites and
function facts of each module for the environment scan (7.2 MB for 5,449 files,
parsed in 42 ms). The rejected whole-tree AST memo retained every tree in one
process; records retain facts across processes and keep no tree.

Only command entry points open records. Library callers and tests compute
directly, so a monkeypatched test cannot publish a result for unpatched code.
Each consumer reads a file's bytes once and decodes them exactly as
`Path.read_text` did, so a concurrent write cannot pair one version's key with
another version's result. An environment-scan worker that reads a file itself
returns the key of the bytes it read, with a JSON snapshot taken before the
parent's helper fixpoint mutates function facts. Records unused for seven days
expire. A replayed record is renewed after a day. Each scanner keeps four code
generations under `<Molt cache>/repository-scan-records/`. A narrow run (one
selected file, a fixture checkout) therefore cannot evict the tree's records, and
a fully replayed run writes nothing. Parser `SyntaxWarning`s stay silent, so a
replayed run prints exactly what a computed run prints.

This follows [Ruff's per-file cache](https://docs.astral.sh/ruff/settings/#cache-dir),
[Go's content-addressed build and test cache](https://pkg.go.dev/cmd/go#hdr-Build_and_test_caching),
[Bazel's action cache](https://bazel.build/remote/caching) and
[Cargo's fingerprints](https://doc.rust-lang.org/nightly/nightly-rustc/cargo/core/compiler/fingerprint/index.html).
Two measured choices differ. Ruff keys a file by its metadata; records key by
content. Reading and hashing the 57 MB of tracked Python costs about 0.3 s of CPU
on this host, and a content key is safe to share across worktrees and cannot
miss a same-size rewrite inside one timestamp tick. The stat-keyed
`file-digests` store could later skip reading unchanged files. Go's test cache
traces the files and environment a test reads; records cover only pure per-file
operations, whose inputs are their arguments. Whole-gate tracing would repeat the
soundness gaps (environment reads, stat probes) that rejected the HF-263
test-result cache.

**Result.** Three alternating base/new rounds of all 24 gates at the lane head,
on one host under a competing load average of 14 to 25, every gate passing:

| Gate | Base wall / CPU (s) | Records wall / CPU (s) | Peak RSS (MB) |
|---|---|---|---|
| `check_ratio_direction` | 9.27 / 8.73 | 0.76 / 0.57 | 186 → 47 |
| `check_subprocess_guard_coverage` | 6.12 / 5.89 | 0.55 / 0.53 | 160 → 38 |
| `encoding_gate --check` | 5.50 / 5.30 | 0.27 / 0.26 | 159 → 32 |
| `check_environment_registry --check` | 4.21 / 19.89 | 3.55 / 7.84 | 165 → 153 |
| All 24 gates (sum of medians) | 72.0 / 105.7 | 53.0 / 76.4 | |

The four record consumers fall from 25.1 to 5.1 s wall and from 39.8 to 9.2 s
CPU. Per-round totals were 73.0, 82.0 and 66.0 s for the base and 57.7, 60.4 and
36.4 s with records; the unchanged gates carry the host noise. An earlier
alternating run under a load of 20 to 60 gave 113.3 → 73.7 s wall and
131.2 → 90.0 s CPU. The base gate code run on the records tree, the
cold records run and the warm records run print byte-identical output for all 24
gates (generator timing lines excluded). The environment inventory's 10,511 sites
are byte-identical between cold and warm runs. Against the base code, only the
line numbers of sites inside the two edited gate files move.

The structural patch probe groups calls by their enclosing function in one AST
traversal, then checks each group's source order. A nested function's installer
cannot authorize a patch in its parent: defining a helper does not execute it.
Function, async-function and nested-method controls execute that distinction in
CPython. The old traversal revisited each descendant for every enclosing
function, making work grow with nesting depth. The replacement costs
`O(N + sum(K_f log K_f))` for N nodes and K_f calls per function, retaining only
call references for the current file. The source-order sort preserves the
existing installation-before-patch rule. It keeps no Python tree across files.

The environment-registry scan keeps its existing bounded process pool alive
through strict runtime-writer analysis, instead of ending parallel collection
with a serial lexer tail. One picklable per-file operation retains the same
strict parser and error translation. Explicit first-party module edges are
followed in deduplicated waves before the existing production/test closure runs
in the parent. Workers return compact facts; lexical buffers remain local to a
file. Standalone and one-worker callers use the same operation serially. The
per-module Python and Rust facts are scan records; the writer analysis resolves
module paths on disk, so it is computed on every run. Independent external
module and malformed-source controls require the same diagnostics under serial
and actual process-pool execution.

This follows [Ruff's per-file AST and semantic traversal](https://docs.astral.sh/ruff/contributing/)
and [rustc's combined lint traversal](https://rustc-dev-guide.rust-lang.org/diagnostics/lintstore.html):
share the input analysis while preserving each check's ownership and diagnostic
contract. Parsed-tree scaling controls distinguish traversal cost from parsing,
filesystem access, process startup and the complete gate.

**Second round: one scheduler, shared bytecode and more consumers.** After the
first round, `integrate` still ran the 24 always gates in sequence, so a landing
paid their sum. The always gates now run as one block through the proof
executor (`tools/proof_executor.py`), the scheduler `docs/design/host_capacity.md`
names; `tools/molt_dev_gates.py` owns gate execution. The executor's admission
bounds the block: the plan's `executor_max_workers` and the `repository-policy`
resource class, each capped by `molt.host_capacity` at one GiB per gate (the
largest gate tree peaks at 0.69 GB). Each gate's streams print in declared
order. A gate failure and a moot gate are partition-scoped, so the executor never
cancels a gate declared before the failure, and the first failure in declared
order is the one reported. Later gates are skipped or cancelled through the
guard. Each landing leaves an executor receipt with every gate's duration, peak
RSS and outcome beside the integration lock in the Git common directory. Rule
gates, pytest batches and Cargo gates still run alone and stream live.

`CANONICAL_EXECUTION_ENV` forbids bytecode writes so a gate cannot change the
checkout's import topology, so every gate compiled `tools/`, `src/molt` and the
stdlib from source. Integrate keeps that canonical environment for the proof
queue and gives its gates the DX pycache prefix (`molt.dx.scratch_dir`,
`"pycache"`) with writes allowed. Bytecode stays outside the checkout. CPython
validates each cached module by source mtime and size; an exclusive clean
checkout changes only between gate runs, through Git writes with new mtimes.

`structural_audit` routes its six per-file probes through scan records: debt
markers, Python and Rust stub surfaces, both compatibility inventories (keyed by
the proved flag they read), process-wide test patches, build-failure skips and
scratch-authority sites (keyed by language and test-module role). A release
receipt never replays. `tools/generators.py`, the command that renders every
generator, passes scan records to a generator whose `generated_outputs` declares
them: the coverage audit rows (keyed by file and the configuration flag), each
target runtime's East_Asian_Width facts (keyed by its admitted runtime closure
digest and launch argv) and the stringprep tables (keyed by the Unicode data
version and the stringprep source digest). A first-party module imported after
records open is recorded by checkout path and content digest, and a replaying
run checks the same content in its own checkout first.

Measured on this Mac, alternating three rounds of the round-one tip (serial,
`02b9f5d7e`) against the second-round tip (the integrate block), at a competing
load average of 13 to 20:

| | Wall (s) | CPU (s) | Peak RSS |
|---|---|---|---|
| Round-one tip, serial | 57.5 (50.6 to 74.2) | 80.9 | 0.69 GB (largest process) |
| Second round, executor block | 18.9 (15.6 to 20.4) | 67.8 | 3.1 GB (process tree) |

Single steps, each alternating three rounds: the executor block alone took the
serial 66 s to 33 s wall at unchanged CPU (about 105 s at load 20 to 40); the
bytecode prefix then cut the block's CPU from 96.2 to 88.5 s and its wall from
23.2 to 21.4 s (medians, load 15 to 19). Standalone and warm, `structural_audit`
takes 13.1 s computed and 4.5 s replayed, and `generators.py check` renders in
8.0 s instead of 12.9 s. Base gate code run on the second-round tree, a cold run
and a warm run print byte-identical output for all 24 gates; 898 of 900
structural findings are identical and the other two sit in
`tools/structural_audit.py` itself.

**Remaining cost.** The block's floor is `generators.py check` (about 12 s in
the block, 8 s of rendering): `gen_protocol` imports the whole frontend and runs
ruff twice, and process start, toolchain admission and target-runtime identity
capture take about 4 s. `structural_audit` (7 to 8 s in the block) still
projects every Rust file in a worker pool for the probes that are not records
yet (semantic fallthroughs, lowering gaps, duplicate authorities, kitchen-sink
regions). The executor observes the source with `git status` before each
scheduling wave and after each completion. The plan's ceiling of four commands
keeps the slowest gates waiting behind quick ones; longest-first ordering from
receipt durations is the design's next step.

**Gate replay is a design gap.** Reusing a passing gate's result needs its
complete admitted inputs, command, environment and tools (HF-334). The receipt
records the source tree, the plan authority, the os, architecture and Python
version and the declared toolchain fingerprints. It does not record the gate
environment, and local always gates declare no input paths: their only complete
in-tree identity is the whole source tree. Several always gates also read tools
outside their declared toolchains (`python`, `git`, `generators`):
`check_rust_toolchain` reads rustup state, `build_graph_audit` runs
`cargo metadata` over the Cargo registry, and the ruff gates use the
environment's ruff. A pure scanner's content-level reuse check reads and hashes
every input, which is what its warm scan-record path already costs. A
millisecond reuse therefore needs the plan to declare, per command, input paths
whose Git tree ids key the receipt, a complete toolchain list, and an
environment digest in the receipt identity. Those declarations belong in
`tools/proof_plan.toml`. Pytest commands stay excluded (HF-263).

The live release ledger records accepted scope and outstanding audit-cost work.

### 2.3 Policy projections and execution freshness

The generated proof-plan JSON describes policy: commands, dependencies, resource
budgets, toolchains and declared authority input paths. Its v8 schema omits the
execution-source digest. Source edits that leave the represented policy unchanged
cannot change that projection; editing the represented policy still does. The generator
therefore avoids reading and hashing the authority closure just to render JSON.
No replacement stamp, cache or artifact is introduced.

Execution receipts retain the complete live authority digest, source commit,
source tree, toolchain identity and evidence checks. Receipt production and
verification compute their identities from the canonical authority, independently
of the projection. A regression control mutates an actual declared input file,
requires the old receipt to fail, and requires the policy projection to remain
byte-identical. A policy-change control requires a different projection.

This applies the distinction between a generated command graph and execution
freshness visible in [CMake's compilation database](https://cmake.org/cmake/help/latest/variable/CMAKE_EXPORT_COMPILE_COMMANDS.html)
and [Ninja's build graph and log](https://ninja-build.org/manual.html#ref_log).
Molt keeps its existing execution-attestation contract. Removing the redundant
stamp prevents source-only edits outside explicit metadata inputs from selecting
metadata proofs merely by rewriting the generated JSON. Explicit `ci-wiring`
inputs still select those checks; this is no universal proof-selection exemption.
These are development-only costs and add no work to emitted programs.

---

### 2.4 Declared toolchains before execution capture

The proof plan declares providers; its shared projection owns their environment.
The queue applies declared SDK availability before filtering and capturing the
execution environment. Native LLVM, MLIR and TableGen receive the same verified
SDK, while an external `llvm-config` keeps its distinct executable-search root.
Compiler source and request working directory are separate inputs, including
managed-install attestation for relative state-root selectors. The queue's WASI
role selection preserves explicit external tools without requiring an unrelated
SDK or selecting target linker/libc/CRT flags. The Cargo/runtime plan owns that
C ABI. The generic command guard preserves the admitted
environment and does no Cargo-feature parsing or SDK discovery.

This follows [Bazel's declared toolchain dependencies](https://bazel.build/extending/toolchains)
and respects [Cargo's explicit environment inputs](https://doc.rust-lang.org/cargo/reference/environment-variables.html).
Availability groups declared tools by provider and verifies a selected native
SDK once per projection; the guard does no second SDK discovery. Provisioning, exact tool
capture and build-capacity admission retain their existing owners. This removes
duplicate development-side work and adds no emitted-program work. Actual image
custody and the complete release matrix remain under V1-01/V1-28.

### 2.5 CLI records follow their execution owner

Records constructed and consumed by one CLI stage live in that stage's module.
`models.py` retains records shared across stages; an annotation alone does not
create a runtime import edge. The HF-234 migration moves 35 definitions into
their 19 existing owners, removes the old definitions, and moves consumers to
the owning modules. Class bodies, defaults and field order are preserved.
There is no registry, factory, compatibility export or additional cache.

This removes class construction and transitive imports from commands that do
not use those stages. Work is proportional to the definitions in modules the
command actually loads; a full compiler invocation still constructs the records
its stages need. Type-only backreferences remain under `TYPE_CHECKING`, so the
shared records do not import their consumers at runtime. Worker exceptions keep
one module identity, qualified by a fresh interpreter unpickling both ordinary
and timeout failures without preloading their owner.

Five fresh CPython 3.12 observations on macOS arm64, with bytecode writes
disabled on both sides, put the timing helper's cumulative import median at
132.3 ms before and 2.7 ms after. Shared-model self time falls from 25.4 to
16.9 ms; frontend-worker cumulative import changes only from 407.5 to 392.4 ms.
These are import diagnostics, not whole-build speedups. Ordinary unchanged
builds have their own early-cutoff path. Linux aarch64 native 3.12/dev/full
observations, retaining the same compiler target and output path, take 0.594 s
before and 0.590/0.583 s after, including the command guard. Both binaries run
with an empty execution PATH and print the expected result. Initial cache
admission is excluded; these few observations establish no whole-build speedup.
This is compiler startup work; emitted native and WASM programs acquire no
record machinery or additional runtime work.

### 2.6 Checkout state retention (HF-313)

Under a shared artifact root a checkout keeps regenerable state named by
`molt.dx.checkout_component`: its Cargo target, its uv environments, its
pytest cache and its Hypothesis home. Each Cargo target, and each pinned
session below it, owns a build-control root named by
`molt.build_state_layout.build_state_address`. The names are one-way hashes,
so they cannot tell the disk guard who owns a root or whether anyone still
uses it. On 2026-10-20 such roots of removed worktrees held 136.5 GiB on one
external volume, 12.0 GiB on another and 8.2 GiB on the internal disk, and 613
build-control roots held 9.2 GiB under one artifact root.

Leading tools retire regenerable state by last use, not by ownership guesses:

- Go's build cache sets an entry's mtime when it is used, at most once an hour,
  and a trim runs at most once a day (`trim.txt`) and deletes entries unused
  for 5 days, a limit chosen from measured reuse
  ([cmd/go cache.go](https://go.googlesource.com/go/+/6b974e2bd3197bf6ff2245f5b24c0fccef352903/src/cmd/go/internal/cache/cache.go)).
- Cargo records the last use of every global cache entry in a tracker with
  deferred writes; build commands run an automatic GC once a day that deletes
  data unused for 1 month (3 months for downloads), and `cargo clean gc`
  applies age and size limits on demand. Entries that predate the tracker
  enter it as used now and age out
  ([global_cache_tracker.rs](https://doc.rust-lang.org/nightly/nightly-rustc/src/cargo/workspace/global_cache_tracker.rs.html),
  [cache cleaning](https://blog.rust-lang.org/2023/12/11/cargo-cache-cleaning/)).
- Bazel's disk cache GC applies a maximum size and age while the server idles
  ([diskcache](https://bazel.googlesource.com/bazel/+/refs/heads/master/src/tools/diskcache/README.md)).
- Bazel's output base, keyed like ours by a hash of the workspace path, keeps
  the workspace path in a file inside it. Nix takes per-process temporary-root
  leases only because its collector deletes by reachability, not by age
  ([gc.cc](https://git.lix.systems/lix-project/lix/src/commit/85e3b9b87141669801f8feac18b0e7e308619cce/src/libstore/gc.cc)).

Molt follows Go and Cargo. `molt.checkout_state` owns the layout
(`CHECKOUT_STATE_CONTAINERS`), the projection of one checkout's roots, and two
files inside each root:

- `.molt-state-owner.json`, the owner record, published once (as Bazel does
  for its output base) and only when it binds: the owner is an absolute
  checkout path whose component the root's name carries, or, for build
  control, the Cargo target whose address names the root.
- `.molt-last-use`, a stamp that every use refreshes at most once an hour
  (`record_checkout_state_use`): `RunContext.canonical_env` records common
  checkout state and the selected target and build control; a CLI build process
  records its selected target and build control once. A pinned session records
  both itself and its enclosing checkout target, without refreshing idle sibling
  sessions or their build controls. A root's last use is its newest activity
  mtime (the root and its immediate children, the stamp among them), and a
  checkout target also includes its managed sessions' activity. A use that writes nothing
  still counts, and roots that predate this design age out like any other;
  nothing guesses their owner.

A pinned session's Cargo target, `<target>/sessions/<session>` below a checkout
target or a plain clone's own `target` (`molt.dx.session_scoped_target_dir`),
is a root of the same policy (`is_session_target`). Its place and its session
name identify it; it needs no owner record, because the target that holds it
owns it. The place that selects it stamps it: `RunContext.canonical_env` for a
pinned `MOLT_SESSION_ID`, and a CLI build's target resolution
(`record_checkout_target_use`, which stamps the session and then the target
that holds it). The session stamp adds one `stat` to that once-per-process
path. Measured on the integrator Mac (APFS, 2026-10-21, median of 7 x 20,000
calls with fresh stamps): a bare `os.stat` costs 1.1 us, the session stamp 4.1-4.5 us
with its path handling, and a pinned session's whole target use 14-15 us
against 9.5-10 us for a checkout target.

One policy decides retirement (`checkout_state_decision`); `tools/disk_guard.py`
applies it through `assert_safe_to_delete` and `molt.file_deletion`:

- A root idle past `MOLT_CHECKOUT_STATE_MAX_AGE_DAYS` (default 30, Cargo's
  limit for locally recreatable data) is reclaimed whether its owner exists or
  not; rebuilding it is the accepted cost.
- A pinned session's target idle past `MOLT_SESSION_TARGET_IDLE_HOURS`
  (default 6) is reclaimed. A session pins isolated work (a benchmark, a
  test shard, a differential run), not a checkout's lasting build. The
  6 hours are the limit that the disk guard's lane registry applied; no
  measurement has given a reason to change it.
- A root whose recorded owner is gone, while the owner's parent exists, is
  reclaimed once idle past the protection window (2 hours, above the stamp's
  1-hour resolution). A missing parent may be an unmounted volume. A
  session's build control is orphaned once its session is gone, also when
  the session went with its checkout target.
- No idle limit may undercut the protection window; a smaller declared value
  fails the collection.
- Under disk pressure, roots idle past the window are evicted oldest first
  until the free-space target is met, session targets among them.
- A held lock keeps a root: Cargo's per-profile `.cargo-lock` (Cargo locks
  `<target>/<profile>` and `<target>/<triple>/<profile>`, and a pinned session
  builds below its checkout target, never at the target root, which the
  guard's earlier probe read), build control's build and diff
  locks, and a backend daemon suite's owner lock. A Cargo target is also kept
  by a lock in the build control that it or a session inside it owns, so a
  checkout target never retires under its sessions' builds. An explicit
  `CARGO_TARGET_DIR`, every session target named by the current
  `MOLT_SESSION_ID`, and the running interpreter's own environment keep a
  root too. These are facts about live use; guard markers elsewhere in the
  artifact root are not.
- Build control follows its target: one GC retires the build control of the
  targets it reclaimed, and worktree retirement reclaims the build control of
  the removed checkout's target and sessions with them. A checkout target's
  retirement takes its session targets along.

The disk guard applies the policy at most once a day from
`RunContext.canonical_env` (a stamp at `target/.molt-checkout-state-gc`, so the
common case costs one `stat`), and on demand with `tools/disk_guard.py --gc`.
Worktree retirement (`integrate --cleanup-worktree` and `lane retire`)
retires the worktree's daemons, removes it, and then reclaims its roots at
once, without the idle window: it is the explicit owner action, as `bazel
clean --expunge` is. A failed removal keeps the roots.

Before removing a worktree, the canonical
retirement consumer binds its complete known inventory from
`checkout_state_roots`, including pinned sessions and their build controls.
It uses one captured process environment to bind and reclaim that inventory;
the caller's selected Cargo target, uv environment, build control and session
may belong to another checkout. Ordinary command use remains selection-scoped.
Integration, standalone cleanup, lane retirement and drift pruning share this
retirement preparation and reclaim boundary. Managed pinned sessions below
`target/checkouts/<checkout-component>/sessions/<session-component>` use this
same exclusive retirement protocol and their canonical session-idle limit,
including explicitly selected sessions and sessions that predate registration.
The session-idle clock is the existing `RetentionLimits` authority. Recent child activity and held child locks protect an enclosing
target; candidate selection counts nested byte trees once.

Run capsules obtain their target from `RunContext.canonical_env` or
`development_artifact_env` (`create_dirs=False` when merely preparing a capsule),
then retain that resolved environment for the command. They may pin
`MOLT_SESSION_ID` when isolation is intentional; they do not assemble a target
path themselves. A command using an explicitly selected managed session still
records its owner and last use. Arbitrary caller-selected targets outside the
managed layout retain their caller's lifetime/custody, as do owned temporary
release/differential output roots. The disk guard's existing legacy allow-set
(`target/<configured lane glob>` and top-level `cargo-target-*`) remains explicit:
those targets now take the same last-use stamp and retire under the existing
30-day max-age limit, or under pressure outside the protection window. They
have no checkout owner, so retirement cannot guess which checkout owns them.
Every selected direct `target/<component>` and top-level `cargo-target-*` holds
its exact external lifetime lock independently of the command's glob policy;
a collector with different globs therefore cannot race that use. Structural
lock admission grants no deletion permission: the one disk-guard allow-set and
hard denylist still protect foreign paths and shared output names.
Shared toolchains, compilation caches and retained proof evidence have their
own existing owners and are not checkout-state roots.

Repository command execution carries its source owner explicitly from
`CommandExecutor.repo_root`, already admitted by `import_file`, through
`ProcessStateUse`, guard loading and `HarnessExecutionContext`. Compiler inputs
selected by `MOLT_SOURCE_ROOT` remain unchanged caller input; they cannot redirect
the executing tool's control plane. Direct compiler callers retain the default
compiler-source admission, including installed-source refusal when guard tools
are absent. A loaded foreign guard or dependency is always rejected. One command
resolves the owner once, validates its two source markers once in the loader,
and checks the three loaded guard module identities; it adds no inventory scan
or guard reload. The Windows controller binds this same owner before changing
its physical cwd, while the worker restores the original logical cwd. Checkout
provisioning carries the same optional execution owner in its existing input
specifications and through every guarded provisioner and bounded probe; the
owner is excluded from content/action digests. Standalone provisioning retains
its compiler-source default. Unguarded metadata probes need only the installed
Molt package and never require source-only guard tools.

Managed use-publication and retirement share the same external file-lock
transaction. Each operation admits a finite set of canonical root coordinates,
including managed target ancestry and selected build control. Shared use holds
survive for the actual owned command or persistent worker lifetime; retirement
requires exclusive admission and revalidates path identity, owner and policy
before deletion. Stamp refresh remains hourly and does not replace this lifetime
hold. Independent shared readers coexist; retirement cannot pass a live reader.

The exact structural use envelope also covers caller-selected direct
`target/<component>` and top-level `cargo-target-*`, even outside the current
collector's configured root. This is deliberate: an independently authorized
collector must see the same exact target lock. Shared/protected output names
use the one hard denylist and receive no stamp or lifetime metadata; ordinary
outside-layout paths remain untouched. Admission costs one finite external
lock per selected root/dependency and an hourly stamp check, independent of
artifact bytes or sibling count. Structural use never adopts checkout ownership
or expands a collector's deletion allow-set. For an explicit top-level
`$HOME/cargo-target-x`, the stable interlock directory is `$HOME/target/.disk_guard`;
only its finite metadata is created, never ownership of that target.

POSIX exec transfers these finite reader descriptors to the actual command.
Inheriting descendants conservatively retain state, including a long-lived
daemon that inherits a descriptor. Preserving exec's identity, signals and exit
status accepts that retention; arbitrary daemon independence would require an
additional supervisor. External interlock files deliberately survive payload
retirement: they are global coordination metadata, O(distinct root identities)
small files with no per-operation growth. Unlinking a held lock would split inode
admission between existing waiters and new openers; sweeping them needs a separate
generation-safe protocol and is outside payload retirement.


Child handoff must prove that the intended child has acquired its own hold before
the parent releases its hold. An unproved startup or descendant closure retains
custody through the owning parent's lifetime instead of treating an exception,
process wrapper collection or a stale marker as proof of death. An escaped
descendant after that parent exits has no managed lifetime guarantee. Managed subprocess, guard, daemon, timer, pytest and
queue consumers use that same owner; they do not add a second retention clock.
Finite root admission performs bounded path and lock work per operation, with
no process spawn or compiled-application cost. Raw Cargo or arbitrary external
users outside managed command custody do not acquire this guarantee implicitly.

Detached name admission serializes observation, explicit replacement and startup
under one OS lock outside the replaceable directory. The nonce-bound startup
acknowledgement identifies the actual worker, which may differ from a Windows
interpreter launcher. Missing acknowledgement or unknown process state cannot
authorize replacement. Windows liveness uses the shared query-only process
handle and a zero-time wait, including for a process whose exit code is 259;
signal zero is only a POSIX query. Query and handle-close errors preserve unknown
state. Terminal `rc` publication follows command closure, resource cleanup and
crash-log closure, with no subsequent worker writes into the replaceable state.

This adapts [Go's cache lifetime contract](https://go.dev/src/cmd/go/internal/cache/cache.go),
which keeps a retrieved output available until the cache owner closes, to
Molt's actual managed-resource holds. It also applies SQLite's
[explicit commit-point discipline](https://www.sqlite.org/atomiccommit.html)
to terminal publication. Molt does not need database durability or a new journal
for disposable build state: incomplete startup remains inspectable and refuses
replacement. These mechanisms run in development tooling and add no work to
compiled applications.

This policy replaced an append-only registry of lane targets in
`target/.disk_guard/registry.jsonl`, which retired a registered directory
6 hours after its latest registration and at once on lane completion. A
registration measured nothing about use, and nothing registered a session
target pinned under pytest: on 2026-10-21 none of the 14 session targets under
the integrator Mac's `~/Molt/target/checkouts` was in the registry. Its
completion reclaim duplicated worktree retirement. Hand-named targets under
`target/` (`MOLT_DISK_GUARD_LANE_GLOBS`) and flat `cargo-target-*` directories
carry no stamp, so they are reclaimed only under disk pressure.

### 2.7 Process-lifetime records cross no durability barrier (HF-382)

Every guarded command wrote its custody records durably: 29 `fsync` calls for
one guarded `true`. Ten came from the active guard marker
(`tools/memory_guard_core/active_custody.py`: one exclusive create and four
updates, each a staged-file `fsync` and a parent-directory `fsync`), nineteen
from guard scratch (`molt.temporary_artifacts`: the owner, terminal and index
records and the payload move). Measured on the integrator Mac on 2026-10-21,
with the guard state root on its external USB APFS volume and a load average
near 16: a guarded `true` took 483 ms against 278 ms with `os.fsync` stubbed
out (median of 20, p90 1,138 ms against 335 ms). A staged file's `fsync` cost
about 9 ms there; a directory's cost 0.1 ms.

These records describe processes. A marker names a guard, its child and their
groups; a scratch owner names the guard that leases its payload. A process
crash loses no page-cache write, so a dead producer's last record is complete.
An OS crash or power loss kills every process a record names, so no live
process loses custody when a record is lost. Leading systems keep such state
the same way:

- systemd places runtime state in `/run`, "a tmpfs file system ... flushed on
  boot" ([UAPI.9](https://uapi-group.org/specifications/specs/linux_file_system_hierarchy/)):
  a reboot discards it, so nothing reads it across a crash.
- Bazel's client holds an `fcntl` lock on the output base and writes its owner
  (`pid=...`) into the lock file without `fsync`, for people to read, not for
  parsing ([blaze_util_posix.cc](https://github.com/bazelbuild/bazel/blob/master/src/main/cpp/blaze_util_posix.cc)).
  Its action cache journals every 3 s, checks its own integrity, and moves a
  corrupt cache aside to start empty
  ([CompactPersistentActionCache.java](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/actions/cache/CompactPersistentActionCache.java)).
- Cargo's locks are `flock` and `LockFileEx` locks, released with their file
  handle ([flock.rs](https://doc.rust-lang.org/nightly/nightly-rustc/src/cargo/util/flock.rs.html)):
  the kernel, not a durable record, proves the holder dead.
- SQLite in WAL mode with `synchronous=NORMAL` "might roll back following a
  power loss or system crash" but stays consistent, and "transactions are
  durable across application crashes"
  ([PRAGMA synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous)).

Two facts bound what a barrier would buy. On macOS, `fsync` moves data to the
drive but not to stable media: "if the drive loses power or the OS crashes,
the application may find that only some or none of their data was written"
([fsync(2)](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html)),
so the old DURABLE records were never durable against power loss there. And
an ordering barrier is no cheaper: on the external volume `F_BARRIERFSYNC`
cost 25 to 58 ms and `F_FULLFSYNC` 27 to 41 ms per call against 0.3 to 1.7 ms
for `fsync` (median of 30, two rounds). A directory `fsync` per operation
instead of per file saves nothing, because the file flush is the cost.

So the marker and scratch records use `PublicationDurability.RECONSTRUCTIBLE`.
`atomic_write_bytes(exclusive=True)` and `write_exact` accept it: exclusive
create is a no-replace rename whose atomicity needs no flush, and it keeps
every custody check. `namespace_publish_directory_exclusive` takes the same
parameter for the payload move. The one exclusive-rename authority requests
Windows write-through only for a durable rename; a reconstructible one, and
`namespace_move_exclusive`, use `os.rename` (MoveFileExW without
REPLACE_EXISTING), which refuses a rival without a barrier. A guarded run now issues no `fsync`
(`tests/test_memory_guard_tool.py::test_a_guarded_run_issues_no_fsync`).

Without a barrier, an OS crash can persist the writes of different files in
any order (no cross-file ordering is promised on APFS or ext4), and a file
whose rename persisted before its data reads back empty: ext4 forces the data
of a replacing rename before the rename commits (`auto_da_alloc`), not that
of a new name ([ext4](https://docs.kernel.org/admin-guide/ext4.html)). Each
reader therefore meets a record absent, empty or torn, or older than its
neighbours:

- A marker reader judges any readable generation by one process snapshot, as
  before. An absent marker protects nothing that still runs. An empty one is
  unreadable: it stays protective, and the reconciliation report names the
  `tools/memory_guard_custody.py --release MARKER --apply` command.
- A scratch reader holds the generation lock, so no state writer is active.
  Final receipt removal moves the whole reclaimed generation outside that
  lock: POSIX permits this while a reader holds the old file open; Windows
  defers the move. A missing receipt whose generation has also disappeared
  therefore follows the existing completed-removal path in both the pending
  sweep and dead-guard reconciliation. A missing receipt in a surviving
  generation stays crash residue. This distinction adds one namespace probe
  only after a missing-receipt error, with no normal-path syscall, lock,
  retry, allocation or retained metadata. An
  absent or undecodable receipt, a generation without its lock file, or a
  payload out of step with its owner (a retained payload that is missing, a
  reclaimed one that survives, a move into the generation that a crash undid)
  is crash residue: the generation becomes `blocked`, keeps its payload, and
  is reported once. A repeated report would fail every later guarded command
  with status 125. A decodable record that names another token, target or
  digest is never residue and stays fail-closed. Adoption of a dead guard's
  lease treats an existing terminal receipt as the finisher's uncommitted
  retirement, which also fixes a defect that the durable protocol had: a
  finisher that failed between its terminal receipt and its index left a
  marker that never resolved. Reconciliation republishes a missing index of a
  retiring, retained or reclaiming owner, because the index is a projection.

`tests/test_temporary_artifacts.py::test_every_crash_state_keeps_custody_and_reports_once`
rebuilds 1,920 such states from the bytes the real protocol wrote (every owner
generation or an absent or empty one, a terminal receipt absent, torn or
valid, the index absent, staged or published, the payload at its target, in
its generation, partly deleted or gone, the lock present or not) and runs the
pending sweep and the marker reconciliation three times in both orders. Its
oracle is custody policy: only the run's own payload is deleted, never after
its receipts show a deletion started, never without a valid terminal receipt;
every surviving payload or generation is reported; the sweep reports at most
once. The previous readers violate it in 1,826 of the 1,920 states (they
relied on write order); removing any one of the eleven reader rules by
mutation fails it.

The records that keep `DURABLE` describe state that outlives the processes: a
Cargo-incremental quarantine receipt names a directory that survives the run,
and artifact, cache-entry and release publications are outputs other
processes trust. Neither runs on a successful guarded command.

## 3. The structural facts/mechanisms to build (each tied to the class it retires)

### FACT-A: The recompile-blast-radius ratchet (retires DX-1)

**Problem class:** today, "did my crate cut actually isolate the build?" is
answered by hand (dx_baseline §5 measured it manually) and *regresses silently*
when someone adds a cross-crate `use` that re-couples layers. The 21b DAG is a
*design*; nothing enforces it.

**The fact:** a generated, checkable **crate-edge manifest** + a
**blast-radius oracle**. For each workspace crate, the manifest records its
declared upstream deps (from `Cargo.toml`) and a canary "touch one source file →
which crates does `cargo build --build-plan`/`-Z unstable-options` (or a stable
`cargo build --timings` parse) recompile?" The oracle asserts the *measured*
recompile set ⊆ the *declared* downstream cone. A new back-edge that re-couples
`molt-ir` to a backend (the exact regression 21b §"Flags" warns about) makes the
measured set exceed the declared cone → **CI red**, with the offending `use`
named.

**Why this is the structural fix, not a band-aid:** it makes DX-1 *unexpressible*
— you cannot land a layering violation without the ratchet catching it, the same
way `tools/gen_op_kinds.py --check` makes a missing opcode-classifier
unexpressible (STRUCTURAL_AUDIT_BOARD §"Discovery-vs-authority rule"). It
composes with the existing `structural_audit.py --check` ratchet (add
`max_crate_blast_radius` and `crate_layer_backedges` as new ratchet metrics
alongside `god_files`).

**Files:** new `tools/build_graph_audit.py` (emits
`docs/design/foundation/BUILD_GRAPH_BOARD.md`, mirrors `structural_audit.py`'s
`--write-board`/`--check` shape); a `[[crate]]` table in a new
`runtime/crate_graph.toml` (the declared DAG, the single source of truth the 21b
extractions converge on); wired into `tools/ci_gate.py`.

### FACT-B: The crate-ownership + write-set collision oracle (retires DX-2)

**Problem class:** `coordination.json` records `owned_paths` but the collision
check (`broad_lane_collisions`, `agent_coordination.py:1155`) only fires for two
*broad-sweep coordinators* on the *same target root + lane*. Two *implementers*
editing overlapping files in the same crate get **no warning** — the doc says
"never revert partner work" (MULTI_AGENT_COORDINATION §"Collision Rules") but
nothing *checks* it. That is an advisory, not a fact.

**The fact:** elevate `owned_paths` to a first-class **write-set** and make
`agent_coordination.py check` FAIL-CLOSED (exit 2) when two *active* records
(any role) have overlapping write-sets that are not a declared
integrator→patch-producer relationship. Overlap is computed at *crate + path-glob*
granularity (the 21b crate boundary is the natural ownership unit; a file glob is
the sub-crate unit for 21a/21d module work). The check already loads all records
(`load_records`) and already classifies roles — this extends the existing
collision computation, it does not add a new system.

**Why structural:** it turns "be respectful of partner work" from prose into a
checkable obligation (52 §0: "make verification un-gameable"). It composes with
FACT-C: `molt dx start` writes the ownership record *and* runs the collision
check *before* the first build, so collision is caught at acquisition, not at
merge.

**Files:** extend `agent_coordination.py` (`broad_lane_collisions` →
`write_set_collisions` + keep the broad-lane one); extend `coordination.json`
schema (bump `SCHEMA_VERSION` to 2) with `write_set` (list of crate/glob) +
`integrator_for` (optional task id, declaring a non-colliding patch relationship);
extend `tests/test_agent_coordination.py`.

### FACT-C: The canonical build-environment authority `molt dx` (retires DX-3)

**Problem class:** the throughput env used to be a **POSIX-only shell script**
(`throughput_env.sh`) — on Windows (the lead's host, per env) agents hand-set
`MOLT_SESSION_ID`, `CARGO_TARGET_DIR`, `SCCACHE_DIR`, daemon socket dir, etc.,
*per shell command*. That ritual differs by platform and is a class of silent
misconfiguration (wrong target dir → lock collision → killed builds;
`MULTI_AGENT_COORDINATION` Windows traps §). The CLI's `_maybe_enable_sccache`
does NOT set the shared `SCCACHE_DIR` — only the shell wrapper does, so Windows
agents get no cross-worktree cache sharing.

**The fact:** a single cross-platform `molt dx` subcommand group that *is* the
build-environment authority, resolving every knob from facts:
- `molt dx env` — the Python/Rust port of `throughput_env.sh` (works on Windows):
  resolves + exports one coherent RunContext fact set. Ordinary builds get
  `CARGO_TARGET_DIR`=`<root>/target` for warm incremental reuse; `--session-id`
  opts perf/bench/test-shard/proof lanes into
  `CARGO_TARGET_DIR`=`<root>/target/sessions/<sid>`. It also sets
  `SCCACHE_DIR`=`<root>/.sccache`
  + `SCCACHE_CACHE_SIZE`, daemon socket dir, `MOLT_CACHE`, diff roots, `TMPDIR`.
  Emits `--print` (eval-able / PowerShell `Invoke-Expression`-able) and `--apply`
  (writes a `.molt/dx.env` the CLI auto-loads). **This makes the LANDED
  session-isolation + sccache wiring reachable identically on every platform.**
- `molt dx check` — the pre-flight: validates `CARGO_TARGET_DIR` ==
  `MOLT_DIFF_CARGO_TARGET_DIR`, target dir is non-stale, sccache reachable,
  ≤3 daemons, no stale sockets, runs FACT-B collision check. Subsumes the
  `molt doctor`/`tools/check_compile_throughput.py` build-env checks
  (`cli/__init__.py:32690` sccache probe) into one authority. This is doc 08
  Phase-1c's "`molt dx check`" promise, generalized.
- `molt dx clean` — thin alias to the canonical `molt clean --apply
  --kill-processes` + `process_sentinel.py` (CLAUDE.md "Safe Execution"); never a
  second process-kill path (Bootstrap Authority: one authority per concern).

**Why structural:** one authority, fact-derived, cross-platform replaces N
per-agent shell rituals → the misconfiguration class is unexpressible (you cannot
forget to set a knob the authority sets). It also sets `SCCACHE_DIR` from the CLI
build path (not just the shell), so the cross-worktree cache hit (08 BX-3, the
~85% dependency-closure hit) works on Windows too.

**Files:** new `src/molt/cli/dx.py` (handler family, per 21d's package pattern);
a `_dx_env_facts()` resolver reusing `_session_target_dir` (`:13595`),
`_maybe_enable_sccache` (`:11761`), `_backend_daemon_socket_dir` (`:13644`);
extend `_maybe_enable_sccache` to set `SCCACHE_DIR`/`SCCACHE_CACHE_SIZE` from the
resolved root when unset; register `dx` in the `cli/__init__.py` dispatch
(`:40228` region); a tiny `tools/run_context_env.py` cross-platform path
(`throughput_env.sh` already shells to it — promote that logic to the authority).

### FACT-D: The unified observability + pass-delta plane (retires DX-4)

**Problem class:** env-var proliferation — `TIR_DUMP`, `MOLT_TIR_DUMP`,
`MOLT_DUMP_IR`, `MOLT_DUMP_CLIF*` (6 CLIF vars in `main.rs:84-89`),
`MOLT_VERIFY_ANALYSIS`, `MOLT_DEBUG_LOWER_FUNC` — each a separate ad-hoc toggle
with inconsistent filter syntax. And the audit board explicitly lists as MISSING:
a **pass-delta ledger** ("which pass loses Repr / adds boxing / increases generic
calls / RC events — needed to attribute drift") and a **fact graph** ("per-value
provenance to explain 'why is this boxed?'") — STRUCTURAL_AUDIT_BOARD
§"TOOLING GAPS". Without these, "why did the compiler do X / which pass changed
it" is bisection archaeology.

**The fact (two parts):**
1. **One filter authority `MOLT_DX_DUMP=fn:pass:stage`** parsed once, consumed by
   *both* the Python frontend TIR dump (`src/molt/debug/ir.py`, which already has
   `--function`/`--pass`/`--stage` filtering and `pre-midend`/`post-midend`
   stages) *and* the backend TIR/CLIF printers (`molt-tir` printer + the
   `MOLT_DUMP_CLIF*` family). A `DxDumpFilter` struct (Rust) +
   `DxDumpFilter` dataclass (Python) sharing one documented grammar; old env vars
   become back-compat aliases (the doc-08 Phase-1c `TirDumpConfig` design,
   generalized to span frontend+backend and CLIF, and unified with the
   already-built `debug/ir.py` filtering). The daemon already forwards
   `MOLT_TIR_DUMP`+`MOLT_DUMP_IR`+CLIF vars (`main.rs`); add `MOLT_DX_DUMP` to
   `DAEMON_REQUEST_ENV_KEYS`.
2. **The pass-delta ledger** — a first-class, per-pass *fact-diff* emitted by the
   `molt-tir` pass manager: for each pass, a structured record of
   `{repr_changes, boxes_added, rc_ops_delta, generic_calls_delta,
   ops_added/removed}` keyed by function. This is NOT a printf — it reads the
   *same generated op_kinds / Repr facts* the passes consume (no second
   authority), and emits to the canonical debug-artifact dir
   (`MOLT_DEBUG_ARTIFACT_DIR`, already daemon-forwarded `main.rs:98`). `molt debug
   pass-delta <prog>` renders it; `tools/pass_delta_dashboard.py` (audit board
   names this exact path as "not built") aggregates it. This composes directly
   with the perf arc: 51 §"posture — fix the REPRESENTATION" asks "which FACT is
   missing?"; the ledger *attributes* a boxing/RC regression to a pass in one
   query instead of a bisect.

**Why structural:** the ledger makes "a pass silently degraded the IR" an
*attributable, ratchet-able* event (you can CI-gate "no pass adds net boxing on
the verified subset") rather than a perf mystery. It is the DX-side dual of the
semantic control plane (46) — provenance for *compiler decisions* the way 46 is
provenance for *value semantics*. The pass manager already has the
`MOLT_VERIFY_ANALYSIS` hook (`00_integrated_parallel_program.md:29`,
`pass_manager`) to thread it through.

**Files:** `molt-tir` printer + a new `tir/pass_delta.rs` (the ledger record,
fed by the existing pass-manager loop); `src/molt/debug/dx_dump.py` (the shared
filter grammar + `pass-delta` renderer); `tools/pass_delta_dashboard.py` (NEW,
the audit-board-named gap); register `MOLT_DX_DUMP` in `main.rs`
`DAEMON_REQUEST_ENV_KEYS`.

---

## 4. Concrete phases (dependency order; each independently landable, green gates)

**Universal gate methodology (the fd58a0a93 / dx_baseline §9 / 21 §3 contract),
applied to every phase below.** Each phase is its own complete structural piece
(CLAUDE.md "structural change as the unit of work"); intermediate commits are
acceptable only when each is itself complete + carries a baton note.

```
G0  Isolated env: export MOLT_SESSION_ID=dx-<phase>; CARGO_TARGET_DIR=target/sessions/$MOLT_SESSION_ID
G1  Zero-warning build, CI-exact (no --lib, tests compile too):
      cargo clippy -p <crate> --features <set> -- -D warnings
G2  Full lib suites for the touched crate(s) (cargo test -p <crate> ...).
G3  Move-only proof where claimed: byte-identical artifacts + stderr diagnostics
      on a fixed corpus (python -m molt build --target native --rebuild before/after, diff).
G4  Differential e2e via the guarded harness (python -m molt test / tests/molt_diff.py),
      NEVER a raw binary (CLAUDE.md Safe Execution); byte-identical vs CPython.
G5  Symbol identity on crate moves (nm the rlib; C-ABI surface unchanged).
G6  DX-specific perf gate (build-time): the measured metric the phase claims,
      reported per dx_baseline §5 discipline (cold AND warm, isolated target dir,
      contention noted), classified GREEN/RED_STABLE/RED_NOISY/TIE/DIMENSIONAL_WIN
      (CLAUDE.md perf-claims discipline). No runtime-perf gate is needed for
      build-graph phases (release-fast never builds the shipped runtime —
      dx_baseline §13 proves user-binary perf is structurally untouched).
```

Phases are grouped into three lanes that map onto the council three-lane model
(§5): **Lane DX-I = build-graph completion (FACT-A + 21b crates)**, **Lane DX-II
= coordination/env facts (FACT-B + FACT-C)**, **Lane DX-III = observability
(FACT-D)**. DX-II and DX-III are *Python/tooling* (no Rust compile, near-zero
blast radius) and can run **fully in parallel** with DX-I from day one.

---

### Phase 1 — FACT-A: the blast-radius ratchet (Lane DX-I, foundation) — ~2 days

The cut-enforcement fact must exist *before* the 21b crate fan-out, so each
extraction lands against a ratchet that proves it isolated the build.

- **1a.** Author `runtime/crate_graph.toml` = the declared DAG (the 21b §"Target
  crate graph" topology: `molt-ir ← molt-passes ← molt-lower ← {backends} ←
  molt-backend`). Seed it with the *current* (pre-fan-out) reality: `molt-tir`
  (extracted) + `molt-backend` (still monolithic below it) + `molt-runtime`
  satellites. The TOML is the single source of truth the extractions converge on.
- **1b.** `tools/build_graph_audit.py`: parse all workspace `Cargo.toml` deps;
  for a representative touched file per crate, measure the recompile set via
  `cargo build --timings` JSON (stable) or `--build-plan` (parse the unit graph);
  assert measured ⊆ declared cone. `--write-board` →
  `docs/design/foundation/BUILD_GRAPH_BOARD.md`; `--check` ratchets
  `max_crate_blast_radius` + `crate_layer_backedges` (= 0).
- **1c.** Wire `--check` into `tools/ci_gate.py` and add the two metrics to the
  `structural_audit.py` board's ratchet table (one board, consistent shape).
- **Gates:** G1/G2 trivial (tooling). G6 = the board reproduces dx_baseline §5's
  measured numbers (a tir-pass edit recompiles `molt-backend`+daemon TODAY; the
  ratchet *records* that as the baseline cone, which subsequent phases SHRINK).
- **Composes with:** 21b (the extractions consume this ratchet); 51 §3 (a new
  scoreboard); STRUCTURAL_AUDIT_BOARD (same `--check` ratchet machinery).

### Phase 2 — FACT-C: `molt dx` build-env authority (Lane DX-II, parallel) — ~2 days

Independent of all Rust work; unblocks every agent's loop on Windows immediately.

- **2a.** `src/molt/cli/dx.py` with `_dx_env_facts()` reusing `_session_target_dir`,
  `_maybe_enable_sccache`, `_backend_daemon_socket_dir`, the canonical roots from
  `tools/run_context_env.py`. `molt dx env --print/--apply` cross-platform
  (PowerShell + POSIX output). Writes/reads `.molt/dx.env`.
- **2b.** Extend `_maybe_enable_sccache` (`cli/__init__.py:11761`) to set
  `SCCACHE_DIR=<root>/.sccache` + `SCCACHE_CACHE_SIZE` when unset (so Windows CLI
  builds share the cross-worktree cache, not just `throughput_env.sh` POSIX
  users). Add `.sccache/` to `.gitignore` (verify not already present).
- **2c.** `molt dx check` — fold the existing `molt doctor` sccache/profile/target
  probes (`cli/__init__.py:32690`) into one authority; validate target-dir
  consistency + staleness; ≤3 daemons + no stale sockets (reuse
  `backend_daemon_custody`); call FACT-B's collision check (Phase 3).
- **2d.** `molt dx clean` = thin alias to `molt clean --apply --kill-processes`
  (NO second kill path).
- **2e.** Update CLAUDE.md "Concurrent Development" + `MULTI_AGENT_COORDINATION.md`
  Startup Discovery to recommend `molt dx env --apply` as the cross-platform
  bootstrap (keeping `throughput_env.sh` as the POSIX fast path it already is).
- **Gates:** G1/G2 = `pytest tests/cli/` + new `tests/cli/test_dx.py` (env
  resolution on win/posix via monkeypatched `os.name`; `--print` round-trips;
  `check` exit codes). G3 N/A (new subcommand, additive). Entry-point invariant
  (21d §4): `python -m molt dx --help` exits 0; existing `--help` corpus unchanged.
- **Composes with:** 08 Phase-1b/1c (fulfills the deferred `molt dx check` +
  shared-sccache promises, cross-platform); 21d (handler in the `cli/` package).

### Phase 3 — FACT-B: write-set collision oracle (Lane DX-II, parallel) — ~1.5 days

- **3a.** Bump `coordination.json` `SCHEMA_VERSION` → 2; add `write_set`
  (crate + path-glob list) + `integrator_for` fields to `build_record`
  (`agent_coordination.py:692`) + `init` CLI flags.
- **3b.** `write_set_collisions()` alongside `broad_lane_collisions`: two *active*
  records (any role) with overlapping write-sets and no integrator relationship →
  collision; `check` returns exit 2. Overlap at crate granularity (cheap) then
  glob refinement.
- **3c.** `agent_coordination.py check` includes both collision families; `molt dx
  check` (Phase 2c) calls it. Extend `tests/test_agent_coordination.py` (overlap,
  integrator-exemption, schema-v2 round-trip, the Windows BOM path stays green).
- **Gates:** G1/G2 = the agent-coordination proof lane (`agent_coordination.py:268`
  already self-describes it) + `check_subprocess_guard_coverage.py`. Backward-compat:
  v1 records (no `write_set`) degrade gracefully (treated as path-glob from
  `owned_paths`).
- **Composes with:** the LANDED coordination index; 52 §0 (un-gameable verifier);
  MULTI_AGENT_COORDINATION Collision Rules (turns prose into a check).

### Phase 4 — 21b crate fan-out, ratchet-gated (Lane DX-I) — ~8–12 days, multi-agent

This is the **keystone build-cache win** and the largest piece. It executes the
21b §"Ranked extraction sequence" S1–S8 — but now *each extraction lands against
the FACT-A ratchet (Phase 1)*, so "did it isolate the build?" is machine-proven,
not hand-measured. 21b is the authoritative topology; this arc adds only the gate
wiring + the parallel-execution mapping. Sub-phases (21b S1–S8), each its own
move-only commit with G1–G6:

- **4.S1** Split `molt-tir` → `molt-ir` (vocabulary+transport+`Repr`+std-leaves;
  `molt-tir` keeps passes+lowering, deps `molt-ir`). FIRST; the matches!-oracle
  audit (21 §4) as ops cross the boundary; cut the 4 test-only passes→lowering
  refs behind `molt-tir/test-util` (21b §1). **Gate add:** FACT-A board must show
  `molt-ir` blast-radius does NOT include passes/backends.
- **4.S2** Split residual → `molt-passes` ← `molt-lower` (`ir_rewrites.rs`
  migrates into `molt-lower` per 21b flag #4). [seq:S1]
- **4.S3** Extract `molt-codegen-abi` (NaN-box, header-layout, and type-id ABI
  facts formerly in backend-local constants + helpers from `molt-backend/lib.rs`;
  rewrite WASM value-encoding duplicates to import the shared authority).
  **[∥]** S2 (only needs `molt-ir`). G3 byte-identical gates the wasm de-dup.
- **4.S4** Extract `molt-backend-llvm` (leaf). [seq:S2,S3]; **coordinate the
  active LLVM lane** (21 §0.3 — never extract a crate from under an active editor;
  FACT-B makes that collision FAIL-CLOSED, not advisory).
- **4.S5 / 4.S6** Extract `molt-backend-wasm`; `molt-backend-luau`+`molt-backend-rust`.
  **[∥]** each other and S4 (disjoint crates → 3 agents at once, FACT-B-guarded).
- **4.S7** Extract `molt-backend-native` (deps `molt-lower`+abi+opt llvm;
  `use super::*`→explicit `use molt_lower::…`). LAST backend (riskiest;
  symbol-identity G5); follows S4 + the in-flight 21a `fc/` work.
- **4.S8** Reduce `molt-backend` to the thin driver + daemon bin; per-backend
  features → `dep:` activations (21b §Layer 4). Final fan-in.
- **Gates per sub-phase:** full G1–G6. **G6 is the headline:** after S2, a tir-pass
  edit's FACT-A blast-radius must EXCLUDE the 5 backends (the dx_baseline §5 "tir
  edit costs the same as a native edit" pathology is now *machine-proven gone*);
  after S7, an `fc/arith.rs` edit recompiles one codegen unit in
  `molt-backend-native` only. Each sub-phase must SHRINK the FACT-A board's
  `max_crate_blast_radius` ratchet (it may only go down).
- **Composes with:** 21b (topology), 21a (the `fc/` function-split that S7
  consumes), 08 Phase-3 (this IS that, refined to per-backend crates + gated).

### Phase 5 — FACT-D part 1: unified `MOLT_DX_DUMP` filter (Lane DX-III, parallel) — ~2 days

- **5a.** `DxDumpFilter` grammar (`fn:pass:stage`, any field optional) — one Rust
  struct in the `molt-tir` printer + one Python dataclass in
  `src/molt/debug/dx_dump.py`. Reuse `debug/ir.py`'s existing
  function/pass/stage filtering (it already does `pre-midend`/`post-midend`).
- **5b.** Route the backend `TIR_DUMP`/`MOLT_TIR_DUMP`/`MOLT_DUMP_IR` +
  `MOLT_DUMP_CLIF*` family through `DxDumpFilter`; old vars = back-compat aliases
  (doc-08 Phase-1c `TirDumpConfig`, generalized to span frontend+backend+CLIF).
- **5c.** Add `MOLT_DX_DUMP` to `DAEMON_REQUEST_ENV_KEYS` (`main.rs:63`).
- **Gates:** G1/G2; behavioral test `MOLT_DX_DUMP=fib:gvn:post` prints only
  `fib` after GVN; old vars still work (alias regression test).
- **Composes with:** the LANDED `debug/` package + daemon env forwarding.

### Phase 6 — FACT-D part 2: the pass-delta ledger (Lane DX-III) — ~3 days

- **6a.** `molt-tir/src/tir/pass_delta.rs`: per-pass `{repr_changes, boxes_added,
  rc_ops_delta, generic_calls_delta, ops_added/removed}` keyed by function, read
  from the *generated op_kinds/Repr facts* the passes already consume (NO second
  authority — STRUCTURAL_AUDIT_BOARD discovery-vs-authority rule). Emitted to
  `MOLT_DEBUG_ARTIFACT_DIR` (daemon-forwarded). Threaded through the pass-manager
  loop (same hook as `MOLT_VERIFY_ANALYSIS`).
- **6b.** `molt debug pass-delta <prog>` renderer + `tools/pass_delta_dashboard.py`
  (the audit-board-named "not built" gap, STRUCTURAL_AUDIT_BOARD §"TOOLING GAPS").
- **6c.** (Stretch, gated separately) a CI ratchet "no pass adds net boxing /
  net RC ops on the verified subset" — a perf-correctness fact (51 §"posture").
- **Gates:** G1/G2; a fixture program with a known pass-induced box shows the
  attributing pass in the ledger; dashboard aggregates a multi-function run.
- **Composes with:** 46 (semantic control plane — the value-provenance dual),
  51 §1 (attributes "which FACT is missing" per pass), the perf scoreboards.

**Stop-anywhere property:** Phases 1–3 and 5 each deliver standalone value
(ratchet / cross-platform env / collision safety / unified dump) and can land in
any interleaving. Phase 4 is the long pole but is itself S1–S8 stop-anywhere
(each sub-phase a complete crate cut). Phase 6 depends on 5a's filter grammar.

---

## 5. How this composes with the decomposition (21a–e) and the parallel execution model

**With the decomposition program:**
- **21 (program) / 21b (crate graph):** Phase 4 *is* the 21b extraction sequence
  S1–S8, executed under the FACT-A ratchet. This arc does not invent topology — it
  adds the *machine-checked cut-enforcement* (Phase 1) and the *parallel-execution
  + collision safety* (FACT-B/C) that 21b assumes but does not provide. 21b §"Flags"
  warns of layering regressions; FACT-A makes them CI-red.
- **21a (function-split of `compile_func_inner`):** LANDED through M1.16. Phase
  4.S7 (`molt-backend-native` extraction) *consumes* the `fc/` tree — the
  `use super::*` ancestry becomes `use molt_lower::…` (21b S7). No conflict: 21a
  is intra-crate (codegen-unit parallelism), 4.S7 is the crate boundary
  (cache-isolation); they are the two halves dx_baseline §6 distinguishes.
- **21c (frontend mixins):** IN FLIGHT. FACT-B (write-set collision) directly
  protects the frontend split — the audit board shows `frontend/visitors/calls.py`
  (8,733) is the next contention hotspot; the collision oracle stops two agents
  colliding on it. FACT-D's `MOLT_DX_DUMP` unifies the frontend `debug/ir.py`
  dump with the backend dump (one grammar across the lowering boundary).
- **21d (cli package):** DONE through Phase 0+. FACT-C's `src/molt/cli/dx.py` is a
  *new handler family in the existing `cli/` package* — it follows 21d's pattern
  exactly (handler imports from `_shared`/pipeline; dispatch stays in `__init__`),
  and benefits from the smaller, parallel-ownable cli modules 21d created.
- **21e:** no `21e` exists in the tree (21 has a–d); if a future `21e` is the
  runtime-satellite completion (Move R), FACT-A's ratchet covers the satellite
  crates too (the `crate_graph.toml` includes them).

**With the parallel multi-agent execution model (the council three-lane model,
51/00/CLAUDE.md):**
- **Lane mapping:** DX-I (build-graph: Phase 1, 4) = infra/decomposition (council
  Lane C, "makes A&B faster"); DX-II (env+collision: Phase 2, 3) = Lane C tooling;
  DX-III (observability: Phase 5, 6) = Lane C that *feeds* Lane B perf
  (the pass-delta ledger attributes perf regressions). **None of these is Lane A
  (P0 memory safety)** — so per the council doctrine, DX work *never blocks* the
  P0 corruption/finalizer lane and is correctly subordinate to it.
- **Non-overlapping files (continuous lanes):** DX-II/III touch
  `tools/*.py` + `src/molt/cli/dx.py` + `src/molt/debug/*` + `molt-tir` printer —
  disjoint from the Lane A `tir/passes/*` ownership and the Lane B
  benchmark/codegen files. DX-I Phase 4 is the *only* DX work that touches hot
  backend crates, and it is itself FACT-B-guarded so its sub-phases parallelize
  across agents on disjoint new crates (21b §"Parallelization": after S2+S3, three
  agents at once on {S4→S7}, {S5}, {S6}).
- **The dogfood loop:** this arc's deliverables are *used by the swarm that builds
  the rest of the portfolio* — FACT-C is run at every agent startup, FACT-B at
  every ownership acquisition, FACT-A at every CI run, FACT-D at every perf
  investigation. DX is the one arc whose output compounds across *all other arcs'*
  velocity (the §0 derivative claim).

**Cross-arc dependencies (explicit):**
- Perf arcs (06/03/04/05, 51 Lane B) *depend on* FACT-D (Phase 6): the pass-delta
  ledger is how a perf custodian attributes a regression to a pass instead of
  bisecting — it operationalizes 51 §"posture: fix the REPRESENTATION" by naming
  the pass that lost the fact.
- The 21b extraction (Phase 4) *depends on* FACT-A (Phase 1): without the ratchet,
  a cut that fails to isolate the build (the exact dx_baseline §4 "lib.rs split
  didn't help" failure) ships silently.
- FACT-C/B (Phase 2/3) *depend on nothing* — pure Python/tooling, land first,
  immediately de-risk every other arc's multi-agent execution.
- The semantic control plane (46) and FACT-D (Phase 6) are duals (compiler-decision
  provenance vs value-semantics provenance) — they should share the
  debug-artifact emission convention (`MOLT_DEBUG_ARTIFACT_DIR`).

---

## 6. Risks + structural (not band-aid) treatment

| Risk | Where it bites | Structural treatment (NOT a workaround) |
|---|---|---|
| **`cargo --timings`/`--build-plan` JSON is unstable across cargo versions** → FACT-A oracle breaks | Phase 1b | Pin the parse to the *stable* `--timings` JSON fields (unit name + crate), with a versioned adapter; if a field moves, the adapter fails LOUD (not silently passes). Do NOT screen-scrape human output. The oracle is a fact-consumer; treat cargo's machine output as the fact source with a checked schema. |
| **Blast-radius measurement is noisy (contention from other agents)** → false ratchet flips | Phase 1, 4 G6 | Measure in an *isolated* `target/sessions/<sid>` (LANDED isolation), report cold AND warm, classify RED_NOISY vs RED_STABLE (CLAUDE.md perf discipline). The ratchet gates on the *declared cone ⊇ measured set* (a structural containment), NOT on absolute seconds — so wall-clock noise cannot flip it; only a genuine new back-edge can. |
| **Crate fan-out (Phase 4) introduces a miscompile via a moved `matches!` oracle** (default-false on a missed arm) | 4.S1/S2/S7 | 21 §4's matches!-oracle audit is a HARD per-sub-phase gate: every `matches!(op.kind/opcode, …)` that crosses a boundary converts to an exhaustive `match` (compiler-enforced). STRUCTURAL_AUDIT_BOARD already tracks these (73 `semantic_fallthrough` findings); the fan-out must not *increase* that ratchet. |
| **Extracting a crate from under the active LLVM editor** (21 §0.3) | 4.S4/S7 | FACT-B (Phase 3) makes this collision FAIL-CLOSED: the LLVM lane's `write_set` covers `llvm_backend/`; an extraction agent's overlapping write-set is refused at acquisition. This is the *structural* version of "coordinate a freeze window." |
| **`molt dx` becomes a second source of truth for env/profile** (duplicating `_resolve_*_cargo_profile_name`, the daemon paths) | Phase 2 | `molt dx` is a thin *composer* over the EXISTING resolvers (`_session_target_dir`, `_maybe_enable_sccache`, `_resolve_backend_cargo_profile_name`, `_backend_daemon_socket_dir`) — it adds no new resolution logic, only a cross-platform surface + the missing `SCCACHE_DIR` set. One authority per concern (Bootstrap Authority doctrine). `molt dx clean` aliases `molt clean` (no second kill path). |
| **Pass-delta ledger becomes a second op-semantics authority** | Phase 6 | The ledger READS generated op_kinds/Repr facts (the same ones passes consume); it asserts no semantics of its own (STRUCTURAL_AUDIT_BOARD discovery-vs-authority rule). It is provenance OF decisions, not a decider. |
| **Ledger overhead slows the daemon hot path** | Phase 6 | Gated behind `MOLT_DX_DUMP`/`MOLT_DEBUG_ARTIFACT_DIR` (off by default, daemon-forwarded only when set) — zero cost on the normal build path; it is an *instrument*, like the LANDED `MOLT_VERIFY_ANALYSIS`. The 51 §"posture" lesson: every optimization lands WITH its firing instrument — the ledger is that instrument, made uniform. |
| **Windows path/encoding hazards in the new tooling** (the recurring cp1252 read_text bug class, MEMORY.md) | Phase 2, 3 | All new file I/O uses explicit `encoding="utf-8"`; reuse the BOM-tolerant `_decode_record_bytes` (`agent_coordination.py:1112`) pattern for any record read; `molt dx env` emits PowerShell-safe output (no here-strings, no `2>&1` on native exes — the env doc's PowerShell rules). New `tests/cli/test_dx.py` runs the win path via monkeypatched `os.name`. |
| **The FACT-A ratchet blocks a legitimate, intentional re-coupling** | Phase 1, 4 | The ratchet gates on `crate_graph.toml` (the *declared* DAG). An intentional new edge is a one-line TOML change reviewed as the deliberate architectural decision it is — the ratchet forces the coupling to be *explicit and reviewed*, never silent. That is the feature, not a bug. |
| **Stop-anywhere illusion: Phase 4 left half-done = two parallel sources of truth** | Phase 4 | Each 4.Sx is a *complete* crate cut (move-only, G1–G6 green) — never a partial fix toward the next (CLAUDE.md "structural change as the unit of work"). If the session ends mid-fan-out, the baton note records exactly which S-phase is next; the tree is always in a consistent N-crate state, never a hybrid. |

---

## 7. Verification appendix (the measurement discipline per phase)

- **Build-time metrics** (Phase 1, 4 G6): `tools/build_graph_audit.py` (NEW) +
  the existing `tools/bench_backend_incremental.py` (cold/warm/edit phases,
  isolated target dir) + `cargo --timings` JSON. Report
  `edit-target → measured-recompile-set → declared-cone → ⊆? → wall (cold/warm)`,
  classified per CLAUDE.md. The ratchet authority is the *containment*, not the
  seconds.
- **Parity oracle** (all phases G4): `tests/molt_diff.py` basic+stdlib vs system
  CPython ≥3.12 (52 §A.1 hard invariant); byte-identical, zero tolerance.
- **Move-only proof** (Phase 4 G3): byte-identical `.o`/`.wasm`/`.ll` + stderr
  diagnostics before/after each crate cut (21 §3 G3).
- **Coordination proof** (Phase 2, 3): `pytest tests/test_agent_coordination.py`
  + `tests/cli/test_dx.py` + `tools/check_subprocess_guard_coverage.py` (the
  agent-coordination proof lane, `agent_coordination.py:268`).
- **Observability proof** (Phase 5, 6): behavioral filter tests + a fixture with a
  known pass-induced box attributed correctly in the ledger.
- **Ratchet integration** (Phase 1c): `tools/structural_audit.py --check` and
  `tools/build_graph_audit.py --check` both green in `tools/ci_gate.py`; the
  god-file ratchet (`max_god_file_lines`) must not regress (this arc shrinks it
  via Phase 4, never grows it).
- **No runtime-perf gate needed for build-graph phases:** dx_baseline §13 proves
  `release-fast` never builds the shipped runtime (that is `release-output`), so
  user-binary perf is structurally untouched by build-iteration changes — the
  bench-regression gate is satisfied by construction (state this in each Phase-4
  landing report rather than re-running benches blind).

---

## 8. The one-paragraph executable summary (for the integrator)

DX is the derivative on the whole 10-year program: faster, collision-free,
observable loops multiply every other arc's velocity. The first generation
(thin LTO, `molt-tir` extraction, `fc/` function-split, `cli/` package, session
isolation, the coordination index) has LANDED under 08/21. This arc builds the
second generation as four checkable FACTS, each retiring a DX class: **FACT-A**
the blast-radius ratchet (`build_graph_audit.py` + `crate_graph.toml`, makes
layering regressions CI-red — Phase 1); **FACT-B** the write-set collision oracle
(FAIL-CLOSED in `agent_coordination.py`, makes partner-collision unexpressible —
Phase 3); **FACT-C** the cross-platform `molt dx` build-env authority
(`src/molt/cli/dx.py`, makes Windows/POSIX misconfiguration unexpressible, fulfills
08's deferred `molt dx check`+shared-sccache — Phase 2); **FACT-D** the unified
`MOLT_DX_DUMP` filter + pass-delta ledger (`pass_delta.rs` +
`pass_delta_dashboard.py`, makes "why did the compiler do X / which pass" a
one-query answer, builds the audit-board-named gaps — Phase 5/6). The keystone
build-cache win, **Phase 4**, executes the 21b crate fan-out S1–S8 under the
FACT-A ratchet so each cut's build-isolation is machine-proven. DX-II/III
(Phase 2/3/5/6) are Python/tooling, near-zero blast radius, land first and in
parallel; DX-I (Phase 1/4) is the long pole, itself stop-anywhere. Everything
composes with — never duplicates — 21a/21b/21c/21d, obeys the council three-lane
model (all DX work is Lane C, subordinate to the P0 memory-safety Lane A), and
the perf arc depends on FACT-D to attribute regressions to passes.

---

*Design only / executable plan. Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>*
