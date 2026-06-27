# Operations Guide

This document consolidates remote access, logging, progress reporting, and
multi-agent workflow rules.

## Version Policy
Molt targets **Python 3.12+** semantics only. Do not spend effort on <=3.11
compatibility. If 3.12/3.13/3.14 differ, document the chosen target in specs/tests.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see [docs/spec/STATUS.md](docs/spec/STATUS.md)).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **Windows Codex stall timing**: when Codex appears stalled around a long Molt proof lane, run `uv run --python 3.12 python tools/agent_coordination.py codex-stall -- <proof-command>`. The diagnostic mirrors child stdout/stderr live but writes only timing metadata to `logs/agents/codex_stall/*.json`: first-output gaps, per-stream idle spans, byte/chunk counts, elapsed time, and return code. It does not read, delete, repair, or rewrite Codex state, and it launches the proof command through `tools/memory_guard.py` by default; pass `--no-memory-guard` only when the direct child is already intentionally guarded or the command is a non-proof probe.
- **Windows process-table sampling budget**: process-sentinel cleanup uses a parent-enforced Windows snapshot helper with `MOLT_WINDOWS_PROCESS_SNAPSHOT_TIMEOUT_SEC` defaulting to 5 seconds. If sampling exceeds the budget, the helper is terminated and the snapshot fails closed to an empty process table so repo-scoped kill decisions are not made from partial host-control-plane evidence. Hot memory-guard RSS polling keeps the in-process sampler plus the same fail-closed deadline to avoid spawning a Python helper every poll. Set the variable to a positive float to tune the budget; set `0`/`off` only for an intentional local investigation where an unbounded process-table read is acceptable.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.
- **Node PATH drift (macOS shells)**: some shells resolve `/usr/local/bin/node` (for example Node 14) before modern Homebrew/fnm nodes. Set `MOLT_NODE_BIN=/opt/homebrew/bin/node` (or another Node >= 18 path) for deterministic wasm lanes.
- **Node wasm instability/noise**: use deterministic Node wasm flags (`--no-warnings --no-wasm-tier-up --no-wasm-dynamic-tiering --wasm-num-compilation-tasks=1`) to avoid warning-noise diffs and post-run Zone OOM incidents on large linked modules. Linked Node parity runners disable the inherited direct-child virtual-address clamp with `MOLT_WASM_TEST_CHILD_RLIMIT_GB=0` because V8 reserves large address ranges that are not RSS pressure; recursive RSS/tree/global guard limits remain authoritative for real memory pressure.

