# Wave A: Correctness Fortress Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kill every P0/P1 correctness blocker so real programs run end-to-end, then re-enable TIR for 20-30% performance recovery.

**Architecture:** Five parallel tracks: upgrade Cranelift and restructure loop IR (A1), fix stdlib attribute resolution (A2), fix daemon lock contention (A3), fix tuple MRO and genexpr enumerate (A5). Track A4 (TIR re-enablement) starts after A1 completes. All tracks converge at a shared exit gate that must pass before Wave C/B begin.

**Tech Stack:** Rust (Cranelift 0.130→latest, `cranelift-codegen`, `cranelift-frontend`, `cranelift-module`, `cranelift-object`), Python 3.12 frontend, pytest, cargo test, Molt differential harness

**Spec:** `docs/superpowers/specs/2026-03-27-operation-greenfield-design.md`

---

### Task 1: Upgrade Cranelift to Latest Stable

**Files:**
- Modify: `runtime/molt-backend/Cargo.toml:9-13,34-37`
- Modify: `Cargo.toml:282-287` (remove vendor patch)
- Delete: `vendor/cranelift-codegen-0.130.0/` (patched crate)
- Delete: `vendor/cranelift-frontend-0.130.0/` (if present)

- [ ] **Step 1: Check latest Cranelift version on crates.io**

Run:
```bash
cargo search cranelift-codegen 2>/dev/null | head -1
```

Expected: version string like `cranelift-codegen = "0.130.0"` or higher. If 0.130 is still latest, proceed with upgrade anyway — the vendor patches will be removed and we'll restructure IR instead.

- [ ] **Step 2: Update Cranelift dependency versions in `runtime/molt-backend/Cargo.toml`**

Change all five cranelift crate versions from `"0.130"` to the latest version found in Step 1. All five must match:
- `cranelift-codegen`
- `cranelift-frontend`
- `cranelift-module`
- `cranelift-object`
- `cranelift-native`

Also update the two `target`-specific `cranelift-codegen` entries at lines 34-37.

- [ ] **Step 3: Remove the vendor patch from workspace `Cargo.toml`**

Remove the `[patch.crates-io]` section at lines 286-287:
```toml
[patch.crates-io]
cranelift-codegen = { path = "vendor/cranelift-codegen-0.130.0" }
```

- [ ] **Step 4: Delete the vendor directory**

Run:
```bash
rm -rf vendor/cranelift-codegen-0.130.0
rm -rf vendor/cranelift-frontend-0.130.0
```

- [ ] **Step 5: Verify the upgrade compiles**

Run:
```bash
cargo check -p molt-backend --features native-backend
```

Expected: compiles cleanly. If there are API breaking changes from the Cranelift upgrade, fix them — consult the Cranelift changelog for migration notes.

- [ ] **Step 6: Run Rust backend tests**

Run:
```bash
cargo test -p molt-backend --features native-backend -- --nocapture
```

Expected: all existing tests pass with the new Cranelift version.

- [ ] **Step 7: Test nested loop compilation**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -m molt.cli run --profile dev -c "
for i in range(3):
    for j in range(3):
        print(i, j)
"
```

Expected: prints 9 lines (0 0, 0 1, 0 2, 1 0, ... 2 2). If the Cranelift upgrade alone fixes this, document it but still proceed to Task 2 for durable IR restructuring.

- [ ] **Step 8: Commit**

Run:
```bash
git add -A runtime/molt-backend/Cargo.toml Cargo.toml Cargo.lock
git rm -rf vendor/cranelift-codegen-0.130.0
git commit -m "chore: upgrade Cranelift to latest, remove vendor patches"
```

### Task 2: Restructure Loop IR for Nested Loop Durability

**Files:**
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs:12719-12850`

The current code at line 12815 has `!contains_nested_loop` which forces nested loops into a structured-frame path. The problem is that the structured-frame path emits IR that Cranelift's egraph optimizer can miscompile. The fix is to restructure the IR emission so nested loops emit correct IR by construction — not dependent on optimizer behavior.

- [ ] **Step 1: Write a failing nested loop differential test**

