# Wave B: Ecosystem Unlock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unlock the `six -> attrs -> click` third-party library pipeline, deepen stdlib intrinsic coverage, and close IR semantic debt — so Molt runs real Python libraries end-to-end.

**Architecture:** Five tracks: B1 fixes `six` compilation (depends on Wave A stdlib fix), B2 fixes `click` (depends on B1), B3 validates `attrs` end-to-end (depends on B1), B4 completes stdlib intrinsic tranche 1 (independent after Wave A gate), B5 hardens IR semantic coverage (independent after Wave A gate). B1/B4/B5 can run in parallel after Wave A.

**Tech Stack:** Python 3.12 frontend, Rust runtime/backend, intrinsic manifest tooling (`tools/gen_intrinsics.py`), pytest, Molt differential harness

**Spec:** `docs/superpowers/specs/2026-03-27-operation-greenfield-design.md`

---

### Task 1: Fix `six` Compilation (Track B1, depends on Wave A)

**Files:**
- Modify: `src/molt/frontend/__init__.py:1171,7290-7340,9904` (module_global_mutations, _store_local_value)
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs` (if codegen)

- [ ] **Step 1: Write a failing differential test for six import**

Create `tests/differential/basic/import_six.py`:
```python
import six
print(six.PY3)
print(six.text_type.__name__)
# six.moves iteration exercises module-scope variable scoping
names = [attr for attr in dir(six.moves) if not attr.startswith('_')]
print(len(names) > 0)
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_six.py --jobs 1
```

Expected: FAIL (iteration error in `_moved_attributes` due to module-scope variable scoping).

- [ ] **Step 3: Diagnose the module-scope variable scoping issue**

The `six` module's `_moved_attributes` is populated by module-level loops that iterate over tuples and store results into module-scope variables. The frontend tracks these in `module_global_mutations` (line 1171 in `__init__.py`) and emits `DICT_SET` ops to write to the module dict (line 7290-7340 in `_store_local_value`).

Run with debug output:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 MOLT_DUMP_IR=1 uv run --python 3.12 python3 -m molt.cli run --profile dev -c "import six" 2>logs/six_import_debug.log
```

Check the IR for `six` module initialization — verify that module-level loop variables are correctly emitted as module dict stores, not local variable stores.

- [ ] **Step 4: Fix the module-scope variable handling**

The most likely issue: module-level for-loops that iterate over comprehensions or complex expressions don't correctly add all target variables to `module_global_mutations`. The `_collect_mutation_targets()` or equivalent function may miss variables that are:
1. Assigned inside nested comprehensions at module scope
2. Used as loop targets in `for x in ...` at module scope
3. Re-assigned conditionally in if/else at module scope

The fix: ensure `module_global_mutations` captures all variables that are assigned at module scope inside loops, comprehensions, or conditional blocks — not just simple assignments.

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_six.py --jobs 1
```

Expected: PASS.

- [ ] **Step 6: Run the full differential suite**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

Run:
```bash
git add src/molt/frontend/__init__.py tests/differential/basic/import_six.py
git commit -m "fix: module-scope variable scoping handles six's _moved_attributes iteration"
```

### Task 2: Fix `click` Compilation (Track B2, depends on B1)

**Files:**
- Modify: `runtime/molt-backend/src/passes.rs:2260` (megafunction splitting)
- Modify: `runtime/molt-backend/src/lib.rs:2243-2253` (Cranelift fallback)
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs`

- [ ] **Step 1: Write a failing differential test for click import**

Create `tests/differential/basic/import_click.py`:
```python
import click

@click.command()
@click.option('--name', default='World', help='Name to greet')
def hello(name):
    print(f'Hello {name}!')

# Don't invoke the CLI, just verify import and decoration worked
print(type(hello).__name__)
print(hello.name)
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_click.py --jobs 1
```

Expected: FAIL (backend complexity limits).

- [ ] **Step 3: Diagnose the backend complexity limit**