## Differential Suite (Operational Controls)
- **Memory profiling**: per-test RSS metrics are collected by default. Set `MOLT_DIFF_MEASURE_RSS=0` only for a deliberately lighter local investigation.
- **Build profile default**: diff harness defaults to `--build-profile dev` (override with `--build-profile release` or `MOLT_DIFF_BUILD_PROFILE=release` for release validation).
- **Summary sidecar**: `MOLT_DIFF_ROOT/summary.json` (or `MOLT_DIFF_SUMMARY=<path>`) records jobs, limits, and RSS aggregates.
- **Failure queue**: failed tests are written to `MOLT_DIFF_ROOT/failures.txt` (override with `MOLT_DIFF_FAILURES` or `--failures-output`).
- **OOM retry**: OOM failures retry once with `--jobs 1` by default (`MOLT_DIFF_RETRY_OOM=0` disables).
- **Memory caps**: per-process OS rlimits default to the same adaptive per-process budget as the RSS guard where the host OS can enforce them; fast-start RSS polling and POSIX child `rusage` checks cover platforms that cannot lower process virtual-memory limits enough to be useful. Override with `MOLT_DIFF_CHILD_RLIMIT_GB`, legacy `MOLT_DIFF_RLIMIT_GB`/`MOLT_DIFF_RLIMIT_MB`, or disable with `MOLT_DIFF_CHILD_RLIMIT_GB=0` only for an explicit local investigation.
- **In-harness memory guard**: always active for differential runs; `MOLT_DIFF_MEMORY_GUARD=0` and `MOLT_MEMORY_GUARD=0` are ignored for this harness so conformance work cannot accidentally bypass custody. It is layered on top of OS rlimits and delegates subprocess execution plus suite-wide process-group custody to `tools/harness_memory_guard.py`. It enforces per-process, per-test-tree, and global active diff RSS ceilings with `MOLT_DIFF_MAX_PROCESS_RSS_GB`, `MOLT_DIFF_MAX_TREE_RSS_GB`, `MOLT_DIFF_GLOBAL_RSS_LIMIT_GB`, and `MOLT_DIFF_MEMORY_GUARD_POLL_SEC`; unset values use the same shared fallback names as the other harnesses (`MOLT_MAX_PROCESS_RSS_GB`, `MOLT_MAX_TOTAL_RSS_GB`, `MOLT_MAX_GLOBAL_RSS_GB`, `MOLT_MEMORY_GUARD_POLL_SEC`). The inherited OS limit is controlled separately by `MOLT_DIFF_CHILD_RLIMIT_GB` or shared `MOLT_CHILD_RLIMIT_GB` and defaults to the live adaptive per-process budget, clamped by the active tree/global budgets. Default caps are adaptive from live available memory (Linux `MemAvailable`; macOS `vm_stat` free/inactive/speculative/purgeable pages) and physical RAM, preserving a small reserve for the OS and other workloads while scaling up on large-memory hosts and keeping the reserve floor small enough for hosted runners. Adaptive caps are refreshed in the shared subprocess guard and the suite-level repo sentinel; the already-observed guarded process-tree RSS is accounted back into the live available-memory budget so Molt can use its allocation without self-tightening while still scaling down when unrelated environment pressure appears. Once a child or grandchild is observed, the shared guard remembers its PID and process group so reparented or new-session descendants remain part of per-tree and global RSS accounting. Guard trips terminate tracked process groups, write `MOLT_DIFF_ROOT/memory_guard/tripped.json`, append bounded event/sample telemetry under `MOLT_DIFF_ROOT/memory_guard/`, and force a nonzero summary with `<memory_guard>` in `failed_files`. Diff auto-parallelism uses a separate adaptive scheduler budget (`MOLT_DIFF_MEM_PER_JOB_GB` override) instead of treating the worst-case process-tree kill ceiling as one job, so large-memory workstations can scale job count while cumulative RSS remains governed by the shared suite sentinel.
- **Memory-guard live stream**: `python3 tools/memory_guard_stream.py --diff-root "$MOLT_DIFF_ROOT"` tails the guard's compact NDJSON stream without running its own process sampler. For zero extra viewer process, set `MOLT_DIFF_MEMORY_GUARD_STREAM=stderr` (or `stdout`, `json-stderr`, `json-stdout`) before launching `tests/molt_diff.py`. Telemetry is bounded by default (`MOLT_DIFF_MEMORY_GUARD_MAX_EVENT_MB=1`, `MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB=2`, one rotated sibling per stream) and sample writes are rate-limited with `MOLT_DIFF_MEMORY_GUARD_SAMPLE_INTERVAL_SEC` (default 1s). Set `MOLT_DIFF_MEMORY_GUARD_WRITE_SAMPLES=0` to keep only trip/event evidence plus the live stream.
- **Standalone guarded commands**: use `python3 tools/memory_guard.py --max-rss-gb <gb> --max-total-rss-gb <gb> --stream stderr -- <command...>` for arbitrary local commands. The guard wraps the direct child in an OS resource-limit launcher by default (`--child-rlimit-gb`) using an adaptive virtual-memory clamp separate from the RSS budgets. Virtual-memory-heavy runtimes such as Node/WASM may need an explicit child rlimit above the RSS budgets or `0` to disable that layer; RSS remains bounded by recursive polling. Recursive polling is stateful for accounting: descendants observed before reparenting or session changes continue to be tracked for per-tree/global RSS. Before RSS aggregation or teardown, the low-level guard excludes the current ancestor chain plus Claude, Codex/app-server/renderer/node-repl, and other protected host/control-plane process groups. Teardown stays deliberately narrower: the guard terminates the private root process group created for the direct child, then terminates exact tracked escaped PIDs by PID, never protected ancestor/control-plane groups or child-reported process groups that might also contain Claude, Codex, or other host processes; the raw process-group terminator re-samples protected PGIDs immediately before SIGTERM as the final actuator-side refusal. RSS trips, timeouts, signal exits, and orphan cleanup emit incident-only repro context with command, cwd, limits, selected safe env knobs, pytest identity, optional file-backed pytest current-node snapshot, guard process IDs, sampled parent lineage, and bounded host-control-plane PGID/process samples. `--stream` emits live samples without creating artifacts; `--samples-jsonl <path>` writes bounded samples and rotates once by default (`--samples-max-mb`, default 2 MB).
- **Guarded Cargo interruption custody**: canonical dev/CI/DX environments default `CARGO_INCREMENTAL=0`, while an explicit operator-provided value remains authoritative for local incremental-debug work. When the standalone/shared memory guard observes Cargo/rustc/rustdoc and the guarded command is interrupted by an RSS trip, timeout, termination-settle failure, orphan cleanup, or signal exit, it quarantines only `*/incremental` directories under the effective `CARGO_TARGET_DIR` into `target/.molt_state/quarantine/cargo_incremental/<receipt>/`, writes a bounded receipt when state is moved, includes the receipt in summary JSON and stderr, and prunes old quarantine roots by default. Quarantine receipts are ignored `target/` evidence: they survive ordinary guard retention until pruned, but explicit `molt clean --apply` removes them with the rest of the allowlisted target artifacts. Ordinary Cargo compile failures do not trigger cleanup, and the guard never deletes the whole target directory as a failure side effect.
- **Shared harness guard**: benchmark, profiling, `molt build` Cargo backend/runtime builds, extension C compile/link, post-link optimizers, `molt bench`, `molt profile`, `molt test`, `molt validate`, `molt harness` quality layers, direct CLI run/compare/parity child execution, conformance, regrtest, compliance, CLI tests, native compile/run tests, dev-test runner subprocesses, artifact cleanup helpers, surface-probe tests, property, mutation, runtime-compat, Rust-backend, formal-methods, compile-progress, codegen-quality analysis, wasm-link external tools, wasm test helpers, cross-platform smoke, and continuous-test harnesses use the default-on shared guard in `tools/harness_memory_guard.py`. Direct pytest entrypoints are also guarded before collection by root `sitecustomize.py`, the packaged `molt.pytest_memory_guard_bootstrap` entry point, and the repo-configured `molt.pytest_memory_guard_config_plugin` fallback; disabling those guard plugins via argv or `PYTEST_ADDOPTS`, `--noconftest`, unsafe `--confcutdir`, unsafe pytest `-c`, or `PYTEST_DISABLE_PLUGIN_AUTOLOAD` without the explicit repo guard config plugin fails closed before tests run. Direct `tests/**.py` scripts and `python -m tests.*` module launches re-exec through the same guard via path-local `tests/*/sitecustomize.py` routers plus project `src/sitecustomize.py` for `uv run`/editable project interpreters, without adding harness files inside differential corpus directories. Re-execed pytest and parent-launched test commands install canonical `MOLT_PYTEST_CURRENT_TEST_FILE` custody under `tmp/pytest-memory-guard/`; serial pytest writes the bounded active-node JSON there, while xdist workers write bounded per-worker sidecars under the aggregate file's `.d/` sibling so workers cannot overwrite each other. Parent-side guard repro payloads reject noncanonical current-test paths, include all bounded worker records, and mark the record whose pid lineage matches the violating process when live samples can prove it. `tests/conftest.py` owns the final live-ancestor assertion plus env/plugin validation, path/session/sentinel setup after custody is already established. `tools/check_subprocess_guard_coverage.py` is part of the lint and smoke-validation surfaces and fails if new raw subprocess calls appear outside documented guard internals, bounded metadata probes, or interactive Popen paths with explicit process custody. The shared limits are `*_MAX_PROCESS_RSS_GB`, `*_MAX_TOTAL_RSS_GB`/`*_MAX_TREE_RSS_GB`, `*_MAX_GLOBAL_RSS_GB`/`*_GLOBAL_RSS_LIMIT_GB`, `*_CHILD_RLIMIT_GB`/`*_MAX_CHILD_RLIMIT_GB`, and `*_MEMORY_GUARD_POLL_SEC`; unset family-specific values fall back to `MOLT_MAX_PROCESS_RSS_GB`, `MOLT_MAX_TOTAL_RSS_GB`, `MOLT_MAX_GLOBAL_RSS_GB`, `MOLT_CHILD_RLIMIT_GB`, and `MOLT_MEMORY_GUARD_POLL_SEC` with adaptive live-memory defaults, a 100 ms steady poll, a 20 ms fast-start poll window for allocator spikes, and POSIX post-exit `rusage` classification for short-lived allocation spikes. Guarded environments also install canonical repo-local artifact/cache roots and set `MOLT_SESSION_ID=guard-<pid>` when the caller has not supplied one, preserving caller-provided session ids for multi-agent isolation. Adaptive RSS defaults refresh during the run, so large-memory workstations scale up automatically and active suites scale down under competing memory pressure while preserving the OS/workload reserve; implausibly large explicit RSS overrides are still capped by that live adaptive budget instead of bypassing the reserve. In Codex-interactive shells (`CODEX_SHELL=1` or a Codex origin marker), unset dynamic shared-guard defaults are capped at 18 GB per process, 24 GB per process tree, and 36 GB global so backend proof work trips Molt-owned groups before starving the Codex control plane; explicit RSS overrides remain operator authority and are still subject to the shared hard/live clamps. Test-family adapters and `tools/guarded_exec.py` also honor `*_TIMEOUT_SEC` or `MOLT_TEST_PROCESS_TIMEOUT_SEC` before their bounded defaults; streamed guarded commands emit keepalive lines every `*_KEEPALIVE_SEC`/`*_KEEPALIVE_SECS` or `MOLT_SUBPROCESS_KEEPALIVE_SECS` seconds by default so long silent CI work stays observable; set the timeout env to `0` only for an intentionally unbounded local investigation. Harness-launched process trees default the direct-child virtual-memory ceiling to the live per-process RSS budget, capped by the tree/global budgets, plus recursive process-tree RSS polling. Explicit child-rlimit settings are treated as virtual-address-space policy and are honored up to the hard child-rlimit cap so Node/V8-style WASM runtimes can reserve address space while the RSS guard remains authoritative. The shared low-level guard uses the same persistent lineage tracker as differential runs, so observed recursive descendants stay covered across ppid, process group, and session changes for accounting, while protected ancestor plus Claude/Codex/app-server/renderer/node-repl control-plane groups are excluded before RSS aggregation and teardown; teardown remains scoped to the guarded root process group plus exact tracked escaped PIDs. Automatic repo sentinels scope violation/drain kill sets to the guarded current process tree, while explicit stale-preflight and `tools/process_sentinel.py` cleanup remain repo-scoped operator actions. Sentinels reuse the low-level guard's host-control-plane token authority, exclude ancestor plus Claude/Codex app/control-plane groups before kill decisions, and record skipped protected groups once per sentinel as `repo_process_guard_protected_host_group` with pids, command, guard start, observation time, and action. RSS trips, timeouts, signal exits, orphan cleanup, and termination-settle failures append operator-facing diagnostics with incident time, elapsed duration, cleanup scope, reason, bounded repro context, and the next corrective action; repo-sentinel trip, drain, and stale-preflight JSONL events also carry `process_samples`, `external_parent_pids`, resolved limits, kill scope, killer/victim attribution, SIGTERM/SIGKILL metadata, and the same bounded repro context. The post-timeout/process-trip settle wait is bounded by `MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC` so CI reports the stuck process state instead of waiting forever after SIGTERM/SIGKILL. Before each standalone guarded subprocess starts, the guard conservatively drains only repo-scoped orphaned Molt process groups older than `*_STALE_ORPHAN_SEC`/`MOLT_STALE_ORPHAN_SEC` (default 3600 seconds) or orphaned pytest-style groups older than `*_STALE_PYTEST_SEC`/`MOLT_STALE_PYTEST_SEC` (default 900 seconds); set `*_STALE_ORPHAN_CLEANUP=0` or `MOLT_STALE_ORPHAN_CLEANUP=0` to disable that preflight for a deliberate investigation. The stale preflight records process age from `ps etimes`, kill time, reason, pids, command, external parents, process samples, repro, and next action in stderr plus `tmp/harness_memory_guard/memory_guard/*_stale_preflight.jsonl`. Every shared guarded subprocess also starts a short-lived repo-scoped sentinel when no suite-level sentinel is already active, so benchmark/conformance/regrtest-adjacent helper tools inherit cumulative Molt-process RSS protection by default. Standalone guarded subprocesses clean their own tracked descendants at parent exit; when a suite-level sentinel is active, per-command orphan cleanup is deferred to that sentinel so intentional warm daemons remain available during the run and are drained at scope exit. Long-running suites should use `guarded_harness_scope` or `repo_process_sentinel` so one sentinel spans the whole run. The sentinel snapshots pre-existing groups on entry and drains only newly spawned Molt groups on exit, so warm daemons remain available during the run but cannot accumulate after successful benchmarks, conformance, regrtest, or pytest sessions. Benchmark timing reads the guard's child pre-`exec` elapsed time, so Python guard startup is not charged to Molt/CPython/WASM runtime samples.
- **Current-tree guard ownership**: repo-sentinel current-tree ownership survives reparenting only while live parent lineage or repo/Molt command identity still proves the process group belongs to the guarded launch. Numeric process-group reuse alone is not ownership proof; a later host process that inherits an old PGID must be excluded from drain and violation kill sets, and stale-preflight cleanup must ignore orphaned host processes that lack Molt/repo command identity. The standalone `tools/process_sentinel.py` terminator also re-samples ancestor plus Claude/Codex/app-server/renderer/node-repl protected PGIDs before signaling, so direct cleanup commands share the same final kill refusal.
- **Process cleanup custody**: cleanup authority is Molt-owned process identity, not repo path, process name, stale PID, parent shell, or Codex/Claude ancestry. Only live-proved Molt build/test/bench workers, backend daemons, runtime children, and guard-owned process groups may be signaled. Codex, Claude, app-server, renderer, node-repl, MCP/plugin helpers, shell hosts, Git pollers, ancestors, and other host-control-plane processes are never cleanup targets; ambiguous ownership must skip and preserve evidence.
- **Guarded-command hotspot profile**: shared guarded subprocesses write structured `guarded_command_profile` JSONL events only for incidents by default, keeping routine successful test/build runs clean. Incident events include prefix, session id, command, cwd, return code, elapsed time, memory-guard limits, peak RSS when sampled, violation/timeout/signal/orphan status, GitHub Actions context when present, and a bounded repro payload with pytest/Codex/Molt env hints, parent-process lineage, and host-control-plane process topology. Set `MOLT_GUARD_PROFILE=all` or provide `MOLT_GUARD_PROFILE_LOG=<path>` for an intentional all-command capture; set `MOLT_GUARD_PROFILE=0` to disable profile writes entirely. The log rotates to `commands.jsonl.1` at 16 MB by default (`MOLT_GUARD_PROFILE_MAX_MB`, set `<=0` only for an intentional unbounded local capture). `tools/guarded_exec.py` also prints the elapsed time and profile path for each wrapped CI/dev command. Use `MOLT_GUARD_PROFILE_LOG=<path>` to move this log to another canonical artifact path for a specific run, and use `python3 tools/profile_hotspots.py --limit 20` to summarize slowest events and grouped command families when all-command profiling is enabled.
- **Repo-sentinel incident schema**: shared harness repo-sentinel JSONL events distinguish termination from observation. Claimed kills carry `claim_status=claimed`, `termination.attempted=true`, `killed_at`, and `killer_*`; PGIDs already claimed by another guard carry `claim_status=already_claimed`, `termination.attempted=false`, `observed_at`, and `observer_*` so diagnostics do not falsely attribute a kill to the wrong guard.
- **Output startup and binary-size audit**: use `python3 tools/output_startup_size_audit.py --targets all --build-profiles dev,release --backends all --samples 5` when startup or output-size regressions are suspected. The audit builds the same hello-world probe through the normal CLI for native, linked-WASM, Luau, and MLIR targets, records artifact bytes for each target/profile/backend row, measures same-path and fresh-path startup wherever a canonical runner exists, and writes a JSON artifact to `bench/results/output_startup_size_audit_<timestamp>.json`. Native fresh-path copies expose dyld/code-signature fixed costs; linked-WASM rows run under deterministic Node flags when Node is available; Luau/MLIR rows keep size custody and record an explicit skipped-runner reason when local startup is not canonical. Pass `--max-artifact-mb` and `--max-fresh-start-ms` only when intentionally enforcing a budget; otherwise the audit is observational so current regressions can be measured before setting the ratchet.
- **Adaptive resource pressure plan**: `tools/resource_pressure.py` is the shared policy layer above the memory guard. CI (`tools/ci_resource_env.py`), compile-slot custody (`tools/compile_governor.py`), and differential auto-parallelism (`tests/molt_diff.py`) all consume the same `molt.resource_pressure.v1` JSON-shaped plan so selected Cargo jobs, compile slots, active-process caps, diff jobs, memory source, reserve, pressure level, and rationale are explainable from one contract. Existing family-specific env overrides still win, but unset defaults are derived from live adaptive memory and CPU capacity instead of fixed host assumptions. Use `python3 tools/ci_resource_env.py --dry-run --json` to print the current hosted-runner/dev-runner policy without mutating `$GITHUB_ENV`.
- **Runtime resource tracker**: manifest/env limits (`MOLT_RESOURCE_MAX_MEMORY`, `MOLT_RESOURCE_MAX_DURATION_MS`, `MOLT_RESOURCE_MAX_ALLOCATIONS`, `MOLT_RESOURCE_MAX_RECURSION_DEPTH`) install a runtime-wide tracker for the current thread and future worker threads. Core object allocations, object-owned Vec backing stores (lists/tuples/dicts/sets/bytearrays, builders, dataclass fields, memoryview shape/stride metadata, map/zip iterator vectors), scope arenas, JSON parse temp arenas, and WASM scratch buffers reserve budget before touching the allocator and roll reservations back on allocator failure. Vec growth routes through tracked reserve hooks; teardown uses best-effort tracker release paths so Rust TLS destructor order cannot abort process cleanup. WASM split-runtime hosts must provide real `molt_resource_on_allocate` and `molt_resource_on_free` exports; missing resource hooks fail export validation instead of installing no-op host fallbacks.
- **Runtime child resource envelope**: runtime process-spawn intrinsics propagate the active `MOLT_RESOURCE_MAX_*` limits into children and clamp explicit child env values so nested Molt children may tighten but not widen the parent envelope. On Unix, runtime child spawns also install a pre-`exec` memory rlimit from the tighter of `MOLT_RESOURCE_MAX_MEMORY`, `MOLT_CHILD_RLIMIT_BYTES`, or `MOLT_CHILD_RLIMIT_GB` unless the shared child rlimit is explicitly set to `0` for an intentional unbounded investigation. Runtime byte streams enforce queued-byte backpressure (`MOLT_STREAM_MAX_QUEUED_BYTES`, or `MOLT_PROCESS_PIPE_MAX_QUEUED_BYTES` for process pipes) so subprocess stdout/stderr readers block instead of accumulating unbounded pipe buffers in the parent.
- **Out-of-band process sentinel**: use `python3 tools/process_sentinel.py --once --kill-all` to stop live-proved repo-scoped Molt build/test/bench process groups from prior sessions, or `python3 tools/process_sentinel.py --kill-all --until-clean-sec 30 --max-runtime-sec 120` to drain delayed stale Molt launches until the repo has stayed quiet. The clean-window mode performs a final scan before success and processes any newly observed delayed Molt launch instead of returning on stale quiet-state. For conservative hygiene, use `python3 tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900`, which kills only matched Molt-owned groups whose external parent is init/launchd and whose `ps etimes` age exceeds the selected threshold. It streams to stderr/stdout only, writes no artifacts by default, enforces per-process/per-group/global ceilings, reports action, reason, kill scope, killer/victim attribution, SIGTERM/SIGKILL metadata, `killed_at`/`observed_at`, elapsed sentinel time, process age when available, pids, external parents, sampled process rows, bounded repro context, and next action, and uses the same adaptive live-memory budget unless overridden with `--max-global-rss-gb`. Use `--json` when a parent process needs NDJSON incident payloads instead of human text.
- **Ignored artifact cleanup**: use `molt clean`, `python3 tools/artifact_cleanup.py`, or `python3 tools/dev.py clean-artifacts` for a dry-run of canonical ignored build/cache/tmp/log/result cleanup, including friend-suite checkout caches under `bench/friends/repos/`. Add `--apply` to delete and `--kill-processes` to run the process sentinel before deletion; that sentinel pass may drain only live-proved Molt-owned workers and is never Codex, Claude, app-server, renderer, node-repl, shell, Git, MCP/plugin, or host-control-plane cleanup. In JSON mode, cleanup runs the sentinel with `--json` and embeds parsed `sentinel_events` in the cleanup report; raw sentinel stdout/stderr are included only with `--verbose`. All three entrypoints route through `tools/artifact_cleanup.py`, and both the optional sentinel pass and the `git clean -X` pass run through the default-on `MOLT_DEV_CLEANUP` memory guard. The cleanup pathspec is allowlisted, so tracked files, dirty partner work, `.venv/`, `.omx/`, `third_party/`, fuzz corpora, and test corpora are not default cleanup targets; Cargo incremental quarantine receipts under `target/.molt_state/quarantine/cargo_incremental/` are target artifacts and are intentionally removed by explicit apply-mode cleanup.
- **Wrapper policy**: diff runs disable `RUSTC_WRAPPER`/`sccache` by default for portability. Opt in with `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1` on hosts where wrapper caches are known-good.
- **Pass-log pruning**: when `--log-dir` is enabled, per-test logs for passing tests are pruned by default to reduce clutter. Set `MOLT_DIFF_LOG_PASSES=1` to keep pass logs.
- **Backend daemon policy**: diff runs use `MOLT_DIFF_BACKEND_DAEMON` when set; otherwise defaults to platform-safe auto (`0` on macOS, `1` elsewhere).
- **Backend daemon identity custody**: backend daemon sidecars under `target/.molt_state/backend_daemon/` are JSON identity records (`*.identity.json`) with pid, socket path, project root, cargo profile, config digest, backend binary, and command snapshot. `src/molt/backend_daemon_custody.py` is the shared authority for parsing these sidecars, matching process-table commands, accepting socket-health probes, and issuing verified termination/escalation. A PID or explicit socket env alone never authorizes `SIGTERM`/`SIGKILL`: CLI restart/stale-cleanup paths, `tests/molt_diff.py`, `tools/bench.py`, `tools/bench_wasm.py`, and `tools/bench_individual.py --isolate-daemon` verify the recorded identity first, then revalidate before escalation. Native/WASM benchmark pruning canonicalizes `MOLT_SESSION_ID` before cleanup and terminates only current-session identity records, so concurrent warm daemon/cache state survives. `tools/compile_progress.py` only cleans marker-scoped compiler children and never sweeps backend daemons. Legacy raw `*.pid` files are cleanup debris only and are removed without signaling. `tools/verify_native_binary_valid.sh` builds daemon-off and no longer performs blanket daemon `pkill`.
- **dyld hardening**: after a dyld import-format incident, diff runs force `MOLT_BACKEND_DAEMON=0` for safety. Set `MOLT_DIFF_QUARANTINE_ON_DYLD=1` only when you need cold target/state quarantine.
- **Local dyld fallback**: on macOS, dyld retries/quarantine can route to local `/tmp` lanes (`MOLT_DIFF_DYLD_LOCAL_FALLBACK=1`, default). Override root with `MOLT_DIFF_DYLD_LOCAL_ROOT=<abs path>`.
- **Cache hardening**: set `MOLT_DIFF_FORCE_NO_CACHE=1|0` to force/disable `--no-cache`. Default is cache-enabled on all platforms; dyld guard/retry can force no-cache for the current incident-scoped run.
- **Batch compile server cooldown**: diff workers now require repeated consecutive batch-server failures before entering cooldown. Tune with `MOLT_DIFF_BATCH_COMPILE_SERVER_DISABLE_AFTER_FAILURES=<n>` and `MOLT_DIFF_BATCH_COMPILE_SERVER_DISABLE_COOLDOWN_SEC=<seconds>`.
- **Shared target pinning (macOS)**: explicitly set both `CARGO_TARGET_DIR` and `MOLT_DIFF_CARGO_TARGET_DIR` to the same shared path for diff runs so workers do not drift onto ad-hoc/default targets and duplicate rebuilds.
- **Interrupted-run cleanup**: before a new long sweep, clear stale Molt-owned harness workers through the custody-aware sentinel (`tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900`). Do not use raw PID, name, process-group, parent-chain, `pkill`, `killall`, `taskkill`, or `Stop-Process` cleanup for diff recovery; ambiguous ownership must skip and leave evidence. Keep one supervising diff run per shared target.

