# Molt Bench Summary

Generated: 2026-01-17T20:18:28Z

## Inputs
- Native: `bench/results/bench.json`; git_rev=ac8ca9b528119e74fbe00e013a080c07912215c2; created_at=2026-01-17T20:18:19.877428+00:00; system=cpu_count=8, load_avg=[3.029296875, 2.38525390625, 1.86962890625], machine=x86_64, platform=macOS-26.2-x86_64-i386-64bit-Mach-O, python=3.14.0
- WASM: `bench/results/bench_wasm.json`; git_rev=ac8ca9b528119e74fbe00e013a080c07912215c2; created_at=2026-01-17T20:15:12.861950+00:00; system=cpu_count=8, load_avg=[2.4970703125, 2.02001953125, 1.65185546875], machine=x86_64, platform=macOS-26.2-x86_64-i386-64bit-Mach-O, python=3.14.0

## Summary
- Benchmarks: 45 total; native ok 45/45; wasm ok 45/45.
- Median native speedup vs CPython: 3.71x.
- Median wasm speedup vs CPython: 0.48x.
- Median wasm/native ratio: 4.75x.
- Native regressions (< 1.0x): 11.

## Regressions (Native < 1.0x)
| Benchmark | Speedup | Molt s | CPython s |
| --- | --- | --- | --- |
| bench_struct | 0.20x | 1.806032 | 0.352661 |
| bench_csv_parse_wide | 0.26x | 0.475398 | 0.124748 |
| bench_deeply_nested_loop | 0.28x | 2.845578 | 0.787288 |
| bench_attr_access | 0.39x | 0.323849 | 0.126803 |
| bench_tuple_pack | 0.41x | 0.324701 | 0.132465 |
| bench_tuple_index | 0.43x | 0.275180 | 0.119614 |
| bench_descriptor_property | 0.44x | 0.291214 | 0.127162 |
| bench_fib | 0.46x | 0.308011 | 0.140917 |
| bench_csv_parse | 0.50x | 0.143173 | 0.070898 |
| bench_try_except | 0.88x | 0.083110 | 0.073344 |

## WASM vs Native (Slowest)
| Benchmark | WASM s | Native s | WASM/Native |
| --- | --- | --- | --- |
| bench_str_find_unicode_warm | 0.097667 | 0.008460 | 11.54x |
| bench_channel_throughput | 0.215259 | 0.018654 | 11.54x |
| bench_sum | 0.075801 | 0.007366 | 10.29x |
| bench_str_find_unicode | 0.104879 | 0.010286 | 10.20x |
| bench_str_startswith | 0.084843 | 0.008427 | 10.07x |
| bench_str_count | 0.083032 | 0.008368 | 9.92x |
| bench_str_endswith | 0.082414 | 0.008551 | 9.64x |
| bench_bytearray_find | 0.089532 | 0.009526 | 9.40x |
| bench_memoryview_tobytes | 0.089829 | 0.009772 | 9.19x |
| bench_bytes_replace | 0.086040 | 0.009736 | 8.84x |

