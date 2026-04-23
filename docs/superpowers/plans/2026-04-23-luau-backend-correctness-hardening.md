# Luau Backend Correctness Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `--target luau` fail closed, remove silent semantic gaps from the admitted path, reconcile proof/code/docs drift, and establish the path toward full Luau parity and performance coverage.

**Architecture:** The immediate slice hardens the existing string-emitting backend while creating gates that make unsupported semantics explicit. The long-term migration moves emission and optimization authority into structured `LuauIR`, backed by generated support matrices, correspondence checks, Lune differential tests, `luau-analyze`, and benchmark evidence.

**Tech Stack:** Rust (`runtime/molt-backend`), Python CLI/tests/tools, Lean/Quint formal surfaces, Lune/Luau analyzer, repository documentation under `docs/spec`.

---

### Task 1: Fail-Closed Luau Build Path

**Files:**
- Modify: `runtime/molt-backend/src/main.rs`
- Modify: `runtime/molt-backend/src/luau.rs`
- Test: `runtime/molt-backend/src/luau.rs`

- [ ] **Step 1: Write failing tests**

Add Rust unit coverage that:
- `compile_checked()` rejects `matmul` unsupported markers.
- `compile_via_ir()` returns an error or is removed/replaced so validation failures cannot silently produce unchecked output.
- The default Luau backend dispatch uses checked compilation.

- [ ] **Step 2: Run failing tests**

Run:
```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
cargo test -p molt-backend --features luau-backend luau::tests::test_compile_checked_rejects_unsupported_matmul
```

Expected: FAIL before implementation because default build/preview paths can bypass validation.

- [ ] **Step 3: Implement fail-closed compile path**

Change Luau CLI/backend dispatch to call `compile_checked()` by default and propagate validation errors as process failures. Delete or replace the fail-open `compile_via_ir()` warning fallback.

- [ ] **Step 4: Run green tests**

Run the same targeted Cargo test command. Expected: PASS.

- [ ] **Step 5: Stage**

Run:
```bash
git add runtime/molt-backend/src/main.rs runtime/molt-backend/src/luau.rs
```

### Task 2: Reject All Semantic Stub Markers

**Files:**
- Modify: `runtime/molt-backend/src/luau.rs`
- Test: `runtime/molt-backend/src/luau.rs`

- [ ] **Step 1: Write failing tests**

Add validation tests for all currently silent semantic markers:
- `[async: ...]`
- `[file: ...]`
- `[context: ...]`
- `[internal: ...]`
- `[stub: ...]`
- explicit accepted semantic gaps for identity or missing support

- [ ] **Step 2: Run failing tests**

Run targeted Cargo tests for the validation functions. Expected: FAIL because `validate_luau_source()` currently allows several marker classes.

- [ ] **Step 3: Implement stricter validation**

Replace marker-specific permissive validation with a fail-closed classifier. Only comments proven non-semantic may remain allowed.

- [ ] **Step 4: Run green tests**

Run the targeted validation tests and then all Luau backend unit tests.

- [ ] **Step 5: Stage**

Run:
```bash
git add runtime/molt-backend/src/luau.rs
```

### Task 3: Proof-Code Correspondence Green

**Files:**
- Modify: `formal/lean/MoltTIR/Backend/LuauEmit.lean`
- Modify: `formal/lean/MoltTIR/Backend/LuauCorrect.lean`
- Modify: `formal/lean/BACKEND_PROOF_STATUS.md`
- Modify: `docs/spec/areas/formal/CERTIFICATION_STATUS.md`
- Test: `tools/check_correspondence.py`

- [ ] **Step 1: Write failing correspondence expectations**

Use current failures as the regression baseline:
```bash
python3 tools/check_correspondence.py --category luau_builtins --json
python3 tools/check_correspondence.py --category luau_operators --json
```

Expected initial failures:
- `list_get -> molt_list_get`
- `list_set -> molt_list_set`
- operator rows for `and`, `or`, and `in`

- [ ] **Step 2: Fix Lean/code correspondence truthfully**

Update Lean mappings to match real backend semantics, not aspirational helpers. Do not add inert strings to Rust just to satisfy checks.

- [ ] **Step 3: Close or downgrade the Luau proof claim**

Either close the remaining `sorry` in `LuauCorrect.lean` or update proof status documents to stop claiming the file is sorry-free.

- [ ] **Step 4: Run green correspondence checks**

Run both correspondence commands again. Expected: exit 0.

- [ ] **Step 5: Stage**

Run:
```bash
git add formal/lean/MoltTIR/Backend/LuauEmit.lean formal/lean/MoltTIR/Backend/LuauCorrect.lean formal/lean/BACKEND_PROOF_STATUS.md docs/spec/areas/formal/CERTIFICATION_STATUS.md
```

### Task 4: Luau Support Matrix

**Files:**
- Create: `tools/gen_luau_support_matrix.py`
- Create: `docs/spec/areas/compiler/luau_support_matrix.generated.md`
- Modify: `docs/spec/areas/compiler/LUAU_BACKEND_OPTIMIZATION.md`
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Generate current op support table**