## Debug Commands

Use the canonical `molt debug` surface for retained debug artifacts and
machine-readable summaries.

Currently wired commands:

- `molt debug repro <source.py> [--compare]`
- `molt debug ir <source.py> --stage pre-midend|post-midend|all`
- `molt debug verify`
- `molt debug trace <source.py> [--family callargs|call_bind_ic|function_bind_meta|backend_timing|compile_func] [--assert-no-pending-on-success]`
- `molt debug reduce <source.py|manifest.json> --oracle-json <oracle> [--eval-command <cmd>] [--eval-timeout <sec>]`
- `molt debug bisect <source.py|manifest.json> --passes <a,b,c> --oracle-json <oracle> --eval-command <cmd>`
- `molt debug diff <summary.json> [--failure-queue <failures.txt>]`
- `molt debug perf <profile.json|profile.log>...`

Current contract:

- manifests are written under `tmp/debug/` by default;
- retained outputs use `logs/debug/` when `--out` is provided;
- `--format json` emits a stable summary payload suitable for automation;
- unsupported or invalid requests must fail via explicit structured payloads,
  not silent no-ops.

Version and platform rules:

- treat Python version as an explicit dimension (`py312`/`py313`/`py314`) when
  behavior differs;
- treat host/process features as capability-gated, not assumed;
- do not silently inherit POSIX-only behavior for process control, timeouts, or
  profiling support.

