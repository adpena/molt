# Tool Registry — the high-signal tools, run by default. DO NOT REBUILD THESE.

**STOP.** Before you build any analysis / verification / perf / audit tool, it
almost certainly already exists. This is the canonical, agent-facing catalog of
molt's highest-signal tools. Agents kept rediscovering and rebuilding these
(two agents independently re-authored `perf_authority.py`; the float and int
repr cuts were built twice from opposite ends) — that waste ends here.

## Canonical proof authority

```
python3 tools/proof_plan.py --check
python3 tools/proof_plan.py --run-family <family> --receipt proof-receipts/<family>.json
```

`tools/proof_plan.toml` is the sole hosted command, dependency, timeout,
resource, toolchain, and evidence authority. `tools/proof_plan.py` executes that
typed DAG under guarded custody and writes hash-bound receipts. A
`github-matrix` family (`python_unit`, `platform_portability`) runs one job per
runner cell; locally `--run-family` runs the cell that matches the host OS and
architecture, or takes `--matrix-cell <cell>`. Workflows own
runner setup and artifact transport only. Its `always` list and path `rule`s are
the landing gates `tools/molt_dev.py integrate` runs; locally,
`tools/proof_plan.py --run-command <id>` runs any single row.

Nightly shard execution is one authority family: `tools/nightly_prepare.py`
builds the immutable runtime and digest-bound plan, `tools/nightly_sharding.py`
owns exact corpus closure/LPT execution/aggregation, and
`tools/nightly_profile_feedback.py fit` reduces successful aggregate item
timings into a compact next-run candidate. The proof plan's sharded scheduled
families own each program's shard count and shard budget
(`shard_timeout_minutes`, `shard_overhead_seconds`); the plan carries the
resulting shard deadline, so a slow shard stops itself and publishes its
evidence before its job is killed. A shard prints its verdict and, when it
failed, the child's output tails; an aggregate prints its verdict, its
integrity errors and every non-passing item, and an incomplete aggregate is a
named failure rather than a contract error. The checked-in
`config/nightly_shard_profile.json` is promoted only through
`tools/nightly_profile_feedback.py promote`; CI publishes candidates and never
self-commits or invents an external scheduling authority.

## Catalog (by purpose)

### Verify / drift-gates (proof-plan landing gates and `repository_policy` rows — fail closed)
- `tools/structural_audit.py --check` - duplicate-authority / kitchen-sink and undecomposed-god-file ratchet. Use `--path <file-or-dir> [--json]` for fast diagnostic attribution on touched files; `--path` is intentionally refused with ratchet-writing/checking modes.
- `tools/build_graph_audit.py --check` - recompile-blast-radius ratchet (doc 56 FACT-A): fails if a cross-crate dep re-couples a layer (back-edge) or widens a crate's downstream recompile cone past `tools/build_graph_baseline.json`. Declared DAG = `runtime/crate_graph.toml`; reads `cargo metadata` only (no compile). `--write-board` -> `docs/design/foundation/BUILD_GRAPH_BOARD.md`.
- `tools/check_runtime_symbol_owners.py` - one `#[no_mangle] extern "C"` owner per runtime satellite symbol.
- `tools/check_perf_gate_wiring.py` — the canonical perf gate must fire on main.
- `tools/check_ratio_direction.py` — no raw `t/t` ratio outside the signed authority.
- `tools/check_perf_freshness.py` — no stale/unstamped perf doc with citable numbers.
- `tools/audit_op_kinds.py` / `tools/gen_op_kinds.py --check` — op-kind authority + drift.
- `tools/gen_frontend_diagnostics.py --check` — generated frontend rejection
  codes and the structural gate forbidding direct compiler `NotImplementedError`,
  direct `compat.unsupported`, untyped rejection construction, and unused codes.
