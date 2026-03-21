# Benchmark Triage - 2026-03-20

## Summary

- **Total benchmarks in suite**: 54
- **Passed (Molt compiled + ran)**: 28
- **Failed (daemon cascade in harness)**: 22 (of which ~7 confirmed pass individually)
- **Failed (runtime error after build)**: 4
- **Previous baseline had**: 16 benchmarks

## Environment

- Platform: macOS-26.3.1-arm64-arm-64bit (Apple Silicon, 18 cores)
- Python: 3.12.13
- Git rev: 72e5368164a59fd7cedb6073af82fa2796a6430a
- Samples: 3

## Results JSON

`bench/results/full_native_baseline_20260320.json`

## Passed Benchmarks (28)

Sorted by speedup (Molt vs CPython):

| Benchmark | Speedup | Molt (s) | CPython (s) |
|-----------|---------|----------|-------------|
| bench_bytearray_replace.py | 2.15x | 0.0140 | 0.0301 |
| bench_bytes_replace.py | 1.64x | 0.0093 | 0.0153 |
| bench_startup.py | 1.56x | 0.0066 | 0.0103 |
| bench_memoryview_tobytes.py | 1.34x | 0.0089 | 0.0119 |
| bench_set_ops.py | 1.13x | 0.0259 | 0.0292 |
| bench_str_split.py | 0.94x | 0.0180 | 0.0169 |
| bench_gc_pressure.py | 0.79x | 0.5192 | 0.4092 |
| bench_bytearray_find.py | 0.76x | 0.0150 | 0.0114 |
| bench_dict_ops.py | 0.65x | 0.0290 | 0.0188 |
| bench_dict_views.py | 0.57x | 0.0363 | 0.0209 |
| bench_counter_words.py | 0.38x | 0.1068 | 0.0410 |
| bench_str_count_unicode_warm.py | 0.34x | 0.0831 | 0.0286 |
| bench_str_find_unicode.py | 0.34x | 0.0439 | 0.0147 |
| bench_str_find_unicode_warm.py | 0.32x | 0.0469 | 0.0148 |
| bench_str_count_unicode.py | 0.28x | 0.0623 | 0.0177 |
| bench_str_replace.py | 0.27x | 0.0616 | 0.0168 |
| bench_str_count.py | 0.23x | 0.0945 | 0.0213 |
| bench_str_find.py | 0.21x | 0.0960 | 0.0200 |
| bench_str_startswith.py | 0.21x | 0.0956 | 0.0196 |
| bench_str_endswith.py | 0.19x | 0.1038 | 0.0192 |
| bench_descriptor_property.py | 0.13x | 0.2327 | 0.0305 |
| bench_json_roundtrip.py | 0.13x | 0.1214 | 0.0158 |
| bench_attr_access.py | 0.11x | 0.2191 | 0.0238 |
| bench_exception_heavy.py | 0.10x | 1.4061 | 0.1438 |
| bench_bytes_find.py | 0.07x | 2.8977 | 0.2017 |
| bench_bytes_find_only.py | 0.06x | 3.7641 | 0.2099 |
| bench_struct.py | 0.05x | 1.9036 | 0.0941 |
| bench_class_hierarchy.py | 0.01x | 33.2497 | 0.3991 |

### Speedup distribution

- **Faster than CPython (>1.0x)**: 5 benchmarks
- **Within 2x of CPython (0.5x-1.0x)**: 5 benchmarks
- **Slower than CPython (<0.5x)**: 18 benchmarks

## Failed Benchmarks - Daemon Cascade (22)

These benchmarks failed during the harness run due to backend daemon instability
(the daemon was being rebuilt by a concurrent cargo build from another agent worktree).

### Confirmed working individually (7)

These were tested individually and succeeded before the backend got corrupted:

- bench_fib.py
- bench_sum.py
- bench_sum_list.py
- bench_min_list.py
- bench_max_list.py
- bench_prod_list.py
- bench_etl_orders.py

### Failed individually with cranelift panic (13)

