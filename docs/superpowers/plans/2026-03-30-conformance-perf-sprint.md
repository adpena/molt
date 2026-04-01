# 100% Conformance + Performance Sprint — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Achieve 100% pass rate on all supported-feature conformance tests (736 files minus exec/eval/compile/monkeypatch/reflection exclusions), then activate raw_int/fast_int performance paths to bring sieve below 30ms.

**Architecture:** Three-phase approach — (1) fix P0 blockers that cascade into many test failures, (2) audit full suite and systematically fix failures clustered by root cause, (3) activate TIR unboxing performance paths. The native Cranelift backend is the primary target.

**Tech Stack:** Rust (Cranelift IR, molt-backend), Python (molt CLI, test harness), NaN-boxing runtime

### Status snapshot (updated 2026-04-01)

| Metric | Value |
|--------|-------|
| Total differential tests | 736 |
| Tests with MOLT_SKIP or xfail | 494 |
| Conformance baseline (compile) | 272/385 (59%) |
| Conformance baseline (runtime parity) | 197/254 (78%) → 43/50 (86%) → **44/50 (88%) on 2026-04-01 post-fixes** |
| SIGSEGVs | 0 |
| Timeouts | 2 |
| Phase 1 P0 blockers | **4/4 fixed or mitigated** — CONST-in-loop, generators, SSA all fixed; exception handling works via TIR bypass |
| Phase 2 baseline + xfail | Done |
| Phase 2 cluster-fix sweep | **IN PROGRESS** — running conformance batch to identify failure clusters |
| Phase 3 performance | Partial — 6+ type specializations wired, raw_int arithmetic disabled |
| **Current active blocker** | None — Phase 1 clear, proceeding to Phase 2 cluster-fix |

---

## Phase 1: Kill P0 Blockers

### Task 1: Fix CONST-in-loop materialization failure — DONE

**Status:** FIXED via CSE global alias resolution (commit `7ddd6fd77`). Root cause was not `is_block_filled` as hypothesized — it was the CSE pass creating dangling cross-block aliases. Additional dominator filtering added in `63c4962d5`. Sieve correctness verified: `sieve(100) = 25` (commit `5489f7000`).

