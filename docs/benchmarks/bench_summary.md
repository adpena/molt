# Molt Bench Summary

## Inputs
- Native: `bench/results/full_native_refresh_20260519.json`; git_rev=1a7796eec67d6023d3a63520395723803733564e; created_at=2026-05-19T17:27:09.954967+00:00; timing_mode=warm_throughput, warmup=1, samples=3; system=cpu_count=18, load_avg=[3.71484375, 3.837890625, 4.2490234375], machine=arm64, platform=macOS-26.4-arm64-arm-64bit, python=3.12.13
- WASM: `bench/results/bench_wasm_20260328_102249.json`; git_rev=2b80788b94edbe8a99b883e09e78decbf798ed45; created_at=2026-03-28T10:22:49.339021+00:00; timing_mode=legacy-unknown, warmup=1, samples=3; system=cpu_count=18, load_avg=[5.90234375, 5.3505859375, 4.70947265625], machine=arm64, platform=macOS-26.4-arm64-arm-64bit, python=3.12.13
- NOTE: native and wasm results come from different git revisions; interpret combined ratios cautiously.

## Summary
- Benchmarks: 56 total; native ok 49/56; wasm ok 0/1.
- Median native speedup vs CPython: 1.63x.
- Median wasm speedup vs CPython: -.
- Median wasm/native ratio: -.
- Native regressions (< 1.0x): 8.
- Comparator coverage: PyPy 0/56, Codon 0/56, Nuitka 0/56, Pyodide 0/56.
- Missing wasm entries: bench_async_await, bench_attr_access, bench_bytearray_find, bench_bytearray_replace, bench_bytes_find, bench_bytes_find_only, bench_bytes_replace, bench_channel_throughput, bench_class_hierarchy, bench_counter_words, bench_csv_parse, bench_csv_parse_wide, bench_deeply_nested_loop, bench_descriptor_property, bench_dict_comprehension, bench_dict_ops, bench_dict_views, bench_etl_orders, bench_exception_heavy, bench_fib, bench_gc_pressure, bench_generator_iter, bench_import_time, bench_json_roundtrip, bench_list_ops, bench_list_slice, bench_matrix_math, bench_max_list, bench_memoryview_tobytes, bench_min_list, bench_parse_msgpack, bench_procedural_gen, bench_prod_list, bench_ptr_registry, bench_range_iter, bench_set_ops, bench_startup, bench_str_count, bench_str_count_unicode, bench_str_count_unicode_warm, bench_str_endswith, bench_str_find, bench_str_find_unicode, bench_str_find_unicode_warm, bench_str_join, bench_str_replace, bench_str_split, bench_str_startswith, bench_struct, bench_sum_list, bench_sum_list_hints, bench_try_except, bench_tuple_index, bench_tuple_pack, bench_tuple_slice.

## Regressions (Native < 1.0x)
| Benchmark | Speedup | Molt s | CPython s |
| --- | --- | --- | --- |
| bench_exception_heavy | 0.06x | 1.957929 | 0.121389 |
| bench_json_roundtrip | 0.15x | 0.102909 | 0.015420 |
| bench_counter_words | 0.31x | 0.069943 | 0.021645 |
| bench_etl_orders | 0.46x | 0.100697 | 0.045929 |
| bench_csv_parse_wide | 0.61x | 0.048072 | 0.029429 |
| bench_csv_parse | 0.74x | 0.023211 | 0.017281 |
| bench_generator_iter | 0.83x | 0.018378 | 0.015236 |
| bench_tuple_pack | 0.94x | 0.024365 | 0.022812 |

## WASM vs Native (Slowest)
| Benchmark | WASM s | Native s | WASM/Native |
| --- | --- | --- | --- |
| - | - | - | - |

## Molt vs PyPy (Both OK)
| Benchmark | Molt s | Comparator s | Molt/Comparator |
| --- | --- | --- | --- |
| - | - | - | - |

