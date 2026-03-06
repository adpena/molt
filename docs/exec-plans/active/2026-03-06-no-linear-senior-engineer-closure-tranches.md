# Senior Engineer Closure Tranches

Last updated: 2026-03-06

## Header

- Title: Senior Engineer Closure Tranches
- Owner: Codex + swarm explorers
- Linked Linear issue(s): none (ad hoc execution tranche)
- Milestone: compiler/runtime hardening
- Priority: P0
- Start date: 2026-03-06
- Target date: 2026-03-06

## Problem Statement

Molt has several high-leverage structural lanes that are individually promising but not yet proven to a senior-engineer bar: compiler/runtime invariants, RT2 concurrency ownership, `libmolt` extension contract breadth, stdlib concurrency lowering, and Luau backend contract clarity. The immediate goal is to turn those from “promising” into explicitly bounded, reproducibly verified tranches with no silent fallback semantics or test-only behavior.

## Constraints

- Determinism/security constraints: use external-volume artifacts only; avoid shared-target nondeterminism when proving a tranche; reject silent fallback or contract widening.
- Compatibility constraints (`py312/py313/py314`, native/wasm): target Python 3.12+ semantics; native and wasm behavior must stay explicitly documented; wasm thread absence remains capability-gated until RT2 says otherwise.
- Runtime/lowering constraints: stdlib behavior must remain intrinsic-first; `molt` stays compiler/runtime core, `builtins` + `molt.stdlib.*` remain CPython-facing, and `moltlib.*` remains the canonical Molt user API layer.

## Plan

1. Freeze and prove the in-flight `libmolt` + Luau tranche.
   - Scope: header-contract breadth for real extension sources; Luau backend MVP contract and dead-code elimination rules.
   - Evidence command(s):
     - `uv run --python 3.12 pytest -q tests/cli/test_cli_extension_commands.py -k 'numpy_generated_header_surface_smoke or numpy_public_overlay_header_surface_smoke or numpy_arrayscalar_source_shape_smoke or numpy_header_arrayobject_batch_smoke or type_module_thread_and_datetime_symbols_supported or numpy_batch_symbols_supported'`
     - `cargo test -p molt-backend`
     - `uv run --python 3.12 python3 -m molt.cli build --profile dev --target luau examples/hello.py`
2. Close the compiler/runtime invariant seam.
   - Scope: IR verifier coverage for `CALL_INDIRECT`, `INVOKE_FFI`, `GUARD_TAG`, `GUARD_DICT_SHAPE`, and ownership/lifetime operations.
   - Evidence command(s):
     - `uv run --python 3.12 pytest -q tests/test_check_molt_ir_ops.py tests/test_frontend_ir_alias_ops.py tests/test_frontend_midend_passes.py`
     - `uv run --python 3.12 python3 tests/molt_diff.py --build-profile dev --jobs 1 tests/differential/basic/call_indirect_dynamic_callable.py tests/differential/basic/call_indirect_noncallable_deopt.py tests/differential/basic/invoke_ffi_os_getcwd.py tests/differential/basic/guard_tag_type_hint_fail.py tests/differential/basic/guard_dict_shape_mutation.py`
3. Freeze RT2 runtime-state and concurrency ownership.
   - Scope: per-runtime ownership, thread interaction rules, shutdown semantics, and busy-path documentation before wider expansion.
   - Evidence command(s):
     - `cargo test -p molt-runtime gil_release_guard_drops_runtime_lock_temporarily -- --exact`
     - `uv run --python 3.12 pytest -q tests/test_wasm_thread_exception_path.py tests/test_wasm_runtime_heavy_regressions.py -k thread`
4. Run the next stdlib burndown cycle on the `threading` family.
   - Scope: `threading`, `_thread`, `_threading_local`, and downstream `asyncio.to_thread` unlocks using intrinsic-backed runtime semantics.
   - Evidence command(s):
     - `uv run --python 3.12 python3 tools/check_stdlib_intrinsics.py --fallback-intrinsic-backed-only`
     - `uv run --python 3.12 python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
     - `uv run --python 3.12 python3 tests/molt_diff.py --build-profile dev --jobs 1 tests/differential/stdlib/threading_basic.py tests/differential/stdlib/threading_primitives_basic.py tests/differential/stdlib/threading_condition_wait_for.py tests/differential/stdlib/threading_rlock_reentrancy.py tests/differential/stdlib/threading_local_isolation.py tests/differential/stdlib/threading_timer_basic.py tests/differential/stdlib/asyncio_to_thread_propagation.py`
5. Formalize the outcome and keep the repo honest.
   - Scope: sync specs/docs, capture any remaining blockers explicitly, and land validated work on `main`.
   - Evidence command(s):
     - `uv run --python 3.12 python3 tools/dev.py lint`
     - `git status --short`

## Acceptance Gates

- Tests: all tranche-local Python and Rust tests green with reproducible command lines.
- Differential checks: focused diff lanes green for the touched semantics before any broader sweep.
- Bench/perf checks: no unbounded perf regressions on hot integer/runtime paths; add targeted measurements when a tranche touches hot loops or runtime scheduling.
- Formal checks (Lean/Quint), when required: required for Symphony/orchestration changes only, not for the current backend/header tranche.
- Docs to sync: `README.md`, `docs/spec/STATUS.md`, `ROADMAP.md`, and any touched compat/runtime contract docs.

## Risks And Rollback

- Risks: silently broadening Luau support beyond what the backend can preserve; source-only header growth without real compile proof; concurrency tranche bleeding into wasm policy drift.
- Mitigations: add narrow contract tests, reject unsupported IR explicitly, keep wasm behavior capability-gated, and verify headers by compiling representative snippets.
- Rollback path: revert the tranche-local files only and preserve the execution plan plus failing proof artifacts for the next pass.

## Outcome

- Final status: in progress
- Evidence artifact paths: external-volume test/build outputs under `/Volumes/APDataStore/Molt`
- Follow-up TODOs:
  - Use the swarm findings to split the `threading` burndown into runtime/intrinsics/wrapper/doc ownership slices.
  - Convert the Luau backend from “best-effort transpiler” to an explicitly validated MVP subset before widening the supported IR.