Current status note:

- `molt debug trace` now wraps the currently wired call-bind/backend trace families
  (`callargs`, `call_bind_ic`, `function_bind_meta`, `backend_timing`, `compile_func`) and records the effective
  low-level trace env knobs in the manifest;
- `backend_timing` enables the existing backend compile-time timing surface
  (`MOLT_BACKEND_TIMING=1`) through the same canonical trace command;
- `compile_func` enables the per-function backend compile trace surface
  (`MOLT_TRACE_COMPILE_FUNC=1`) through the same canonical trace command;
- `--assert-no-pending-on-success` enables the central
  `MOLT_ASSERT_NO_PENDING_ON_SUCCESS=1` trap in the clean runtime call core;
- wider runtime assertion rollout beyond that central call core is still under
  active build-out;
- `molt debug reduce` and `molt debug bisect` now accept canonical oracles plus
  evaluator commands, explicit evaluator timeouts, and manifest-backed
  reduction/bisection evidence;
- contested runtime work, especially around call-bind ownership, must not be
  forced through while partner changes are active.

## Linear Workspace Hygiene
- Refresh the repo-backed local Linear artifacts from current TODO contracts with `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .` to inspect drift.
- Apply the refreshed local seed/manifests/index with `python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root . --apply`.
- `refresh-local-artifacts` is codebase-canonical by default: it harvests structured `TODO(...)` contracts from `src/`, `runtime/`, `tools/`, `tests/`, `formal/`, and `demo/`, materializes the full deduped leaf inventory into `ops/linear/seed_backlog.json`, then reduces that inventory into grouped/category manifests under `ops/linear/manifests/*.json` and updates counts in `ops/linear/manifests/index.json`.
- Use `--source-mode all` only for audits that explicitly want stale doc signal mixed in; the default `codebase` mode is the source of truth for prioritization and live sync.
- The grouped manifests are the live-sync source of truth. They preserve strong urgency via worst-leaf Linear priority plus explicit impact/pressure rollups in the issue title/description/metadata, while keeping non-code signal secondary.
- The local artifact contract is now two-layer:
  - `ops/linear/seed_backlog.json`: full normalized leaf inventory for auditability.
  - `ops/linear/manifests/*.json`: grouped/category issues sized for the live Linear workspace.

## On-Call Runbook (Stdlib Intrinsics Gates)
Use this section when `python3 tools/check_stdlib_intrinsics.py` fails in CI or
on-call triage.

### Standard First Commands
Run these first to establish current state:
```bash
python3 tools/sync_stdlib_top_level_stubs.py
python3 tools/sync_stdlib_submodule_stubs.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
cat tools/stdlib_intrinsics_ratchet.json
python3 tools/check_stdlib_intrinsics.py --update-doc
```

### WASM Runner Quick Triage
Use these when wasm parity lanes fail before module semantics are reached:
```bash
which -a node
node -v
MOLT_NODE_BIN=/opt/homebrew/bin/node node -v
MOLT_NODE_BIN=/opt/homebrew/bin/node node -e "const m=require('node:wasi'); console.log(typeof m.WASI)"
which wasmtime
wasmtime --version
```

### WASM Runtime Artifact Failure Strings
Use these when `molt build --target wasm` fails with runtime-artifact validation errors.

`Runtime wasm build produced invalid artifact`
```bash
# Validate the current runtime artifact and detect zero-filled/corrupt output.
RUNTIME="$CARGO_TARGET_DIR/wasm32-wasip1/release/molt_runtime.wasm"
ls -l "$RUNTIME"
xxd -l 16 "$RUNTIME"
wasm-tools validate "$RUNTIME"

# Compare release vs release-fast wasm runtime outputs directly.
export TEST_ROOT=/Volumes/APDataStore/Molt/cargo-target-wasm-profile-check
export CARGO_TARGET_DIR="$TEST_ROOT/release"
cargo build --package molt-runtime --profile release --target wasm32-wasip1
xxd -l 16 "$CARGO_TARGET_DIR/wasm32-wasip1/release/molt_runtime.wasm"

export CARGO_TARGET_DIR="$TEST_ROOT/release-fast"
cargo build --package molt-runtime --profile release-fast --target wasm32-wasip1
xxd -l 16 "$CARGO_TARGET_DIR/wasm32-wasip1/release-fast/molt_runtime.wasm"
wasm-tools validate "$CARGO_TARGET_DIR/wasm32-wasip1/release-fast/molt_runtime.wasm"
```

