# Molt Bench Summary

## Inputs
- Native: `bench/results/full_native_baseline_20260320.json`; git_rev=72e5368164a59fd7cedb6073af82fa2796a6430a; created_at=2026-03-21T02:52:46.950072+00:00; system=cpu_count=18, load_avg=[12.78857421875, 14.8154296875, 11.0595703125], machine=arm64, platform=macOS-26.3.1-arm64-arm-64bit, python=3.12.13
- WASM: `bench/results/bench_wasm_20260328_102249.json`; git_rev=2b80788b94edbe8a99b883e09e78decbf798ed45; created_at=2026-03-28T10:22:49.339021+00:00; system=cpu_count=18, load_avg=[5.90234375, 5.3505859375, 4.70947265625], machine=arm64, platform=macOS-26.4-arm64-arm-64bit, python=3.12.13
- NOTE: native and wasm results come from different git revisions; interpret combined ratios cautiously.

## Summary
- Benchmarks: 54 total; native ok 28/54; wasm ok 0/1.
- Median native speedup vs CPython: 0.30x.
- Median wasm speedup vs CPython: -.
- Median wasm/native ratio: -.
- Native regressions (< 1.0x): 23.
- Comparator coverage: PyPy 51/54, Codon 0/54, Nuitka 0/54, Pyodide 0/54.
- Missing wasm entries: bench_async_await, bench_attr_access, bench_bytearray_find, bench_bytearray_replace, bench_bytes_find, bench_bytes_find_only, bench_bytes_replace, bench_channel_throughput, bench_class_hierarchy, bench_counter_words, bench_csv_parse, bench_csv_parse_wide, bench_deeply_nested_loop, bench_descriptor_property, bench_dict_comprehension, bench_dict_ops, bench_dict_views, bench_etl_orders, bench_exception_heavy, bench_fib, bench_gc_pressure, bench_generator_iter, bench_json_roundtrip, bench_list_ops, bench_list_slice, bench_matrix_math, bench_max_list, bench_memoryview_tobytes, bench_min_list, bench_parse_msgpack, bench_prod_list, bench_ptr_registry, bench_range_iter, bench_set_ops, bench_startup, bench_str_count, bench_str_count_unicode, bench_str_count_unicode_warm, bench_str_endswith, bench_str_find, bench_str_find_unicode, bench_str_find_unicode_warm, bench_str_join, bench_str_replace, bench_str_split, bench_str_startswith, bench_struct, bench_sum_list, bench_sum_list_hints, bench_try_except, bench_tuple_index, bench_tuple_pack, bench_tuple_slice.

## Regressions (Native < 1.0x)
| Benchmark | Speedup | Molt s | CPython s |
| --- | --- | --- | --- |
| bench_class_hierarchy | 0.01x | 33.249677 | 0.399121 |
| bench_struct | 0.05x | 1.903567 | 0.094102 |
| bench_bytes_find_only | 0.06x | 3.764117 | 0.209854 |
| bench_bytes_find | 0.07x | 2.897665 | 0.201727 |
| bench_exception_heavy | 0.10x | 1.406110 | 0.143784 |
| bench_attr_access | 0.11x | 0.219055 | 0.023784 |
| bench_json_roundtrip | 0.13x | 0.121444 | 0.015798 |
| bench_descriptor_property | 0.13x | 0.232723 | 0.030456 |
| bench_str_endswith | 0.19x | 0.103849 | 0.019240 |
| bench_str_startswith | 0.21x | 0.095630 | 0.019630 |

## WASM vs Native (Slowest)
| Benchmark | WASM s | Native s | WASM/Native |
| --- | --- | --- | --- |
| - | - | - | - |