Create `tests/differential/basic/nested_indexed_loops.py`:
```python
# Nested indexed loops must produce correct output
results = []
for i in range(3):
    for j in range(3):
        results.append((i, j))
print(results)
# Expected: [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (2, 0), (2, 1), (2, 2)]
```

- [ ] **Step 2: Run the differential test to verify it fails**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/nested_indexed_loops.py --jobs 1
```

Expected: FAIL (miscompiled output from Cranelift optimizer pruning inner loop body).

- [ ] **Step 3: Restructure the `loop_index_start` handler for nested loops**

In `runtime/molt-backend/src/native_backend/function_compiler.rs`, the current approach at line 12812-12815 disables linearized loops when `contains_nested_loop` is true. The structured-frame fallback must emit IR that:

1. Creates proper Cranelift loop blocks with explicit back-edges for each nesting level
2. Passes the loop counter as a block parameter (not an SSA Variable) to avoid egraph pruning
3. Seals blocks in correct order — inner loop blocks before outer loop's back-edge

The key change: in the structured frame path (when `allow_linearized_loop` is false), use `builder.append_block_param()` for the loop counter instead of `builder.declare_var()` + `builder.def_var()`. This makes the counter value structurally visible to the optimizer as a live block parameter, preventing the egraph from treating the inner loop body as dead.

Modify the block at lines 12850+ where the structured loop frame is set up:
- Add the counter as a block parameter to the loop header block
- Pass the initial counter value (0 or start) via the entry branch
- Pass the incremented counter value via the back-edge branch
- Read the counter from the block parameter instead of from an SSA variable

- [ ] **Step 4: Run the differential test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/nested_indexed_loops.py --jobs 1
```

Expected: PASS.

- [ ] **Step 5: Run the full differential suite**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: all 2,617+ tests pass. No regressions from the IR restructuring.

- [ ] **Step 6: Run Rust backend tests**

Run:
```bash
cargo test -p molt-backend --features native-backend -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Add a triple-nested loop differential test for durability**

Create `tests/differential/basic/triple_nested_loops.py`:
```python
# Triple-nested loops — stress test for loop IR durability
results = []
for i in range(2):
    for j in range(2):
        for k in range(2):
            results.append((i, j, k))
print(results)
# Expected: [(0,0,0),(0,0,1),(0,1,0),(0,1,1),(1,0,0),(1,0,1),(1,1,0),(1,1,1)]
```

- [ ] **Step 8: Run the triple-nested test**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/triple_nested_loops.py --jobs 1
```

Expected: PASS.

- [ ] **Step 9: Commit**

Run:
```bash
git add runtime/molt-backend/src/native_backend/function_compiler.rs tests/differential/basic/nested_indexed_loops.py tests/differential/basic/triple_nested_loops.py
git commit -m "fix: restructure nested loop IR to use block params — immune to egraph pruning"
```

### Task 3: Fix Stdlib AttributeError

**Files:**
- Modify: `runtime/molt-runtime/src/builtins/attr.rs:785-858` (MRO attribute lookup)
- Modify: `runtime/molt-runtime/src/builtins/attr.rs:1093-1106` (dispatch)
- Possibly modify: `runtime/molt-backend/src/native_backend/function_compiler.rs` (if codegen)
- Possibly modify: `src/molt/frontend/__init__.py` (if IR emission)

- [ ] **Step 1: Write a failing differential test**

Create `tests/differential/basic/stdlib_attr_access.py`:
```python
import sys
print(sys.platform)
print(sys.version_info.major)
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/stdlib_attr_access.py --jobs 1
```

Expected: FAIL with AttributeError (no attribute name in message).

- [ ] **Step 3: Reproduce and diagnose the root cause**