## Combined Table
| Benchmark | Native OK | CPython s | Molt s | Speedup | WASM OK | WASM s | WASM/Native | WASM/CPython |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| bench_async_await | yes | 0.108769 | 0.087063 | 1.25x | yes | 0.162742 | 1.87x | 0.67x |
| bench_attr_access | yes | 0.126803 | 0.323849 | 0.39x | yes | 0.635910 | 1.96x | 0.20x |
| bench_bytearray_find | yes | 0.042873 | 0.009526 | 4.50x | yes | 0.089532 | 9.40x | 0.48x |
| bench_bytearray_replace | yes | 0.080703 | 0.012931 | 6.24x | yes | 0.089816 | 6.95x | 0.90x |
| bench_bytes_find | yes | 0.046383 | 0.009256 | 5.01x | yes | 0.080547 | 8.70x | 0.58x |
| bench_bytes_find_only | yes | 0.115751 | 0.039595 | 2.92x | yes | 0.316062 | 7.98x | 0.37x |
| bench_bytes_replace | yes | 0.049325 | 0.009736 | 5.07x | yes | 0.086040 | 8.84x | 0.57x |
| bench_channel_throughput | yes | 0.528359 | 0.018654 | 28.32x | yes | 0.215259 | 11.54x | 2.45x |
| bench_csv_parse | yes | 0.070898 | 0.143173 | 0.50x | yes | 0.287323 | 2.01x | 0.25x |
| bench_csv_parse_wide | yes | 0.124748 | 0.475398 | 0.26x | yes | 0.961990 | 2.02x | 0.13x |
| bench_deeply_nested_loop | yes | 0.787288 | 2.845578 | 0.28x | yes | 4.802099 | 1.69x | 0.16x |
| bench_descriptor_property | yes | 0.127162 | 0.291214 | 0.44x | yes | 1.134415 | 3.90x | 0.11x |
| bench_dict_ops | yes | 0.047493 | 0.017811 | 2.67x | yes | 0.103835 | 5.83x | 0.46x |
| bench_dict_views | yes | 0.045986 | 0.023311 | 1.97x | yes | 0.113895 | 4.89x | 0.40x |
| bench_fib | yes | 0.140917 | 0.308011 | 0.46x | yes | 0.433885 | 1.41x | 0.32x |
| bench_generator_iter | yes | 0.043235 | 0.038438 | 1.12x | yes | 0.146979 | 3.82x | 0.29x |
| bench_list_ops | yes | 0.045166 | 0.017068 | 2.65x | yes | 0.100872 | 5.91x | 0.45x |
| bench_list_slice | yes | 0.049966 | 0.022788 | 2.19x | yes | 0.108276 | 4.75x | 0.46x |
| bench_matrix_math | yes | 0.096508 | 0.020898 | 4.62x | yes | 0.108334 | 5.18x | 0.89x |
| bench_max_list | yes | 0.139021 | 0.028983 | 4.80x | yes | 0.134046 | 4.62x | 1.04x |
| bench_memoryview_tobytes | yes | 0.041694 | 0.009772 | 4.27x | yes | 0.089829 | 9.19x | 0.46x |
| bench_min_list | yes | 0.139752 | 0.028789 | 4.85x | yes | 0.134006 | 4.65x | 1.04x |
| bench_parse_msgpack | yes | 0.115905 | 0.031238 | 3.71x | yes | 0.116127 | 3.72x | 1.00x |
| bench_prod_list | yes | 0.094773 | 0.015162 | 6.25x | yes | 0.092914 | 6.13x | 1.02x |
| bench_ptr_registry | yes | 1.257353 | 0.103391 | 12.16x | yes | 0.225374 | 2.18x | 5.58x |
| bench_range_iter | yes | 0.066701 | 0.057100 | 1.17x | yes | 0.174967 | 3.06x | 0.38x |
| bench_str_count | yes | 0.045518 | 0.008368 | 5.44x | yes | 0.083032 | 9.92x | 0.55x |
| bench_str_count_unicode | yes | 0.045831 | 0.022239 | 2.06x | yes | 0.090160 | 4.05x | 0.51x |
| bench_str_count_unicode_warm | yes | 0.095304 | 0.022493 | 4.24x | yes | 0.091392 | 4.06x | 1.04x |
| bench_str_endswith | yes | 0.043677 | 0.008551 | 5.11x | yes | 0.082414 | 9.64x | 0.53x |
| bench_str_find | yes | 0.046631 | 0.011077 | 4.21x | yes | 0.096645 | 8.72x | 0.48x |
| bench_str_find_unicode | yes | 0.052689 | 0.010286 | 5.12x | yes | 0.104879 | 10.20x | 0.50x |
| bench_str_find_unicode_warm | yes | 0.043601 | 0.008460 | 5.15x | yes | 0.097667 | 11.54x | 0.45x |
| bench_str_join | yes | 0.074313 | 0.078198 | 0.95x | yes | 0.178673 | 2.28x | 0.42x |
| bench_str_replace | yes | 0.044119 | 0.009595 | 4.60x | yes | 0.082130 | 8.56x | 0.54x |
| bench_str_split | yes | 0.070736 | 0.040535 | 1.75x | yes | 0.110282 | 2.72x | 0.64x |
| bench_str_startswith | yes | 0.043674 | 0.008427 | 5.18x | yes | 0.084843 | 10.07x | 0.51x |
| bench_struct | yes | 0.352661 | 1.806032 | 0.20x | yes | 4.597943 | 2.55x | 0.08x |
| bench_sum | yes | 1.599771 | 0.007366 | 217.18x | yes | 0.075801 | 10.29x | 21.10x |
| bench_sum_list | yes | 0.183830 | 0.027800 | 6.61x | yes | 0.136062 | 4.89x | 1.35x |
| bench_sum_list_hints | yes | 0.187804 | 0.027696 | 6.78x | yes | 0.133627 | 4.82x | 1.41x |
| bench_try_except | yes | 0.073344 | 0.083110 | 0.88x | yes | 0.213383 | 2.57x | 0.34x |
| bench_tuple_index | yes | 0.119614 | 0.275180 | 0.43x | yes | 0.534308 | 1.94x | 0.22x |
| bench_tuple_pack | yes | 0.132465 | 0.324701 | 0.41x | yes | 0.560217 | 1.73x | 0.24x |
| bench_tuple_slice | yes | 0.050222 | 0.028417 | 1.77x | yes | 0.120245 | 4.23x | 0.42x |

Generated by `tools/bench_report.py`.
