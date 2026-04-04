# Dynamic-First Representation-Aware Backend Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hint-driven `SimpleIR` backend recovery and shadow-based
unboxed tracking with a shared representation-aware SSA lowering path for
native, WASM, and future LLVM backends.

**Architecture:** Keep semantic typed SSA in TIR, materialize explicit
representation-aware SSA in LIR, and make backends consume that contract
directly. Migrate op-family by op-family, but do not add any new permanent dual
architecture or user-visible fallback path.

**Tech Stack:** Rust (`molt-backend`, Cranelift, wasm encoder), existing TIR
pipeline, differential tests, benchmark harness, docs/spec infrastructure

---

## File Map

| Path | Responsibility |
| --- | --- |
| `runtime/molt-backend/src/tir/types.rs` | semantic TIR type lattice; may gain explicit mapping hooks to representation |
| `runtime/molt-backend/src/tir/mod.rs` | TIR module exports; wire new LIR modules in canonical order |
| `runtime/molt-backend/src/tir/function.rs` | shared function/block/value structures; source of SSA ownership |
| `runtime/molt-backend/src/tir/verify.rs` | verifier surface; extend for representation-aware invariants |
| `runtime/molt-backend/src/tir/printer.rs` | debug printing for new LIR/representation state |
| `runtime/molt-backend/src/tir/lower_from_simple.rs` | current frontend transport import; keep as transport ingress only |
| `runtime/molt-backend/src/tir/type_refine.rs` | semantic type refinement feeding representation choice |
| `runtime/molt-backend/src/tir/lower_to_simple.rs` | current transport egress; shrink or remove architectural responsibility over time |
| `runtime/molt-backend/src/tir/ssa.rs` | SSA and block-param mechanics; join/block-param invariants must stay representation-safe |
| `runtime/molt-backend/src/tir/ops.rs` | TIR/LIR op surface; add explicit conversion/representation ops only if needed |
| `runtime/molt-backend/src/tir/lir.rs` | new representation-aware SSA IR types |
| `runtime/molt-backend/src/tir/lower_to_lir.rs` | new lowering from typed TIR to representation-aware LIR |
| `runtime/molt-backend/src/tir/verify_lir.rs` | verifier for representation and join invariants |
| `runtime/molt-backend/src/native_backend/function_compiler.rs` | consume LIR directly; delete shadow recovery |
| `runtime/molt-backend/src/wasm.rs` | consume LIR directly for parity on the same contract |
| `runtime/molt-backend/src/llvm_backend/lowering.rs` | align revived LLVM path to the same contract |
| `runtime/molt-backend/src/ir.rs` | current `SimpleIR` transport; keep transitional only, then shrink/remove hints |
| `runtime/molt-backend/tests/` | Rust regression tests for join/loop/overflow/representation behavior |
| `tests/differential/basic/` | correctness regressions for arithmetic, loops, joins, truthiness |
| `tests/test_codec_lowering.py` | Python-side lowering assertions; update away from hint-centric expectations |
| `docs/spec/areas/compiler/0100_MOLT_IR.md` | canonical IR contract |
| `docs/spec/areas/compiler/SIMPLE_IR_JSON_SCHEMA.md` | current transport-only status; eventually reduce further |
| `docs/spec/areas/compiler/NATIVE_BACKEND_OPTIMIZATION.md` | current-state backend audit |
| `docs/spec/STATUS.md` | current-state blocker wording |
| `ROADMAP.md` | forward-looking priority wording |

## Coordination Constraints

- There is active partner work in `runtime/molt-backend/src/native_backend/function_compiler.rs`.
- Read and integrate with partner changes carefully; do not revert or overwrite
  them.
- Do not expand `raw_int_shadow`, `fast_int`, or transport-only hint fields as
  a new architecture.
- Every build/test command must set `MOLT_SESSION_ID` and canonical env roots.

