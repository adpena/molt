# Bench Individual Daemon Custody Plan

## Design

`tools/bench_individual.py --isolate-daemon` must not kill every
`molt-backend` process on the host. Benchmark isolation should be scoped to the
current benchmark session and backend daemon socket, matching the existing
multi-agent custody model: preserve unrelated partner daemons, prune only
unsafe duplicate/socketless/stale daemons that belong to the current session,
and emit actionable cleanup telemetry.

Replace the global `molt-backend` kill helper with daemon discovery keyed by
`--socket` and the current session id. A benchmark may kill its own daemon when
the socket path or command carries the active `MOLT_SESSION_ID`; it may also
prune duplicates or missing-socket daemons for that same session. Foreign
sessions remain untouched.

## Files

- `tools/bench_individual.py`
- `tests/test_bench_individual_tool.py`
- `docs/BENCHMARKING.md`
- `docs/spec/areas/perf/0008-benchmarking.md`

## Tests

- Existing focused baseline:
  `uv run --python 3.12 python3 -m pytest tests/test_bench_individual_tool.py -q`
- Add regression coverage for:
  - `--isolate-daemon` cleanup preserving foreign session daemons.
  - cleanup reporting killed pids, sockets, elapsed time, and reason.
  - malformed/non-daemon process rows being ignored.
- Run the focused test file after the change, then the relevant broader gates.

## Risks

- Socket path parsing must remain conservative; if the daemon command does not
  expose `--socket`, the cleanup helper must not infer ownership from a broad
  `molt-backend` substring.
- Benchmark cold-isolation semantics should remain usable for the current
  session while avoiding collateral damage to partner agents.

## Exit Criteria

- No helper in `tools/bench_individual.py` kills all `molt-backend` processes.
- The isolate-daemon path kills only current-session backend daemons and reports
  why, when, over what elapsed span, and which pids/sockets were affected.
- Focused tests and the smallest convincing local proof matrix pass.
