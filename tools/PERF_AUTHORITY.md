# Performance Authority

Molt has one citable performance authority:

```text
tools/perf_scoreboard.py --set core --backend native --backend llvm \
  --profile release-fast --samples 5 --warmup 2 --repeat 5 \
  --classify --require-quiescent
```

That gate owns the release-fast performance contract because it records cold
and warm timings, native+LLVM backend parity, repeat-CI classification,
quiescence, provenance, and stale-tree status. It is the only lane allowed to
publish `authoritative=true`.

## Non-Canonical Lanes

`tools/bench.py` and `bench/harness.py` still measure useful development
signals, but their JSON outputs are not the perf contract. They must stamp a
top-level `provenance` object from `tools/perf_authority.py` with:

- `authoritative: false`
- `source: "non-canonical"`
- `lane`: the emitting tool path
- `profile`: the actual measured profile
- `canonical_gate`: the full `tools/perf_scoreboard.py --set core --backend native --backend llvm --profile release-fast --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent` command

These lanes are for debugging, triage, and local comparison. Do not cite them as
release performance evidence.

## Ratio Rule

All non-canonical lanes must compute speedup through
`perf_authority.safe_speedup(cpython_time, molt_time)`.

`safe_speedup` returns `None` whenever either timing is missing, non-finite, or
non-positive. A build failure, daemon crash, runaway, or missing `molt_time`
must render as `n/a`, never as a finite regression or win.

The direction is fixed:

```text
speedup = cpython_time / molt_time
```

Values greater than `1.0` mean Molt is faster. The inverse field
`molt_cpython_ratio` must remain `molt_time / cpython_time`.

## Freshness Rule

Historical markdown snapshots are routing context, not current evidence. A perf
document whose recorded `git_rev` is not on `origin/main`, or whose generated
timestamp is stale relative to `perf_authority.DEFAULT_STALE_DAYS`, must be
treated as non-authoritative and point readers back to the canonical gate.

See also:

- `docs/perf/SCOREBOARD.md`
- `docs/design/foundation/64_perf_scoreboards_and_harness.md`