## Task 1: Introduce Canonical Representation-Aware LIR And Verifier

**Files:**
- Create: `runtime/molt-backend/src/tir/lir.rs`
- Create: `runtime/molt-backend/src/tir/lower_to_lir.rs`
- Create: `runtime/molt-backend/src/tir/verify_lir.rs`
- Modify: `runtime/molt-backend/src/tir/mod.rs`
- Modify: `runtime/molt-backend/src/tir/printer.rs`
- Test: `runtime/molt-backend/tests/lir_representation_invariants.rs`

- [ ] **Step 1: Write failing Rust tests for the core invariants**

Cover:

- every SSA value has one representation;
- join blocks cannot merge mismatched incoming representations without explicit
  conversion;
- loop-carried `I64` values remain `I64` through block params;
- explicit `box` / `unbox` transitions are preserved by the verifier.

- [ ] **Step 2: Run the new test target and verify it fails**

Run:

```bash
export MOLT_SESSION_ID=lir-spec-1
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
cargo test -p molt-backend --features native-backend --test lir_representation_invariants -- --nocapture
```

Expected: FAIL because the new LIR types and verifier do not exist yet.

- [ ] **Step 3: Add `lir.rs` with the minimum canonical surface**

Define:

- representation enum (`DynBox`, `I64`, `F64`, `Bool1`);
- LIR value id / block param / function structures;
- explicit conversion ops sufficient for box/unbox/materialize semantics;
- typed block params and typed SSA values.

Do not add speculative future lanes in this first slice.

- [ ] **Step 4: Add `verify_lir.rs`**

Implement:

- dominance-safe representation checking;
- join input compatibility checks;
- no implicit repr change across block edges;
- explicit conversion requirement on mismatched incoming values.

- [ ] **Step 5: Wire module exports and debug printing**

Update:

- `tir/mod.rs`
- `tir/printer.rs`

so LIR can be inspected and verified in tests and diagnostics.

- [ ] **Step 6: Run the new tests and fix the verifier until green**

Run the Task 1 test target again and make it pass without weakening the
invariants.

- [ ] **Step 7: Commit**

```bash
git add runtime/molt-backend/src/tir/lir.rs runtime/molt-backend/src/tir/lower_to_lir.rs runtime/molt-backend/src/tir/verify_lir.rs runtime/molt-backend/src/tir/mod.rs runtime/molt-backend/src/tir/printer.rs runtime/molt-backend/tests/lir_representation_invariants.rs
git commit -m "backend: add representation-aware LIR contract"
```

## Task 2: Lower Typed TIR To Representation-Aware LIR For Hot Scalar Ops

**Files:**
- Modify: `runtime/molt-backend/src/tir/type_refine.rs`
- Modify: `runtime/molt-backend/src/tir/lower_to_lir.rs`
- Modify: `runtime/molt-backend/src/tir/verify.rs`
- Test: `runtime/molt-backend/tests/lir_scalar_lowering.rs`

- [ ] **Step 1: Write failing lowering tests**

Cover:

