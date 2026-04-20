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
- Use canonical repo-local roots: `target/`, `tmp/diff`, `.molt_cache/`, and `.uv-cache/`.
- If `MOLT_EXT_ROOT` is set, place those same roots under it explicitly.

## Gate Matrix

| Gate | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| G0 | Fast compile sanity | `cargo check -p molt-runtime -p molt-backend` | Exit 0; no compile errors. |
| G1 | Lint/type hygiene | `uv run --python 3.12 python3 tools/dev.py lint` | Exit 0; lint/format/type checks clean. |
| G2 | Core lowering + IR lane regression smoke | `uv run --python 3.12 python3 -m molt.cli debug verify --format json && uv run --python 3.12 pytest -q tests/test_codec_lowering.py` | Exit 0; IR inventory/semantic gate and lowering smoke remain green. |
| G3 | Tier 0/1 differential parity (basic + stdlib lanes) | `MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_RLIMIT_GB=10 MOLT_DIFF_TIMEOUT=180 MOLT_DIFF_FAILURES=ir_probe_failures.txt uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib && uv run --python 3.12 python3 -m molt.cli debug verify --require-probe-execution --probe-rss-metrics rss_metrics.jsonl --failure-queue ir_probe_failures.txt --format json` | Exit 0; both differential lanes are green with RSS recorded, and required IR probes executed with `status=ok` and absent from failure queue. |
| G4 | Cross-version Python test sweep | `uv run --python 3.12 python3 tools/dev.py test` | Exit 0; 3.12/3.13/3.14 sweep green. |
| G5 | CPython parity regression lane (periodic/pre-release) | `uv run --python 3.12 python3 tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare` | Exit 0; expected skips only; summary and junit emitted. |
| G6 | Runtime feedback artifact validation (guard/deopt instrumentation surface) | `PYTHONPATH=src MOLT_PROFILE=1 MOLT_RUNTIME_FEEDBACK=1 MOLT_RUNTIME_FEEDBACK_FILE=target/molt_runtime_feedback_gate.json uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py && uv run --python 3.12 python3 tools/check_runtime_feedback.py target/molt_runtime_feedback_gate.json` | Exit 0; feedback artifact exists and schema keys validate, including required `deopt_reasons` counters for `call_indirect`, `invoke_ffi`, `guard_tag`, and `guard_dict_shape` lanes plus the guard-layout mismatch breakdown keys. |

## Import / Bootstrap Must-Pass Matrix

Use these lanes for import-system, package-entry, and bootstrap regressions. The commands below point at existing in-tree tests and are not a claim that any lane is currently green.

| Lane | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| IB0 | Native import/bootstrap regressions | `uv run --python 3.12 pytest -q tests/test_native_import_bootstrap_regressions.py tests/test_stdlib_package_bootstrap_surface.py tests/test_import_runtime_private_module_surfaces.py tests/test_intrinsics_bootstrap_contract.py` | Existing native coverage stays green for package-entry bootstrap identity, direct and relative imports, stdlib package bootstrap, frozen import runtime surfaces, and the bootstrap contract. |
| IB1 | WASM import/bootstrap smoke | `uv run --python 3.12 pytest -q tests/test_wasm_importlib_smoke.py tests/test_wasm_importlib_package_bootstrap.py` | Existing WASM coverage stays green for `importlib`/`importlib.machinery` bootstrap, module-body execution, package-relative imports, and both linked and split-runtime execution paths. |
| IB2 | Differential import semantics | `uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/stdlib/importlib_basic.py tests/differential/stdlib/importlib_import_module_basic.py tests/differential/stdlib/importlib_relative_import_from_package.py tests/differential/stdlib/importlib_import_module_helper_constant.py tests/differential/stdlib/importlib_support_bootstrap.py` | Differential coverage stays green for basic import resolution, `import_module`, relative package imports, and bootstrap support behavior. |

## GPU / Browser Host Must-Pass Matrix

| Lane | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| GB0 | Native compiled kernel parity | `uv run --python 3.12 pytest -q tests/test_gpu_kernel_compiled.py tests/test_gpu_api.py -k 'compiled_gpu_kernel_vector_add_matches_interpreted_semantics or compiled_gpu_kernel_vector_add_uses_metal_backend_when_enabled or compiled_gpu_kernel_vector_add_uses_webgpu_backend_when_enabled or gpu_kernel_call_lowers_to_first_class_gpu_launch_ir or gpu_kernel_descriptor_is_attached_to_function_metadata or kernel_simulation or kernel_scalar_multiply'` | Native compiled kernel lowering, sequential semantics, and explicit Metal/WebGPU backend lanes stay green. |
| GB1 | Split-runtime wasm compiled kernel parity | `uv run --python 3.12 pytest -q tests/test_wasm_split_runtime.py -k split_runtime_compiled_gpu_kernel_vector_add_matches_expected_output` | Split-runtime wasm compiled kernel stays correct for the baseline vector-add lane. |
| GB2 | Browser-host WebGPU dispatch contract | `uv run --python 3.12 pytest -q tests/test_wasm_browser_gpu_host.py -k compiled_gpu_kernel_uses_webgpu_dispatch` | Browser-host wasm compiled kernel uses the WebGPU dispatch boundary rather than the sequential fallback and produces the expected output. |

Required hardening gate details for IR dedicated probes (part of G3):
- `uv run --python 3.12 python3 -m molt.cli debug verify --require-probe-execution --probe-rss-metrics <MOLT_DIFF_ROOT>/rss_metrics.jsonl --failure-queue <failure-queue-path> --format json`
- Pass criteria: all required probes executed with `status=ok` and none present in the failure queue.

## Required Differential Runtime Controls
- Preferred environment:
  - `MOLT_DIFF_MEASURE_RSS=1`
  - `MOLT_DIFF_TIMEOUT=180`
  - `MOLT_DIFF_RLIMIT_GB=10`
  - `MOLT_DIFF_ROOT=${MOLT_EXT_ROOT:-$PWD}/tmp/diff`
  - `MOLT_CACHE=${MOLT_EXT_ROOT:-$PWD}/.molt_cache`
- If RSS grows rapidly, terminate the run, record abort details and last RSS in [tests/differential/INDEX.md](tests/differential/INDEX.md), then rerun with lower parallelism.

## Minimal Sign-off Checklist
- [ ] G0 through G3 passed for every runtime/compiler semantic change.
- [ ] G4 passed before merge for broad-impact changes.
- [ ] G5 passed for release prep and parity-focused work.
- [ ] G6 passed for changes that affect guard/deopt/profiling instrumentation.
- [ ] [tests/differential/INDEX.md](tests/differential/INDEX.md) updated after diff runs (date, host python, totals, failures, RSS notes).

## Related Docs
- [docs/ROADMAP_90_DAYS.md](docs/ROADMAP_90_DAYS.md)
- [docs/spec/areas/testing/0007-testing.md](docs/spec/areas/testing/0007-testing.md)
- [docs/OPERATIONS.md](docs/OPERATIONS.md)
- [docs/spec/STATUS.md](docs/spec/STATUS.md)
