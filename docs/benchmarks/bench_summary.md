# Molt Bench Summary

## Inputs
- Native: `bench/results/full_native_refresh_20260522_tracked_vec.json`; git_rev=8cb430a3feedce2023c0c857371b5b73525dc98f; created_at=2026-05-23T00:19:56.674455+00:00; timing_mode=warm_throughput, warmup=1, samples=3; system=cpu_count=18, load_avg=[4.6904296875, 5.2109375, 5.55322265625], machine=arm64, platform=macOS-26.4-arm64-arm-64bit, python=3.12.13
- WASM: `bench/results/bench_wasm_20260522_tracked_vec.json`; git_rev=8cb430a3feedce2023c0c857371b5b73525dc98f; created_at=2026-05-23T00:46:16.865341+00:00; timing_mode=legacy-unknown, warmup=0, samples=1; system=cpu_count=18, load_avg=[11.31298828125, 11.22021484375, 9.830078125], machine=arm64, platform=macOS-26.4-arm64-arm-64bit, python=3.12.13

## Summary
- Benchmarks: 56 total; native ok 56/56; wasm ok 53/56.
- Median native speedup vs CPython: 1.02x.
- Median wasm speedup vs CPython: 0.14x.
- Median wasm/native ratio: 7.57x.
- Native regressions (< 1.0x): 18.
- Comparator coverage: PyPy 0/56, Codon 0/56, Nuitka 0/56, Pyodide 0/56.

## Regressions (Native < 1.0x)
| Benchmark | Speedup | Molt s | CPython s |
| --- | --- | --- | --- |
| bench_struct | 0.04x | 2.793627 | 0.099860 |
| bench_exception_heavy | 0.55x | 0.226549 | 0.125517 |
| bench_csv_parse_wide | 0.56x | 0.065287 | 0.036650 |
| bench_etl_orders | 0.64x | 0.098573 | 0.062983 |
| bench_parse_msgpack | 0.86x | 0.039361 | 0.033661 |
| bench_csv_parse | 0.88x | 0.038152 | 0.033450 |
| bench_tuple_slice | 0.93x | 0.036794 | 0.034359 |
| bench_str_find | 0.95x | 0.035382 | 0.033763 |
| bench_set_ops | 0.96x | 0.035918 | 0.034393 |
| bench_try_except | 0.96x | 0.037528 | 0.036085 |

