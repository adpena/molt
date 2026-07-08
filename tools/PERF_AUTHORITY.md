# Performance Authority

Molt has one citable performance authority:

```text
tools/perf_scoreboard.py --set core --backend native --backend llvm \
  --profile release-fast --samples 5 --warmup 2 --repeat 5 \
  --classify --require-quiescent --quiescence-wait-s 180 \
  --quiescence-poll-s 15
```

That gate owns the release-fast performance contract because it records cold
and warm timings, native+LLVM backend parity, repeat-CI classification,
quiescence, provenance, and stale-tree status. It is the only lane allowed to
publish `authoritative=true`.

`tools/release_exit_gate.py` treats every `status: pass` criterion as a typed
receipt, not a generic file attachment. E1 must include a
`pact-witness-acceptance` receipt with a real `candidate_outputs.npz` path and a
`pact witness acceptance PASS` verdict. E2 must include a canonical
`cpython_floor_scoreboard` JSON artifact that is schema-valid, `authoritative:
true`, fresh under `DEFAULT_STALE_DAYS`, on `origin/main` ancestry, and has
`summary.gate_fails == false`. The receipt command must be the canonical
native+LLVM release-fast gate above (`--set core`, `--backend native`,
`--backend llvm`, `--samples 5`, `--warmup 2`, `--repeat 5`, `--classify`,
`--require-quiescent`, `--quiescence-wait-s 180`, and
`--quiescence-poll-s 15`) and the scoreboard itself must contain classified
release-fast cells for both native and LLVM across the full canonical core suite
(`bench_suites.BENCHMARKS`), with backend binary identity receipts for both
backends. E3 must include a
`tools/parity_gate.py` receipt with the no-Tier-1-violations PASS verdict.

The same release gate treats E4 `status: pass` as typed structural evidence, not
a generic JSON attachment. E4 must include the canonicalization contract JSON,
the structural-audit JSON, the degrade-to-slow gate report, and a fail-closed
gate receipt. The two metric artifacts are compared against their checked-in
baselines and fail closed on any regression; the poison receipt must contain the
`fail-closed gate: OK` verdict.

## Non-Canonical Lanes

`tools/bench.py` and `bench/harness.py` still measure useful development
signals, but their JSON outputs are not the perf contract. They must stamp a
top-level `provenance` object from `tools/perf_authority.py` with:

- `authoritative: false`
- `source: "non-canonical"`
- `lane`: the emitting tool path
- `profile`: the actual measured profile
- `canonical_gate`: the full `tools/perf_scoreboard.py --set core --backend native --backend llvm --profile release-fast --samples 5 --warmup 2 --repeat 5 --classify --require-quiescent --quiescence-wait-s 180 --quiescence-poll-s 15` command

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

Checked-in root `bench/scoreboard/*.json` CPython-floor boards are current
evidence only when they are schema-valid, generated at the current `origin/main`
tip, `authoritative: true`, fresh, and `summary.gate_fails == false`. Any older,
red, non-authoritative, or schema-legacy board must carry the structured
`perf_authority` stale metadata; then it is only a historical fixture and cannot
serve as E2 proof.

See also:

- `docs/perf/SCOREBOARD.md`
- `docs/design/foundation/64_perf_scoreboards_and_harness.md`