- integer constants lower to `I64` when proven;
- float constants lower to `F64` when proven;
- boolean comparisons lower to `Bool1`;
- unsupported/mixed lanes materialize to `DynBox`;
- overflow-capable integer arithmetic emits an explicit escape edge/materialize
  operation, not truncating reboxing.

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
export MOLT_SESSION_ID=lir-spec-2
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
cargo test -p molt-backend --features native-backend --test lir_scalar_lowering -- --nocapture
```

Expected: FAIL because lowering still feeds the old hint-based transport.

- [ ] **Step 3: Add deterministic representation selection rules**

Use `type_refine.rs` and TIR facts to choose:

- `I64`
- `F64`
- `Bool1`
- `DynBox`

Do not encode the choice as `fast_int`/`fast_float` bits in LIR.

- [ ] **Step 4: Lower arithmetic/comparison/control hot ops into LIR**

Implement explicit LIR lowering for:

- constants;
- `add`, `sub`, `mul`;
- `lt`, `le`, `gt`, `ge`, `eq`, `ne`;
- truthiness-to-branch conversion;
- box/unbox ops.

- [ ] **Step 5: Add explicit overflow/materialization semantics**

When `I64` arithmetic can overflow:

- emit explicit checked arithmetic result handling;
- materialize to `DynBox` on overflow;
- preserve exact Python integer semantics.

- [ ] **Step 6: Run the new scalar-lowering tests**

Make the lowering deterministic and green.

- [ ] **Step 7: Commit**

```bash
git add runtime/molt-backend/src/tir/type_refine.rs runtime/molt-backend/src/tir/lower_to_lir.rs runtime/molt-backend/src/tir/verify.rs runtime/molt-backend/tests/lir_scalar_lowering.rs
git commit -m "backend: lower hot scalar ops to representation-aware LIR"
```

## Task 3: Cut Native Join, Loop, And Block-Param Handling Over To LIR

**Files:**
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs`
- Test: `runtime/molt-backend/tests/entry_block_param_shadow.rs`
- Create: `runtime/molt-backend/tests/lir_loop_and_join_regressions.rs`

- [ ] **Step 1: Write failing native regressions**

Cover:

- entry block params are created before instructions;
- structured `if` joins consume typed block params directly;
- nested loop-carried integer variables remain unboxed through block params;
- no backend-local shadow bookkeeping is required for correctness.

- [ ] **Step 2: Run the focused native regression targets**

Run:

```bash
export MOLT_SESSION_ID=lir-spec-3
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
cargo test -p molt-backend --features native-backend --test entry_block_param_shadow --test lir_loop_and_join_regressions -- --nocapture
```

Expected: FAIL until the native backend is using LIR joins rather than shadow
state.

- [ ] **Step 3: Change native lowering to consume LIR block params directly**

Make:

- merge-block params representation-aware;
- loop-carried values representation-aware;
- branch conditions consume `Bool1` directly where available.

- [ ] **Step 4: Delete or collapse backend-local shadow bookkeeping for the cut-over slice**

For the op families migrated in this task, remove:

- shadow-only join synchronization;
- duplicate merge-block param recovery;
- any requirement that boxed names remain the source of truth.

- [ ] **Step 5: Verify native dev and release paths**

Run:

```bash
export MOLT_SESSION_ID=lir-spec-3-cli
export PYTHONPATH=$PWD/src
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$PWD/target
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
export MOLT_BACKEND_DAEMON=0
./.venv/bin/python -m molt.cli build --json --build-profile dev tests/differential/basic/stdlib_attr_access.py --output tmp/lir-dev-json
./.venv/bin/python -m molt.cli build --build-profile release tests/differential/basic/stdlib_attr_access.py --output tmp/lir-release-cli
```

Expected: successful builds, no block-param invariant panic.

- [ ] **Step 6: Commit**

```bash
git add runtime/molt-backend/src/native_backend/function_compiler.rs runtime/molt-backend/tests/entry_block_param_shadow.rs runtime/molt-backend/tests/lir_loop_and_join_regressions.rs
git commit -m "backend: consume LIR joins and loop params in native backend"
```

## Task 4: Cut WASM To The Same Representation Contract

**Files:**
- Modify: `runtime/molt-backend/src/wasm.rs`
- Modify: `runtime/molt-backend/src/tir/lower_to_wasm.rs`
- Test: `tests/test_wasm_class_smoke.py`
- Create: `runtime/molt-backend/tests/lir_wasm_repr_regressions.rs`

- [ ] **Step 1: Write failing WASM regressions**

Cover:

- typed join/block-param handling in WASM lowering;
- `I64` loop counters remain `I64`;
- boxed materialization only occurs at explicit boundaries.

- [ ] **Step 2: Run the failing WASM-focused tests**

