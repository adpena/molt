# Minimum Must-Pass Matrix (Tier 0/1 + Diff Parity)

Status: Active
Owner: testing + runtime + frontend + tooling
Last updated: 2026-06-12

## Purpose
Define the minimum command matrix that must pass before we treat Tier 0/1 work as shippable.
This is the executable gate for the Month 1 "must-pass" roadmap item.

## Global Rules
- Run commands from repo root.
- Use `uv run --python ...`; do not call `.venv` interpreters directly.
- Differential runs must include `MOLT_DIFF_MEASURE_RSS=1`.
- Keep the adaptive harness memory guard active for every test run. Direct
  pytest entrypoints enter custody through root `sitecustomize.py` and the
  packaged `molt.pytest_memory_guard_bootstrap` pytest entry point before
  collection; the repo-configured `molt.pytest_memory_guard_config_plugin`
  keeps the same guard active when pytest entry-point autoload is disabled.
  Unguarded pytest re-execs through `tools/memory_guard.py`, forged guard env
  markers fail closed unless a live repo memory-guard ancestor is verified, and
  `--noconftest` / unsafe `--confcutdir` / unsafe pytest `-c` /
  guard-plugin disabling through argv or `PYTEST_ADDOPTS` are rejected before
  tests can run. `PYTEST_DISABLE_PLUGIN_AUTOLOAD` requires the explicit repo
  guard config plugin.
  Python interpreter-option forms and programmatic `pytest.main()` launches use
  pytest's own initial hook args as the custody authority. Guarded pytest runs
  keep a bounded `MOLT_PYTEST_CURRENT_TEST_FILE` current-node snapshot so RSS
  incidents name the active test/phase from parent-side diagnostics instead of
  relying only on child-mutated environment state. Legacy
  `*_MEMORY_GUARD=0` env knobs are ignored by the shared harness; set explicit
  caps only for a deliberate narrower investigation, never to disable RSS
  custody. Tempfile-backed subprocess capture, suite calibration, wasm diff,
  DX build timing, perf-scoreboard launchers, and CLI smoke probes also stay on
  the shared guard path.
- For maintainer/agent proof lanes and heavy local differential, conformance,
  benchmark, or CI-style runs, resolve artifact roots through `molt dx env`,
  `molt dx run`, or `tools/run_context_env.py --prefer-external-artifacts
  --dx`; on Windows checkouts on `C:`, use a healthy non-`C:` root unless an
  explicit emergency override is set.
- Public users and lightweight local examples may compile in place, use
  Molt/Cargo defaults, or choose roots with explicit flags/environment
  variables. Repo-local roots (`target/`, `tmp/diff`, `.molt_cache/`,
  `.uv-cache/`) are the fallback/user-default shape, not heavy agent-lane
  guidance.
- Bench conformance setup must not override explicit canonical artifact env
  vars; unset keys derive from the active artifact root, while explicitly set
  roots remain independent and authoritative.
- In multi-agent sessions, follow
  [docs/ops/MULTI_AGENT_COORDINATION.md](../../../ops/MULTI_AGENT_COORDINATION.md):
  one broad-sweep coordinator owns each shared target root while other agents
  use targeted proof, failure-queue reduction, or non-colliding structural work.
  Run `uv run --python 3.12 python tools/agent_coordination.py env` and
  `uv run --python 3.12 python tools/agent_coordination.py check` before
  starting broad differential, regrtest, conformance, or validation lanes.

## Gate Matrix

