# Sprint: 100% Conformance + Performance Activation

**Date:** 2026-03-30
**Type:** Single deep session
**Exit criteria:** 100% pass rate on supported-feature conformance suite + sieve < 30ms

## Scope

### In scope
- All Python 3.12+ features: control flow, functions, classes, decorators, descriptors, metaclasses, async/await, generators, async generators, comprehensions, exception handling, stdlib modules, builtins
- Performance: raw_int TIR unboxing pass activation, sieve benchmark < 30ms

### Out of scope (by parity contract)
- `exec()` / `eval()` / `compile()` — no dynamic code generation
- Runtime monkeypatching of builtins/stdlib
- Unrestricted reflection (`__code__`, `__globals__` mutation)

Tests exercising these features will be tagged `xfail` with documented reason.

## Phase 1: Kill P0 Blockers

### 1.1 CONST-in-loop materialization failure
- **Symptom:** `eq(n, 1)` inside while loop receives `b=0x0` for constant `1` on all iterations
- **Hypothesis:** `is_block_filled` flag in main op compilation loop is `true` when CONST op runs, causing ops to be skipped
- **Investigation:** Add `eprintln!` inside `"const"` handler (~`function_compiler.rs:1171`) gated by `MOLT_DEBUG_CONST=1`, print `is_block_filled`, `op_idx`, `op.value`
- **Test file:** `/tmp/test_eq_loop.py` (already exists)
- **Success:** `print(n)` outputs `1` (break fires at n==1)

### 1.2 Sieve regression
- **Symptom:** sieve returns 0 after commit 49fc7d33 (0-init for loop vars)
- **Likely same root cause** as 1.1 — verify after CONST fix
- **If independent:** bisect between 49fc7d33 and 15feab12

### 1.3 Generator state machine
- **Symptom:** Generator yields only first element
- **Root cause:** State machine bug in poll function compilation
- **Test:** `list(x for x in [1,2,3])` should return `[1, 2, 3]`

### 1.4 `__annotations__` SIGSEGV
- **Symptom:** `__annotations__` access crashes with SIGSEGV
- **Investigation:** Check runtime attribute dispatch for `__annotations__` key

### 1.5 TIR verification failure
- **Symptom:** `builtins__molt_module_chunk_2` — invalid SSA from partner TIR optimization
- **Investigation:** Run with `MOLT_SSA_DIAG=1` (partner's diagnostic already in unstaged changes)

## Phase 2: Conformance Audit + Systematic Sweep

### 2.1 Baseline capture
- Run all 736 tests in `tests/differential/basic/`
- Capture: pass / fail / crash / timeout for each
- Record baseline pass rate

### 2.2 Triage
- Tag tests for exec/eval/compile/monkeypatch/reflection as `xfail`
- Cluster remaining failures by root cause category:
  - **SSA/codegen:** Variable resolution, phi merging, block ordering
  - **Runtime:** Type dispatch, attribute lookup, protocol methods
  - **Stdlib:** Missing builtins, incorrect behavior
  - **Control flow:** Loop/break/continue/exception interaction
  - **OOP:** MRO, descriptors, metaclasses, `__init_subclass__`
  - **Async:** Event loop, coroutine protocol, async generators
  - **Generators:** yield/yield from/send/throw/close

### 2.3 Fix by cluster
- Priority order: biggest cluster first (most failures per fix)
- After each cluster fix: re-run full suite, record delta
- Continue until 100% of supported tests pass

### 2.4 Regression guard
- Any fix that breaks a previously-passing test is immediately reverted and reworked
- Commit after every successful build (per project policy)

## Phase 3: Performance

### 3.1 Activate raw_int fast paths
- **Problem:** TIR emits `raw_int` but TIR I64 != raw i64 at SimpleIR level
- **Fix:** Implement explicit unbox/rebox insertion in TIR unboxing pass
- Already wired for add/sub/mul/lt — needs activation

### 3.2 Sieve < 30ms
- Current: 43ms (CPython: 23ms)
- Profile NaN-boxing overhead in sieve hot loop
- Target: < 30ms (1.3x CPython, down from 1.9x)

### 3.3 Measure and report
- Benchmark: sieve, fib(30), startup
- Compare before/after raw_int activation
- Document results

## Constraints

- Max 2 build-triggering agents concurrently (OOM risk)
- Commit + push after every successful build
- Never revert unstaged partner changes
- `git add` immediately after every file write
- Preserve partner diagnostic instrumentation in `lib.rs` and `tir/ssa.rs`