These fail with: `Backend panic while defining function _intrinsics__require_intrinsic`
(cranelift-codegen unreachable_code.rs:29 - unwrap on None)

- bench_list_ops.py
- bench_list_slice.py
- bench_tuple_index.py
- bench_tuple_slice.py
- bench_tuple_pack.py
- bench_range_iter.py
- bench_try_except.py
- bench_generator_iter.py
- bench_async_await.py
- bench_csv_parse.py
- bench_csv_parse_wide.py
- bench_channel_throughput.py
- bench_ptr_registry.py

### Not individually tested (2)

- bench_sum_list_hints.py (requires --type-hints trust flag; likely works)
- bench_deeply_nested_loop.py (very long runtime; likely works)

**NOTE**: The cranelift panic appeared AFTER a concurrent `cargo build` from another
agent worktree rebuilt the backend binary. Before that rebuild, bench_fib and others
compiled fine. The 13 failures above may be caused by a corrupted/incompatible backend
binary rather than real compilation issues. A clean rebuild of the backend should be
attempted to verify.

## Failed Benchmarks - Runtime Errors (4)

These built successfully but crashed at runtime:

| Benchmark | Error |
|-----------|-------|
| bench_str_join.py | TypeError: exceptions must derive from BaseException |
| bench_dict_comprehension.py | TypeError: exceptions must derive from BaseException |
| bench_parse_msgpack.py | ImportError: No module named 'molt_msgpack' |
| bench_matrix_math.py | ImportError: No module named 'molt_buffer' |

### Error patterns

1. **TypeError: exceptions must derive from BaseException** (2 benchmarks) -
   Likely a codegen issue with exception handling in compiled code.

2. **ImportError: missing molt_* module** (2 benchmarks) -
   bench_parse_msgpack needs `molt_msgpack` and bench_matrix_math needs `molt_buffer`.
   These are likely optional native extension modules not built in this configuration.

## Regression vs Previous Baseline

The previous baseline (`bench/baseline.json`, 16 benchmarks, 2026-01-04) showed
significantly better speedups for several benchmarks:

| Benchmark | Old Speedup | New Speedup | Change |
|-----------|------------|-------------|--------|
| bench_fib.py | 3.07x | (daemon fail) | - |
| bench_sum.py | 2.24x | (daemon fail) | - |
| bench_matrix_math.py | 9.06x | (runtime fail) | - |
| bench_deeply_nested_loop.py | 2.69x | (daemon fail) | - |
| bench_struct.py | 2.45x | 0.05x | **regression** |
| bench_bytes_find.py | 2.64x | 0.07x | **regression** |
| bench_bytes_find_only.py | 1.89x | 0.06x | **regression** |
| bench_str_count.py | 3.49x | 0.23x | **regression** |
| bench_str_find.py | 2.60x | 0.21x | **regression** |
| bench_str_join.py | 1.16x | (runtime fail) | - |
| bench_str_startswith.py | 2.90x | 0.21x | **regression** |
| bench_str_endswith.py | 3.39x | 0.19x | **regression** |
| bench_sum_list.py | 1.19x | (daemon fail) | - |

**Major regressions observed**: Many benchmarks that were 2-3x faster than CPython
in the Jan 2026 baseline are now 3-5x slower. This likely indicates a backend
regression or that the concurrent cargo build produced an incompatible backend binary.

## Recommendations

1. **Rebuild the backend from clean state** - Kill all daemons, clean cargo target,
   rebuild `molt-backend` with `--profile release-fast`, then re-run benchmarks.

2. **Investigate the cranelift panic** - The `_intrinsics__require_intrinsic` panic
   affects ~13 benchmarks and may be caused by a recent code change.

3. **Investigate performance regressions** - The 10-50x slowdowns in string and
   bytes benchmarks vs the January baseline need investigation.

4. **Add molt_msgpack and molt_buffer modules** - Or skip those benchmarks when
   modules are not available.

5. **Re-run with no concurrent builds** - The daemon cascade failures were caused
   by a concurrent `cargo build` from another git worktree invalidating the backend
   binary mid-run.