Parse `runtime/molt-backend/src/luau.rs` op arms and emit support categories:
`implemented-exact`, `implemented-target-limited`, `compile-error`, `runtime-capability-error`, `not-admitted`, `unknown`.

- [ ] **Step 2: Add strict checker mode**

The generator must fail if any op is `unknown` or if docs mention a stale Luau support claim.

- [ ] **Step 3: Update docs from generated facts**

Document current support using the generated file and remove stale workaround claims.

- [ ] **Step 4: Verify**

Run:
```bash
python3 tools/gen_luau_support_matrix.py --write
python3 tools/gen_luau_support_matrix.py --check
```

- [ ] **Step 5: Stage**

Run:
```bash
git add tools/gen_luau_support_matrix.py docs/spec/areas/compiler/luau_support_matrix.generated.md docs/spec/areas/compiler/LUAU_BACKEND_OPTIMIZATION.md docs/spec/STATUS.md ROADMAP.md
```

### Task 5: Luau Differential Gate Expansion

**Files:**
- Modify: `tests/luau/test_luau_differential.py`
- Modify: `tests/luau/test_molt_luau_correctness.py`
- Modify: `tools/check_luau_static.py`
- Modify: `.github/workflows/molt-wasm-ci.yml`

- [ ] **Step 1: Replace silent skips with support decisions**

Unsupported features must map to explicit support-matrix entries instead of blanket string-based skips.

- [ ] **Step 2: Require real Luau runtime/analyzer in Luau CI lane**

CI should run at least one `lune` smoke and one `luau-analyze` static check when testing Luau changes.

- [ ] **Step 3: Add parity cases**

Add CPython-vs-Lune cases for identity, bool arithmetic, tuple/multi-return, module attr resolution, list/dict mutation, and exception behavior.

- [ ] **Step 4: Verify**

Run targeted Luau test subsets and static checker.

- [ ] **Step 5: Stage**

Run:
```bash
git add tests/luau/test_luau_differential.py tests/luau/test_molt_luau_correctness.py tools/check_luau_static.py .github/workflows/molt-wasm-ci.yml
```

### Task 6: Structured LuauIR Migration

**Files:**
- Modify: `runtime/molt-backend/src/luau_ir.rs`
- Modify: `runtime/molt-backend/src/luau_lower.rs`
- Modify: `runtime/molt-backend/src/luau.rs`
- Test: `runtime/molt-backend/src/luau_lower.rs`

- [ ] **Step 1: Pick one op family**

Start with constants, arithmetic, comparisons, and simple returns.

- [ ] **Step 2: Add equivalence tests**

For the selected op family, legacy source and LuauIR source must be behavior-equivalent under Lune and structurally validated by `luau-analyze`.

- [ ] **Step 3: Migrate optimization logic**

Move source-text optimizations for the selected family into LuauIR passes using def-use information.

- [ ] **Step 4: Verify**

Run targeted Rust unit tests and Luau differential tests.

- [ ] **Step 5: Stage**

Run:
```bash
git add runtime/molt-backend/src/luau_ir.rs runtime/molt-backend/src/luau_lower.rs runtime/molt-backend/src/luau.rs
```

### Task 7: Luau Performance Baseline and Optimization Gates

**Files:**
- Modify: `bench/luau/run_benchmarks.py`
- Modify: `tools/benchmark_luau_vs_cpython.py`
- Create: `bench/results/luau/README.md`
- Modify: `docs/spec/areas/compiler/LUAU_BACKEND_OPTIMIZATION.md`

- [ ] **Step 1: Baseline current performance**

Run Luau-compatible benchmarks and write JSON under `bench/results/luau/`.

- [ ] **Step 2: Add regression threshold support**

Benchmark tooling must compare against baseline and fail on configured regressions.

- [ ] **Step 3: Land measured optimizations only**

Implement table preallocation, callargs elimination, builtin wrapper elimination, and loop canonicalization only with parity and benchmark evidence.

- [ ] **Step 4: Verify**

Run benchmark subset, Luau correctness corpus, and static analyzer.

- [ ] **Step 5: Stage**

Run:
```bash
git add bench/luau/run_benchmarks.py tools/benchmark_luau_vs_cpython.py bench/results/luau/README.md docs/spec/areas/compiler/LUAU_BACKEND_OPTIMIZATION.md
```

### Task 8: Roblox Deploy Hardening

**Files:**
- Modify: `src/molt/cli.py`
- Modify: `docs/cli-reference.md`
- Test: `tests/` CLI/deploy tests for Roblox target

- [ ] **Step 1: Add failing CLI tests**

Deploy should fail if Luau validation fails, static analysis is required and unavailable, or generated artifact is missing.

- [ ] **Step 2: Implement deploy validation pipeline**

`molt deploy roblox` should run checked build, optional Lune smoke, optional analyzer, and produce explicit artifact metadata.

- [ ] **Step 3: Verify**

Run targeted CLI tests and a dry-run deploy build.

- [ ] **Step 4: Stage**

Run:
```bash
git add src/molt/cli.py docs/cli-reference.md tests
```