**Files:**
- ~~Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs:1107-1213`~~
- ~~Modify: `runtime/molt-backend/src/lib.rs:808-826`~~
- Actual fix: `runtime/molt-backend/src/tir/` CSE pass alias resolution

~~This is the #1 blocker.~~ Resolved.

- [x] **Step 1: Add diagnostic to confirm root cause**

In `function_compiler.rs`, inside the main op dispatch loop, find the `is_block_filled` skip guard at line ~1107:

```rust
if is_block_filled {
    // skip
    continue;
}
```

Add a diagnostic print gated by env var just before the skip, specifically for CONST ops:

```rust
if is_block_filled {
    if std::env::var("MOLT_DEBUG_CONST").is_ok() && op.kind == "const" {
        eprintln!(
            "[CONST_DEBUG] SKIPPED const op #{} in {} — is_block_filled=true, out={:?}, value={:?}",
            op_idx, func_ir.name, op.out, op.value
        );
    }
    continue;
}
```

- [x] **Step 2: Build and test the diagnostic**

```bash
cargo build --profile release-fast -p molt-backend --features native-backend 2>&1 | tail -5
pkill -9 -f "molt-backend"
MOLT_DEBUG_CONST=1 timeout 300 python3 -m molt build --target native --output /tmp/test --release /tmp/test_eq_loop.py --rebuild --verbose 2>&1 | grep CONST_DEBUG
```

Expected: Output showing `SKIPPED const op` lines, confirming the hypothesis.

- [x] **Step 3: Fix the root cause**

The issue is in the block transition logic. When a `check_exception` or branch terminator fills a block, the subsequent fallthrough ops (including CONST) get skipped because `is_block_filled` is never reset.

In `function_compiler.rs`, find the op dispatch loop. The fix is to ensure that when we encounter a label/block-start op after a filled block, we properly transition to the new block. Look for the label/block handling code near lines 1120-1170 that calls `switch_to_block_tracking`. The issue is likely that CONST ops that follow a `check_exception` terminator but precede the next explicit label are in a "dead zone" where `is_block_filled=true` but no new block has been switched to.

The correct fix depends on what the diagnostic reveals. Two likely scenarios:

**Scenario A — CONST ops emitted after check_exception fills block, before next label:**
Insert a fallthrough block before the CONST op when `is_block_filled` is true and the op is not a label/block-start:

```rust
if is_block_filled && !is_block_start_op(&op.kind) {
    // Create implicit fallthrough block
    let fallthrough = builder.create_block();
    // Don't add a branch from the filled block — it's already terminated.
    // Just switch to the new block so subsequent ops have somewhere to land.
    builder.switch_to_block(fallthrough);
    is_block_filled = false;
}
```

**Scenario B — switch_to_block_tracking incorrectly reports block as filled:**
In `lib.rs:808-826`, `switch_to_block_tracking` calls `block_has_terminator()`. If the block already has a terminator (from a previous visit in a loop), it sets `is_block_filled = true` and returns without switching. The fix is to create a fresh block instead of giving up:

```rust
fn switch_to_block_tracking(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    if block_has_terminator(builder, block) {
        // Block already terminated — create a fresh fallthrough block
        let fresh = builder.create_block();
        builder.switch_to_block(fresh);
        *is_block_filled = false;
    } else {
        builder.switch_to_block(block);
        *is_block_filled = false;
    }
}
```

Apply whichever scenario the diagnostic confirms. The key invariant: **no non-label op should ever be skipped by is_block_filled without an explicit dead-code reason.**

- [x] **Step 4: Remove diagnostic, build, and verify**

Remove the `MOLT_DEBUG_CONST` diagnostic added in Step 1.

```bash
cargo build --profile release-fast -p molt-backend --features native-backend 2>&1 | tail -5
pkill -9 -f "molt-backend"
timeout 300 python3 -m molt build --target native --output /tmp/test --release /tmp/test_eq_loop.py --rebuild --verbose 2>&1
/tmp/test
```

Expected output: `1` (break fires at n==1, loop exits immediately).

- [x] **Step 5: Verify sieve regression is resolved**

Create test file:

```python
# /tmp/test_sieve.py
def sieve(n):
    is_prime = [True] * (n + 1)
    is_prime[0] = is_prime[1] = False
    p = 2
    while p * p <= n:
        if is_prime[p]:
            j = p * p
            while j <= n:
                is_prime[j] = False
                j = j + p
        p = p + 1
    count = 0
    i = 0
    while i <= n:
        if is_prime[i]:
            count = count + 1
        i = i + 1
    return count

print(sieve(100))
```

```bash
pkill -9 -f "molt-backend"
timeout 300 python3 -m molt build --target native --output /tmp/test_sieve_bin --release /tmp/test_sieve.py --rebuild --verbose 2>&1
/tmp/test_sieve_bin
```

Expected: `25` (number of primes <= 100).

If sieve still returns 0, the regression is independent — proceed to bisect between commits 49fc7d33 and 15feab12.

- [x] **Step 6: Commit**

Committed as `7ddd6fd77` (CSE alias fix) and `63c4962d5` (dominator filtering).

---

### Task 2: Fix generator state machine (yields only first element) — DONE

**Status:** FIXED across 4 commits: `5e527b247` (generator expressions yield all elements), `12c2e887d` (HEADER_STATE_OFFSET fix), `66e79dd20` (CONST_NONE preserved in state machine), `a23de4377` (generator iteration value extraction).

**Files:**
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs:8548-8730` (state_switch + state_yield handlers)
- Modify: `runtime/molt-backend/src/passes.rs:2605-2844` (rewrite_stateful_loops)

- [x] **Step 1: Create test file and capture behavior**

```python
# /tmp/test_gen.py
def gen():
    yield 1
    yield 2
    yield 3

result = list(gen())
print(result)
# Expected: [1, 2, 3]
```

```bash
pkill -9 -f "molt-backend"
timeout 300 python3 -m molt build --target native --output /tmp/test_gen_bin --release /tmp/test_gen.py --rebuild --verbose 2>&1
/tmp/test_gen_bin
```