## WASM vs Native (Slowest)
| Benchmark | WASM s | Native s | WASM/Native |
| --- | --- | --- | --- |
| bench_class_hierarchy | 2.159632 | 0.059399 | 36.36x |
| bench_deeply_nested_loop | 0.801145 | 0.032833 | 24.40x |
| bench_fib | 1.217280 | 0.063327 | 19.22x |
| bench_tuple_pack | 0.617845 | 0.035785 | 17.27x |
| bench_tuple_index | 0.549995 | 0.033341 | 16.50x |
| bench_exception_heavy | 3.195713 | 0.226549 | 14.11x |
| bench_struct | 37.599707 | 2.793627 | 13.46x |
| bench_attr_access | 0.420170 | 0.032965 | 12.75x |
| bench_etl_orders | 1.211991 | 0.098573 | 12.30x |
| bench_generator_iter | 0.404712 | 0.033313 | 12.15x |

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
| bench_async_await | yes | 0.062018 | - | - | - | - | - | - | - | - | 12.569134 | 0.062619 | 21692.695312 | 1.01x | - | - | - | - | no | - | - | - |
| bench_attr_access | yes | 0.033940 | - | - | - | - | - | - | - | - | 1.949524 | 0.032965 | 7605.539062 | 0.97x | - | - | - | - | yes | 0.420170 | 12.75x | 12.38x |
| bench_bytearray_find | yes | 0.036735 | - | - | - | - | - | - | - | - | 2.039961 | 0.034802 | 7605.523438 | 0.95x | - | - | - | - | yes | 0.263587 | 7.57x | 7.18x |
| bench_bytearray_replace | yes | 0.033962 | - | - | - | - | - | - | - | - | 2.236336 | 0.034134 | 7605.531250 | 1.01x | - | - | - | - | yes | 0.301103 | 8.82x | 8.87x |
| bench_bytes_find | yes | 0.165214 | - | - | - | - | - | - | - | - | 2.102550 | 0.033051 | 7605.539062 | 0.20x | - | - | - | - | yes | 0.124020 | 3.75x | 0.75x |
| bench_bytes_find_only | yes | 0.216414 | - | - | - | - | - | - | - | - | 2.158126 | 0.034504 | 7605.539062 | 0.16x | - | - | - | - | yes | 0.203196 | 5.89x | 0.94x |
| bench_bytes_replace | yes | 0.034750 | - | - | - | - | - | - | - | - | 2.065831 | 0.034768 | 7605.523438 | 1.00x | - | - | - | - | yes | 0.172363 | 4.96x | 4.96x |
| bench_channel_throughput | yes | - | - | - | - | - | - | - | - | - | 14.723215 | 0.060747 | 24354.968750 | - | - | - | - | - | no | - | - | - |
| bench_class_hierarchy | yes | 0.411938 | - | - | - | - | - | - | - | - | 2.554800 | 0.059399 | 7621.664062 | 0.14x | - | - | - | - | yes | 2.159632 | 36.36x | 5.24x |
| bench_counter_words | yes | 0.035508 | - | - | - | - | - | - | - | - | 5.257597 | 0.033815 | 11930.382812 | 0.95x | - | - | - | - | yes | 0.390949 | 11.56x | 11.01x |
| bench_csv_parse | yes | 0.033450 | - | - | - | - | - | - | - | - | 2.091007 | 0.038152 | 7621.664062 | 1.14x | - | - | - | - | yes | 0.377928 | 9.91x | 11.30x |
| bench_csv_parse_wide | yes | 0.036650 | - | - | - | - | - | - | - | - | 2.080900 | 0.065287 | 7637.804688 | 1.78x | - | - | - | - | yes | 0.561080 | 8.59x | 15.31x |
| bench_deeply_nested_loop | yes | 0.035361 | - | - | - | - | - | - | - | - | 2.130073 | 0.032833 | 7605.531250 | 0.93x | - | - | - | - | yes | 0.801145 | 24.40x | 22.66x |
| bench_descriptor_property | yes | 0.033836 | - | - | - | - | - | - | - | - | 2.291096 | 0.034602 | 7605.546875 | 1.02x | - | - | - | - | yes | 0.419307 | 12.12x | 12.39x |
| bench_dict_comprehension | yes | 0.035800 | - | - | - | - | - | - | - | - | 2.169325 | 0.035184 | 7605.531250 | 0.98x | - | - | - | - | yes | 0.396356 | 11.27x | 11.07x |
| bench_dict_ops | yes | 0.036433 | - | - | - | - | - | - | - | - | 1.943491 | 0.034439 | 7605.515625 | 0.95x | - | - | - | - | yes | 0.154613 | 4.49x | 4.24x |
| bench_dict_views | yes | 0.034152 | - | - | - | - | - | - | - | - | 1.980585 | 0.032457 | 7605.523438 | 0.95x | - | - | - | - | yes | 0.155806 | 4.80x | 4.56x |
| bench_etl_orders | yes | 0.062983 | - | - | - | - | - | - | - | - | 4.940541 | 0.098573 | 11187.960938 | 1.57x | - | - | - | - | yes | 1.211991 | 12.30x | 19.24x |
| bench_exception_heavy | yes | 0.125517 | - | - | - | - | - | - | - | - | 2.094477 | 0.226549 | 7605.523438 | 1.80x | - | - | - | - | yes | 3.195713 | 14.11x | 25.46x |
| bench_fib | yes | 0.066991 | - | - | - | - | - | - | - | - | 3.189328 | 0.063327 | 7605.515625 | 0.95x | - | - | - | - | yes | 1.217280 | 19.22x | 18.17x |
| bench_gc_pressure | yes | 0.438192 | - | - | - | - | - | - | - | - | 2.057116 | 0.330864 | 7605.523438 | 0.76x | - | - | - | - | yes | 3.938662 | 11.90x | 8.99x |
| bench_generator_iter | yes | 0.038033 | - | - | - | - | - | - | - | - | 4.364602 | 0.033313 | 10542.601562 | 0.88x | - | - | - | - | yes | 0.404712 | 12.15x | 10.64x |
| bench_import_time | yes | 0.035080 | - | - | - | - | - | - | - | - | 3.474034 | 0.034275 | 8993.476562 | 0.98x | - | - | - | - | yes | 0.312194 | 9.11x | 8.90x |
| bench_json_roundtrip | yes | 0.036458 | - | - | - | - | - | - | - | - | 3.761391 | 0.035467 | 7718.492188 | 0.97x | - | - | - | - | yes | 0.169791 | 4.79x | 4.66x |
| bench_list_ops | yes | 0.033599 | - | - | - | - | - | - | - | - | 2.032337 | 0.032953 | 7605.515625 | 0.98x | - | - | - | - | yes | 0.156600 | 4.75x | 4.66x |
| bench_list_slice | yes | 0.034547 | - | - | - | - | - | - | - | - | 2.012921 | 0.033976 | 7605.523438 | 0.98x | - | - | - | - | yes | 0.160481 | 4.72x | 4.65x |
| bench_matrix_math | yes | 0.034038 | - | - | - | - | - | - | - | - | 2.200941 | 0.034013 | 7605.539062 | 1.00x | - | - | - | - | yes | 0.159165 | 4.68x | 4.68x |
| bench_max_list | yes | 0.036209 | - | - | - | - | - | - | - | - | 1.945954 | 0.033562 | 7605.515625 | 0.93x | - | - | - | - | yes | 0.128660 | 3.83x | 3.55x |
| bench_memoryview_tobytes | yes | 0.040819 | - | - | - | - | - | - | - | - | 2.192332 | 0.034016 | 7605.531250 | 0.83x | - | - | - | - | yes | 0.220900 | 6.49x | 5.41x |
| bench_min_list | yes | 0.034909 | - | - | - | - | - | - | - | - | 1.960818 | 0.033394 | 7605.515625 | 0.96x | - | - | - | - | yes | 0.129181 | 3.87x | 3.70x |
| bench_parse_msgpack | yes | 0.033661 | - | - | - | - | - | - | - | - | 53.418219 | 0.039361 | 15304.929688 | 1.17x | - | - | - | - | yes | 0.213730 | 5.43x | 6.35x |
| bench_procedural_gen | yes | 0.034364 | - | - | - | - | - | - | - | - | 2.513173 | 0.034344 | 7718.507812 | 1.00x | - | - | - | - | yes | 0.384965 | 11.21x | 11.20x |
| bench_prod_list | yes | 0.039262 | - | - | - | - | - | - | - | - | 1.978217 | 0.037652 | 7605.523438 | 0.96x | - | - | - | - | yes | 0.136190 | 3.62x | 3.47x |
| bench_ptr_registry | yes | - | - | - | - | - | - | - | - | - | 7.974736 | 0.034933 | 15173.742188 | - | - | - | - | - | no | - | - | - |
| bench_range_iter | yes | 0.034850 | - | - | - | - | - | - | - | - | 2.087621 | 0.034789 | 7605.523438 | 1.00x | - | - | - | - | yes | 0.125845 | 3.62x | 3.61x |
| bench_set_ops | yes | 0.034393 | - | - | - | - | - | - | - | - | 2.084884 | 0.035918 | 7605.515625 | 1.04x | - | - | - | - | yes | 0.189184 | 5.27x | 5.50x |
| bench_startup | yes | 0.035904 | - | - | - | - | - | - | - | - | 2.331754 | 0.036186 | 7605.515625 | 1.01x | - | - | - | - | yes | 0.133629 | 3.69x | 3.72x |
| bench_str_count | yes | 0.035425 | - | - | - | - | - | - | - | - | 2.183134 | 0.034534 | 7605.539062 | 0.97x | - | - | - | - | yes | 0.368270 | 10.66x | 10.40x |
| bench_str_count_unicode | yes | 0.035748 | - | - | - | - | - | - | - | - | 2.110646 | 0.036421 | 7605.546875 | 1.02x | - | - | - | - | yes | 0.294545 | 8.09x | 8.24x |
| bench_str_count_unicode_warm | yes | 0.036924 | - | - | - | - | - | - | - | - | 2.111100 | 0.032717 | 7605.546875 | 0.89x | - | - | - | - | yes | 0.324914 | 9.93x | 8.80x |
| bench_str_endswith | yes | 0.037079 | - | - | - | - | - | - | - | - | 2.078673 | 0.033200 | 7605.539062 | 0.90x | - | - | - | - | yes | 0.339258 | 10.22x | 9.15x |
| bench_str_find | yes | 0.033763 | - | - | - | - | - | - | - | - | 1.992785 | 0.035382 | 7605.531250 | 1.05x | - | - | - | - | yes | 0.364311 | 10.30x | 10.79x |
| bench_str_find_unicode | yes | 0.035694 | - | - | - | - | - | - | - | - | 2.197485 | 0.034528 | 7605.539062 | 0.97x | - | - | - | - | yes | 0.228159 | 6.61x | 6.39x |
| bench_str_find_unicode_warm | yes | 0.034244 | - | - | - | - | - | - | - | - | 2.079984 | 0.034122 | 7605.546875 | 1.00x | - | - | - | - | yes | 0.242465 | 7.11x | 7.08x |
| bench_str_join | yes | 0.034805 | - | - | - | - | - | - | - | - | 2.099292 | 0.033822 | 7605.515625 | 0.97x | - | - | - | - | yes | 0.163802 | 4.84x | 4.71x |
| bench_str_replace | yes | 0.036973 | - | - | - | - | - | - | - | - | 2.177824 | 0.036231 | 7605.523438 | 0.98x | - | - | - | - | yes | 0.293608 | 8.10x | 7.94x |
| bench_str_split | yes | 0.034559 | - | - | - | - | - | - | - | - | 2.076423 | 0.035239 | 7605.539062 | 1.02x | - | - | - | - | yes | 0.168187 | 4.77x | 4.87x |
| bench_str_startswith | yes | 0.035172 | - | - | - | - | - | - | - | - | 2.095862 | 0.035273 | 7605.539062 | 1.00x | - | - | - | - | yes | 0.368851 | 10.46x | 10.49x |
| bench_struct | yes | 0.099860 | - | - | - | - | - | - | - | - | 1.841191 | 2.793627 | 7605.515625 | 27.98x | - | - | - | - | yes | 37.599707 | 13.46x | 376.52x |
| bench_sum | yes | 0.172992 | - | - | - | - | - | - | - | - | 2.292052 | 0.032669 | 7605.515625 | 0.19x | - | - | - | - | yes | 0.126216 | 3.86x | 0.73x |
| bench_sum_list | yes | 0.034956 | - | - | - | - | - | - | - | - | 1.985888 | 0.033132 | 7605.531250 | 0.95x | - | - | - | - | yes | 0.131766 | 3.98x | 3.77x |
| bench_sum_list_hints | yes | 0.033748 | - | - | - | - | - | - | - | - | 2.242273 | 0.032809 | 7589.398438 | 0.97x | - | - | - | - | yes | 0.155895 | 4.75x | 4.62x |
| bench_try_except | yes | 0.036085 | - | - | - | - | - | - | - | - | 1.996360 | 0.037528 | 7605.523438 | 1.04x | - | - | - | - | yes | 0.241092 | 6.42x | 6.68x |
| bench_tuple_index | yes | 0.039364 | - | - | - | - | - | - | - | - | 2.134287 | 0.033341 | 7605.523438 | 0.85x | - | - | - | - | yes | 0.549995 | 16.50x | 13.97x |
| bench_tuple_pack | yes | 0.036567 | - | - | - | - | - | - | - | - | 2.015834 | 0.035785 | 7605.523438 | 0.98x | - | - | - | - | yes | 0.617845 | 17.27x | 16.90x |
| bench_tuple_slice | yes | 0.034359 | - | - | - | - | - | - | - | - | 2.033719 | 0.036794 | 7605.523438 | 1.07x | - | - | - | - | yes | 0.163087 | 4.43x | 4.75x |

Generated by `tools/bench_report.py`.
