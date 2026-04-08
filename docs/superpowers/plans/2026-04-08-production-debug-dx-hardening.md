# Production Debug DX Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate Molt's debugging, repro, verification, tracing, reduction, differential, and performance tooling into one production-hardened `molt debug` authority with deterministic artifacts, strong invariants, and aggressive legacy cleanup.

**Architecture:** Build a shared Python debug core under `src/molt/debug/` for manifests, IR capture, verifier orchestration, tracing, reduction, diff, and perf. Wire `src/molt/cli.py` to that core, add the required runtime/backend hooks for call-bind traces and stale-exception assertions, then delete or internalize duplicate legacy tool entrypoints so the repo ends with one authoritative interface instead of a script pile.

**Tech Stack:** Python (`argparse`, `pathlib`, existing Molt frontend/CLI), Rust (`molt-runtime`, `molt-backend`), existing differential/bench harnesses, pytest, cargo test, canonical `logs/` and `tmp/` artifact roots

---

## File Map

| Path | Responsibility |
| --- | --- |
| `src/molt/debug/__init__.py` | package exports for the canonical debug subsystem |
| `src/molt/debug/contracts.py` | run/result enums, schema helpers, backend capability model |
| `src/molt/debug/manifest.py` | manifest writing, artifact path allocation under `logs/debug` and `tmp/debug` |
| `src/molt/debug/ir.py` | canonical IR snapshot capture and rendering for `molt debug ir` |
| `src/molt/debug/verify.py` | verifier registry, `check_molt_ir_ops` migration, result rendering |
| `src/molt/debug/trace.py` | trace family selection, env normalization, artifact capture |
| `src/molt/debug/reduce.py` | reducer orchestration and oracle normalization |
| `src/molt/debug/bisect.py` | first-bad-pass and configuration bisection |
| `src/molt/debug/diff.py` | canonical debug-facing differential execution wrapper |
| `src/molt/debug/perf.py` | debug perf summaries, counter rendering, profile ingestion |
| `src/molt/cli.py` | canonical `molt debug` subcommands and shared flag plumbing |
| `tools/ir_dump.py` | delete or reduce to non-user-facing delegate after migration |
| `tools/ir_probe_supervisor.py` | delete or reduce to non-user-facing delegate after migration |
| `tools/profile_analyze.py` | delete or reduce to non-user-facing delegate after migration |
| `tools/check_molt_ir_ops.py` | migrate inventory/probe verification semantics into `src/molt/debug/verify.py`; remove standalone-authority behavior |
| `runtime/molt-runtime/src/call/bind.rs` | required call-bind trace families and no-pending-on-success assertion hooks |
| `runtime/molt-runtime/src/object/ops.rs` | runtime counter export surface for debug perf summaries |
| `runtime/molt-runtime/src/lib.rs` | runtime debug hook exports and module wiring |
| `runtime/molt-runtime/tests/` | runtime integration tests for trace/assert behavior |
| `runtime/molt-backend/src/lib.rs` | backend capability reporting and canonical debug hook wiring |
| `runtime/molt-backend/src/wasm.rs` | replace ad hoc `TIR_DUMP` behavior with shared capability-driven dump hooks |
| `runtime/molt-backend/src/tir/printer.rs` | backend IR formatting reused by the canonical debug surface |
| `tests/cli/test_cli_debug.py` | new CLI coverage for `molt debug ...` |
| `tests/test_debug_manifest.py` | manifest/artifact root contract coverage |
| `tests/test_debug_ir.py` | IR dump API coverage |
| `tests/test_debug_verify.py` | verifier API and migrated `check_molt_ir_ops` coverage |
| `tests/test_debug_reduce.py` | reducer oracle contract coverage |
| `tests/test_debug_diff.py` | debug diff command and manifest coverage |
| `tests/test_debug_perf.py` | debug perf summary coverage |
| `tests/test_ir_probe_supervisor.py` | delete or rewrite if its target becomes internal-only |
| `tests/test_check_molt_ir_ops.py` | delete or migrate assertions to `tests/test_debug_verify.py` |
| `docs/OPERATIONS.md` | canonical debug commands, artifact roots, cleanup policy |
| `docs/DEVELOPER_GUIDE.md` | debug architecture and ownership map |
| `docs/INDEX.md` | documentation entrypoint updates for the new debug surface |
| `docs/superpowers/specs/2026-04-08-production-debug-dx-hardening-design.md` | approved design; keep aligned if implementation changes architecture |

