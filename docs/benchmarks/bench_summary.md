# Molt Bench Summary

Generated: 2026-01-16T07:43:54Z

## Inputs
- Native: `bench/results/bench.json`; git_rev=7d31c358d57c5e95197f04e793c200d06e47adb5; created_at=2026-01-16T07:21:07.407543+00:00; system=machine=x86_64, platform=macOS-26.2-x86_64-i386-64bit-Mach-O, python=3.14.0
- WASM: `bench/results/bench_wasm.json`; git_rev=7d31c358d57c5e95197f04e793c200d06e47adb5; created_at=2026-01-16T07:26:24.766714+00:00; system=machine=x86_64, platform=macOS-26.2-x86_64-i386-64bit-Mach-O, python=3.14.0

## Summary
- Benchmarks: 44 total; native ok 44/44; wasm ok 44/44.
- Median native speedup vs CPython: 4.15x.
- Median wasm speedup vs CPython: 0.53x.
- Median wasm/native ratio: 5.93x.
- Native regressions (< 1.0x): 7.

## Regressions (Native < 1.0x)
| Benchmark | Speedup | Molt s | CPython s |
| --- | --- | --- | --- |
| bench_deeply_nested_loop | 0.37x | 2.163907 | 0.798798 |
| bench_tuple_index | 0.58x | 0.216054 | 0.125339 |
| bench_tuple_pack | 0.60x | 0.211092 | 0.126185 |
| bench_csv_parse_wide | 0.61x | 0.216390 | 0.132361 |
| bench_struct | 0.64x | 0.553865 | 0.354862 |
| bench_csv_parse | 0.88x | 0.083577 | 0.073286 |
| bench_attr_access | 0.90x | 0.139881 | 0.126458 |

## WASM vs Native (Slowest)
| Benchmark | WASM s | Native s | WASM/Native |
| --- | --- | --- | --- |
| bench_str_find_unicode_warm | 0.102006 | 0.009439 | 10.81x |
| bench_sum | 0.075524 | 0.007547 | 10.01x |
| bench_channel_throughput | 0.115834 | 0.011993 | 9.66x |
| bench_str_find_unicode | 0.107121 | 0.011098 | 9.65x |
| bench_bytearray_find | 0.091441 | 0.009623 | 9.50x |
| bench_str_count | 0.083086 | 0.008770 | 9.47x |
| bench_str_endswith | 0.080800 | 0.009119 | 8.86x |
| bench_str_find | 0.099658 | 0.011251 | 8.86x |
| bench_str_startswith | 0.082362 | 0.009315 | 8.84x |
| bench_memoryview_tobytes | 0.084346 | 0.010015 | 8.42x |

