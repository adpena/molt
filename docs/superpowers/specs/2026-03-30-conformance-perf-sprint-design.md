# Sprint: 100% Conformance + Performance Activation

**Date:** 2026-03-30
**Type:** Single deep session
**Exit criteria:** 100% pass rate on supported-feature conformance suite + sieve < 30ms

### Progress snapshot (updated 2026-04-01)

| Area | Status |
|------|--------|
| Phase 1 P0 blockers | 3/5 fixed (CONST-in-loop, generators, sieve regression). `__annotations__` likely fixed (0 SIGSEGVs). **TIR exception handling is the new #1 blocker.** |
| Phase 2 baseline | Done — 78% runtime parity (197/254), 0 SIGSEGVs |
| Phase 2 xfail tags | Done — 158 tests tagged (commit `0cd0cb40e`), 494/736 total with skip/xfail |
| Phase 2 cluster-fix | Not started |
| Phase 3 performance | Partial — 6+ type specializations wired, raw_int arithmetic disabled (NaN-box truncation) |
| Exit criteria met? | **No** — 78% not 100%; sieve timing unconfirmed |

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

### 1.1 CONST-in-loop materialization failure — ✅ FIXED
- **Root cause:** CSE pass creating dangling cross-block aliases (not `is_block_filled` as hypothesized)
- **Fix:** CSE global alias resolution (`7ddd6fd77`) + dominator filtering (`63c4962d5`)
- **Verified:** `sieve(100) = 25` correct

### 1.2 Sieve regression — ✅ FIXED
- **Same root cause** as 1.1 — CSE alias fix resolved both
- **Verified:** `sieve(100000) = 9592` correct (commit `5489f7000`)

### 1.3 Generator state machine — ✅ FIXED
- **Fix:** 4 commits: `5e527b247` (yield all elements), `12c2e887d` (HEADER_STATE_OFFSET), `66e79dd20` (CONST_NONE preserved), `a23de4377` (iteration value extraction)
- **Verified:** `list(x for x in [1,2,3])` returns `[1, 2, 3]`

### 1.4 `__annotations__` SIGSEGV — likely resolved (needs retest)
- 0 SIGSEGVs in baseline. Broader phi/type_id fixes likely resolved this indirectly.
- Related: `84c8e7fbe` stripped `__annotate__` call sites

### 1.5 TIR verification failure — ✅ FIXED (but new blocker emerged)
- **Fix:** SSA two-pass dominator walk (`db42ea341`), sealed blocks, TIR default-ON (`d6b3692ac`)
- **NEW BLOCKER (B9):** TIR `lower_to_simple` strips exception labels → all try/except fails. Root cause documented in `0639abad3`. WIP `a2c6be8e0`. This is the **current #1 blocker**.

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

### 3.1 Activate raw_int fast paths — PARTIAL
- 6+ type specializations wired via TIR type inference (`07f782c38`, `3b2325af6`)
- fast_int paths guarded against bigint pointers (`868052bdf`)
- list[int] flat i64 storage end-to-end (`db6952258`, `42af7f0f3`)
- **raw_int arithmetic chains DISABLED** (commit `5489f7000`) — `box_int_value` truncates beyond 47-bit NaN-box range. Only comparison chains (icmp-based) remain active.
- **TODO:** Insert overflow guards to re-enable arithmetic chains safely

### 3.2 Sieve < 30ms — UNCONFIRMED
- Memory notes reference 13ms with type specialization, but not in formal baseline
- raw_int arithmetic disabled means full fast path not active
- Need fresh benchmark with current codebase

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