## Coordination Constraints

- There is active partner work in this repository. Read partner-modified files carefully and do not overwrite or revert unrelated changes.
- Do not create a second permanent debug stack. Every new behavior must live in `src/molt/debug/` or in backend/runtime hooks consumed by it.
- Legacy scripts are not a compatibility product. They are either deleted or made non-user-facing delegates and then removed before the end of convergence.
- Every build/test command that may compile Rust or run backend-integrated CLI flows must set `MOLT_SESSION_ID` and the canonical artifact env vars first.
- Keep artifacts under canonical roots only: `logs/`, `tmp/`, and `target/`.
- Use TDD for each task slice: write failing tests first, verify failure, implement minimum code, rerun, then commit.

## Shared Command Prefix For Build/Test Steps

Use this env prelude for every CLI/Rust verification step that can touch builds:

```bash
export MOLT_SESSION_ID=debug-dx-plan
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
```

## Task 1: Create The Shared Debug Core And Canonical CLI Scaffold

**Files:**
- Create: `src/molt/debug/__init__.py`
- Create: `src/molt/debug/contracts.py`
- Create: `src/molt/debug/manifest.py`
- Create: `tests/cli/test_cli_debug.py`
- Create: `tests/test_debug_manifest.py`
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Write the failing CLI/debug-manifest tests**

Add tests that prove:

- `molt debug --help` lists the canonical subcommands;
- `molt debug ir --help` and `molt debug verify --help` exist;
- a debug command writes a manifest under `tmp/debug/` by default;
- `--out` redirects retained output under `logs/debug/` when requested.

- [ ] **Step 2: Run the new tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-t1
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/cli/test_cli_debug.py tests/test_debug_manifest.py
```

Expected: FAIL because the `molt debug` command family and manifest helpers do not exist yet.

- [ ] **Step 3: Add `contracts.py` with the canonical result/manifest primitives**

Implement:

- debug subcommand enum or string constants;
- status/failure-class enums;
- capability record structure;
- normalized JSON payload helpers.

- [ ] **Step 4: Add `manifest.py`**

Implement:

- canonical artifact root selection;
- run-id generation;
- manifest serialization;
- text/json summary helpers shared by all debug commands.

- [ ] **Step 5: Add the initial `molt debug` parser scaffold**

Wire `src/molt/cli.py` so:

- `debug` is a first-class subcommand;
- the canonical debug subcommands exist, even if some initially return structured `unsupported/not yet wired` results;
- subcommands share selectors like `--function`, `--module`, `--pass`, `--backend`, `--profile`, `--format`, and `--out`.

- [ ] **Step 6: Run the tests again and make them pass**

Re-run the Task 1 pytest command and make the CLI/manifest contract green.

- [ ] **Step 7: Commit**

```bash
git add src/molt/debug/__init__.py src/molt/debug/contracts.py src/molt/debug/manifest.py src/molt/cli.py tests/cli/test_cli_debug.py tests/test_debug_manifest.py
git commit -m "cli: add canonical debug command scaffold"
```

## Task 2: Consolidate IR Dumping And Verifier Semantics Behind `molt debug`

**Files:**
- Create: `src/molt/debug/ir.py`
- Create: `src/molt/debug/verify.py`
- Create: `tests/test_debug_ir.py`
- Create: `tests/test_debug_verify.py`
- Modify: `tools/ir_dump.py`
- Modify: `tools/check_molt_ir_ops.py`
- Modify: `src/molt/cli.py`
- Modify: `tests/test_check_molt_ir_ops.py`

- [ ] **Step 1: Write the failing IR and verifier tests**

Cover:

- `molt debug ir` can dump pre/post/all stages in text and JSON;
- `--function` filtering only emits the selected function;
- `molt debug verify` exposes migrated IR inventory/probe checks;
- verifier results include function/pass/artifact references in the JSON payload.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-t2
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/test_debug_ir.py tests/test_debug_verify.py
```

Expected: FAIL because the shared IR/verify modules and CLI wiring do not exist yet.

- [ ] **Step 3: Move `tools/ir_dump.py` logic into `src/molt/debug/ir.py`**

Implement:

- shared IR snapshot capture helpers;
- text and JSON renderers;
- function/module/pass filtering;
- manifest emission through `manifest.py`.

- [ ] **Step 4: Move `check_molt_ir_ops` semantics into `src/molt/debug/verify.py`**

Implement:

- a verifier registry with named verifier bundles;
- migrated inventory/probe validation logic;
- canonical result payloads used by both `molt debug verify` and any other command that enables verifiers.

- [ ] **Step 5: Rewire the standalone scripts**

Make `tools/ir_dump.py` and `tools/check_molt_ir_ops.py` either:

- import the shared modules and act as non-user-facing delegates for in-repo automation only, or
- delete them and update all call sites in the same task if the delegate layer is unnecessary.

- [ ] **Step 6: Wire `molt debug ir` and `molt debug verify` in `src/molt/cli.py`**

Ensure both commands:

- emit text by default;
- emit stable JSON with `--format json`;
- write manifests;
- return explicit unsupported-capability results instead of silent omission.

- [ ] **Step 7: Update the migrated tests and make them pass**

Re-run the Task 2 pytest command and keep `tests/test_check_molt_ir_ops.py` only if it still validates internal delegate behavior. Otherwise, move its assertions into `tests/test_debug_verify.py` and delete the old file.

- [ ] **Step 8: Commit**

```bash
git add src/molt/debug/ir.py src/molt/debug/verify.py src/molt/cli.py tools/ir_dump.py tools/check_molt_ir_ops.py tests/test_debug_ir.py tests/test_debug_verify.py tests/test_check_molt_ir_ops.py
git commit -m "debug: consolidate IR dump and verifier surfaces"
```

## Task 3: Add LLVM/NVIDIA-Grade Runtime Trace And Assertion Hooks

**Files:**
- Modify: `runtime/molt-runtime/src/call/bind.rs`
- Modify: `runtime/molt-runtime/src/object/ops.rs`
- Modify: `runtime/molt-runtime/src/lib.rs`
- Create: `runtime/molt-runtime/tests/debug_call_bind.rs`
- Create: `src/molt/debug/trace.py`
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Write the failing runtime and CLI trace tests**

Cover:

- `MOLT_TRACE_CALL_BIND_IC=1` logs install/bypass with explicit reason codes;
- `MOLT_TRACE_CALLARGS=1` records builder contents before `molt_call_bind`;
- `MOLT_TRACE_FUNCTION_BIND_META=1` records bind metadata exactly as observed;
- `MOLT_ASSERT_NO_PENDING_ON_SUCCESS=1` traps or returns a deterministic verifier/assert failure when a success path leaves a pending exception behind;
- `molt debug trace` is the public way to enable the same trace families.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-t3
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
cargo test -p molt-runtime --test debug_call_bind -- --nocapture
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/cli/test_cli_debug.py -k debug_trace
```

Expected: FAIL because the structured trace/assert contract is incomplete.

- [ ] **Step 3: Introduce a shared trace-settings layer in `bind.rs`**

Refactor the current direct env checks so:

- trace family enablement is centralized;
- reason-code emission is explicit;
- builder and metadata dumps share one formatting path;
- `molt debug trace` can map flags onto the same low-level knobs.

- [ ] **Step 4: Implement the missing required trace payloads**

Add:

- IC install/bypass reason logging;
- callargs builder dump before bind;
- function bind metadata dump at bind time.

- [ ] **Step 5: Implement `MOLT_ASSERT_NO_PENDING_ON_SUCCESS=1`**

Add a deterministic success-path assertion that:

- checks for pending exception state before returning success from the relevant runtime function/module paths;
- emits an attributable failure message;
- integrates cleanly with verifier output and optional `SIGTRAP` debugging.

- [ ] **Step 6: Expose the trace family through `src/molt/debug/trace.py` and CLI wiring**

Ensure `molt debug trace`:

- sets the right env;
- records the enabled trace families in the manifest;
- supports `--function`, `--module`, and `--pass` filtering where implemented.

- [ ] **Step 7: Re-run runtime and CLI tests until green**

Keep the runtime tests and CLI tests green without weakening the trace or assertion semantics.

- [ ] **Step 8: Commit**

```bash
git add runtime/molt-runtime/src/call/bind.rs runtime/molt-runtime/src/object/ops.rs runtime/molt-runtime/src/lib.rs runtime/molt-runtime/tests/debug_call_bind.rs src/molt/debug/trace.py src/molt/cli.py tests/cli/test_cli_debug.py
git commit -m "runtime: add structured debug trace and success-path assertions"
```

## Task 4: Build The Canonical Reducer And Bisection Engine

**Files:**
- Create: `src/molt/debug/reduce.py`
- Create: `src/molt/debug/bisect.py`
- Create: `tests/test_debug_reduce.py`
- Modify: `src/molt/cli.py`
- Modify: `tools/ir_probe_supervisor.py`

- [ ] **Step 1: Write the failing reducer/bisector tests**

Cover:

- canonical oracle normalization;
- reduction from a manifest or source file;
- first-bad-pass bisection result shape;
- backend/profile/IC toggle bisection result shape;
- promotion-ready reduced manifest contents.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-t4
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/test_debug_reduce.py tests/test_ir_probe_supervisor.py
```