- `tools/check_generator_manifest.py --check` — the semantic-fact-plane meta-gate
  (doc 59): every generated authority is registered in `tools/generator_manifest.toml`
  + `--check`-gated, no orphan `@generated` files, and no NEW silent-default `match`
  over a closed enum domain (`OpCode`/`Terminator` — the dispatch-handler-mirror-hazard
  class; down-only ratchet in `generator_manifest_baseline.json`). The generator
  manifest is THE authority for "what is a generated authority and how is it gated";
  add a row before building any new `gen_*.py`.
- `cargo test op_family` — native dispatch↔handler disjointness (drift unexpressible).

### Measure / perf (the CANONICAL source of truth — cite only these)
- `tools/perf_scoreboard.py --profile release-fast --classify` — the ONLY citable
  perf board (CPython floor, cold+warm, quiescent, provenance-gated). Non-canonical
  lanes (`bench.py`, `bench/harness.py`) self-stamp `authoritative=false`.
- `tools/perf_board.py <scoreboard.json>` — project the source board into the FIVE
  gated boards (CPython/Backend/Profile/PyPy/Codon), each its own artifact + exit
  code (doc 64 §3.2). A native win cannot hide a wasm regression. Pure consumer.
- `tools/perf_history.py <board.json>... --gate [--record]` — board-vs-history
  regression gate (doc 64 Phase 4): fails on a previously-green cell that
  regressed; only authoritative boards become baselines (Rule 2).
- `tools/check_perf_plane_gate.py` — falsifiable self-proof that the plane gate
  FAILS on a synthetic CPython-red (`repository.perf-plane-gate`; a gate that
  cannot fail is vacuous).
- `molt.metric_ratios.signed_ratio` — the SOLE ratio authority (explicit direction).
- `tools/perf_causality.py` + `tools/pass_delta_dashboard.py` — attribute a CPython-RED
  to its missing IR fact (do not re-derive perf causality by hand).
- `tools/dx_build_timer.py` — build wall-clock (prime/cold/incremental/test-lib),
  including pass-edit probes such as `inc-value_range` and `inc-gvn`. Molt
  scenarios default to ordinary CLI `--molt-measurement latency`; `attribution`
  opts into diagnostics. `--molt-edited-source` supplies an explicit edit pair.

### Audit / repr (the evidence engine)
- `runtime/molt-backend/src/bin/typed_repr_report.rs` — per-function scalar-repr +
  alloc-site audit (the substrate for the coupled Repr-trace / molt-check).
- `runtime/molt-backend/src/tir/verify_lir_repr.rs` — register-passability invariant.

### Build / run (always use these, never a raw binary)
- `python -m molt build --release ...` — the perf-gate build profile (default `dev`
  is unoptimized, NOT the perf gate). Needs `.venv/Scripts/python.exe` on this host.
- `tools/proof_queue.py` - the mandatory ledger/queue for expensive or
  contention-heavy proof lanes (Cargo, WASM/browser, benchmarks, stress,
  conformance). Run `status` first; use `exec`/`submit` with `--reason`,
  `--resource-family`, `--contention-key`, `--scope`, and `--note`; cite the
  log/evidence. Append-only notes and acyclic proof DAG edges are exported in
  evidence JSON and generated notebooks; use `--depends-on` for scheduled proof
  dependencies and `link` for post-submit lineage.
  Proof commands using `uv run` must use
  `uv run --active --project . --python 3.12 ...`; the queue rejects non-active
  `uv run` because throwaway env creation is false work. Queue sessions are
  contention-key-scoped, so serialized lanes reuse Cargo/uv caches while
  independent keys stay isolated.
  Pact witness lanes are named presets here: `pact-witness-acceptance` owns the
  heavy Kernel A native/WASM aperture with required `--target native|wasm`
  and portable acceptance receipts, and `pact-witness-oracle` owns the
  regenerated fixture/reference parity sanity check.
- `tools/safe_run.py --rss-mb N --timeout S -- <binary>` — CLI adapter for the
  shared RSS/walltime guard. Python measurement consumers use its typed
  `_run_measured_command` result directly, avoiding wrapper processes and stderr
  metadata parsing. Both paths require complete descendant closure.