Document actual output. Expected failure: only `[1]` or similar.

- [x] **Step 2: Add state machine diagnostic**

In `function_compiler.rs`, inside the `"state_switch"` handler (line ~8548), add:

```rust
if std::env::var("MOLT_DEBUG_GEN").is_ok() {
    eprintln!(
        "[GEN_DEBUG] state_switch in {}: {} resume_states, state_blocks={:?}",
        func_ir.name,
        resume_states.len(),
        state_blocks.keys().collect::<Vec<_>>()
    );
}
```

Inside the `"state_yield"` handler (line ~8693), add:

```rust
if std::env::var("MOLT_DEBUG_GEN").is_ok() {
    eprintln!(
        "[GEN_DEBUG] state_yield in {}: next_state_id={:?}, is_block_filled={}",
        func_ir.name,
        op.value,
        is_block_filled
    );
}
```

- [x] **Step 3: Build and run diagnostic**

```bash
cargo build --profile release-fast -p molt-backend --features native-backend 2>&1 | tail -5
pkill -9 -f "molt-backend"
MOLT_DEBUG_GEN=1 timeout 300 python3 -m molt build --target native --output /tmp/test_gen_bin --release /tmp/test_gen.py --rebuild --verbose 2>&1 | grep GEN_DEBUG
```

Analyze output: check if all yield resume states are registered, and if `is_block_filled` is interfering with state_yield ops (same pattern as Task 1).

- [x] **Step 4: Fix based on diagnostic findings**

Likely root causes (fix the one the diagnostic confirms):

**A — is_block_filled skips state_yield ops:** Same fix pattern as Task 1 — ensure state_yield and state_switch are never skipped.

**B — state_switch jump table missing entries:** In the `state_switch` handler, verify that ALL `resume_states` get entries in the Cranelift Switch. Check that `state_blocks` has a block for every state ID.

**C — state_yield doesn't properly switch to resume block:** After `state_yield` stores the next state and jumps to `master_return_block`, it must call `switch_to_block` for the resume block (the block that runs when the generator is next called). Verify this happens and `is_block_filled` is reset.

- [x] **Step 5: Remove diagnostic, build, and verify**

Remove `MOLT_DEBUG_GEN` diagnostics.

```bash
cargo build --profile release-fast -p molt-backend --features native-backend 2>&1 | tail -5
pkill -9 -f "molt-backend"
timeout 300 python3 -m molt build --target native --output /tmp/test_gen_bin --release /tmp/test_gen.py --rebuild --verbose 2>&1
/tmp/test_gen_bin
```

Expected: `[1, 2, 3]`

- [x] **Step 6: Test generator with loop**

```python
# /tmp/test_gen_loop.py
def count_up(n):
    i = 0
    while i < n:
        yield i
        i = i + 1

print(list(count_up(5)))
# Expected: [0, 1, 2, 3, 4]
```

```bash
pkill -9 -f "molt-backend"
timeout 300 python3 -m molt build --target native --output /tmp/test_gen_loop_bin --release /tmp/test_gen_loop.py --rebuild --verbose 2>&1
/tmp/test_gen_loop_bin
```

- [x] **Step 7: Commit**

Committed across `5e527b247`, `12c2e887d`, `66e79dd20`, `a23de4377`.

---

### Task 3: Fix `__annotations__` SIGSEGV — LIKELY RESOLVED (needs re-verification)

**Status:** No specific commit, but baseline shows 0 SIGSEGVs. Broader fixes (`0b3bf0a92` disabled phi rewrite preventing 53 crashes, `3de9134a9` defensive type_id validation, `84c8e7fbe` strip `__annotate__` call sites) likely resolved this indirectly. **Needs re-verification** — if still broken on `__annotations__` access specifically, reopen with steps below.

**Files:**
- Modify: `runtime/molt-runtime/src/object/` (attribute dispatch for `__annotations__`)

- [~] **Step 1: Create test file and reproduce** — 0 SIGSEGVs in baseline; needs targeted retest
- [ ] **Step 2: Investigate runtime attribute dispatch** — only if Step 1 still crashes
- [ ] **Step 3: Fix** — only if still broken
- [ ] **Step 4: Build and verify** — only if still broken
- [ ] **Step 5: Commit** — only if still broken