Expected: FAIL because reducer oracles and bisect output schemas are not yet canonicalized.

- [ ] **Step 3: Implement the normalized oracle model in `reduce.py`**

Support at least:

- exit classification;
- verifier failure classification;
- structured diff mismatch;
- trace/invariant signature match;
- manifest predicate checks.

- [ ] **Step 4: Implement `molt debug reduce`**

Support:

- input source path or prior manifest;
- canonical reduced artifact directory;
- retained failure signature;
- promotion-target recommendation in the output manifest.

- [ ] **Step 5: Implement `molt debug bisect`**

Support:

- first bad pass identification;
- pass-window narrowing;
- backend/profile/IC configuration toggles.

- [ ] **Step 6: Collapse or delete `tools/ir_probe_supervisor.py`**

Either:

- migrate the remaining useful logic into `src/molt/debug/bisect.py` or `src/molt/debug/verify.py`, or
- delete the tool and rewrite its coverage against the new CLI/API surface.

- [ ] **Step 7: Re-run the reducer/bisector tests and make them pass**

Re-run the Task 4 pytest command and keep only tests that target the canonical surface.

- [ ] **Step 8: Commit**

```bash
git add src/molt/debug/reduce.py src/molt/debug/bisect.py src/molt/cli.py tools/ir_probe_supervisor.py tests/test_debug_reduce.py tests/test_ir_probe_supervisor.py
git commit -m "debug: add canonical reducer and bisection engine"
```

## Task 5: Integrate Differential And Performance Debugging With Shared Manifests

**Files:**
- Create: `src/molt/debug/diff.py`
- Create: `src/molt/debug/perf.py`
- Create: `tests/test_debug_diff.py`
- Create: `tests/test_debug_perf.py`
- Modify: `src/molt/cli.py`
- Modify: `tools/profile_analyze.py`
- Modify: `tests/molt_diff.py`
- Modify: `runtime/molt-runtime/src/object/ops.rs`
- Modify: `runtime/molt-backend/src/lib.rs`
- Modify: `runtime/molt-backend/src/wasm.rs`

- [ ] **Step 1: Write the failing diff/perf tests**

Cover:

- `molt debug diff` wraps CPython-vs-Molt and backend/config differential runs with canonical manifests;
- `molt debug perf` renders per-pass timing and runtime counters in text and JSON;
- backend capability reporting is surfaced in manifests when a requested IR/perf lane is unsupported;
- existing profile-analyze behavior is either migrated or deleted.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-t5
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/test_debug_diff.py tests/test_debug_perf.py tests/test_molt_diff_expected_failures.py
```

Expected: FAIL because diff/perf still live in fragmented tool paths.

- [ ] **Step 3: Implement `src/molt/debug/diff.py`**

Provide a debug-facing orchestration layer that:

- records comparison dimensions;
- captures mismatch class;
- retains output references in the manifest;
- does not duplicate the differential harness semantics.

- [ ] **Step 4: Implement `src/molt/debug/perf.py`**

Provide:

- pass timing summaries;
- runtime counter summaries from `object/ops.rs`;
- profile-log ingestion formerly living in `tools/profile_analyze.py`;
- structured JSON output suitable for bench/perf regression tooling.

- [ ] **Step 5: Replace ad hoc backend dump/capability plumbing**

Update backend code so:

- `runtime/molt-backend/src/lib.rs` reports backend debug capabilities through one shared surface;
- `runtime/molt-backend/src/wasm.rs` stops acting like `TIR_DUMP` is a standalone public interface;
- backend IR formatting is reusable by `molt debug ir` and `molt debug perf`.

- [ ] **Step 6: Rewire or delete `tools/profile_analyze.py`**

Move any still-valuable parsing logic into `src/molt/debug/perf.py` and delete the standalone-authority script unless an internal automation delegate is still strictly needed.

- [ ] **Step 7: Re-run the diff/perf tests until green**

Re-run the Task 5 pytest command and keep the outputs manifest-backed and deterministic.

- [ ] **Step 8: Commit**

```bash
git add src/molt/debug/diff.py src/molt/debug/perf.py src/molt/cli.py tools/profile_analyze.py tests/molt_diff.py tests/test_debug_diff.py tests/test_debug_perf.py runtime/molt-runtime/src/object/ops.rs runtime/molt-backend/src/lib.rs runtime/molt-backend/src/wasm.rs
git commit -m "debug: integrate differential and perf tooling"
```

## Task 6: Delete Legacy Clutter, Rewrite Docs, And Close Convergence Gaps

**Files:**
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/DEVELOPER_GUIDE.md`
- Modify: `docs/INDEX.md`
- Modify/Delete: `tools/ir_dump.py`
- Modify/Delete: `tools/ir_probe_supervisor.py`
- Modify/Delete: `tools/profile_analyze.py`
- Modify/Delete: `tools/check_molt_ir_ops.py`
- Modify/Delete: `tests/test_ir_probe_supervisor.py`
- Modify/Delete: `tests/test_check_molt_ir_ops.py`
- Modify: `tests/cli/test_cli_debug.py`