## Combined Table
| Benchmark | Native OK | CPython s | Molt s | Speedup | WASM OK | WASM s | WASM/Native | WASM/CPython |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| bench_async_await | yes | 0.118989 | 0.079308 | 1.50x | yes | 0.132355 | 1.67x | 0.90x |
| bench_attr_access | yes | 0.126458 | 0.139881 | 0.90x | yes | 0.298498 | 2.13x | 0.42x |
| bench_bytearray_find | yes | 0.043893 | 0.009623 | 4.56x | yes | 0.091441 | 9.50x | 0.48x |
| bench_bytearray_replace | yes | 0.082306 | 0.012990 | 6.34x | yes | 0.093768 | 7.22x | 0.88x |
| bench_bytes_find | yes | 0.048446 | 0.010070 | 4.81x | yes | 0.078253 | 7.77x | 0.62x |
| bench_bytes_find_only | yes | 0.122952 | 0.043705 | 2.81x | yes | 0.317507 | 7.26x | 0.39x |
| bench_bytes_replace | yes | 0.052537 | 0.010420 | 5.04x | yes | 0.083526 | 8.02x | 0.63x |
| bench_channel_throughput | yes | 0.521540 | 0.011993 | 43.49x | yes | 0.115834 | 9.66x | 4.50x |
| bench_csv_parse | yes | 0.073286 | 0.083577 | 0.88x | yes | 0.183552 | 2.20x | 0.40x |
| bench_csv_parse_wide | yes | 0.132361 | 0.216390 | 0.61x | yes | 0.556353 | 2.57x | 0.24x |
| bench_deeply_nested_loop | yes | 0.798798 | 2.163907 | 0.37x | yes | 2.523830 | 1.17x | 0.32x |
| bench_descriptor_property | yes | 0.132997 | 0.111016 | 1.20x | yes | 0.771796 | 6.95x | 0.17x |
| bench_dict_ops | yes | 0.048099 | 0.015836 | 3.04x | yes | 0.095505 | 6.03x | 0.50x |
| bench_dict_views | yes | 0.047707 | 0.018292 | 2.61x | yes | 0.102685 | 5.61x | 0.46x |
| bench_fib | yes | 0.148370 | 0.137430 | 1.08x | yes | 0.266188 | 1.94x | 0.56x |
| bench_generator_iter | yes | 0.044451 | 0.022096 | 2.01x | yes | 0.109326 | 4.95x | 0.41x |
| bench_list_ops | yes | 0.047291 | 0.015338 | 3.08x | yes | 0.096719 | 6.31x | 0.49x |
| bench_list_slice | yes | 0.050595 | 0.020438 | 2.48x | yes | 0.100076 | 4.90x | 0.51x |
| bench_matrix_math | yes | 0.102670 | 0.016939 | 6.06x | yes | 0.095749 | 5.65x | 1.07x |
| bench_max_list | yes | 0.144656 | 0.016570 | 8.73x | yes | 0.113107 | 6.83x | 1.28x |
| bench_memoryview_tobytes | yes | 0.044645 | 0.010015 | 4.46x | yes | 0.084346 | 8.42x | 0.53x |
| bench_min_list | yes | 0.145249 | 0.016425 | 8.84x | yes | 0.116415 | 7.09x | 1.25x |
| bench_parse_msgpack | yes | 0.179954 | 0.018815 | 9.56x | yes | 0.098067 | 5.21x | 1.84x |
| bench_prod_list | yes | 0.096722 | 0.016028 | 6.03x | yes | 0.089179 | 5.56x | 1.08x |
| bench_range_iter | yes | 0.069601 | 0.046078 | 1.51x | yes | 0.133188 | 2.89x | 0.52x |
| bench_str_count | yes | 0.047811 | 0.008770 | 5.45x | yes | 0.083086 | 9.47x | 0.58x |
| bench_str_count_unicode | yes | 0.045004 | 0.023095 | 1.95x | yes | 0.089681 | 3.88x | 0.50x |
| bench_str_count_unicode_warm | yes | 0.097515 | 0.023770 | 4.10x | yes | 0.090224 | 3.80x | 1.08x |
| bench_str_endswith | yes | 0.045877 | 0.009119 | 5.03x | yes | 0.080800 | 8.86x | 0.57x |
| bench_str_find | yes | 0.047223 | 0.011251 | 4.20x | yes | 0.099658 | 8.86x | 0.47x |
| bench_str_find_unicode | yes | 0.054506 | 0.011098 | 4.91x | yes | 0.107121 | 9.65x | 0.51x |
| bench_str_find_unicode_warm | yes | 0.044643 | 0.009439 | 4.73x | yes | 0.102006 | 10.81x | 0.44x |
| bench_str_join | yes | 0.076286 | 0.030122 | 2.53x | yes | 0.103945 | 3.45x | 0.73x |
| bench_str_replace | yes | 0.046242 | 0.010974 | 4.21x | yes | 0.082212 | 7.49x | 0.56x |
| bench_str_split | yes | 0.073099 | 0.015755 | 4.64x | yes | 0.091789 | 5.83x | 0.80x |
| bench_str_startswith | yes | 0.043790 | 0.009315 | 4.70x | yes | 0.082362 | 8.84x | 0.53x |
| bench_struct | yes | 0.354862 | 0.553865 | 0.64x | yes | 2.938570 | 5.31x | 0.12x |
| bench_sum | yes | 1.709724 | 0.007547 | 226.55x | yes | 0.075524 | 10.01x | 22.64x |
| bench_sum_list | yes | 0.185769 | 0.016571 | 11.21x | yes | 0.116283 | 7.02x | 1.60x |
| bench_sum_list_hints | yes | 0.188335 | 0.014962 | 12.59x | yes | 0.116136 | 7.76x | 1.62x |
| bench_try_except | yes | 0.075433 | 0.068291 | 1.10x | yes | 0.162839 | 2.38x | 0.46x |
| bench_tuple_index | yes | 0.125339 | 0.216054 | 0.58x | yes | 0.328026 | 1.52x | 0.38x |
| bench_tuple_pack | yes | 0.126185 | 0.211092 | 0.60x | yes | 0.328368 | 1.56x | 0.38x |
| bench_tuple_slice | yes | 0.050927 | 0.022117 | 2.30x | yes | 0.102723 | 4.64x | 0.50x |

Generated by `tools/bench_report.py`.