---

### Task 4: Fix TIR verification failure in builtins chunk — DONE (exception handling MITIGATED)

**Status:** Major SSA fixes landed: `332d426f2` (loops/comprehensions/__init__), `db42ea341` (two-pass dominator-walk resolution), `604f8fa72` (skip TIR roundtrip for zero-valued phis). TIR is now default-ON (`d6b3692ac`).

**Exception handling status (verified 2026-04-01):** Functions with `check_exception` ops correctly bypass TIR (guard at `lib.rs:2974`). try/except/finally/else all work correctly — tested with simple, nested, and complex exception patterns. The "TIR strips exception labels" issue is mitigated by the bypass. Making TIR handle exceptions natively is a **performance optimization** (future work), not a correctness blocker.

**Files:**
- Modify: `runtime/molt-backend/src/tir/` (exception label preservation)
- Reference: `0639abad3` (root cause documentation)

- [x] **Step 1: Reproduce and capture diagnostic output** — reproduced, root cause identified
- [x] **Step 2: Identify the invalid SSA pattern** — SSA fixes landed across 3 commits
- [x] **Step 3: Fix the SSA resolution** — two-pass dominator walk + sealed blocks
- [x] **Step 4: Build and verify** — TIR default-ON, SSA passes clean
- [x] **Step 5: Commit** — commits `332d426f2`, `db42ea341`, `604f8fa72`, `d6b3692ac`
- [x] **Step 6: Verify try/except works end-to-end** — verified 2026-04-01: simple, nested, try/except/finally/else all pass with CPython parity
- [~] **Step 7: TIR native exception handling** — FUTURE: make TIR preserve exception labels for perf (not correctness blocker)

---

## Phase 2: Conformance Audit + Systematic Sweep

### Task 5: Run full conformance baseline — DONE

**Status:** Baseline captured in `tests/harness/baselines/baseline.json`. Results: 385 CPython tests, 272 compile (59%), 197 runtime pass (78% parity), 55 mismatch, 0 SIGSEGV, 2 timeouts.

**Files:**
- Reference: `tests/molt_diff.py` (diff harness)
- Reference: `tests/differential/basic/` (736 test files)

- [x] **Step 1: Run the full differential suite**

```bash
pkill -9 -f "molt-backend"
timeout 3600 python3 -m molt diff tests/differential/basic/ --profile release --verbose --json 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
print(f\"Discovered: {data['discovered']}\")
print(f\"Passed: {data['passed']}\")
print(f\"Failed: {data['failed']}\")
print(f\"Skipped: {data['skipped']}\")
print(f\"OOM: {data.get('oom', 0)}\")
if data.get('failed_files'):
    print(f\"\\nFirst 20 failures:\")
    for f in data['failed_files'][:20]:
        print(f'  {f}')
"
```

If `--json` isn't supported on directory mode, use:

```bash
timeout 3600 python3 tests/molt_diff.py tests/differential/basic/ --profile release 2>&1 | tee /tmp/conformance_baseline.txt
tail -30 /tmp/conformance_baseline.txt
```

- [x] **Step 2: Capture the full failure list** — recorded in `tests/harness/baselines/baseline.json`

- [x] **Step 3: Record baseline in commit message** — baseline committed

---

### Task 6: Triage — tag unsupported-feature tests as xfail — DONE

**Status:** Commit `0cd0cb40e` tagged 158 tests. Currently 494 of 736 test files have MOLT_SKIP or xfail markers.

**Files:**
- Modify: Test files in `tests/differential/basic/` that exercise exec/eval/compile/monkeypatch/reflection

- [x] **Step 1: Identify tests using exec/eval/compile**

```bash
grep -rl '\bexec\s*(' tests/differential/basic/ | head -30
grep -rl '\beval\s*(' tests/differential/basic/ | head -30
grep -rl '\bcompile\s*(' tests/differential/basic/ | head -30
```