| Gate | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| G0 | Fast compile sanity | `cargo check -p molt-runtime -p molt-backend` | Exit 0; no compile errors. |
| G1 | Lint/type hygiene | `uv run --python 3.12 python tools/dev.py lint` | Exit 0; lint/format/type checks clean. |
| G2 | Core lowering + IR lane regression smoke | `uv run --python 3.12 python -m molt.cli debug verify --format json && uv run --python 3.12 pytest -q tests/test_codec_lowering.py` | Exit 0; IR inventory/semantic gate and lowering smoke remain green. |
| G3 | Tier 0/1 differential parity (basic + stdlib lanes) | `MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_TIMEOUT=180 MOLT_DIFF_FAILURES=ir_probe_failures.txt uv run --python 3.12 python -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib && uv run --python 3.12 python -m molt.cli debug verify --require-probe-execution --probe-rss-metrics rss_metrics.jsonl --failure-queue ir_probe_failures.txt --format json` | Exit 0; both differential lanes are green with RSS recorded, and required IR probes executed with `status=ok` and absent from failure queue. |
| G4 | Cross-version Python test sweep | `uv run --python 3.12 python tools/dev.py test` | Exit 0; 3.12/3.13/3.14 sweep green. |
| G5 | CPython parity regression lane (periodic/pre-release) | `uv run --python 3.12 python tools/cpython_regrtest.py --uv --uv-python 3.12 --uv-prepare` | Exit 0; expected skips only; summary and junit emitted. |
| G6 | Runtime feedback artifact validation (guard/deopt instrumentation surface) | `PYTHONPATH=src MOLT_PROFILE=1 MOLT_RUNTIME_FEEDBACK=1 MOLT_RUNTIME_FEEDBACK_FILE=target/molt_runtime_feedback_gate.json uv run --python 3.12 python -m molt.cli run --profile dev examples/hello.py && uv run --python 3.12 python tools/check_runtime_feedback.py target/molt_runtime_feedback_gate.json` | Exit 0; feedback artifact exists and schema keys validate, including required `deopt_reasons` counters for `call_indirect`, `invoke_ffi`, `guard_tag`, and `guard_dict_shape` lanes plus the guard-layout mismatch breakdown keys. |

## Import / Bootstrap Must-Pass Matrix

Use these lanes for import-system, package-entry, and bootstrap regressions. The commands below point at existing in-tree tests and are not a claim that any lane is currently green.

| Lane | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| IB0 | Native import/bootstrap regressions | `uv run --python 3.12 pytest -q tests/test_native_import_bootstrap_regressions.py tests/test_stdlib_package_bootstrap_surface.py tests/test_import_runtime_private_module_surfaces.py tests/test_intrinsics_bootstrap_contract.py` | Existing native coverage stays green for package-entry bootstrap identity, direct and relative imports, stdlib package bootstrap, frozen import runtime surfaces, and the bootstrap contract. |
| IB1 | WASM import/bootstrap smoke | `uv run --python 3.12 pytest -q tests/test_wasm_importlib_smoke.py tests/test_wasm_importlib_package_bootstrap.py` | Existing WASM coverage stays green for `importlib`/`importlib.machinery` bootstrap, module-body execution, package-relative imports, and both linked and split-runtime execution paths. |
| IB2 | Differential import semantics | `MOLT_DIFF_MEASURE_RSS=1 MOLT_DIFF_SUMMARY=tmp/diff/importlib_9file_summary.json uv run --python 3.12 python -u tests/molt_diff.py --stdlib-profile full --jobs 1 --log-file logs/importlib_9file.log tests/differential/stdlib/importlib_basic.py tests/differential/stdlib/importlib_import_module_basic.py tests/differential/stdlib/importlib_import_module_helper_constant.py tests/differential/stdlib/importlib_import_module_helper_dotted.py tests/differential/stdlib/importlib_import_module_helper_submodule.py tests/differential/stdlib/importlib_import_module_relative_package_typeerror.py tests/differential/stdlib/importlib_relative_import_from_package.py tests/differential/stdlib/importlib_runtime_state_payload_intrinsic.py tests/differential/stdlib/importlib_support_bootstrap.py` | Acceptance requires basic import resolution, folded `importlib.import_module` transaction paths, dotted/submodule helpers, relative package imports, runtime-state payload custody, and bootstrap support behavior to stay green; valid summary/log receipts must record 9 passed, 0 failed with RSS enabled. |

## Ownership / Exception Must-Pass Matrix

| Lane | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| ER0 | ExceptionRegion drop ownership | `cargo test -p molt-backend --features native-backend exception_region -- --nocapture` | TIR ExceptionRegion diagnostics and native DropInsertion transport stay green for the proven handler MatchRef release slice; any missing, ambiguous, or too-early release facts fail closed before backend lowering. |