`Runtime wasm recovery build produced invalid artifact`
```bash
# Force fallback profile lane (default fallback is release-fast).
export MOLT_WASM_RUNTIME_FALLBACK_PROFILE=release-fast
export MOLT_WASM_FORCE_CC=1
uv run --python 3.12 python -m molt.cli build --target wasm --require-linked examples/hello.py
```

### Failure String -> Copy-Paste Response
`stdlib intrinsics lint failed: stdlib top-level coverage gate violated`
```bash
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: top-level module/package duplicate mapping`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
root = Path("src/molt/stdlib")
mods = {}
pkgs = {}
for p in root.rglob("*.py"):
    rel = p.relative_to(root)
    if p.name == "__init__.py" and len(rel.parts) == 2:
        pkgs[rel.parts[0]] = str(rel)
    elif len(rel.parts) == 1 and p.suffix == ".py":
        mods[p.stem] = str(rel)
for name in sorted(set(mods) & set(pkgs)):
    print(name, mods[name], pkgs[name])
PY
```

`stdlib intrinsics lint failed: stdlib package kind gate violated`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
import runpy
base = runpy.run_path("tools/stdlib_module_union.py")
required_packages = set(base["STDLIB_PACKAGE_UNION"])
root = Path("src/molt/stdlib")
for name in sorted(required_packages):
    mod = root / f"{name}.py"
    pkg = root / name / "__init__.py"
    if mod.exists() and not pkg.exists():
        print(f"convert-to-package: {mod} -> {pkg}")
PY
```

`stdlib intrinsics lint failed: stdlib submodule coverage gate violated`
```bash
python3 tools/sync_stdlib_submodule_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: submodule/package duplicate mapping`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
root = Path("src/molt/stdlib")
mods = {}
pkgs = {}
for p in root.rglob("*.py"):
    rel = p.relative_to(root)
    if p.name == "__init__.py":
        if len(rel.parts) > 1:
            pkgs[".".join(rel.parts[:-1])] = str(rel)
    else:
        mods[".".join((*rel.parts[:-1], p.stem))] = str(rel)
for name in sorted((set(mods) & set(pkgs))):
    if "." in name:
        print(name, mods[name], pkgs[name])
PY
```

`stdlib intrinsics lint failed: stdlib subpackage kind gate violated`
```bash
uv run --python 3.12 python - <<'PY'
from pathlib import Path
import runpy
base = runpy.run_path("tools/stdlib_module_union.py")
required = set(base["STDLIB_PY_SUBPACKAGE_UNION"])
root = Path("src/molt/stdlib")
for name in sorted(required):
    parts = name.split(".")
    mod = root.joinpath(*parts[:-1], f"{parts[-1]}.py")
    pkg = root.joinpath(*parts, "__init__.py")
    if mod.exists() and not pkg.exists():
        print(f"convert-to-package: {mod} -> {pkg}")
PY
```

`stdlib intrinsics lint failed: unknown intrinsic names`
```bash
python3 tools/gen_intrinsics.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: unknown strict-import modules requested`
```bash
python3 tools/check_stdlib_intrinsics.py --allowlist-modules builtins,sys,types,importlib,importlib.machinery,importlib.util
```

`stdlib intrinsics lint failed: strict-import roots must be intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
rg -n "TODO\\(stdlib[^,]*,.*status:(missing|partial|planned|divergent)" src/molt/stdlib
```

`stdlib intrinsics lint failed: bootstrap strict roots are incomplete`
```bash
ls src/molt/stdlib/builtins.py src/molt/stdlib/sys.py src/molt/stdlib/types.py
ls src/molt/stdlib/importlib/__init__.py src/molt/stdlib/importlib/machinery.py src/molt/stdlib/importlib/util.py
```

`stdlib intrinsics lint failed: bootstrap strict closure must be intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: bootstrap modules must be intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: non-python-only modules cannot depend on python-only stdlib modules`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: strict-import allowlist violated (intrinsic-backed roots imported non-intrinsic-backed stdlib modules)`
```bash
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
```

`stdlib intrinsics lint failed: strict-import roots used forbidden fallback patterns`
```bash
rg -n "require_optional_intrinsic|load_intrinsic|except\\s+(ImportError|ModuleNotFoundError|Exception|BaseException)" src/molt/stdlib
python3 tools/check_stdlib_intrinsics.py --critical-allowlist
```

`stdlib intrinsics lint failed: intrinsic-backed modules used forbidden fallback patterns`
```bash
rg -n "require_optional_intrinsic|load_intrinsic|except\\s+(ImportError|ModuleNotFoundError|Exception|BaseException)" src/molt/stdlib
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: all-stdlib fallback gate violated`
```bash
python3 tools/check_stdlib_intrinsics.py
rg -n "require_optional_intrinsic|load_intrinsic|except\\s+(ImportError|ModuleNotFoundError|Exception|BaseException)" src/molt/stdlib
```

`stdlib intrinsics lint failed: intrinsic runtime fallback gate violated`
```bash
rg -n "except .*:\\s*$|pass$" src/molt/stdlib/json/__init__.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: zero non-intrinsic gate violated`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/sync_stdlib_submodule_stubs.py --write
```

`stdlib intrinsics lint failed: intrinsic-partial ratchet gate violated`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
cat tools/stdlib_intrinsics_ratchet.json
# Lower intrinsic-partial modules first, then tighten max_intrinsic_partial.
```

`stdlib intrinsics lint failed: full-coverage attestation references unknown modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
print("covered_count", len(covered))
for name in sorted(covered):
    print(name)
PY
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract has non-attested modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
contract = set(manifest.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {}).keys())
print("contract_without_attestation", sorted(contract - covered))
PY
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract missing modules`
```bash
python3 - <<'PY'
import runpy
manifest = runpy.run_path("tools/stdlib_full_coverage_manifest.py")
covered = set(manifest.get("STDLIB_FULLY_COVERED_MODULES", ()))
contract = set(manifest.get("STDLIB_REQUIRED_INTRINSICS_BY_MODULE", {}).keys())
print("attested_without_contract", sorted(covered - contract))
PY
```

`stdlib intrinsics lint failed: full-coverage modules must remain intrinsic-backed`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only --json-out /tmp/stdlib_intrinsics.json
python3 - <<'PY'
import json
payload = json.load(open("/tmp/stdlib_intrinsics.json"))
statuses = {entry["module"]: entry["status"] for entry in payload["modules"]}
for name in payload.get("fully_covered_modules", []):
    if statuses.get(name) != "intrinsic-backed":
        print(name, statuses.get(name))
PY
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract references unknown intrinsics`
```bash
python3 tools/gen_intrinsics.py
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: full-coverage intrinsic contract violated`
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only --json-out /tmp/stdlib_intrinsics.json
python3 - <<'PY'
import json
payload = json.load(open("/tmp/stdlib_intrinsics.json"))
print(payload.get("full_coverage_required_intrinsics", {}))
PY
```

`Host fallback imports (\`_py_*\`) are forbidden in stdlib modules.` (file-level scan error)
```bash
rg -n "import _py_|from _py_|__import__\\(|import_module\\(" src/molt/stdlib
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed: ...` (dynamic message, e.g. baseline invalid/missing)
```bash
python3 tools/gen_stdlib_module_union.py
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/sync_stdlib_submodule_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
```

`stdlib intrinsics lint failed:` (file-level scan errors block)
```bash
python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only
uv run --python 3.12 ruff format src/molt/stdlib tools/check_stdlib_intrinsics.py
```

### Differential Expected-Failure Policy (Too Dynamic)
Use this for intentionally unsupported dynamism (for example `exec`/`eval`)
that is documented by vision/break-policy constraints.

```bash
# 1) Declare the planned differential tests in:
#    tools/stdlib_full_coverage_manifest.py
#    TOO_DYNAMIC_EXPECTED_FAILURE_TESTS
#
# 2) Run differential lane as normal; harness auto-converts fail->pass as XFAIL
#    only when CPython passes and the test path is in that manifest tuple.
uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic/exec_locals_scope.py

# 3) XPASS is treated as failure; remove stale expected-failure entries when
#    Molt gains support.
```