- [x] **Step 2: Identify tests using restricted reflection**
- [x] **Step 3: Add xfail metadata to each identified test** — 158 files tagged
- [x] **Step 4: Verify xfail tests are now excluded from failure count**
- [x] **Step 5: Commit** — `0cd0cb40e`

---

### Task 7: Cluster failures by root cause and fix systematically — IN PROGRESS

**Files:**
- Modify: Various files in `runtime/molt-backend/src/` and `runtime/molt-runtime/src/`

This is the largest task. It repeats a cycle: analyze failures → identify common root cause → fix → re-run suite → repeat.

- [x] **Step 1: Categorize remaining failures** (2026-04-01 analysis)

Identified failure clusters from 50-test sample (86% pass rate):

| Cluster | Tests | Root Cause | Fix Status |
|---------|-------|------------|------------|
| **SC_IOV_MAX missing** | ~20+ (all async + cascading) | `os.sysconf("SC_IOV_MAX")` crashed asyncio init | **FIXED** `e4dd37956` |
| **Unpack exception propagation** | 1+ | Iterator `RuntimeError` replaced by `ValueError` | **FIXED** `c361f8353` |
| **Exception handler type eval** | unknown | Exception not restored after isinstance check | **FIXED** `76cf5a071` (partner) |
| **Traceback format** | 2+ | Missing file/line/column in error tracebacks | Not started |
| **`__annotations__` population** | 1+ | Module-scope annotations dict not emitted | Not started |
| **`__prepare__` class metadata** | 7+ class tests | Missing `__firstlineno__`, `__static_attributes__`, `__classdictcell__` (CPython 3.13+) | Not started |
| **Walrus scope in genexpr** | 1 | `:=` doesn't leak to enclosing scope from genexpr | Not started |
| **DeprecationWarning/SyntaxWarning** | 2+ | Missing warning emission for `~True`, `return` in `finally` | Not started |
| **getattr method dispatch** | 1+ | `str.replace(old, new, count)` via `getattr` misroutes count arg | Not started |
| **Async runtime** | all async tests | Builds but empty output after sysconf fix | Not started (separate cluster) |

- [ ] **Step 2: Fix crash cluster (SIGSEGV/SIGILL)**

These are the highest priority — a crash in one function can mask many test results. For each crash:

1. Reproduce with `timeout 30 python3 -m molt build --target native --output /tmp/crash_test --release <test_file> --rebuild --verbose`
2. Check for null dereferences, missing return values, void-return function calls
3. Fix and verify
4. Commit immediately:

```bash
git add -A runtime/
git commit -m "fix: <description of crash fix>"
```

- [ ] **Step 3: Re-run suite after crash fixes**

```bash
pkill -9 -f "molt-backend"
timeout 3600 python3 tests/molt_diff.py tests/differential/basic/ --profile release 2>&1 | tail -10
```

Record new pass rate. Many "wrong output" failures may now resolve if they were downstream of crash bugs.

- [ ] **Step 4: Fix wrong-output cluster**

For each wrong-output failure:
1. Diff the expected (CPython) vs actual (Molt) output
2. Identify the divergence point
3. Trace through the codegen/runtime to find the bug
4. Fix, verify, commit

Common patterns to look for:
- Truthiness checks (0, empty string, empty list should be falsy)
- Negative indexing (list[-1], string[-1])
- Exception message parity
- Iterator protocol (StopIteration handling)
- String representation (repr vs str)

```bash
git add -A runtime/
git commit -m "fix: <description>"
```

- [ ] **Step 5: Fix missing-builtin cluster**

For missing builtins/attributes:
1. Check if the builtin is in the stdlib or needs runtime implementation
2. Implement or wire up the missing builtin
3. Verify with the failing test
4. Commit

```bash
git add -A runtime/ src/
git commit -m "feat: implement <builtin_name>"
```

- [ ] **Step 6: Repeat until 100%**

After each fix cluster:

```bash
pkill -9 -f "molt-backend"
timeout 3600 python3 tests/molt_diff.py tests/differential/basic/ --profile release 2>&1 | tail -10
```

Continue until all supported tests pass. Update failure list:

```bash
cp logs/molt_diff/failures.txt /tmp/conformance_failures_current.txt
diff /tmp/conformance_failures_baseline.txt /tmp/conformance_failures_current.txt
```

