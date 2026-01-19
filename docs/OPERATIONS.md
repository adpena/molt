# Operations Guide

This document consolidates remote access, logging, progress reporting, and
multi-agent workflow rules.

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
```

### Benchmarking rules
- Benchmarks must be reproducible
- Record:
  - git commit
  - machine info
  - date/time
- For WASM benches, capture the native CPython baseline and compute WASM vs
  CPython ratios.
- When validating linked WASM runs, pass `--linked` to `tools/bench_wasm.py` and
  note in logs whether the run used linked or fallback mode.
- Prefer CSV or JSON outputs
- Do not overwrite previous results
- Super bench runs (`tools/bench.py --super`, `tools/bench_wasm.py --super`)
  execute 10 samples and store mean/median/variance/range stats in JSON output;
  reserve these for release tagging or explicit requests.

### Why this matters
- Enables async review
- Enables regression detection
- Enables phone-based inspection

Benchmarks that cannot survive a disconnect are not acceptable.

### Profiling harness
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

## Multi-Agent Workflow
This section standardizes parallel agent work on Molt.

### Access and tooling
- Agents may use `gh` (GitHub CLI) and git over SSH.
- Agents are expected to create branches, open PRs, and merge when approved.
- Proactively commit work in logical chunks with clear messages.

### Locking and ownership
- Use `docs/AGENT_LOCKS.md` to claim files/directories before editing.
- Claims must be explicit and removed promptly when work finishes.

### Work partitioning
- Assign each agent a scoped area (runtime/frontend/docs/tests) and avoid
  overlap.
- If cross-cutting changes are required, coordinate and update locks first.

### Communication rules
- Always announce: scope, files touched, and expected tests.
- Keep status updates short and explicit.
- Flag any risky changes early.

### Quality gates
- Run extensive linting and tests before PRs or merges.
- Prefer `tools/dev.py lint` + `tools/dev.py test`, plus relevant `cargo`
  check/test.
- Do not merge if tests are failing unless explicitly approved.

### Merge discipline
- Merge only after tests pass and conflicts are resolved.
- If two agents touch the same area, rebase and re-validate.