- `tools/linux_vm.py check|push|release|render|apply` — Molt's Lima Linux VMs
  on a macOS host. `tools/linux_vm.toml` is their one definition, which
  mounts no host path: `check` reports drift (exit 1), `push` sends a commit
  into a guest clone over Lima's SSH under `refs/heads/host/`, `release` frees
  the host files a VM that still mounts host paths pins, and `apply` installs
  a definition and restarts only an idle guest, starting it as a service
  launch that Lima's hostagent outlives. Lane retirement releases first.
- `tests/molt_diff.py <files> --jobs 1` — the CPython differential (serial until the
  collective-budget-pool lands; parallel gives false FAILs). `--target native,wasm,llvm,luau`
  (or `all`) runs the MULTI-BACKEND oracle (doc 66): every backend is compared vs CPython AND
  vs each other, so a backend-specific divergence is a FAIL, not invisible. A backend whose
  toolchain is absent is a loud `uncalibrated`, never a silent skip.
- `tools/compat/comparison.py` — the ONE CPython-parity comparison law (stdout canonicalization
  incl. pyperformance/relaxed modes, the exception-signature stderr law, the exit-code laws).
  `tests/molt_diff.py` + `tools/parity_gate.py` both import it; there is no second comparison
  implementation. Proven byte-identical to the inlined original by `tests/test_compat_comparison.py`.
- `tools/compat/backends.py` — the backend adapter registry for the multi-backend oracle
  (native/wasm/llvm/luau): one place that knows how to build+run a file on each backend.
- `tools/parity_gate.py [dir]` — lightweight 3-tier (STRICT/RELAXED/EXCLUDED) `molt run`-based
  parity gate; the tiers are now MODES of `tools/compat/comparison.py` (no duplicate law).

### Runtime safety / process custody
- `tools/memory_guard.py` / `tools/process_sentinel.py` — process custody; the
  classifier authority is `src/molt/process_model.py` (Codex/Claude/
  host-control-plane are NEVER kill targets). Never name/tree-kill.
- `molt.process_model.pid_alive(pid)` — the one pid-liveness probe. A zombie is
  not alive and the probe never reaps; on Linux every thread must have exited,
  since a leader that called `pthread_exit` reads `Z` while other threads run.
  The process samplers drop exited, unreaped processes by the same rule on
  Linux and Darwin. Windows waits on the process object.
  Unknown native query or handle-close errors raise; callers cannot treat
  an unknown observation as proven death. An invalid pid returns False, so a
  caller that acts only on a proven exit checks `valid_pid(pid)` first (the
  one pid-range rule: a positive `int` within `pid_t`, or within a `DWORD`
  through `windows_process_api.valid_windows_pid`, which the Windows process
  queries share).
  Never probe with `os.kill(pid, 0)`: on Windows signal 0 is `CTRL_C_EVENT`.
- `tools/memory_guard_custody.py --json` — inspect guard-marker custody; dry-run
  by default. `--apply` reconciles records whose processes are proven gone and
  retires resolved records into durable `retired/` history; `--release
  MARKER --apply` resolves an inconclusive record after operator review.
  `tools/memory_guard_core/active_custody.py` owns parsing, locked publication,
  reconciliation, retirement and terminal projection for the producer, disk
  guard, pytest bootstrap, proof-queue diagnostics, and final runtime-WASM
  preflight. It never signals processes; PID-only parent reports confer no
  authority.

### Durable design authorities (read before re-designing)
- `docs/design/meta_bug_taxonomy.md` — the meta-bug fix queue (proxy-measurement class).
- `docs/design/foundation/` — the 100-year executable plans (21a-e, 54-67).
- The memory index (`MEMORY.md`) — load-bearing facts not derivable from a quick read.

## The discoverability contract
A tool is "real" only when it is (1) **wired** (run by a proof-plan row or
landing gate, a workflow, or the build pipeline), (2) **fast** (under its tier
wall-clock budget), and (3) **registered here**. No gate enforces registration
yet: a high-signal `tools/*.py` missing from this catalog is a defect to fix here.