## Startup / Size Must-Pass Matrix

| Lane | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| SS0 | Tiny output startup/size baseline | `uv run --python 3.12 python tools/uv_project_env.py --python 3.14 --purpose output-startup-size -- uv run --python 3.14 python tools/output_startup_size_audit.py --targets native --build-profiles release --backends auto --stdlib-profile micro --samples 5 --require-runners --strict --json-out bench/results/output_startup_size_audit.json --out-dir bench/results/output_startup_size_audit_outputs --json` | Baseline artifact records binary bytes plus same-path and cold-first-sighting startup measurements for the native micro profile; this is a ratchet artifact, not a performance win claim. The wrapper gives Python 3.14 a dedicated `tmp/uv-project-envs/` environment instead of rewriting the interactive `.venv`. |

## GPU / Browser Host Must-Pass Matrix

| Lane | Scope | Required Command(s) | Pass Criteria |
| --- | --- | --- | --- |
| GB0 | Native compiled kernel parity | `uv run --python 3.12 pytest -q tests/test_gpu_kernel_compiled.py tests/test_gpu_api.py -k 'compiled_gpu_kernel_vector_add_matches_interpreted_semantics or compiled_gpu_kernel_vector_add_uses_metal_backend_when_enabled or compiled_gpu_kernel_vector_add_uses_webgpu_backend_when_enabled or gpu_kernel_call_lowers_to_first_class_gpu_launch_ir or gpu_kernel_descriptor_is_attached_to_function_metadata or kernel_simulation or kernel_scalar_multiply'` | Native compiled kernel lowering, sequential semantics, and explicit Metal/WebGPU backend lanes stay green. |
| GB1 | Split-runtime wasm compiled kernel parity | `uv run --python 3.12 pytest -q tests/test_wasm_split_runtime.py -k split_runtime_compiled_gpu_kernel_vector_add_matches_expected_output` | Split-runtime wasm compiled kernel stays correct for the baseline vector-add lane. |
| GB2 | Browser-host WebGPU dispatch contract | `uv run --python 3.12 pytest -q tests/test_wasm_browser_gpu_host.py -k compiled_gpu_kernel_uses_webgpu_dispatch` | Browser-host wasm compiled kernel uses the WebGPU dispatch boundary rather than the sequential fallback and produces the expected output. |

Required hardening gate details for IR dedicated probes (part of G3):
- `uv run --python 3.12 python -m molt.cli debug verify --require-probe-execution --probe-rss-metrics <MOLT_DIFF_ROOT>/rss_metrics.jsonl --failure-queue <failure-queue-path> --format json`
- Pass criteria: all required probes executed with `status=ok` and none present in the failure queue.

## Required Differential Runtime Controls
- Preferred environment:
  - `MOLT_DIFF_MEASURE_RSS=1`
  - `MOLT_DIFF_TIMEOUT=180`
  - `export MOLT_SESSION_ID="<unique-session>"`
  - `eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"`
- Default harness memory guards must remain active for differential,
  benchmark, conformance, regrtest, CLI build/run, and equivalent long-running
  Molt workflows. The repo process sentinel treats canonical artifact roots
  (`target/`, `tmp/`, `dist/`, `build/`, `wasm/`, and `bench/results/`) as
  Molt-owned launch surfaces and propagates matched ownership across nested
  child process groups so detached descendants stay inside cumulative RSS
  accounting. Numeric process-group reuse is not ownership proof: current-tree
  drain keeps reparented groups only while live parent lineage or repo/Molt
  command identity still proves ownership. Guard teardown must stay narrower
  than accounting: terminate the
  guarded root process group and exact tracked escaped descendant PIDs, never
  ancestor, Claude/Codex app/control-plane, or child-reported process groups
  that can include unrelated host processes. Skipped protected groups are
  recorded in the sentinel JSONL as `repo_process_guard_protected_host_group`.
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
- [docs/ops/MULTI_AGENT_COORDINATION.md](../../../ops/MULTI_AGENT_COORDINATION.md)
- [docs/spec/STATUS.md](docs/spec/STATUS.md)