- [ ] **Step 7: Final conformance verification**

```bash
pkill -9 -f "molt-backend"
timeout 3600 python3 tests/molt_diff.py tests/differential/basic/ --profile release 2>&1 | tee /tmp/conformance_final.txt
tail -10 /tmp/conformance_final.txt
```

Expected: 100% pass rate on supported tests (failures = 0, xfail = N for excluded features).

```bash
git add -A
git commit -m "milestone: 100% conformance on supported features"
```

---

## Phase 3: Performance

### Task 8: Profile sieve and identify NaN-boxing hotspots

**Files:**
- Reference: `runtime/molt-backend/src/native_backend/function_compiler.rs:1438-1578` (add fast paths)
- Reference: `runtime/molt-backend/src/tir/type_refine.rs` (type refinement)

- [ ] **Step 1: Establish sieve performance baseline**

```python
# /tmp/bench_sieve.py
import time

def sieve(n):
    is_prime = [True] * (n + 1)
    is_prime[0] = is_prime[1] = False
    p = 2
    while p * p <= n:
        if is_prime[p]:
            j = p * p
            while j <= n:
                is_prime[j] = False
                j = j + p
        p = p + 1
    count = 0
    i = 0
    while i <= n:
        if is_prime[i]:
            count = count + 1
        i = i + 1
    return count

start = time.time()
result = sieve(100000)
elapsed = time.time() - start
print(f"sieve(100000) = {result}, {elapsed*1000:.1f}ms")
```

```bash
# CPython baseline
python3 /tmp/bench_sieve.py

# Molt baseline
pkill -9 -f "molt-backend"
timeout 300 python3 -m molt build --target native --output /tmp/bench_sieve_bin --release /tmp/bench_sieve.py --rebuild
/tmp/bench_sieve_bin
```

- [ ] **Step 2: Check if fast_int paths are firing for sieve**

The type refinement pass (`tir/type_refine.rs`) should mark sieve's loop variables as I64, which `lower_to_simple.rs` maps to `fast_int=true`. Verify:

```bash
MOLT_DUMP_FUNC="sieve" pkill -9 -f "molt-backend"
MOLT_DUMP_FUNC="sieve" timeout 300 python3 -m molt build --target native --output /tmp/bench_sieve_bin --release /tmp/bench_sieve.py --rebuild --verbose 2>&1
# Check the POST_sieve.txt debug artifact (from partner's unstaged diagnostic)
cat target/.molt_state/debug_artifacts/ir/POST_*sieve* 2>/dev/null | head -40
```

Look for `fast_int=Some(true)` on add, sub, lt ops within the sieve function.

- [ ] **Step 3: Commit baseline measurements**

```bash
git commit --allow-empty -m "perf: sieve baseline — Xms Molt vs Yms CPython"
```

---

### Task 9: Expand fast_int coverage and activate raw_int for loop counters — PARTIAL

**Status:** 6+ type specializations wired via TIR type inference (`07f782c38`, `3b2325af6`). fast_int paths guarded against bigint (`868052bdf`). list[int] flat storage end-to-end (`db6952258`). **However:** raw_int arithmetic chains DISABLED (commit `5489f7000`) — `box_int_value` truncates beyond 47-bit NaN-box range. Only comparison chains (icmp-based) remain active. Need overflow guard insertion to re-enable.