The current megafunction splitting threshold is `DEFAULT_MAX_FUNCTION_OPS = 4000` (at `passes.rs:2260`). Click's module initialization may exceed this, or the Cranelift register allocator may fail on large functions even after splitting.

Check:
1. Is the failure at the megafunction split pass (too many ops per chunk)?
2. Is the failure at Cranelift compilation (O(n^2) register allocator)?
3. Is the failure in IR emission (missing op types for click's patterns)?

Run with env override to test:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 MOLT_MAX_FUNCTION_OPS=8000 uv run --python 3.12 python3 -m molt.cli run --profile dev -c "import click" 2>logs/click_debug.log
```

- [ ] **Step 4: Fix the compilation**

Based on diagnosis:
- If megafunction limit: the split algorithm at `passes.rs:2248-2260` splits at top-level statement boundaries (`loop_depth==0, if_depth==0`). Click may have deeply nested module initialization that prevents splitting. Fix: split at any `if_depth==0` boundary, not just `loop_depth==0`.
- If Cranelift allocator: the fallback at `lib.rs:2243` emits a trap stub. Instead, try compiling with the optimizer disabled for that specific function before falling back to a trap.
- If IR emission: add the missing op mappings.

No artificial complexity caps — if the IR is valid, it compiles.

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_click.py --jobs 1
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:
```bash
git add runtime/molt-backend/src/passes.rs runtime/molt-backend/src/lib.rs tests/differential/basic/import_click.py
git commit -m "fix: click compiles — remove artificial complexity caps on module initialization"
```

### Task 3: `attrs` End-to-End Validation (Track B3, depends on B1)

**Files:**
- Create: `tests/differential/basic/import_attrs.py`

- [ ] **Step 1: Write a differential test for attrs**

Create `tests/differential/basic/import_attrs.py`:
```python
import attr

@attr.s(auto_attribs=True)
class Point:
    x: float
    y: float

    def distance(self):
        return (self.x ** 2 + self.y ** 2) ** 0.5

p = Point(3.0, 4.0)
print(p)
print(p.distance())
print(attr.fields(Point))
```

- [ ] **Step 2: Run the test**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_attrs.py --jobs 1
```

Expected: PASS (attrs depends on six, which is fixed in B1). If it fails, diagnose and fix.

- [ ] **Step 3: Add a more comprehensive attrs exercise**

Create `tests/differential/basic/attrs_features.py`:
```python
import attr

@attr.s
class Validated:
    name = attr.ib(validator=attr.validators.instance_of(str))
    age = attr.ib(validator=[attr.validators.instance_of(int), attr.validators.gt(0)])

v = Validated(name="Alice", age=30)
print(v)
print(attr.asdict(v))

# Test frozen attrs
@attr.s(frozen=True)
class FrozenPoint:
    x = attr.ib()
    y = attr.ib()

fp = FrozenPoint(1, 2)
print(fp)
try:
    fp.x = 10
except attr.exceptions.FrozenInstanceError:
    print("correctly frozen")
```

- [ ] **Step 4: Run the comprehensive test**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/attrs_features.py --jobs 1
```

Expected: PASS. If failures occur, file them as targeted bugs and fix in-line.

- [ ] **Step 5: Commit**

Run:
```bash
git add tests/differential/basic/import_attrs.py tests/differential/basic/attrs_features.py
git commit -m "test: attrs end-to-end validation — import, auto_attribs, validators, frozen"
```

### Task 4: Stdlib Intrinsic Closure — `functools` (Track B4, first module)

**Files:**
- Modify: `src/molt/stdlib/functools.py`
- Modify: `runtime/molt-runtime/src/intrinsics/manifest.pyi`
- Modify: `src/molt/_intrinsics.pyi` (generated)
- Modify: `runtime/molt-runtime/src/intrinsics/generated.rs` (generated)

The manifest already has 13 functools intrinsics. This task verifies they work end-to-end and fills any gaps.

- [ ] **Step 1: Write a differential test for functools**

Create `tests/differential/stdlib/functools_coverage.py`:
```python
import functools

# partial
def add(a, b):
    return a + b
add5 = functools.partial(add, 5)
print(add5(3))  # 8

# reduce
result = functools.reduce(lambda a, b: a + b, [1, 2, 3, 4])
print(result)  # 10

# wraps
def my_decorator(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        return func(*args, **kwargs)
    return wrapper

@my_decorator
def greet(name):
    """Greeting function"""
    return f"Hello {name}"

print(greet("World"))
print(greet.__name__)
print(greet.__doc__)

# lru_cache
@functools.lru_cache(maxsize=32)
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(10))  # 55
print(fib.cache_info())
```

- [ ] **Step 2: Run the test**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/stdlib/functools_coverage.py --jobs 1
```

Expected: PASS or identify specific missing intrinsics.

- [ ] **Step 3: Fill any intrinsic gaps found in Step 2**

For each missing intrinsic:
1. Add the declaration to `runtime/molt-runtime/src/intrinsics/manifest.pyi`
2. Implement the runtime function in the appropriate Rust module
3. Regenerate:
```bash
python3 tools/gen_intrinsics.py
python3 tools/check_stdlib_intrinsics.py --update-doc
```

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/stdlib/functools_coverage.py --jobs 1
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:
```bash
git add src/molt/stdlib/functools.py runtime/molt-runtime/src/intrinsics/ src/molt/_intrinsics.pyi tests/differential/stdlib/functools_coverage.py
git commit -m "feat: functools intrinsic coverage — partial, reduce, wraps, lru_cache verified"
```

### Task 5: Stdlib Intrinsic Closure — `itertools`

**Files:**
- Modify: `src/molt/stdlib/itertools.py`
- Modify: `runtime/molt-runtime/src/intrinsics/manifest.pyi`
- Modify: `src/molt/_intrinsics.pyi` (generated)
- Modify: `runtime/molt-runtime/src/intrinsics/generated.rs` (generated)

- [ ] **Step 1: Write a differential test for itertools**

Create `tests/differential/stdlib/itertools_coverage.py`:
```python
import itertools

# chain
print(list(itertools.chain([1, 2], [3, 4])))  # [1, 2, 3, 4]

# islice
print(list(itertools.islice(range(100), 5)))  # [0, 1, 2, 3, 4]

# product
print(list(itertools.product("AB", repeat=2)))  # [('A','A'),('A','B'),('B','A'),('B','B')]

# permutations
print(list(itertools.permutations([1, 2, 3], 2)))

# combinations
print(list(itertools.combinations([1, 2, 3], 2)))

# accumulate
print(list(itertools.accumulate([1, 2, 3, 4])))  # [1, 3, 6, 10]

# groupby
data = [("a", 1), ("a", 2), ("b", 3)]
for key, group in itertools.groupby(data, key=lambda x: x[0]):
    print(key, list(group))

# count/cycle/repeat
print(list(itertools.islice(itertools.count(10), 5)))  # [10, 11, 12, 13, 14]
print(list(itertools.islice(itertools.cycle([1, 2, 3]), 7)))  # [1,2,3,1,2,3,1]
print(list(itertools.repeat("x", 3)))  # ['x', 'x', 'x']
```

- [ ] **Step 2: Run the test**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/stdlib/itertools_coverage.py --jobs 1
```

- [ ] **Step 3: Fill any intrinsic gaps, regenerate, and verify**

Same pattern as Task 4 Step 3.

- [ ] **Step 4: Commit**

Run:
```bash
git add src/molt/stdlib/itertools.py runtime/molt-runtime/src/intrinsics/ src/molt/_intrinsics.pyi tests/differential/stdlib/itertools_coverage.py
git commit -m "feat: itertools intrinsic coverage — chain, islice, product, permutations, groupby verified"
```

### Task 6: Stdlib Intrinsic Closure — `operator`

**Files:**
- Modify: `src/molt/stdlib/operator.py`
- Modify: `runtime/molt-runtime/src/intrinsics/manifest.pyi`

- [ ] **Step 1: Write a differential test for operator**

Create `tests/differential/stdlib/operator_coverage.py`:
```python
import operator

# itemgetter
get_first = operator.itemgetter(0)
print(get_first([10, 20, 30]))  # 10

get_multi = operator.itemgetter(0, 2)
print(get_multi([10, 20, 30]))  # (10, 30)

# attrgetter
class Obj:
    x = 5
    y = 10

print(operator.attrgetter('x')(Obj()))  # 5

# arithmetic
print(operator.add(1, 2))  # 3
print(operator.mul(3, 4))  # 12
print(operator.truediv(10, 3))

# comparison
print(operator.lt(1, 2))  # True
print(operator.eq("a", "a"))  # True

# methodcaller
print(operator.methodcaller("upper")("hello"))  # HELLO
```

- [ ] **Step 2: Run, fill gaps, verify, commit**

Same pattern as Tasks 4-5.

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/stdlib/operator_coverage.py --jobs 1
```

- [ ] **Step 3: Commit**

Run:
```bash
git add src/molt/stdlib/operator.py runtime/molt-runtime/src/intrinsics/ src/molt/_intrinsics.pyi tests/differential/stdlib/operator_coverage.py
git commit -m "feat: operator intrinsic coverage — itemgetter, attrgetter, methodcaller verified"
```

### Task 7: Stdlib Intrinsic Closure — `math` and `json`

**Files:**
- Modify: `src/molt/stdlib/math.py`
- Modify: `src/molt/stdlib/json/__init__.py`
- Modify: `runtime/molt-runtime/src/intrinsics/manifest.pyi`

- [ ] **Step 1: Write differential tests for math and json**

Create `tests/differential/stdlib/math_coverage.py`:
```python
import math
print(math.pi)
print(math.e)
print(math.floor(3.7))  # 3
print(math.ceil(3.2))  # 4
print(math.sqrt(16))  # 4.0
print(math.log(math.e))  # 1.0
print(math.sin(0))  # 0.0
print(math.cos(0))  # 1.0
print(math.isnan(float('nan')))  # True
print(math.isinf(float('inf')))  # True
print(math.gcd(12, 8))  # 4
print(math.factorial(5))  # 120
```

Create `tests/differential/stdlib/json_coverage.py`:
```python
import json

# dumps
d = {"name": "Alice", "age": 30, "scores": [95, 87, 92]}
s = json.dumps(d, sort_keys=True)
print(s)

# loads
parsed = json.loads(s)
print(parsed["name"])
print(parsed["scores"])

# dumps with indent
print(json.dumps({"a": 1}, indent=2))

# roundtrip types
for val in [None, True, False, 42, 3.14, "hello", [1, 2], {"k": "v"}]:
    assert json.loads(json.dumps(val)) == val
print("roundtrip OK")
```

- [ ] **Step 2: Run both tests**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/stdlib/math_coverage.py tests/differential/stdlib/json_coverage.py --jobs 1
```

- [ ] **Step 3: Fill gaps, regenerate, verify**

Same pattern. Run regeneration:
```bash
python3 tools/gen_intrinsics.py
python3 tools/gen_stdlib_module_union.py
python3 tools/sync_stdlib_top_level_stubs.py --write
python3 tools/check_stdlib_intrinsics.py --update-doc
```

- [ ] **Step 4: Commit**

Run:
```bash
git add src/molt/stdlib/math.py src/molt/stdlib/json/ runtime/molt-runtime/src/intrinsics/ src/molt/_intrinsics.pyi tests/differential/stdlib/math_coverage.py tests/differential/stdlib/json_coverage.py
git commit -m "feat: math + json intrinsic coverage verified end-to-end"
```

### Task 8: IR Semantic Hardening — Priority Ops (Track B5)

**Files:**
- Modify: `src/molt/frontend/__init__.py` (IR emission)
- Modify: `runtime/molt-backend/src/native_backend/function_compiler.rs` (native codegen)
- Modify: `runtime/molt-backend/src/wasm.rs` (WASM codegen)
- Modify: `tests/test_frontend_midend_passes.py`

- [ ] **Step 1: Run the IR ops gate to identify current gaps**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py
```

Record which ops have partial coverage and what specific semantic assertions are failing.

- [ ] **Step 2: Fix `CALL_INDIRECT` semantic hardening**

Based on gate output, ensure:
1. `CALL_INDIRECT` has dedicated lanes in both native and WASM backends
2. The `molt_call_indirect_ic` runtime bridge is correctly linked
3. Non-callable deopt counter is active
4. Differential probes exist and execute

- [ ] **Step 3: Fix `GUARD_TAG` and `GUARD_DICT_SHAPE` hardening**

Ensure:
1. Both guards have dedicated lanes in native and WASM
2. Deopt counters are linked and counting
3. Type mismatch and shape invalidation produce deterministic compile errors, not panics

- [ ] **Step 4: Replace backend panics with deterministic compile errors**

Search for `panic!` and `unwrap()` in the backend that are reachable from user programs:
```bash
grep -n "panic!\|\.unwrap()" runtime/molt-backend/src/native_backend/function_compiler.rs | head -30
```

For each panic reachable from user IR, replace with a proper compile error that includes the op kind, location, and a human-readable explanation.

- [ ] **Step 5: Run the IR ops gate to verify improvements**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py
UV_NO_SYNC=1 uv run --python 3.12 python3 -m pytest -q tests/test_frontend_midend_passes.py
```

Expected: fewer partial coverage items, all 84 midend tests pass.

- [ ] **Step 6: Commit**

Run:
```bash
git add src/molt/frontend/__init__.py runtime/molt-backend/src/native_backend/function_compiler.rs runtime/molt-backend/src/wasm.rs
git commit -m "fix: IR semantic hardening — CALL_INDIRECT, GUARD_TAG, GUARD_DICT_SHAPE, panic→compile errors"
```

### Task 9: Wave B Exit Gate

- [ ] **Step 1: Run the ecosystem import test**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -m molt.cli run --profile dev -c "import six; import attr; import click; print('ecosystem green')"
```

Expected: prints "ecosystem green".

- [ ] **Step 2: Run the IR ops gate**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/check_molt_ir_ops.py
```

Expected: all P0-required ops at full coverage.

- [ ] **Step 3: Run the midend passes tests**

Run:
```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 -m pytest -q tests/test_frontend_midend_passes.py
```

Expected: all 84 tests pass.

- [ ] **Step 4: Run the full differential suite**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic tests/differential/stdlib --jobs 1
```

Expected: all tests pass including the new stdlib coverage tests.

- [ ] **Step 5: Record final benchmark artifacts**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 tools/bench.py --bench tests/benchmarks/bench_sum.py --output bench/results/bench_wave_b_exit.json
```

- [ ] **Step 6: Update canonical status docs**

Update `docs/spec/STATUS.md` and `ROADMAP.md` with:
- six: COMPILES AND RUNS
- click: COMPILES AND RUNS
- attrs: COMPILES AND RUNS
- functools intrinsics: VERIFIED
- itertools intrinsics: VERIFIED
- operator intrinsics: VERIFIED
- math intrinsics: VERIFIED
- json intrinsics: VERIFIED
- IR ops: P0 ops at full coverage

- [ ] **Step 7: Refresh Linear workspace artifacts**

Run:
```bash
python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
```

- [ ] **Step 8: Commit status updates**

Run:
```bash
git add docs/spec/STATUS.md ROADMAP.md
git commit -m "docs: update status — Wave B ecosystem unlock complete"
```