## Build Throughput (Multi-Agent)
- **Stable cache keys**: the CLI enforces `PYTHONHASHSEED=0` by default to keep IR/cache keys deterministic across invocations.
- **Hash-seed override**: set `MOLT_HASH_SEED=<value>` to override; set `MOLT_HASH_SEED=random` to opt out.
- **Cache invalidation scope**: object-cache keys use IR payload plus runtime/backend/tooling content and source-tree metadata fingerprints. Unrelated stdlib file edits do not invalidate every cached object unless they affect generated IR for that build, but compiler/runtime/tooling source metadata changes do invalidate caches to prevent stale outputs in long-lived processes. Shared exact-key artifacts, including module/function `.o`/`.wasm` cache entries and `stdlib_shared_<key>.o` sidecars, are non-destructive during normal build/link/probe paths: mismatches skip reuse and rebuild/publish under the entry lock, while deletion belongs to explicit retention cleanup (`molt clean`, `tools/throughput_env.sh --apply`, or prune policy).
- **Persisted publication**: source-hash, artifact-sync, import-scan, module-analysis, graph, diagnostics, emitted IR, snapshot, split-runtime, validation, package/archive, vendor file, linker-support, and other persisted JSON/text/byte/file cache or sidecar writers publish through unique same-directory temp files plus atomic replace. Vendored directory tree replacement prepares a hidden temp tree and preserves the previous tree for restore-on-failure; do not describe it as a universal kernel-level atomic directory exchange until every supported OS has an exchange primitive wired. Module-analysis cache identity includes the import scan mode (`full` vs `module_init`) in both the filename key and payload schema, so different graph semantics never alias the same artifact.
- **Lock-check cache**: deterministic lock checks are cached in `target/lock_checks/` to avoid repeated `uv lock --check` and `cargo metadata --locked` on every build; lock-check JSON publishes through the same unique atomic temp-sibling writer as the other persisted cache state.
- **Concurrent rebuild suppression**: backend/runtime Cargo rebuilds acquire file locks under the resolved build-state root's `build_locks/` directory. Default `MOLT_SESSION_ID` runs use isolated `target/sessions/<id>/.molt_state/` directories, while agents that explicitly share `CARGO_TARGET_DIR` or `MOLT_BUILD_STATE_DIR` also share the same lock files and wait instead of duplicating mutable Cargo rebuilds. Deterministic proof/build environments default `CARGO_INCREMENTAL=0`, preserving explicit operator opt-in only for local incremental-debug sessions; if a guarded Cargo process is interrupted, the guard quarantines only Cargo incremental state under the target root instead of deleting shared artifacts. Same-key backend compile cache publication uses session-independent locks under the resolved cache root's `locks/compile.<key>.lock` so explicit `--cache-dir` users and agents with separate target/session roots still serialize writes to the same shared cache entry.
- **Build state override**: set `MOLT_BUILD_STATE_DIR=<path>` to pin lock/fingerprint metadata to a custom shared location; by default it lives under `<CARGO_TARGET_DIR>/.molt_state/`.
- **Benchmark build-cache policy**: `tools/bench.py` and `tools/bench_wasm.py`
  use cache-enabled Molt builds by default, preserving non-destructive
  frontend/backend/runtime cache entries across concurrent benchmark agents.
  Pass `--no-molt-build-cache` only for deliberate no-cache/cold-rebuild
  investigations.
- **sccache auto-enable**: the CLI enables `sccache` automatically when available (`MOLT_USE_SCCACHE=auto`); set `MOLT_USE_SCCACHE=0` to disable. Cargo builds now auto-retry once without `RUSTC_WRAPPER` when a wrapper-level `sccache` failure is detected.
- **Dev profile routing**: `molt ... --profile dev` maps to Cargo profile `dev-fast` by default. Override with `MOLT_DEV_CARGO_PROFILE`; use `MOLT_RELEASE_CARGO_PROFILE` for release lane overrides.
- **Runtime artifact stability**: runtime Cargo feature sets are stable for the selected stdlib profile and target. User import graphs and required WASM exports must not mutate the runtime Cargo command or fingerprint; they only affect user-code lowering, linking/export validation, and cache keys for user artifacts. WASM runtime rebuilds use Cargo `compiler-artifact` JSON as post-build provenance and never pre-delete shared candidate `.wasm`/`.a` artifacts to manufacture freshness; reusable WASM runtime sidecars also carry an `artifact_sha256` for the exact `.wasm`/`.a` bytes, and candidates without that byte digest are rebuilt instead of hydrated. A successful Cargo run that reports no matching `molt_runtime` artifact fails closed instead of reusing an old path guess.
- **Native runtime archive aliases**: Cargo still produces scratch
  `libmolt_runtime.a`, but Molt publishes and links profile-qualified aliases
  under the selected target/profile directory:
  `libmolt_runtime.stdlib_micro.a` and `libmolt_runtime.stdlib_full.a`. Cache
  freshness, daemon staleness checks, and native link inputs must use these
  aliases, including target-triple-specific aliases, so `micro` and `full`
  builds cannot overwrite or stale-check each other through the scratch name.
  The `micro` profile includes collection and filesystem/tempfile intrinsics
  because core/default-profile stdlib imports such as `collections.abc`,
  `copyreg`, `runpy`, and `tempfile` must link against a coherent intrinsic
  surface.
- **Backend daemon**: native backend compiles use a persistent daemon by
  default (`MOLT_BACKEND_DAEMON=1`) to amortize Cranelift cold-start. Tune
  startup with `MOLT_BACKEND_DAEMON_START_TIMEOUT`. The daemon enforces bounded
  request/job/cache state by default; tune with
  `MOLT_BACKEND_DAEMON_REQUEST_LIMIT_BYTES`, `MOLT_BACKEND_DAEMON_MAX_JOBS`,
  and `MOLT_BACKEND_DAEMON_CACHE_MB`.
- **Backend TIR cache memory**: `runtime/molt-backend` keeps TIR artifact bytes
  in an LRU in-memory cache while preserving the disk-backed cache index under
  `MOLT_CACHE`. The default cap is adaptive: explicit
  `MOLT_BACKEND_TIR_CACHE_MEMORY_BYTES` wins, then
  `MOLT_BACKEND_TIR_CACHE_MEMORY_MB`, then propagated memory-guard
  availability/reserve env (`MOLT_MEMORY_AVAILABLE_GB`,
  `MOLT_MEMORY_RESERVE_GB`, and `MOLT_CLI_*` aliases), otherwise total
  physical memory. Disk hits for oversized artifacts are returned without
  retaining them in RAM.
- **One-shot backend stdin limit**: non-daemon backend IR reads from stdin are
  bounded by `MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES` and default to the same
  512 MiB ceiling as daemon requests. Msgpack and NDJSON stdin paths stream
  through the limit; JSON and CBOR stdin paths reject oversized input before
  deserialization rather than buffering unbounded request bodies.
- **Backend IR lease custody**: CLI backend dispatch writes request IR as a
  JSON lease under `tmp/backend-ir-leases/` and passes `ir_path`/`--ir-file` to
  daemon and one-shot backends. The lease writer streams JSON directly to disk
  instead of materializing a second full IR byte buffer, and leases are removed
  after the compile request completes.
- **Daemon warm-hit probe path**: when cache keys are present, the build pipeline now lets the daemon send a probe-only request first and only encodes full IR after a daemon-declared miss. This preserves warm daemon hits without paying full IR encode/send cost on every run.
- **Daemon socket placement**: sockets default to a local temp dir (`MOLT_BACKEND_DAEMON_SOCKET_DIR`, or explicit `MOLT_BACKEND_DAEMON_SOCKET`) so shared/external volumes that do not support Unix sockets do not break daemon startup.
- **Daemon lifecycle**: with `MOLT_SESSION_ID` and the default target root,
  daemon logs and identity/fingerprint sidecars live under
  `target/sessions/<id>/.molt_state/backend_daemon/`; explicit
  `CARGO_TARGET_DIR` or `MOLT_BUILD_STATE_DIR` roots keep sidecars under that
  resolved root's `.molt_state/backend_daemon/`. If an agent sees daemon
  protocol/connectivity errors before a full IR compile request is sent, the
  CLI may restart the daemon once under lock for classified retryable failures.
  After full-request admission, the daemon owns the compile outcome: the client
  fails closed with daemon diagnostics and does not restart or launch a hidden
  one-shot compile that overlaps backend memory. Verified live daemon identities
  also survive short startup readiness misses, because a synchronous daemon may
  be busy compiling and must not be unlinked/rebound by a second daemon.
  Serial pytest assigns a default
  `MOLT_SESSION_ID=pytest-<pid>` when the caller has not supplied one, then the
  session guard drains daemon groups created during that pytest run at session
  finish; xdist workers use worker-scoped session ids and skip the serial
  session sentinel.
