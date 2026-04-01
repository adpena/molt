# Canonical Engineering Burndown Plan: Monty, Buffa, and Runtime/WASM Closure

> Audited and re-consolidated on 2026-03-30 for maximum developer velocity, signal density, and low-overhead execution.
> **Status update 2026-04-01:** Added per-track status markers based on codebase cross-reference audit.

## Why this plan exists

This is now the single canonical engineering execution plan for the remaining repo-local compiler/runtime/tooling work.

The separate `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md` plan remains isolated on purpose because it drives live Linear/workspace operations with a different cadence, credentials boundary, and convergence loop.

## Consolidation outcome

The following engineering child plans are now fully folded into this document and should not be used as separate execution plans:

- `2026-03-26-stdlib-object-partition.md`
- `2026-03-27-wave-a-correctness-fortress.md`
- `2026-03-27-wave-b-ecosystem-unlock.md`
- `2026-03-27-wave-c-wasm-first-class.md`

Already retired before this pass:

- `2026-03-27-wrapper-artifact-contract.md`
- `2026-03-27-branch-integration-into-main.md`
- `2026-03-27-molt-stabilization-and-roadmap-continuation.md`
- `2026-03-27-repo-gap-closure-program.md`
- `2026-03-28-cloudflare-demo-hardening.md`
- `2026-03-28-phase1-wire-and-ship.md`
- `2026-03-28-harness-engineering.md`

## Execution model

- Use this file for all engineering burndown and closure tracking.
- Use `2026-03-26-linear-grouped-backlog.md` as the only separate ops/workspace plan.
- Do not create new child plans unless the work truly needs a distinct operational lane.
- Prefer closing whole tracks with explicit gates over growing more checklist sprawl.

## Burndown board

| Track | Area | Tier | Status | Validation gate | Current blocker |
|---|---|---|---|---|---|
| E0 | Stdlib partition contract | 0 | **~60%** | Focused backend + CLI partition tests | 2 of 3 tests missing (`stdlib_link_fingerprint`, `stdlib_partition_emit_obj`); no daemon partition-root metadata |
| E1 | Wave A correctness exit | 0 | **~85%** | Focused differential + backend + daemon/TIR sweep | Exception handling mitigated (2026-04-01); need Cranelift baseline decision doc + sweep artifact |
| E2 | Buffa/protobuf end-to-end proof | 0 | **~40%** | Molt-facing e2e proof, not crate-local only | Crate has 14 unit tests; zero Python-through-Molt proof paths |
| E3 | Ecosystem unlock (`click`, `attrs`) | 1 | **~10%** | Small ecosystem differential matrix | `import_six.py` exists but MOLT_SKIP'd; `import_click.py`/`import_attrs.py` don't exist |
| E4 | WASM parity and deploy proof | 1 | **~35%** | WASM parity sweep + live deploy proof + benchmark artifact | No current sweep results; no live deploy artifact; benchmarks from 2026-03-28 |
| E5 | Final docs/status convergence | 2 | **~10%** | Canonical docs reflect only proven claims | STATUS.md last updated 2026-03-19; blocked on E2/E3/E4 |

## Tier 0 - run in parallel now

### E0 - Stdlib partition contract

Scope folded from the old stdlib-object-partition residual plan.

Done:
- ✅ `is_user_owned_symbol()` in `main.rs:89` correctly excludes non-entry stdlib `molt_init_*` while keeping entry roots
- ✅ Unit test `user_owned_symbol_whitelist_keeps_only_entry_roots` passes
- ✅ `stdlib_partition_mode_changes_cache_identity` test exists in `test_cli_import_collection.py`

Remaining work:
- add `stdlib_link_fingerprint` test (link fingerprints change when any stdlib partition artifact changes);
- add `stdlib_partition_emit_obj` test (`emit=obj` contract under partition mode);
- add daemon partition-root metadata (daemon request carries partition-root explicitly, not ambient env).

Validation:
- `cargo test -p molt-backend --features native-backend user_owned_symbol_whitelist_keeps_only_entry_roots -- --nocapture`
- `UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_link_fingerprint or stdlib_partition_mode_changes_cache_identity or stdlib_partition_emit_obj'`

### E1 - Wave A correctness exit

Scope folded from the old Wave A residual plan.