Run with debug output:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 MOLT_DUMP_CLIF=1 uv run --python 3.12 python3 -m molt.cli run --profile dev -c "import sys; print(sys.platform)" 2>logs/stdlib_attr_debug.log
```

Examine the log to determine whether the failure is in:
1. **Attribute lookup dispatch** (`attr_lookup_ptr_any` at attr.rs:1093) — stdlib module objects not matching the expected type dispatch
2. **MRO iteration** (`class_attr_lookup_raw_mro` at attr.rs:785) — class hierarchy for stdlib module types not correctly constructed
3. **IR emission** — the frontend emitting incorrect `get_attr` ops for stdlib module attribute access
4. **Codegen** — the backend translating attribute access differently for stdlib vs user code

- [ ] **Step 4: Implement the fix**

Based on diagnosis in Step 3, fix the attribute resolution. The most likely cause (based on memory: "attribute lookup in compiled stdlib class hierarchies fails — was masked by prior SIGSEGV") is that stdlib module objects have a class hierarchy that doesn't match the fast path in `attr_lookup_ptr_any`. The fix must make compiled stdlib module attributes accessible through the same mechanism as user code, not a special path.

- [ ] **Step 5: Run the differential test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/stdlib_attr_access.py --jobs 1
```

Expected: PASS.

- [ ] **Step 6: Add a broader stdlib attribute access test**

Create `tests/differential/basic/stdlib_attr_broad.py`:
```python
import sys
import os
print(sys.platform)
print(sys.version_info.major)
print(os.sep)
print(os.name)
import math
print(math.pi)
print(math.e)
```

- [ ] **Step 7: Run the broader test**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/stdlib_attr_broad.py --jobs 1
```

Expected: PASS.

- [ ] **Step 8: Run the full differential suite to confirm no regressions**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: all tests pass.

- [ ] **Step 9: Commit**

Run:
```bash
git add runtime/molt-runtime/src/builtins/attr.rs tests/differential/basic/stdlib_attr_access.py tests/differential/basic/stdlib_attr_broad.py
git commit -m "fix: stdlib attribute lookup resolves through compiled class hierarchy"
```

### Task 4: Fix Backend Daemon Lock Contention

**Files:**
- Modify: `runtime/molt-backend/src/main.rs:318-416` (DaemonCache)
- Modify: `runtime/molt-backend/src/lib.rs:3436-3439` (backend_env_lock)
- Modify: `src/molt/cli.py` (daemon client)

- [ ] **Step 1: Reproduce the daemon stall**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_daemon_stall_repro.json
```

Expected: stalls or produces unreliable timing. If it completes successfully, the stall may be intermittent — run 5 times in sequence:
```bash
for i in $(seq 1 5); do PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output /dev/null; done
```

- [ ] **Step 2: Diagnose the lock contention**

The daemon has two potential contention points:
1. `backend_env_lock()` in `lib.rs:3436` — a `Mutex<()>` protecting env var mutations during compilation. If compilation is slow, this blocks all other compilation requests.
2. `DaemonCache` in `main.rs:318` — LRU cache with clock-based eviction. The cache operations themselves are fast, but if they hold a lock while the compilation runs, that's the stall.

Check: does the daemon process compilation requests serially (one at a time) or concurrently? If serially, that's the root cause — the daemon must be able to handle multiple concurrent compilation requests.

Run:
```bash
grep -n "Mutex\|RwLock\|lock()\|write()\|read()" runtime/molt-backend/src/main.rs | head -30
```

- [ ] **Step 3: Implement the fix**

Based on diagnosis:
- If the env lock is the bottleneck: scope env mutations to per-compilation state instead of global process env. Use a local env map passed through the compilation context instead of `std::env::set_var`.
- If the cache lock is the bottleneck: separate the cache lookup (fast, locked) from the compilation (slow, unlocked). Pattern: lock → check cache → unlock → compile → lock → insert cache → unlock.
- If serial processing is the issue: use a thread pool or async dispatch for compilation requests.

The fix must not introduce code smell — no "try_lock with fallback" patterns, no "skip daemon for benchmarks" hacks.

- [ ] **Step 4: Verify benchmarks complete with daemon ON**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_daemon_fixed.json
```

Expected: completes reliably. Run 5 times to confirm no intermittent stalls.

- [ ] **Step 5: Verify no stale-socket artifacts**

Run:
```bash
ls -la target/.molt_state/backend_daemon/ 2>/dev/null
PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
ls -la target/.molt_state/backend_daemon/ 2>/dev/null
```

Expected: socket files are clean, no stale artifacts from previous runs.

- [ ] **Step 6: Run Rust backend tests**

Run:
```bash
cargo test -p molt-backend --features native-backend -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:
```bash
git add runtime/molt-backend/src/main.rs runtime/molt-backend/src/lib.rs
git commit -m "fix: eliminate daemon lock contention — concurrent compilation requests"
```