- **One-shot backend capture**: one-shot native backend compiles must capture stdout/stderr through temp-file-backed handles instead of pipe-backed `capture_output=True`. Child/grandchild toolchain processes may inherit stdio; temp-file capture avoids silent stalls where the parent waits forever on a still-open pipe after codegen has already finished.
- **Native runtime overlap**: native builds start runtime verification/build overlap after cache/setup and join it only at the true native link boundary. `emit=obj` skips that async runtime work because there is no native link step to hide it behind.
- **Bootstrap command**: `uv run --python 3.12 python -m molt.cli dx env`
  reports the same canonical DX facts on Windows, macOS, and Linux, and
  `uv run --python 3.12 python -m molt.cli dx run -- <command>` runs a command
  under those facts without shell activation. This is the maintainer/agent
  development and proof-lane bootstrap, not a public compile requirement; real
  users may compile in place, use default Molt/Cargo locations, or pass explicit
  target/output flags. The resolver configures:
  - `MOLT_EXT_ROOT=<artifact-root>` (repo-local by default, or caller-provided external root)
  - `MOLT_CACHE=$MOLT_EXT_ROOT/.molt_cache`
  - `CARGO_TARGET_DIR=$MOLT_EXT_ROOT/target`
  - `MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR` so differential runs reuse the same shared Cargo artifacts by default
  - `MOLT_DIFF_ROOT=$MOLT_EXT_ROOT/tmp/diff` and `MOLT_DIFF_TMPDIR=$MOLT_EXT_ROOT/tmp`
  - `UV_CACHE_DIR=$MOLT_EXT_ROOT/.uv-cache` and `TMPDIR=$MOLT_EXT_ROOT/tmp`
  - `SCCACHE_DIR=$MOLT_EXT_ROOT/.sccache` and `SCCACHE_CACHE_SIZE=<policy default>`
  - `MOLT_USE_SCCACHE=1`, `MOLT_DIFF_ALLOW_RUSTC_WRAPPER=1`, and `CARGO_INCREMENTAL=0` for better cross-agent cacheability
- **Artifact root policy**: throughput bootstrap now prefers `MOLT_EXT_ROOT`
  when set and otherwise uses canonical repo-local roots. Maintainer/agent
  proof lanes should use an external root when available for shared artifacts,
  larger local capacity, and Windows `C:` self-protection. Do not present that
  as a required public CLI default.
- **Cache retention**: the DX resolver emits `MOLT_CACHE_MAX_GB` and
  `MOLT_CACHE_MAX_AGE_DAYS`; the POSIX compatibility wrapper
  `tools/throughput_env.sh --apply` still runs `tools/molt_cache_prune.py`
  using those policy defaults. Override with `MOLT_CACHE_MAX_GB`,
  `MOLT_CACHE_MAX_AGE_DAYS`, or disable via `MOLT_CACHE_PRUNE=0`.
- **Diff run coordination lock**: `tests/molt_diff.py` acquires `<CARGO_TARGET_DIR>/.molt_state/diff_run.lock` so concurrent agents queue instead of running overlapping diff sweeps. Tune with `MOLT_DIFF_RUN_LOCK_WAIT_SEC` (default 900) and `MOLT_DIFF_RUN_LOCK_POLL_SEC`.
- **Matrix harness**: use `tools/throughput_matrix.py` for reproducible single-vs-concurrent build throughput checks (profiles + wrapper modes), with optional differential mini-matrix.
  - Example: `uv run --python 3.12 python tools/throughput_matrix.py --concurrency 2 --timeout-sec 75 --shared-target-dir /Volumes/APDataStore/Molt/cargo-target --run-diff --diff-jobs 2 --diff-timeout-sec 180`
  - Output: `matrix_results.json` under the output root (`$MOLT_EXT_ROOT/...` by default).
  - `matrix_results.json` now includes `gate_status` (thresholds, observed counts, violation details, pass/fail).
  - Use `--fail-on-gate` to return exit code `2` on gate failure.
  - If external root is unavailable, pass `--output-root` explicitly only for an approved emergency override.
  - If rustc prints incremental hard-link fallback warnings, move `--shared-target-dir` to a local APFS/ext4 path.
- **Compile-progress suite**: use `tools/compile_progress.py` for standardized
  cold/warm + cache-hit/no-cache + daemon-on/off compile tracking.
  - Example: `uv run --python 3.12 python tools/compile_progress.py --clean-state`
  - Include `--diagnostics` to capture per-case phase timing + module-reason
    payloads from compiler builds.
  - Queue lane (daemon warm queue): add
    `--cases dev_queue_daemon_on dev_queue_daemon_off`.
  - Release-iteration lane (`release-fast` Cargo profile override): add
    `--cases release_fast_cold release_fast_warm release_fast_nocache_warm`.
  - Under host contention, prefer:
    `--max-retries 2 --retry-backoff-sec 2 --build-lock-timeout-sec 60`.
  - Timeouts are fail-safe: the harness kills only marker-scoped timed-out
    compiler children (`cargo`/`rustc`/`sccache`) before retrying/continuing;
    backend daemons remain under identity custody so warm state is not destroyed
    by a compile-progress retry.
  - `SIGTERM` exits (`rc=143`/`rc=-15`) are treated as retryable.
  - Snapshots (`compile_progress.json` / `.md`) are updated after each
    completed case so interrupted long runs still preserve progress.
  - Use `--resume` in persistent `tmux`/`mosh` sessions to continue interrupted
    long sweeps (especially release lanes) without rerunning completed cases.
  - Outputs: `compile_progress.json` + `compile_progress.md` + per-case logs.
  - Default output root: `$MOLT_EXT_ROOT/compile_progress_<timestamp>`.
  - If external root is unavailable, pass `--output-root` explicitly only for an approved emergency override.
  - Initiative KPI board lives at `docs/benchmarks/compile_progress.md`.
- **Build diagnostics**: enable compiler phase timing + module-inclusion reasons
  on demand with `--diagnostics`.
  - Optional output file: `--diagnostics-file <path>` (relative paths are
    resolved under the build artifacts directory).
  - Example:
    `uv run --python 3.12 python -m molt.cli build --profile dev --no-cache --diagnostics --diagnostics-file build_diag.json examples/hello.py`
  - Midend diagnostics now carry tier-promotion telemetry (`tier_base_summary`,
    `promoted_functions`, `promotion_source_summary`,
    `promotion_hotspots_top`) to verify PGO-guided promotion decisions.
  - For deterministic control experiments, set
    `MOLT_MIDEND_HOT_TIER_PROMOTION=0` to disable hot-function tier promotion.

## Weekly Stdlib Scoreboard
Update this table at least weekly during lowering burn-down.

| Date | intrinsic-backed | intrinsic-partial | probe-only | python-only | missing top-level | missing submodules | native parity pass % | wasm parity pass % | memory regressions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 2026-02-12 | 177 | 696 | 0 | 0 | 0 | 0 | TBD | TBD | TBD |

Rules:
- Any PR that worsens this table requires explicit exception sign-off.
- Ratchet updates (`tools/stdlib_intrinsics_ratchet.json`) must only move downward and must ship with real lowering progress in the same change.

### Fast Dev Playbook (Recommended)
1. `tools/throughput_env.sh --apply`
2. `uv run --python 3.12 python -m molt.cli build --profile dev examples/hello.py --cache-report`
3. `UV_NO_SYNC=1 uv run --python 3.12 python -u tests/molt_diff.py --build-profile dev --jobs 2 tests/differential/basic`

Expected behavior in this lane:
- Runtime/backend Cargo crates rebuild once per fingerprint and profile, with parallel agents waiting on shared build-state locks whenever they share `CARGO_TARGET_DIR` or `MOLT_BUILD_STATE_DIR`.
- Differential jobs are clamped by the in-harness memory guard before launch and are killed with telemetry instead of allowing global RSS to run past the configured safety budget.
- Live memory pressure can be watched with `python3 tools/memory_guard_stream.py --diff-root "$MOLT_DIFF_ROOT"` or emitted directly from the harness with `MOLT_DIFF_MEMORY_GUARD_STREAM=stderr`; neither mode creates an additional artifact tree.
- Backend codegen uses the persistent daemon for native builds when available (fewer cold starts across repeated builds).
- Per-script object artifacts are reused via `MOLT_CACHE` keying when IR+fingerprint inputs are unchanged.

When throughput regresses unexpectedly:
1. Check lock contention under `<CARGO_TARGET_DIR>/.molt_state/build_locks/`.
2. Check `sccache -s` hit rates and wrapper disable/retry logs.
3. Run `tools/throughput_matrix.py` to isolate profile/wrapper/concurrency regressions.

## Remote Access and Persistent Sessions
This project assumes persistent terminal sessions and remote access.
Contributors are expected to use tmux + mosh (or equivalent) for long-running
work.

### Why this exists
Molt involves:
- compilation pipelines
- benchmarking loops
- WASM builds
- profiling and fuzzing

These workflows do not fit fragile, single-terminal sessions.

### Required baseline
All contributors should have:
- tmux (or screen)
- SSH access
- a way to reconnect safely (mosh recommended)

### Expected behaviors
- Long jobs should be run inside tmux
- Logs should be written to disk, not just stdout
- Partial progress should be preserved
- It must be safe to detach and resume

### What this enables
- Asynchronous contribution
- Background compilation and testing
- Phone-based monitoring of progress

## Logging and Benchmark Conventions
Persistent sessions are only useful if output is durable and inspectable.

### Logging rules
- Long-running tasks MUST log to disk
- Logs should be line-oriented text
- Include timestamps and phase markers
- Avoid excessive stdout-only output

Recommended structure:
```
logs/
  compiler/
  benchmarks/
  wasm/
  agents/
  cpython_regrtest/
```

