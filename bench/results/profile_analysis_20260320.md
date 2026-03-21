# Profile Analysis - 2026-03-20

## Benchmarks Profiled

| Benchmark | Status | Output (correctness) |
|---|---|---|
| bench_sum.py | OK | 49999995000000 |
| bench_fib.py | OK | 832040 (fib(30)) |
| bench_sum_list.py | OK | 499999500000 |
| bench_str_split.py | OK | 150001 |
| bench_str_find.py | OK | 500000 |
| bench_matrix_math.py | FAIL | ImportError: No module named 'molt_buffer' |

All outputs match expected CPython results. `bench_matrix_math` requires a
`molt_buffer` extension module that is not available in the standard runtime.

## Environment

- `MOLT_PROFILE=1 MOLT_PROFILE_JSON=1`
- `uv run --python 3.12 python3 -m molt.cli run --trusted <bench>`
- Platform: macOS Darwin 25.3.0 (Apple Silicon)

## Top Observed Counters

### Allocation Pressure

| Benchmark | alloc_count | alloc_string | alloc_dict | alloc_tuple |
|---|---|---|---|---|
| bench_fib | 8,101,402 | 15,085 (0.2%) | 2,693,923 (33.3%) | 2,849 |
| bench_sum_list | 3,023,765 | 2,015,066 (66.6%) | 1,382 | 1,002,845 (33.2%) |
| bench_str_find | 523,827 | 515,103 (98.3%) | 1,387 | 2,850 |
| bench_str_split | 73,827 | 65,104 (88.2%) | 1,387 | 2,850 |
| bench_sum | 23,755 | 15,063 (63.4%) | 1,382 | 2,844 |

Key observations:
- **bench_fib** has massive dict allocation (2.7M dicts) despite being a pure
  recursive integer benchmark. This strongly suggests each recursive call frame
  allocates a new dict for locals, which is the dominant cost.
- **bench_sum_list** shows high string allocation (2M) even though the benchmark
  only does integer arithmetic on a list. This points to repr/conversion overhead
  in the iteration path.
- **bench_str_find** and **bench_str_split** are string-heavy as expected, but
  the 98% string allocation ratio in bench_str_find suggests the string builder
  pattern (`repeat_text`) is allocating intermediate strings aggressively.

### Call Bind IC (Inline Cache)

Across all benchmarks:
- **call_bind_ic_hit**: 25 total (5 per benchmark)
- **call_bind_ic_miss**: 432 total (~86 per benchmark)
- **Hit rate: 5.5%** -- extremely poor

The IC hit rate is catastrophically low. Nearly all call dispatches go through
the slow path. This is likely because the IC is only populated during startup
and the actual hot-loop calls (e.g., `range`, `fib`, `append`, `split`) are
dispatched through a generic path that never warms the IC.

### Attribute Site-Name Cache

- **attr_site_name_hit**: 85 total
- **attr_site_name_miss**: 107 total
- **Hit rate: 44.7%** -- needs improvement

The attribute lookup cache is cold for more than half of all lookups.

## Top 5 Optimization Targets

### 1. Dict Allocation in Recursive Calls (bench_fib: 2.7M dicts)

**Impact: Critical.** `bench_fib` allocates 2,693,923 dicts for a fib(30) call
that should require zero heap dicts. Each recursive call appears to allocate a
fresh dict for the locals frame. Fix: use stack-allocated or pooled locals
frames for functions with a statically known set of local variables.

### 2. String Allocation Pressure (2.6M total across benchmarks)

**Impact: High.** String allocations dominate in bench_str_find (98.3%),
bench_str_split (88.2%), and bench_sum_list (66.6%). The `repeat_text` helper
concatenates via `list.append` + `join`, yet still produces massive intermediate
string objects. Fix: implement small-string optimization (SSO), string interning
for short-lived temporaries, or arena allocation.

### 3. Call Bind IC Miss Rate (95%)

**Impact: High.** Only 5.5% of call-bind lookups hit the inline cache. For
hot loops calling builtins (`range`, `len`, `print`), the IC should be nearly
100%. Fix: ensure the IC is populated on first miss and subsequent calls use
the cached binding. Investigate whether the IC is being invalidated or bypassed
in the native codegen path.

### 4. Tuple Allocation in Iteration (bench_sum_list: 1M tuples)

**Impact: Medium.** `bench_sum_list` allocates 1,002,845 tuples while iterating
a list of integers. This suggests the iterator protocol or list comprehension
is boxing results into tuples. Fix: specialize the list iterator to avoid
tuple wrapping for simple element access.

### 5. Attribute Cache Cold Start (56% miss rate)

**Impact: Medium.** The attribute site-name cache has a 56% miss rate. For
benchmarks that access `.append`, `.split`, `.find`, `.join` in hot loops,
these should be cached after the first lookup. Fix: ensure shape-based
attribute caching is active and not being invalidated by dict allocations
in the locals frame path.

## Raw Profile Data

Profile JSON files are stored in `bench/results/profiles_20260320/`.
Analysis can be reproduced with:

```bash
python3 tools/profile_analyze.py bench/results/profiles_20260320/*.json
```
