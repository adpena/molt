# Minimum Must-Pass Matrix (Tier 0/1 + Diff Parity)

Status: Active
Owner: testing + runtime + frontend + tooling
Last updated: 2026-02-11

## Purpose
Define the minimum command matrix that must pass before we treat Tier 0/1 work as shippable.
This is the executable gate for the Month 1 "must-pass" roadmap item.

## Global Rules
- Run commands from repo root.
- Use `uv run --python ...`; do not call `.venv` interpreters directly.
- Differential runs must include `MOLT_DIFF_MEASURE_RSS=1`.
- Use a 10 GB per-process memory cap for diff runs when supported.
- Use external outdir/cache roots when available (`/Volumes/APDataStore/Molt`).

## Gate Matrix

| Gate | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| G0 | Fast compile sanity | `cargo check -p molt-runtime -p molt-backend` | Exit 0; no compile errors. |
| G1 | Lint/type hygiene | `uv run --python 3.12 python3 tools/dev.py lint` | Exit 0; lint/format/type checks clean. |
| G2 | Core lowering + IR lane regression smoke | `uv run --python 3.12 python3 tools/check_molt_ir_ops.py && uv run --python 3.12 pytest -q tests/test_codec_lowering.py` | Exit 0; IR inventory/semantic gate and lowering smoke remain green. |
| G3 | Tier 0/1 differential parity (basic + stdlib lanes) | `MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_RLIMIT_GB=10 MOLT_DIFF_TIMEOUT=180 MOLT_DIFF_FAILURES=ir_probe_failures.txt uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib && uv run --python 3.12 python3 tools/check_molt_ir_ops.py --require-probe-execution --probe-rss-metrics rss_metrics.jsonl --failure-queue ir_probe_failures.txt` | Exit 0; both differential lanes are green with RSS recorded, and required IR probes executed with `status=ok` and absent from failure queue. |
| G4 | Cross-version Python test sweep | `uv run --python 3.12 python3 tools/dev.py test` | Exit 0; 3.12/3.13/3.14 sweep green. |
| G5 | CPython parity regression lane (periodic/pre-release) | `uv run --python 3.12 python3 tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare` | Exit 0; expected skips only; summary and junit emitted. |
| G6 | Runtime feedback artifact validation (guard/deopt instrumentation surface) | `PYTHONPATH=src MOLT_PROFILE=1 MOLT_RUNTIME_FEEDBACK=1 MOLT_RUNTIME_FEEDBACK_FILE=target/molt_runtime_feedback_gate.json uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py && uv run --python 3.12 python3 tools/check_runtime_feedback.py target/molt_runtime_feedback_gate.json` | Exit 0; feedback artifact exists and schema keys validate, including required `deopt_reasons` counters for `call_indirect`, `invoke_ffi`, `guard_tag`, and `guard_dict_shape` lanes plus the guard-layout mismatch breakdown keys. |

Required hardening gate details for IR dedicated probes (part of G3):
- `uv run --python 3.12 python3 tools/check_molt_ir_ops.py --require-probe-execution --probe-rss-metrics <MOLT_DIFF_ROOT>/rss_metrics.jsonl --failure-queue <failure-queue-path>`
- Pass criteria: all required probes executed with `status=ok` and none present in the failure queue.

## Required Differential Runtime Controls
- Preferred environment:
  - `MOLT_DIFF_MEASURE_RSS=1`
  - `MOLT_DIFF_TIMEOUT=180`
  - `MOLT_DIFF_RLIMIT_GB=10`
  - `MOLT_DIFF_ROOT=/Volumes/APDataStore/Molt` when external volume exists
  - `MOLT_CACHE=/Volumes/APDataStore/Molt/molt_cache` when external volume exists
- If RSS grows rapidly, terminate the run, record abort details and last RSS in `tests/differential/INDEX.md`, then rerun with lower parallelism.

## Minimal Sign-off Checklist
- [ ] G0 through G3 passed for every runtime/compiler semantic change.
- [ ] G4 passed before merge for broad-impact changes.
- [ ] G5 passed for release prep and parity-focused work.
- [ ] G6 passed for changes that affect guard/deopt/profiling instrumentation.
- [ ] `tests/differential/INDEX.md` updated after diff runs (date, host python, totals, failures, RSS notes).

## Related Docs
- `docs/ROADMAP_90_DAYS.md`
- `docs/spec/areas/testing/0007-testing.md`
- `docs/OPERATIONS.md`
- `docs/spec/STATUS.md`