### Benchmarking rules
- Benchmarks must be reproducible
- Record:
  - git commit
  - machine info
  - date/time
- For WASM benches, capture the native CPython baseline and compute WASM vs
  CPython ratios only for rows where `molt_wasm_ok` is true and
  `molt_wasm_time_s` is a positive finite duration.
- When validating linked WASM runs, pass `--linked` to `tools/bench_wasm.py` and
  note in logs whether the run used linked or fallback mode.
- Prefer CSV or JSON outputs
- Do not overwrite previous results
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`)
  execute 10 samples, store raw successful sample arrays plus
  mean/median/variance/range stats in JSON output, and reject partial sample
  failure as benchmark evidence; reserve these for release tagging or explicit
  requests.

### Why this matters
- Enables async review
- Enables regression detection
- Enables phone-based inspection

### CPython regrtest harness
The `tools/cpython_regrtest.py` harness writes one run per directory under
`logs/cpython_regrtest/` and emits per-run `summary.md`, `summary.json`, and
`junit.xml`, plus a root `summary.md`. Each run also writes `diff_summary.md`
and `type_semantics_matrix.md` so parity work stays aligned with the stdlib and
type/semantics matrices; `--rust-coverage` adds `rust_coverage/` output
(requires `cargo-llvm-cov`). Use `--clone` and `--uv-prepare` explicitly when
you want networked downloads or dependency installs.
The shim treats `MOLT_COMPAT_ERROR` results as skipped and records the reason
in `junit.xml`; use `tools/cpython_regrtest_skip.txt` for intentional exclusions.
When `--coverage` is enabled, the harness combines host regrtest coverage with
Molt subprocess coverage (use a Python-based `--molt-cmd` to capture it). The
shim forwards interpreter flags from regrtest to the Molt command to preserve
isolation/warnings behavior.
Regrtest runs set `MOLT_MODULE_ROOTS` and `MOLT_REGRTEST_CPYTHON_DIR` to pull
CPython `Lib/test` sources into the static module graph without mutating host
`PYTHONPATH`.
Regrtest runs set `MOLT_CAPABILITIES=fs.read,env.read` by default; override with
`--molt-capabilities` when needed.

Benchmarks that cannot survive a disconnect are not acceptable.

### Profiling harness
- Use `tools/profile_hotspots.py` first when the question is dev/CI wall clock:
  it reads the default guarded-command profile log and sorts slow subprocesses
  before you decide which runner or workflow deserves deeper profiling.
- Use `tools/output_startup_size_audit.py` when the question is tiny-binary
  startup or output size across targets/profiles/backends: the normal benchmark
  harness can hide first-path code-signature and loader costs by repeatedly
  launching the same executable, and target-only size claims are incomplete
  without the adjacent native/WASM/Luau/MLIR artifact rows.
- Use `tools/profile.py` for repeatable CPU/alloc profiling runs.
- Default outputs land in `logs/benchmarks/profile_<timestamp>/` with a
  `profile_manifest.json` that records git rev, platform, tool choices, env
  overrides, and per-benchmark artifacts/metrics.
- Prefer `--suite top` for Phase 0 profiling, and add `--profile-compiler` when
  you need compiler-side hotspots.
- Use `--molt-profile` to enable runtime counters (`MOLT_PROFILE=1`) and capture
  allocation/dispatch metrics in the manifest.
- Use `--molt-profile-alloc-sites` to record hot string allocation call sites
  (`MOLT_PROFILE_ALLOC_SITES=1`) and parse `molt_profile_string_site` entries
  into the manifest; tune with `--molt-profile-alloc-sites-limit` when you need
  more or fewer entries.
- Add `--summary` to emit a `profile_summary.json` with top alloc counters,
  allocation-per-call ratios, and hot string allocation sites across benches.
- `molt_profile_cpu_features` lines report detected SIMD features so profile
  manifests can capture which vector paths should be reachable on that host.
- Use `--cpu-tool perf-stat --perf-stat-events ...` on Linux for counter stats
  alongside the time/heap profiling.
- For WASM, use `tools/wasm_profile.py --bench bench_sum` to emit Node
  `.cpuprofile` artifacts in `logs/wasm_profile/<timestamp>/` (pair with
  `--linked` when validating single-module link runs).

## Progress Reporting

### Task scaffolding
Use the helper to create a per-task log + report skeleton:
```bash
tools/new-agent-task.sh <task-name>
```

### Minimal “micro-report” (for frequent updates)
Use this when updating every 10–20 minutes.

```markdown
- [<HH:MM>] <status> — <one-line update>. Output: <path>. Next: <one-line next step>. Resume: <command>.
```

### Full report format
```markdown
## <Task Title>

### Status
<short status line>

### Summary
- ...

### Files Touched
- <path> — <one-line change>

### Tests
- <command> → PASS/FAIL

### Notes
- ...

### Resume
<command>
```

### Coverage reporting helpers
Generate differential coverage summaries and keep spec TODOs in sync:
```bash
uv run --python 3.12 python tools/diff_coverage.py
uv run --python 3.12 python tools/check_type_coverage_todos.py
```

## Multi-Agent Workflow
This section standardizes parallel agent work on Molt.

The canonical protocol for proof-lane ownership, task logs, targeted vs broad
validation, and collision handling is
[ops/MULTI_AGENT_COORDINATION.md](ops/MULTI_AGENT_COORDINATION.md). Agents
running long differential, conformance, regrtest, benchmark, or `molt validate`
lanes must follow that protocol first.

### Access and tooling
- Agents may use `gh` (GitHub CLI) and git over SSH.
- Agents are expected to create branches, open PRs, and merge when approved.
- Proactively commit work in logical chunks with clear messages.

### Work partitioning
- Assign each agent a scoped area (runtime/frontend/docs/tests) and avoid
  overlap.
- If cross-cutting changes are required, coordinate early.
- Keep one broad-sweep coordinator per shared target root for full
  differential, CPython regrtest, conformance, or release validation lanes.
  Other agents should run targeted proofs, reduce failure queues, or move to
  non-colliding structural work.

### Communication rules
- Always announce: scope, files touched, and expected tests.
- Keep status updates short and explicit.
- Flag any risky changes early.
- Use `tools/new-agent-task.sh <task-name>` or update an existing
  `logs/agents/<task>/` record before long proof work, including owned files,
  proof lane, artifacts, and next command.
- Use `uv run --python 3.12 python tools/agent_coordination.py env` to capture
  OS/Python/shell facts before choosing platform-specific commands. Then use
  `uv run --python 3.12 python tools/agent_coordination.py scan` to inspect
  active task claims, and `... check` to fail fast on duplicate broad-sweep
  coordinator claims for the same lane and shared target root.

### Quality gates
- Run extensive linting and tests before PRs or merges.
- Prefer `molt setup`, `molt doctor`, and `molt validate` as the canonical
  operator surface. `tools/dev.py` is now a convenience delegate only.
- Use `molt validate --suite smoke` for fast local proof and `molt validate`
  for the heavier full matrix, plus any targeted `cargo` checks required by the
  touched lane.
- Use `molt validate --suite smoke --backend luau` for Luau changes; the lane
  covers generated support-matrix freshness, checked Luau emission,
  Luau runner availability, backend/lowering Rust unit tests, and the targeted
  CPython-vs-Luau smoke under the shared memory guard.
- Executed `molt validate` runs persist their run payload under
  `logs/validate-<suite>-<backend>-<profile>.json` by default. Pass
  `--summary-out` for an explicit custody path; check-only plans only write when
  that option is supplied.
- `tools/dev.py bench` is the canonical convenience entrypoint for a guarded
  local benchmark smoke check. With no arguments it runs the pyproject-owned
  smoke command and writes `bench/results/dev-bench-smoke.json`; pass explicit
  `molt bench` arguments for custom or full benchmark slices.
- `tools/dev.py lint` includes the subprocess guard coverage audit and the
  memory-guard wiring audit; new raw subprocess calls in dev/test/bench
  surfaces must either use `tools/harness_memory_guard.py`/test process-guard
  helpers or document why they are bounded metadata probes or interactive
  guarded Popen paths, and new guard entrypoints must stay visible to the repo
  process sentinel.
- `tools/dev.py gates` persists the pyproject-owned local gate sequence under
  `logs/dev-gates-summary.json` by default. Pass `--summary-out` when a batch
  needs a named custody artifact; failed gates still write the partial command
  list, timings, guard limits, and failure reason before returning nonzero.
- `tools/dev.py test --random-order --random-seed <seed>` is an opt-in DX lane
  for flushing out order dependence. Keep the canonical proof/CI sweeps
  deterministic.
- Do not merge if tests are failing unless explicitly approved.

### Merge discipline
- Merge only after tests pass and conflicts are resolved.
- If two agents touch the same area, rebase and re-validate.