Done:
- ✅ Cranelift 0.130.0 pinned across all targets in `molt-backend/Cargo.toml`
- ✅ TIR default-ON (`d6b3692ac`) with structured CondBranch, nested loop emission, type specializations
- ✅ Test files exist and are NOT skipped: `nested_indexed_loops.py`, `triple_nested_loops.py`, `stdlib_attr_access.py`, `tuple_subclass_mro.py`, `genexpr_enumerate_unpack.py`
- ✅ SSA fixes landed: two-pass dominator walk (`db42ea341`), sealed blocks, loop phi fix

Remaining work:
- record Cranelift 0.130.0 as the intended pinned baseline (decision doc, not just Cargo.toml);
- ~~fix TIR exception handling~~ — MITIGATED (2026-04-01): functions with `check_exception` bypass TIR; try/except/finally/else verified working with CPython parity. Making TIR handle exceptions natively is a future perf optimization.
- run the focused Wave A regression sweep and record pass/fail results as artifact.

Validation:
- `cargo check -p molt-backend --features native-backend`
- `cargo test -p molt-backend --features native-backend -- --nocapture`
- `MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py --jobs 1 tests/differential/basic/nested_indexed_loops.py tests/differential/basic/triple_nested_loops.py tests/differential/basic/stdlib_attr_access.py tests/differential/basic/tuple_subclass_mro.py tests/differential/basic/genexpr_enumerate_unpack.py`

### E2 - Buffa/protobuf end-to-end proof

This stays in the umbrella plan because it cuts across runtime crates, Molt-facing APIs, and documentation.

Remaining work:
- prove the existing `runtime/molt-runtime-protobuf` encode/decode/audit-event implementation is exercised from a Molt-facing surface;
- if that proof does not exist, add the smallest end-to-end test/documented path that makes it real;
- document the supported boundary once proven.

Validation:
- a repo-local Molt-facing proof path plus focused tests under canonical roots;
- `cargo test -p molt-runtime-protobuf`

## Tier 1 - starts once E0 and E1 are green

### E3 - Ecosystem unlock

Scope folded from the old Wave B residual plan.

Done:
- ✅ `tests/differential/basic/import_six.py` exists (but MOLT_SKIP'd — runtime crash: "index out of bounds")

Remaining work:
- fix `six` runtime crash and remove MOLT_SKIP;
- create `tests/differential/basic/import_click.py` differential test;
- create `tests/differential/basic/import_attrs.py` differential test;
- fix reusable semantics at runtime/frontend/backend layers for all three.

Validation:
- `MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_six.py tests/differential/basic/import_click.py tests/differential/basic/import_attrs.py --jobs 1`

### E4 - WASM parity and deploy proof

Scope folded from the old Wave C residual plan.

Done:
- ✅ `test_wasm_codec_parity.py` exists (JSON/CBOR/msgpack roundtrip)
- ✅ `test_cloudflare_demo_verify.py` exists with endpoint verification tooling
- ✅ WASM benchmarks from 2026-03-28 in `bench/results/bench_wasm_20260328_*.json`
- ✅ Split-runtime works, WASM 1.7MB gzipped fits Cloudflare Workers

Remaining work:
- re-run WASM parity sweep with current codebase and record results;
- produce live Cloudflare deploy verification artifact (or record explicit credentials blocker);
- emit current `wave_c_exit_gate.json` benchmark (existing benchmarks are 4 days stale).

Validation:
- `PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/test_wasm_importlib_machinery.py tests/test_wasm_link_validation.py tests/cli/test_cli_wasm_artifact_validation.py`
- `python3 tools/cloudflare_demo_deploy_verify.py --live-base-url <worker-url> --artifact-root logs/cloudflare_demo_verify/live`
- `PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --linked --output bench/results/wave_c_exit_gate.json`

## Tier 2 - starts once E2, E3, and E4 are green

### E5 - Docs and status convergence

Status: Blocked on E2/E3/E4. `docs/spec/STATUS.md` last updated 2026-03-19 — does not reflect TIR default-ON, multi-crate extraction, Monty/Buffa work, Cranelift 0.130, or conformance baseline.

Remaining work:
- refresh `docs/spec/STATUS.md` to reflect current capabilities (TIR, type specializations, conformance 78%, Cranelift 0.130);
- refresh `ROADMAP.md` to reflect completed milestones and current blockers;
- keep `bench/results/`, `logs/`, and `tmp/` as the canonical evidence roots;
- remove this plan only when the engineering burndown has no remaining open tracks.

## Global exit gate

- E0-E5 are either closed or have an explicit blocker recorded in this file.
- The only remaining separate execution plan is `docs/superpowers/plans/2026-03-26-linear-grouped-backlog.md`.
- No new engineering child plans are needed to understand or execute the remaining work.
