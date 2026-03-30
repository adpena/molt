# Wave A: Correctness Fortress Residual Plan

> Audited on 2026-03-30. Most of the original Wave A artifacts already exist in the repo: nested-loop regressions, stdlib attribute coverage, tuple-subclass MRO coverage, genexpr enumerate coverage, and the old vendored Cranelift directories are gone. This plan now tracks only the remaining closure work.

## Audit outcome

- Already landed:
  - `tests/differential/basic/nested_indexed_loops.py`
  - `tests/differential/basic/triple_nested_loops.py`
  - `tests/differential/basic/stdlib_attr_access.py`
  - `tests/differential/basic/tuple_subclass_mro.py`
  - `tests/differential/basic/genexpr_enumerate_unpack.py`
  - no vendored `cranelift-codegen-0.130.0/` or `cranelift-frontend-0.130.0/` directories remain.
- Still incomplete:
  - the plan does not record whether `0.130.0` is still the intended stable Cranelift baseline or whether an upgrade is still required;
  - the wave has no audited exit gate tying together backend tests, differential regressions, daemon behavior, and TIR safety/perf checks.

## Parallel tracks

### Track A1 - Cranelift baseline decision (independent)

- Confirm whether `0.130.0` is still the desired stable Cranelift version.
- If newer stable is required, perform the upgrade and re-run the backend test matrix.
- If `0.130.0` remains correct, record that decision and verify there is no remaining vendor patch dependency.
- Validation:
  - `cargo check -p molt-backend --features native-backend`
  - `cargo test -p molt-backend --features native-backend -- --nocapture`

### Track A2 - Correctness regression sweep (independent)

- Re-run the focused differential cases that represent the Wave A bug class:
  - nested loops;
  - stdlib attribute access;
  - tuple-subclass MRO;
  - genexpr enumerate tuple unpacking.
- Only reopen implementation work if one of these regressions fails.
- Validation:
  - `MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py --jobs 1 tests/differential/basic/nested_indexed_loops.py tests/differential/basic/triple_nested_loops.py tests/differential/basic/stdlib_attr_access.py tests/differential/basic/tuple_subclass_mro.py tests/differential/basic/genexpr_enumerate_unpack.py`

### Track A3 - Daemon and TIR exit validation (independent)

- Verify that current daemon behavior does not reintroduce lock-contention or stale-state regressions on the Wave A scenarios.
- Verify that the current TIR path is still safe on the focused Wave A regression slice and capture one benchmark or compile-time data point if behavior changed.
- Validation:
  - `PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py`
  - `PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --json-out bench/results/wave_a_exit_gate.json --bench sum`

## Exit gate

- A1-A3 all pass with recorded outcomes.
- If any regression remains, keep this file and update it with the reopened blocker.
- If the full exit gate passes with no reopened work, delete this plan on the next audit pass.