Use the repo's existing WASM smoke targets plus the new Rust regression test.

- [ ] **Step 3: Update WASM lowering to consume LIR**

Match the native contract:

- same repr model;
- same join semantics;
- same overflow/materialization rules.

- [ ] **Step 4: Re-run native + WASM parity checks for touched semantics**

Focus on arithmetic, comparisons, joins, loops, and truthiness.

- [ ] **Step 5: Commit**

```bash
git add runtime/molt-backend/src/wasm.rs runtime/molt-backend/src/tir/lower_to_wasm.rs runtime/molt-backend/tests/lir_wasm_repr_regressions.rs tests/test_wasm_class_smoke.py
git commit -m "backend: align wasm lowering to representation-aware LIR"
```

## Task 5: Remove Legacy Hint-Centric Architecture From The Backend Core

**Files:**
- Modify: `runtime/molt-backend/src/ir.rs`
- Modify: `runtime/molt-backend/src/tir/lower_to_simple.rs`
- Modify: `runtime/molt-backend/src/tir/lower_from_simple.rs`
- Modify: `tests/test_codec_lowering.py`
- Modify: docs in `docs/spec/areas/compiler/` and `docs/spec/STATUS.md` if the current-state wording changes

- [ ] **Step 1: Write failing tests or assertions against the legacy architecture**

Cover:

- no new lowering logic depends on backend-local shadow recovery;
- no new hot-path lowering depends on `fast_int` / `raw_int` as the canonical
  representation contract;
- transport hints, if still present, are passive compatibility data only.

- [ ] **Step 2: Shrink legacy transport responsibility**

Move any remaining meaningful representation decisions out of:

- `lower_to_simple.rs`
- `ir.rs`

and into canonical LIR lowering.

- [ ] **Step 3: Delete shadow-centric code for fully migrated op families**

When a family is fully cut over:

- remove shadow maps;
- remove duplicate boxed/unboxed state sync;
- remove transport-specific special cases that exist only to preserve the old
  architecture.

- [ ] **Step 4: Update Python-side lowering tests**

Rewrite tests that currently assert `fast_int`/`raw_int` hints as the primary
artifact. The new assertions should focus on semantic type refinement and the
existence of explicit representation-aware lowering where appropriate.

- [ ] **Step 5: Commit**

```bash
git add runtime/molt-backend/src/ir.rs runtime/molt-backend/src/tir/lower_to_simple.rs runtime/molt-backend/src/tir/lower_from_simple.rs tests/test_codec_lowering.py docs/spec/areas/compiler/0100_MOLT_IR.md docs/spec/areas/compiler/SIMPLE_IR_JSON_SCHEMA.md docs/spec/areas/compiler/NATIVE_BACKEND_OPTIMIZATION.md docs/spec/STATUS.md
git commit -m "backend: remove legacy hint-centric lowering from core path"
```

## Task 6: Conformance, Benchmarks, And Docs Consolidation

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Delete: `docs/superpowers/specs/2026-03-30-conformance-perf-sprint-design.md`
- Delete: `docs/superpowers/plans/2026-03-30-conformance-perf-sprint.md`
- Add or modify targeted differential and benchmark tests

- [ ] **Step 1: Add focused differential regressions**

Prioritize:

- loop-heavy integer kernels;
- nested `if`/phi merges;
- exception-heavy paths that previously suffered from lowering drift;
- float-heavy arithmetic;
- boolean/truthiness-heavy control flow.

- [ ] **Step 2: Add benchmark checkpoints**

Track at minimum:

- `bench_attr_access.py`
- `bench_exception_heavy.py`
- `bench_class_hierarchy.py`
- `bench_struct.py`
- string kernels
- bytes/bytearray find kernels
- a sieve-like loop kernel

- [ ] **Step 3: Re-run canonical doc checks**

Run:

```bash
./.venv/bin/python tools/check_docs_architecture.py
./.venv/bin/python tools/update_status_blocks.py --check
./.venv/bin/python tools/bench_report.py --manifest bench/results/docs_manifest.json --check --update-status-doc
```

- [ ] **Step 4: Delete superseded sprint docs**

Delete the old 2026-03-30 sprint design and plan after verifying the new
canonical spec and migration plan cover the live backend architecture. Do not
keep duplicate "historical" backend design docs in the repo.

- [ ] **Step 5: Final validation sweep**

Run the highest-signal validation set for the migrated slice:

```bash
export MOLT_SESSION_ID=lir-final
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
./.venv/bin/python tools/check_docs_architecture.py
./.venv/bin/python -m pytest -q tests/test_codec_lowering.py tests/test_wasm_class_smoke.py
cargo test -p molt-backend --features native-backend -- --nocapture
```

- [ ] **Step 6: Commit**

```bash
git add docs/spec/STATUS.md ROADMAP.md docs/superpowers/specs/2026-04-03-dynamic-first-representation-aware-backend-design.md docs/superpowers/plans/2026-04-03-dynamic-first-representation-aware-backend-migration.md
git rm docs/superpowers/specs/2026-03-30-conformance-perf-sprint-design.md docs/superpowers/plans/2026-03-30-conformance-perf-sprint.md
git commit -m "docs: consolidate backend architecture around representation-aware LIR"
```

## Task 7: End-To-End CLI, Profile, Target, And Backend Validation Matrix

**Files:**
- Modify: `tests/cli/test_cli_import_collection.py`
- Modify: `tests/test_wasm_class_smoke.py`
- Modify: `tests/test_native_lir_loop_join_semantics.py`
- Modify: `tests/test_native_tir_skip_pattern.py`
- Add targeted smoke helpers under `tests/` if needed
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Define the canonical local E2E matrix**

Cover at minimum:

- CLI commands: `build`, `run`, `compare`, linked-WASM build/run helpers
- profiles: `dev`, `release`
- targets/backends: native/Cranelift and linked WASM
- proof shape: successful build/run, honest rebuild behavior, and clean failure
  behavior for intentionally unsupported semantics

- [ ] **Step 2: Add/refresh end-to-end tests for native lanes**

Cover:

- `molt.cli build` on `dev` and `release`
- direct binary execution after build
- runtime rebuild correctness after source/runtime changes
- unsupported dynamic-execution helpers (`eval`/`exec`) fail honestly without
  bypassing TIR/LIR

- [ ] **Step 3: Add/refresh end-to-end tests for linked-WASM lanes**

Cover:

- linked WASM build success from the CLI
- linked module execution with Node
- class/module-body smoke
- runtime-heavy smoke that exercises the real linked runtime path

- [ ] **Step 4: Add end-to-end UX assertions for failure surfaces**

Cover:

- no false `Successfully built ...` messages after linker/runtime failure
- rebuild-required paths surface the correct error text and nonzero exit
- backend feature mismatches fail with actionable messages

- [ ] **Step 5: Run the canonical E2E matrix**

Run:

```bash
export MOLT_SESSION_ID=lir-e2e
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src ./.venv/bin/python -m pytest -q tests/test_native_tir_skip_pattern.py tests/test_native_lir_loop_join_semantics.py tests/test_wasm_class_smoke.py
```

- [ ] **Step 6: Sync current-state docs**

Update:

- `docs/spec/STATUS.md`
- `ROADMAP.md`

to state that the backend migration now includes an explicit CLI/profile/target
validation matrix rather than only backend-internal proof targets.

- [ ] **Step 7: Commit**

```bash
git add tests/cli/test_cli_import_collection.py tests/test_native_tir_skip_pattern.py tests/test_native_lir_loop_join_semantics.py tests/test_wasm_class_smoke.py docs/spec/STATUS.md ROADMAP.md
git commit -m "tests: add end-to-end CLI/profile/target validation matrix"
```