- [ ] **Step 1: Write the failing documentation/cleanup tests**

Add or update tests that prove:

- the old standalone user-facing commands are no longer documented as authorities;
- `molt debug ...` is the only documented debug surface;
- any retained delegates are internal-only and not presented in CLI help/docs.

- [ ] **Step 2: Run the cleanup tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-t6
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/cli/test_cli_debug.py -k legacy
```

Expected: FAIL because docs and cleanup gates still reference legacy authorities.

- [ ] **Step 3: Delete or internalize the legacy scripts**

Do not leave publicly documented compatibility shims behind. For each legacy script:

- delete it if no automation still requires it;
- otherwise reduce it to a non-user-facing delegate and remove it from docs/help;
- update all in-repo call sites immediately.

- [ ] **Step 4: Rewrite the docs**

Update:

- `docs/OPERATIONS.md`
- `docs/DEVELOPER_GUIDE.md`
- `docs/INDEX.md`

so they point only at the canonical debug surface, artifact locations, verifier bundles, and cleanup policy.

- [ ] **Step 5: Run the focused cleanup tests**

Re-run the Task 6 pytest command and make the cleanup assertions pass.

- [ ] **Step 6: Run the end-to-end verification bundle**

Run:

```bash
export MOLT_SESSION_ID=debug-dx-final
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
UV_NO_SYNC=1 uv run --python 3.12 pytest -q tests/cli/test_cli_debug.py tests/test_debug_manifest.py tests/test_debug_ir.py tests/test_debug_verify.py tests/test_debug_reduce.py tests/test_debug_diff.py tests/test_debug_perf.py
cargo test -p molt-runtime --test debug_call_bind -- --nocapture
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug ir examples/hello.py --format json --out logs/debug/ir/final-smoke.json
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug verify --format json --out logs/debug/verify/final-smoke.json
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug ir --help
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug verify --help
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug trace --help
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug reduce --help
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug bisect --help
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug diff --help
UV_NO_SYNC=1 uv run --python 3.12 python3 -m molt.cli debug perf --help
```

Expected:

- pytest green;
- runtime trace/assert test green;
- functional IR and verifier smoke runs write retained artifacts;
- all canonical debug subcommands present and help text clean;
- no lingering documented legacy authority.

- [ ] **Step 7: Commit**

```bash
git add docs/OPERATIONS.md docs/DEVELOPER_GUIDE.md docs/INDEX.md tools/ir_dump.py tools/ir_probe_supervisor.py tools/profile_analyze.py tools/check_molt_ir_ops.py tests/test_ir_probe_supervisor.py tests/test_check_molt_ir_ops.py tests/cli/test_cli_debug.py
git commit -m "debug: remove legacy tooling authorities"
```

## Local Plan Review Notes

- This plan intentionally converges semantic logic into shared modules before deleting user-facing scripts so we do not lose verifier/probe behavior during cleanup.
- The quality bar is production-hardened compiler DX: deterministic evidence, explicit capability reporting, zero permanent duplicate interfaces, and verifiers/traces that would be acceptable in an LLVM-grade or NVIDIA-grade engineering environment.
- If implementation reveals that a file split in `src/molt/debug/` is too fine-grained, consolidate modules without violating the single-authority rule.