## Molt vs PyPy (Both OK)
| Benchmark | Molt s | Comparator s | Molt/Comparator |
| --- | --- | --- | --- |
| bench_class_hierarchy | 33.249677 | 0.041078 | 809.42x |
| bench_struct | 1.903567 | 0.041849 | 45.49x |
| bench_bytes_find | 2.897665 | 0.079648 | 36.38x |
| bench_exception_heavy | 1.406110 | 0.053166 | 26.45x |
| bench_bytes_find_only | 3.764117 | 0.557311 | 6.75x |
| bench_attr_access | 0.219055 | 0.039539 | 5.54x |
| bench_descriptor_property | 0.232723 | 0.047533 | 4.90x |
| bench_str_endswith | 0.103849 | 0.042742 | 2.43x |
| bench_str_startswith | 0.095630 | 0.044777 | 2.14x |
| bench_str_count | 0.094528 | 0.045583 | 2.07x |

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
| bench_async_await | no | 0.043817 | 0.103757 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_attr_access | yes | 0.023784 | 0.039539 | - | - | - | - | - | - | - | 12.960254 | 0.219055 | 30931.976562 | 9.21x | 5.54x | - | - | - | no | - | - | - |
| bench_bytearray_find | yes | 0.011446 | 0.039507 | - | - | - | - | - | - | - | 11.195212 | 0.014998 | 30910.742188 | 1.31x | 0.38x | - | - | - | no | - | - | - |
| bench_bytearray_replace | yes | 0.030070 | 0.073547 | - | - | - | - | - | - | - | 11.356831 | 0.014000 | 30910.968750 | 0.47x | 0.19x | - | - | - | no | - | - | - |
| bench_bytes_find | yes | 0.201727 | 0.079648 | - | - | - | - | - | - | - | 13.735348 | 2.897665 | 30894.382812 | 14.36x | 36.38x | - | - | - | no | - | - | - |
| bench_bytes_find_only | yes | 0.209854 | 0.557311 | - | - | - | - | - | - | - | 11.652172 | 3.764117 | 30910.835938 | 17.94x | 6.75x | - | - | - | no | - | - | - |
| bench_bytes_replace | yes | 0.015324 | 0.046281 | - | - | - | - | - | - | - | 11.248881 | 0.009325 | 30910.679688 | 0.61x | 0.20x | - | - | - | no | - | - | - |
| bench_channel_throughput | no | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_class_hierarchy | yes | 0.399121 | 0.041078 | - | - | - | - | - | - | - | 69.080161 | 33.249677 | 30780.570312 | 83.31x | 809.42x | - | - | - | no | - | - | - |
| bench_counter_words | yes | 0.041013 | 0.099853 | - | - | - | - | - | - | - | 35.733874 | 0.106845 | 47862.554688 | 2.61x | 1.07x | - | - | - | no | - | - | - |
| bench_csv_parse | no | 0.018588 | 0.043511 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_csv_parse_wide | no | 0.032090 | 0.067662 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_deeply_nested_loop | no | 0.032507 | 0.041053 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_descriptor_property | yes | 0.030456 | 0.047533 | - | - | - | - | - | - | - | 17.958950 | 0.232723 | 30953.187500 | 7.64x | 4.90x | - | - | - | no | - | - | - |
| bench_dict_comprehension | no | 0.023631 | 0.049080 | - | - | - | - | - | - | - | 14.388251 | - | 30751.312500 | - | - | - | - | - | no | - | - | - |
| bench_dict_ops | yes | 0.018818 | 0.053599 | - | - | - | - | - | - | - | 20.092476 | 0.028984 | 30893.937500 | 1.54x | 0.54x | - | - | - | no | - | - | - |
| bench_dict_views | yes | 0.020859 | 0.062324 | - | - | - | - | - | - | - | 22.305234 | 0.036312 | 30926.304688 | 1.74x | 0.58x | - | - | - | no | - | - | - |
| bench_etl_orders | no | 0.054194 | 0.070173 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_exception_heavy | yes | 0.143784 | 0.053166 | - | - | - | - | - | - | - | 37.167246 | 1.406110 | 30684.554688 | 9.78x | 26.45x | - | - | - | no | - | - | - |
| bench_fib | no | 0.060794 | 0.048855 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_gc_pressure | yes | 0.409200 | 0.307045 | - | - | - | - | - | - | - | 10.933512 | 0.519157 | 30684.226562 | 1.27x | 1.69x | - | - | - | no | - | - | - |
| bench_generator_iter | no | 0.016054 | 0.048782 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_json_roundtrip | yes | 0.015798 | 0.059793 | - | - | - | - | - | - | - | 34.797451 | 0.121444 | 42310.460938 | 7.69x | 2.03x | - | - | - | no | - | - | - |
| bench_list_ops | no | 0.014171 | 0.047452 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_list_slice | no | 0.019548 | 0.046296 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_matrix_math | no | 0.031183 | 0.041908 | - | - | - | - | - | - | - | 32.180167 | - | 30930.429688 | - | - | - | - | - | no | - | - | - |
| bench_max_list | no | 0.029036 | 0.041338 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_memoryview_tobytes | yes | 0.011887 | 0.095807 | - | - | - | - | - | - | - | 11.058896 | 0.008857 | 30911.750000 | 0.75x | 0.09x | - | - | - | no | - | - | - |
| bench_min_list | no | 0.035456 | 0.051973 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_parse_msgpack | no | 0.016786 | - | - | - | - | - | - | - | - | 10.965979 | - | 30912.601562 | - | - | - | - | - | no | - | - | - |
| bench_prod_list | no | 0.027408 | 0.042327 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_ptr_registry | no | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_range_iter | no | 0.016508 | 0.042930 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_set_ops | yes | 0.029166 | 0.051906 | - | - | - | - | - | - | - | 38.048615 | 0.025881 | 30700.250000 | 0.89x | 0.50x | - | - | - | no | - | - | - |
| bench_startup | yes | 0.010347 | 0.038309 | - | - | - | - | - | - | - | 12.611910 | 0.006623 | 30667.859375 | 0.64x | 0.17x | - | - | - | no | - | - | - |
| bench_str_count | yes | 0.021285 | 0.045583 | - | - | - | - | - | - | - | 11.538849 | 0.094528 | 30914.007812 | 4.44x | 2.07x | - | - | - | no | - | - | - |
| bench_str_count_unicode | yes | 0.017734 | 0.045389 | - | - | - | - | - | - | - | 11.282721 | 0.062297 | 30914.796875 | 3.51x | 1.37x | - | - | - | no | - | - | - |
| bench_str_count_unicode_warm | yes | 0.028591 | 0.060731 | - | - | - | - | - | - | - | 11.530958 | 0.083139 | 30931.562500 | 2.91x | 1.37x | - | - | - | no | - | - | - |
| bench_str_endswith | yes | 0.019240 | 0.042742 | - | - | - | - | - | - | - | 12.497095 | 0.103849 | 30914.273438 | 5.40x | 2.43x | - | - | - | no | - | - | - |
| bench_str_find | yes | 0.020014 | 0.048759 | - | - | - | - | - | - | - | 11.435626 | 0.095984 | 30913.968750 | 4.80x | 1.97x | - | - | - | no | - | - | - |
| bench_str_find_unicode | yes | 0.014727 | 0.046462 | - | - | - | - | - | - | - | 11.641488 | 0.043869 | 30914.757812 | 2.98x | 0.94x | - | - | - | no | - | - | - |
| bench_str_find_unicode_warm | yes | 0.014788 | 0.046690 | - | - | - | - | - | - | - | 19.669258 | 0.046862 | 30931.484375 | 3.17x | 1.00x | - | - | - | no | - | - | - |
| bench_str_join | no | 0.023788 | 0.043904 | - | - | - | - | - | - | - | 11.213165 | - | 30910.281250 | - | - | - | - | - | no | - | - | - |
| bench_str_replace | yes | 0.016836 | 0.045786 | - | - | - | - | - | - | - | 11.511492 | 0.061619 | 30914.273438 | 3.66x | 1.35x | - | - | - | no | - | - | - |
| bench_str_split | yes | 0.016900 | 0.047830 | - | - | - | - | - | - | - | 34.008610 | 0.017992 | 30914.054688 | 1.06x | 0.38x | - | - | - | no | - | - | - |
| bench_str_startswith | yes | 0.019630 | 0.044777 | - | - | - | - | - | - | - | 10.936105 | 0.095630 | 30914.460938 | 4.87x | 2.14x | - | - | - | no | - | - | - |
| bench_struct | yes | 0.094102 | 0.041849 | - | - | - | - | - | - | - | 31.578684 | 1.903567 | 30931.546875 | 20.23x | 45.49x | - | - | - | no | - | - | - |
| bench_sum | no | 0.165260 | 0.041837 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | 0.000000 | - | 0.00x |
| bench_sum_list | no | 0.031831 | 0.037774 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_sum_list_hints | no | 0.037908 | 0.046685 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_try_except | no | 0.017068 | 0.044597 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_tuple_index | no | 0.023945 | 0.048655 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_tuple_pack | no | 0.028192 | 0.044838 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |
| bench_tuple_slice | no | 0.018484 | 0.045302 | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | no | - | - | - |

Generated by `tools/bench_report.py`.