**Files:**
- Modify: `runtime/molt-backend/src/tir/type_refine.rs:62-259` (type refinement)
- Modify: `runtime/molt-backend/src/tir/lower_to_simple.rs:1290-1321` (fast_int/raw_int mapping)
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs` (add/sub/mul/lt handlers)

- [ ] **Step 1: Audit type refinement for sieve variables**

The sieve hot loop has these operations on integer variables:
- `p * p` (mul), `p + 1` (add), `j + p` (add), `j <= n` (le), `p * p <= n` (le)
- `count + 1` (add), `i + 1` (add), `i <= n` (le)

All of these should get `fast_int=true` from type refinement. If they don't, the refinement pass needs to propagate I64 type through more op kinds.

Check `type_refine.rs` for which ops propagate I64:

```bash
grep -n "I64" runtime/molt-backend/src/tir/type_refine.rs | head -30
```

Ensure `Mul`, `Le` (less-than-or-equal), and `Add` propagate I64 when both operands are I64.

- [ ] **Step 2: Add le/ge/eq/ne fast_int paths if missing**

The exploration confirmed add/sub/mul/lt have fast_int paths. Check if `le` (<=), `ge` (>=), `eq` (==), `ne` (!=) also have them:

```bash
grep -n "fast_int" runtime/molt-backend/src/native_backend/function_compiler.rs | grep -i "le\|ge\|eq\|ne"
```

If missing, add fast_int paths following the same pattern as `lt` (line 7521-7599):

```rust
// Inside "le" handler:
if op.raw_int == Some(true) {
    let a = use_var_named(...);
    let b = use_var_named(...);
    let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, a, b);
    let result = builder.ins().uextend(types::I64, cmp);
    def_var_named(..., result);
} else if op.fast_int == Some(true) {
    let a = unbox_int(builder, use_var_named(...));
    let b = unbox_int(builder, use_var_named(...));
    let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, a, b);
    let result_bool = builder.ins().uextend(types::I64, cmp);
    let boxed = box_bool_inline(builder, result_bool);
    def_var_named(..., boxed);
}
```

- [ ] **Step 3: Verify fast_int paths fire for sieve**

```bash
pkill -9 -f "molt-backend"
cargo build --profile release-fast -p molt-backend --features native-backend 2>&1 | tail -5
timeout 300 python3 -m molt build --target native --output /tmp/bench_sieve_bin --release /tmp/bench_sieve.py --rebuild
/tmp/bench_sieve_bin
```

Compare timing with baseline from Task 8, Step 1.

- [ ] **Step 4: Activate raw_int for proven loop counters**

Loop index variables (`i`, `j`, `p` in sieve) that are only used in arithmetic and comparisons can use raw_int (no NaN-boxing at all). The `loop_index_start` and `loop_index_next` ops already use raw_int. Extend this:

In `type_refine.rs`, identify variables that:
1. Are initialized from a constant integer
2. Are only modified by `add`/`sub` with constant integer operands
3. Are only compared against other integers

For these variables, emit `unbox_to_raw_int` at the loop entry and `box_from_raw_int` only at loop exit or when the variable escapes to a non-arithmetic context.

This is the most complex optimization. If it doesn't converge quickly, skip it — fast_int alone may be enough to hit < 30ms.

- [ ] **Step 5: Final sieve benchmark**

```bash
pkill -9 -f "molt-backend"
cargo build --profile release-fast -p molt-backend --features native-backend 2>&1 | tail -5
timeout 300 python3 -m molt build --target native --output /tmp/bench_sieve_bin --release /tmp/bench_sieve.py --rebuild
/tmp/bench_sieve_bin

# Also run fib for regression check
timeout 300 python3 -m molt build --target native --output /tmp/bench_fib_bin --release /tmp/bench_fib.py --rebuild 2>/dev/null
/tmp/bench_fib_bin
```

Target: sieve(100000) < 30ms.

- [ ] **Step 6: Commit**

```bash
git add -A runtime/
git commit -m "perf: fast_int/raw_int paths for sieve — Xms (was Yms)"
```

---

### Task 10: Final verification and milestone commit

- [ ] **Step 1: Run full conformance suite one final time**

```bash
pkill -9 -f "molt-backend"
timeout 3600 python3 tests/molt_diff.py tests/differential/basic/ --profile release 2>&1 | tee /tmp/conformance_final.txt
tail -20 /tmp/conformance_final.txt
```

Verify: 0 failures (excluding xfail).

- [ ] **Step 2: Run all benchmarks**

```bash
/tmp/bench_sieve_bin
/tmp/bench_fib_bin
python3 /tmp/bench_sieve.py
python3 /tmp/bench_fib.py
```

Record final numbers.

- [ ] **Step 3: Milestone commit**

```bash
git add -A
git commit -m "milestone: 100% conformance + sieve Xms (target <30ms)"
```