### Task 5: Fix Tuple Subclass MRO

**Files:**
- Modify: `runtime/molt-runtime/src/builtins/type_ops.rs:4-53` (MRO/bases resolution)

- [ ] **Step 1: Write a failing differential test**

Create `tests/differential/basic/tuple_subclass_mro.py`:
```python
class MyTuple(tuple):
    pass

t = MyTuple((1, 2, 3))
print(t)
print(type(t).__name__)
print(len(t))
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/tuple_subclass_mro.py --jobs 1
```

Expected: FAIL with "takes no arguments".

- [ ] **Step 3: Diagnose the MRO lookup**

The bug: `MyTuple((42,))` calls `tuple.__new__` via MRO but finds `object.__new__` instead. In `type_ops.rs`, `class_mro_vec()` at line 25 falls back to recursive base traversal if no cached MRO. The issue is likely that `tuple` is a builtin type whose MRO is not constructed from a Python-visible bases tuple, so `class_bases_vec()` at line 38 returns empty or wrong results for builtin types.

The fix: ensure `class_bases_vec()` returns `(tuple,)` for tuple subclasses, and that the MRO resolution checks builtin type `__new__` methods before falling through to `object.__new__`.

- [ ] **Step 4: Implement the MRO fix**

In `type_ops.rs`, the `class_mro_ref` / `class_mro_vec` / `class_bases_vec` functions must correctly handle builtin type bases. For a user-defined `class MyTuple(tuple)`, the MRO should be `[MyTuple, tuple, object]`, and `__new__` lookup should find `tuple.__new__` at position 1, not skip to `object.__new__`.

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/tuple_subclass_mro.py --jobs 1
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:
```bash
git add runtime/molt-runtime/src/builtins/type_ops.rs tests/differential/basic/tuple_subclass_mro.py
git commit -m "fix: tuple subclass MRO resolves tuple.__new__ before object.__new__"
```

### Task 6: Fix Genexpr Enumerate Tuple Unpacking

**Files:**
- Modify: `src/molt/frontend/__init__.py` (genexpr/comprehension compilation)

- [ ] **Step 1: Write a failing differential test**

Create `tests/differential/basic/genexpr_enumerate_unpack.py`:
```python
items = ("a", "b", "c")
result = {k: v for k, v in enumerate(items)}
print(result)
# Expected: {0: 'a', 1: 'b', 2: 'c'}

result2 = [(i, x) for i, x in enumerate([10, 20, 30])]
print(result2)
# Expected: [(0, 10), (1, 20), (2, 30)]
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/genexpr_enumerate_unpack.py --jobs 1
```

Expected: FAIL (tuple unpacking in compiled generator expression body not handled).

- [ ] **Step 3: Fix the generator expression body to handle tuple unpacking**

In `src/molt/frontend/__init__.py`, the genexpr compilation currently raises `NotImplementedError("Unsupported tuple unpacking value")` (per code mapping at line 11228). The fix:

1. When the comprehension target is a `ast.Tuple` (e.g., `k, v` in `for k, v in enumerate(...)`), emit `UNPACK_SEQ` ops to destructure the iterate value into individual variables.
2. The iterate value from `enumerate` is a 2-tuple `(index, value)`. Emit `TUPLE_GET` with index 0 and 1 to extract the components.
3. Bind each unpacked component to its corresponding target variable name.

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/genexpr_enumerate_unpack.py --jobs 1
```

Expected: PASS.

- [ ] **Step 5: Run the full differential suite**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

Run:
```bash
git add src/molt/frontend/__init__.py tests/differential/basic/genexpr_enumerate_unpack.py
git commit -m "fix: genexpr handles tuple unpacking from enumerate results"
```

### Task 7: Re-enable TIR Optimization (depends on Tasks 1-2)

**Files:**
- Modify: `runtime/molt-backend/src/main.rs:883-891` (remove default disable)
- Modify: `runtime/molt-backend/src/tir/lower_to_simple.rs` (fix SSA roundtrip)

- [ ] **Step 1: Identify which ops break the SSA roundtrip**

Run with TIR enabled on a known-working program to see what breaks:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 MOLT_TIR_OPT=1 TIR_DUMP=1 uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py 2>logs/tir_roundtrip_debug.log
```

