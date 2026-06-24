<!-- Foundation blueprint 56. Arc: DEVELOPER EXPERIENCE — build speed + daemon +
concurrent-dev + debugging. Author: portfolio-architect. Date: 2026-06-23.
Design only / executable plan. Time-traveler method: start from the 5-to-100-year
DX end-state, work backward to the structural FACTS that make sub-second edits,
N-agent parallelism, and instant feedback INEVITABLE rather than heroic.
Composes with 08_DX-buildspeed.md (LANDED Phase 1a thin-LTO), dx_baseline.md
(the MEASURED baseline + the "module≠crate / function-is-the-codegen-unit" laws),
21/21a/21b/21d (decomposition program), 51 (10-year roadmap), 52 (autonomous
charter). Every factual claim verified against the tree at branch `main`
(HEAD 1d92bc5cf); verification pointers inlined. No code refactored in this
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

## 2. Current state (VERIFIED against `main` @ 1d92bc5cf) — what already landed

The stale docs (08, dx_baseline) predate large wins. The authoritative current
state, grep/read-verified this session:

| Surface | Verified state | Evidence |
|---|---|---|
| **`molt-tir` crate** | **EXTRACTED** (21 T1 LANDED). `runtime/molt-tir/Cargo.toml` exists; `tir/` is ~73 files there; `agent_coordination.py:289` routes `runtime/molt-tir/src/tir/type_refine.rs` as its own proof lane. | `runtime/molt-tir/Cargo.toml`; workspace member `Cargo.toml:22` |
| **`function_compiler.rs`** | **28,147 lines** (down from 39,043), `fc/` subtree of 39 handler files; M1.1–M1.16 function-extraction LANDED (arith/compare/calls/loops/indexing/…). | `STRUCTURAL_AUDIT_BOARD.md:26,82`; `21a` §"M1.1-M1.16 now landed" |
| **`cli.py` → `cli/` package** | **DONE through 21d Phase 0+.** `src/molt/cli.py` no longer exists; `src/molt/cli/__init__.py` (~41K lines, the residual engine) + `wasm.py`, `deps.py`, `native_toolchain.py`, `completion.py`, `arg_helpers.py`, `maintenance.py`, `debug_helpers.py`. `cli/__main__.py` present (preserves `-m molt.cli`). | `src/molt/cli/*.py` glob; audit board still flags `src/molt/cli.py` 41641 — **board path is STALE** (the line count moved into `cli/__init__.py`) |
| **Frontend mixin split** | **IN FLIGHT** (21c). `frontend/__init__.py` now 27,940 lines (was 44,620); `frontend/visitors/calls.py` (8,733), `frontend/visitors/classes.py` (3,977), `frontend/lowering/serialization.py` (4,413) extracted. | `STRUCTURAL_AUDIT_BOARD.md:26,32,89,113,115`; git status shows `frontend/visitors/classes.py` + `lowering/serialization.py` modified |
| **Cargo profiles** | **MATURE.** `dev`, `dev-fast` (cgu=256, lto=off), `release-fast` (thin LTO — Phase 1a LANDED), `release-output` (fat LTO, opt-z, abort), `release-size`, `wasm-release`, `wasm-release-fallback`, `dev-release`. Per-package hot-crate opt overrides documented + measured. | `Cargo.toml:34-565` |
| **Session isolation** | **LANDED.** `_session_target_dir` → `target/sessions/<sid>` (`cli/__init__.py:13595`); per-session daemon socket + `session_id` in daemon-path cache key (`:13664`). | read-verified |
| **sccache** | Wiring LANDED (`_maybe_enable_sccache` `cli/__init__.py:11761` + retry `:11790`; sets `CARGO_INCREMENTAL=0`). Shared `.sccache` dir + 10G cap wired ONLY in `throughput_env.sh:39-52` (**POSIX-only shell script**; the CLI does NOT set `SCCACHE_DIR`). | read-verified |
| **Coordination facility** | **Windows-robust** discovery index (NOT a lock). `agent_coordination.py`: proof-lane rules, BOM-tolerant record read (`_decode_record_bytes:1112`), `codex-stall` telemetry, `broad_lane_collisions` (only broad-sweep coordinators, only same target+lane). Real serialization authority = harness lock `<CARGO_TARGET_DIR>/.molt_state/diff_run.lock` (`MULTI_AGENT_COORDINATION.md:108`). | read-verified |
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

---

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

**Problem class:** the throughput env is a **POSIX-only shell script**
(`throughput_env.sh`) — on Windows (the lead's host, per env) agents must hand-set
`MOLT_SESSION_ID`, `CARGO_TARGET_DIR`, `SCCACHE_DIR`, daemon socket dir, etc.,
*per shell command* (CLAUDE.md "Concurrent Development" + "MOLT_SESSION_ID must be
set BEFORE any build"). Ritual that differs by platform is a class of silent
misconfiguration (wrong target dir → lock collision → killed builds;
`MULTI_AGENT_COORDINATION` Windows traps §). The CLI's `_maybe_enable_sccache`
does NOT set the shared `SCCACHE_DIR` — only the shell wrapper does, so Windows
agents get no cross-worktree cache sharing.

**The fact:** a single cross-platform `molt dx` subcommand group that *is* the
build-environment authority, resolving every knob from facts:
- `molt dx env` — the Python/Rust port of `throughput_env.sh` (works on Windows):
  resolves + exports `MOLT_SESSION_ID` (defaults to a stable per-worktree id),
  `CARGO_TARGET_DIR`=`target/sessions/<sid>`, `SCCACHE_DIR`=`<root>/.sccache`
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

**Universal gate methodology (the 34e3bddbf / dx_baseline §9 / 21 §3 contract),
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
- **4.S3** Extract `molt-codegen-abi` (NaN-box consts from `native_backend_consts.rs`
  + helpers from `molt-backend/lib.rs`; rewrite `wasm.rs:17` `QNAN` dup to import).
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