## Molt vs Codon (Both OK)
| Benchmark | Molt s | Comparator s | Molt/Comparator |
| --- | --- | --- | --- |
| - | - | - | - |

## Molt vs Nuitka (Both OK)
| Benchmark | Molt s | Comparator s | Molt/Comparator |
| --- | --- | --- | --- |
| - | - | - | - |

## Molt vs Pyodide (Both OK)
| Benchmark | Molt s | Comparator s | Molt/Comparator |
| --- | --- | --- | --- |
| - | - | - | - |

## Combined Table
| Benchmark | Molt OK | CPython s | PyPy s | Codon build s | Codon run s | Codon KB | Nuitka build s | Nuitka run s | Nuitka KB | Pyodide run s | Molt build s | Molt run s | Molt KB | Molt/CPython | Molt/PyPy | Molt/Codon | Molt/Nuitka | Molt/Pyodide | WASM OK | WASM s | WASM/Native | WASM/CPython |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| bench_async_await | no | 0.042179 | - | - | - | - | - | - | - | - | 11.887222 | - | 24287.335938 | - | - | - | - | - | no | - | - | - |
| bench_attr_access | yes | 0.022664 | - | - | - | - | - | - | - | - | 0.620140 | 0.012087 | 7458.476562 | 0.53x | - | - | - | - | no | - | - | - |
| bench_bytearray_find | yes | 0.010788 | - | - | - | - | - | - | - | - | 0.740189 | 0.008557 | 7458.476562 | 0.79x | - | - | - | - | no | - | - | - |
| bench_bytearray_replace | yes | 0.027338 | - | - | - | - | - | - | - | - | 0.747778 | 0.013468 | 7458.484375 | 0.49x | - | - | - | - | no | - | - | - |
| bench_bytes_find | yes | 0.157111 | - | - | - | - | - | - | - | - | 0.801626 | 0.007778 | 7442.351562 | 0.05x | - | - | - | - | no | - | - | - |
| bench_bytes_find_only | yes | 0.186876 | - | - | - | - | - | - | - | - | 0.709859 | 0.026645 | 7442.351562 | 0.14x | - | - | - | - | no | - | - | - |
| bench_bytes_replace | yes | 0.014953 | - | - | - | - | - | - | - | - | 0.887921 | 0.006193 | 7458.476562 | 0.41x | - | - | - | - | no | - | - | - |
| bench_channel_throughput | no | - | - | - | - | - | - | - | - | - | 11.689951 | - | 24335.750000 | - | - | - | - | - | no | - | - | - |
| bench_class_hierarchy | yes | 0.384959 | - | - | - | - | - | - | - | - | 1.079770 | 0.090114 | 7458.492188 | 0.23x | - | - | - | - | no | - | - | - |
| bench_counter_words | yes | 0.021645 | - | - | - | - | - | - | - | - | 4.206579 | 0.069943 | 12718.554688 | 3.23x | - | - | - | - | no | - | - | - |
| bench_csv_parse | yes | 0.017281 | - | - | - | - | - | - | - | - | 0.855474 | 0.023211 | 7458.476562 | 1.34x | - | - | - | - | no | - | - | - |
| bench_csv_parse_wide | yes | 0.029429 | - | - | - | - | - | - | - | - | 0.854247 | 0.048072 | 7474.617188 | 1.63x | - | - | - | - | no | - | - | - |
| bench_deeply_nested_loop | yes | 0.032198 | - | - | - | - | - | - | - | - | 0.766040 | 0.012458 | 7458.484375 | 0.39x | - | - | - | - | no | - | - | - |
| bench_descriptor_property | yes | 0.023821 | - | - | - | - | - | - | - | - | 0.978661 | 0.014930 | 7458.500000 | 0.63x | - | - | - | - | no | - | - | - |
| bench_dict_comprehension | no | 0.022913 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_dict_ops | yes | 0.011500 | - | - | - | - | - | - | - | - | 0.623590 | 0.004766 | 7442.343750 | 0.41x | - | - | - | - | no | - | - | - |
| bench_dict_views | yes | 0.011378 | - | - | - | - | - | - | - | - | 0.658982 | 0.006973 | 7458.476562 | 0.61x | - | - | - | - | no | - | - | - |
| bench_etl_orders | yes | 0.045929 | - | - | - | - | - | - | - | - | 4.093457 | 0.100697 | 12024.476562 | 2.19x | - | - | - | - | no | - | - | - |
| bench_exception_heavy | yes | 0.121389 | - | - | - | - | - | - | - | - | 0.748414 | 1.957929 | 7458.476562 | 16.13x | - | - | - | - | no | - | - | - |
| bench_fib | yes | 0.058108 | - | - | - | - | - | - | - | - | 0.773368 | 0.051725 | 7442.343750 | 0.89x | - | - | - | - | no | - | - | - |
| bench_gc_pressure | yes | 0.422199 | - | - | - | - | - | - | - | - | 0.812146 | 0.385473 | 7442.351562 | 0.91x | - | - | - | - | no | - | - | - |
| bench_generator_iter | yes | 0.015236 | - | - | - | - | - | - | - | - | 3.252918 | 0.018378 | 11153.398438 | 1.21x | - | - | - | - | no | - | - | - |
| bench_import_time | no | 0.021409 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_json_roundtrip | yes | 0.015420 | - | - | - | - | - | - | - | - | 1.127830 | 0.102909 | 7587.570312 | 6.67x | - | - | - | - | no | - | - | - |
| bench_list_ops | yes | 0.011362 | - | - | - | - | - | - | - | - | 0.714547 | 0.005530 | 7442.343750 | 0.49x | - | - | - | - | no | - | - | - |
| bench_list_slice | yes | 0.016544 | - | - | - | - | - | - | - | - | 0.693371 | 0.015395 | 7442.351562 | 0.93x | - | - | - | - | no | - | - | - |
| bench_matrix_math | yes | 0.029719 | - | - | - | - | - | - | - | - | 0.778309 | 0.004924 | 7458.476562 | 0.17x | - | - | - | - | no | - | - | - |
| bench_max_list | yes | 0.026799 | - | - | - | - | - | - | - | - | 0.656822 | 0.009897 | 7442.343750 | 0.37x | - | - | - | - | no | - | - | - |
| bench_memoryview_tobytes | yes | 0.010627 | - | - | - | - | - | - | - | - | 0.763334 | 0.008375 | 7458.484375 | 0.79x | - | - | - | - | no | - | - | - |
| bench_min_list | yes | 0.026204 | - | - | - | - | - | - | - | - | 0.654574 | 0.009117 | 7442.343750 | 0.35x | - | - | - | - | no | - | - | - |
| bench_parse_msgpack | no | 0.016810 | - | - | - | - | - | - | - | - | 0.800556 | - | 7442.351562 | - | - | - | - | - | no | - | - | - |
| bench_procedural_gen | no | 0.016513 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_prod_list | yes | 0.023905 | - | - | - | - | - | - | - | - | 0.661200 | 0.007772 | 7442.351562 | 0.33x | - | - | - | - | no | - | - | - |
| bench_ptr_registry | no | - | - | - | - | - | - | - | - | - | 3.449390 | - | 11185.679688 | - | - | - | - | - | no | - | - | - |
| bench_range_iter | yes | 0.013310 | - | - | - | - | - | - | - | - | 0.715002 | 0.004649 | 7442.351562 | 0.35x | - | - | - | - | no | - | - | - |
| bench_set_ops | yes | 0.014013 | - | - | - | - | - | - | - | - | 0.862116 | 0.010925 | 7442.343750 | 0.78x | - | - | - | - | no | - | - | - |
| bench_startup | yes | 0.009683 | - | - | - | - | - | - | - | - | 0.745429 | 0.004239 | 7442.343750 | 0.44x | - | - | - | - | no | - | - | - |
| bench_str_count | yes | 0.019982 | - | - | - | - | - | - | - | - | 0.835706 | 0.011049 | 7458.476562 | 0.55x | - | - | - | - | no | - | - | - |
| bench_str_count_unicode | yes | 0.017303 | - | - | - | - | - | - | - | - | 0.786692 | 0.011660 | 7458.484375 | 0.67x | - | - | - | - | no | - | - | - |
| bench_str_count_unicode_warm | yes | 0.026257 | - | - | - | - | - | - | - | - | 0.772107 | 0.010529 | 7458.484375 | 0.40x | - | - | - | - | no | - | - | - |
| bench_str_endswith | yes | 0.017828 | - | - | - | - | - | - | - | - | 0.819544 | 0.012590 | 7458.476562 | 0.71x | - | - | - | - | no | - | - | - |
| bench_str_find | yes | 0.017651 | - | - | - | - | - | - | - | - | 0.766042 | 0.014963 | 7458.468750 | 0.85x | - | - | - | - | no | - | - | - |
| bench_str_find_unicode | yes | 0.014412 | - | - | - | - | - | - | - | - | 0.774473 | 0.009205 | 7458.476562 | 0.64x | - | - | - | - | no | - | - | - |
| bench_str_find_unicode_warm | yes | 0.015075 | - | - | - | - | - | - | - | - | 0.749262 | 0.008075 | 7458.484375 | 0.54x | - | - | - | - | no | - | - | - |
| bench_str_join | yes | 0.021469 | - | - | - | - | - | - | - | - | 0.837438 | 0.011421 | 7442.343750 | 0.53x | - | - | - | - | no | - | - | - |
| bench_str_replace | yes | 0.015398 | - | - | - | - | - | - | - | - | 0.748564 | 0.011084 | 7442.351562 | 0.72x | - | - | - | - | no | - | - | - |
| bench_str_split | yes | 0.012973 | - | - | - | - | - | - | - | - | 0.761303 | 0.006309 | 7442.351562 | 0.49x | - | - | - | - | no | - | - | - |
| bench_str_startswith | yes | 0.018388 | - | - | - | - | - | - | - | - | 0.775062 | 0.012100 | 7458.476562 | 0.66x | - | - | - | - | no | - | - | - |
| bench_struct | yes | 0.087884 | - | - | - | - | - | - | - | - | 0.563421 | 0.009376 | 7458.468750 | 0.11x | - | - | - | - | no | - | - | - |
| bench_sum | yes | 0.144394 | - | - | - | - | - | - | - | - | 0.630105 | 0.006356 | 7442.343750 | 0.04x | - | - | - | - | no | - | - | - |
| bench_sum_list | yes | 0.033029 | - | - | - | - | - | - | - | - | 0.677816 | 0.008081 | 7458.484375 | 0.24x | - | - | - | - | no | - | - | - |
| bench_sum_list_hints | yes | 0.029366 | - | - | - | - | - | - | - | - | 1.046568 | 0.009474 | 7442.351562 | 0.32x | - | - | - | - | no | - | - | - |
| bench_try_except | yes | 0.014897 | - | - | - | - | - | - | - | - | 0.707592 | 0.011692 | 7442.351562 | 0.78x | - | - | - | - | no | - | - | - |
| bench_tuple_index | yes | 0.019090 | - | - | - | - | - | - | - | - | 0.729473 | 0.014729 | 7442.351562 | 0.77x | - | - | - | - | no | - | - | - |
| bench_tuple_pack | yes | 0.022812 | - | - | - | - | - | - | - | - | 0.706872 | 0.024365 | 7458.476562 | 1.07x | - | - | - | - | no | - | - | - |
| bench_tuple_slice | yes | 0.017368 | - | - | - | - | - | - | - | - | 0.785005 | 0.016814 | 7442.351562 | 0.97x | - | - | - | - | no | - | - | - |

Generated by `tools/bench_report.py`.