Examine the log: compare the TIR IR against the lowered SimpleIR. The breakage is in `lower_to_simple.rs` where `operand_args(op)` at line 883-885 maps operand ValueIds to SSA variable names. For ops that are `Copy`-mapped (loops, fields, exception stack), the operand connection may be lost because the TIR optimizer may rewrite the ValueId but the lowering doesn't track the rewrite.

- [ ] **Step 2: Fix the SSA roundtrip in `lower_to_simple.rs`**

The `value_var()` function at line 861-863 generates `_v{id}` variable names from `ValueId`. If the TIR optimizer rewrites a value (e.g., copy propagation changes `v5` to `v3`), the lowered SimpleIR must use the rewritten ValueId, not the original.

Check: does the TIR optimizer update operand ValueIds in-place, or does it maintain a separate rewrite map? If the latter, `operand_args()` must consult the rewrite map.

Also check: for `Copy`-mapped ops (where `lower_op` returns `None`), are the copy targets correctly forwarded? A `Copy` op that maps `v5 = copy v3` should make `v5` an alias of `v3` in all subsequent uses.

- [ ] **Step 3: Run targeted tests with TIR enabled**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 MOLT_TIR_OPT=1 uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
MOLT_TIR_OPT=1 MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: PASS on both.

- [ ] **Step 4: Remove the default TIR disable in `main.rs`**

Remove lines 883-891 in `runtime/molt-backend/src/main.rs`:
```rust
// Disable TIR optimization by default — the TIR SSA roundtrip breaks
// operand connections for Copy-mapped ops (loops, fields, exception stack).
// Re-enable with MOLT_TIR_OPT=1 for testing TIR passes.
if std::env::var("MOLT_TIR_OPT").is_err() {
    unsafe {
        std::env::set_var("MOLT_TIR_OPT", "0");
    }
}
```

TIR should now be ON by default (the code in `lib.rs:2478` and `wasm.rs:1511` checks `!= Some("0")`, so absence of the env var means ON).

- [ ] **Step 5: Run the full differential suite with TIR defaulting to ON**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: all tests pass with TIR enabled by default.

- [ ] **Step 6: Benchmark the TIR improvement**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 MOLT_TIR_OPT=0 uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_tir_off.json
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_tir_on.json
```

Expected: measurable improvement with TIR enabled (target: 20-30% based on prior observations).

- [ ] **Step 7: Commit**

Run:
```bash
git add runtime/molt-backend/src/main.rs runtime/molt-backend/src/tir/lower_to_simple.rs
git commit -m "perf: re-enable TIR optimization — fix SSA roundtrip for Copy-mapped ops"
```

### Task 8: Wave A Exit Gate

- [ ] **Step 1: Run the full exit gate validation**

Run all commands — every one must pass:
```bash
cargo test -p molt-backend --features native-backend -- --nocapture
cargo test -p molt-runtime -- --nocapture
PYTHONPATH=src uv run --python 3.12 python3 -m molt.cli run --profile dev examples/hello.py
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: all green.

- [ ] **Step 2: Record benchmark artifacts**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_wave_a_exit.json
```

- [ ] **Step 3: Update canonical status docs**

Update `docs/spec/STATUS.md` and `ROADMAP.md` with:
- Nested loops: FIXED
- Stdlib AttributeError: FIXED
- Daemon lock contention: FIXED
- TIR: RE-ENABLED
- Tuple MRO: FIXED
- Genexpr enumerate: FIXED

- [ ] **Step 4: Refresh Linear workspace artifacts**

Run:
```bash
python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
```

- [ ] **Step 5: Commit status updates**

Run:
```bash
git add docs/spec/STATUS.md ROADMAP.md
git commit -m "docs: update status — Wave A correctness fortress complete"
```
