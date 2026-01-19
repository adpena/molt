# Molt Bench Summary

Generated: 2026-01-19T03:30:11Z

## Inputs
- Native: `bench/results/bench.json`; git_rev=ddb9f5feaa72a0263742f336c24f28e1764ac788; created_at=2026-01-19T03:26:53.101884+00:00; system=cpu_count=8, load_avg=[4.96240234375, 3.81103515625, 2.716796875], machine=x86_64, platform=macOS-26.2-x86_64-i386-64bit-Mach-O, python=3.14.0
- WASM: `bench/results/bench_wasm.json`; git_rev=ddb9f5feaa72a0263742f336c24f28e1764ac788; created_at=2026-01-19T03:30:06.677637+00:00; system=cpu_count=8, load_avg=[3.95263671875, 3.77294921875, 2.90576171875], machine=x86_64, platform=macOS-26.2-x86_64-i386-64bit-Mach-O, python=3.14.0

## Summary
- Benchmarks: 45 total; native ok 45/45; wasm ok 45/45.
- Median native speedup vs CPython: 3.75x.
- Median wasm speedup vs CPython: 0.47x.
- Median wasm/native ratio: 4.81x.
- Native regressions (< 1.0x): 11.

## Regressions (Native < 1.0x)
| Benchmark | Speedup | Molt s | CPython s |
| --- | --- | --- | --- |
| bench_struct | 0.20x | 1.768211 | 0.355121 |
| bench_csv_parse_wide | 0.26x | 0.465612 | 0.122600 |
| bench_deeply_nested_loop | 0.31x | 2.831630 | 0.880638 |
| bench_attr_access | 0.40x | 0.316276 | 0.126484 |
| bench_tuple_pack | 0.42x | 0.315874 | 0.133168 |
| bench_tuple_index | 0.42x | 0.275005 | 0.116831 |
| bench_descriptor_property | 0.44x | 0.284706 | 0.124652 |
| bench_fib | 0.49x | 0.304091 | 0.147676 |
| bench_csv_parse | 0.50x | 0.142315 | 0.070785 |
| bench_try_except | 0.88x | 0.082298 | 0.072029 |

## WASM vs Native (Slowest)
| Benchmark | WASM s | Native s | WASM/Native |
| --- | --- | --- | --- |
| bench_channel_throughput | 0.864098 | 0.019661 | 43.95x |
| bench_str_find_unicode_warm | 0.097624 | 0.008429 | 11.58x |
| bench_sum | 0.076071 | 0.007107 | 10.70x |
| bench_str_find_unicode | 0.104713 | 0.010190 | 10.28x |
| bench_str_count | 0.084374 | 0.008305 | 10.16x |
| bench_async_await | 0.853161 | 0.085831 | 9.94x |
| bench_str_startswith | 0.083598 | 0.008457 | 9.89x |
| bench_str_endswith | 0.083920 | 0.008629 | 9.73x |
| bench_bytearray_find | 0.089695 | 0.009472 | 9.47x |
| bench_str_find | 0.098384 | 0.010568 | 9.31x |

## Combined Table
| Benchmark | Native OK | CPython s | Molt s | Speedup | WASM OK | WASM s | WASM/Native | WASM/CPython |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| bench_async_await | yes | 0.107982 | 0.085831 | 1.26x | yes | 0.853161 | 9.94x | 0.13x |
| bench_attr_access | yes | 0.126484 | 0.316276 | 0.40x | yes | 0.619463 | 1.96x | 0.20x |
| bench_bytearray_find | yes | 0.042231 | 0.009472 | 4.46x | yes | 0.089695 | 9.47x | 0.47x |
| bench_bytearray_replace | yes | 0.080459 | 0.012787 | 6.29x | yes | 0.090928 | 7.11x | 0.88x |
| bench_bytes_find | yes | 0.045948 | 0.009174 | 5.01x | yes | 0.082374 | 8.98x | 0.56x |
| bench_bytes_find_only | yes | 0.117319 | 0.039688 | 2.96x | yes | 0.313968 | 7.91x | 0.37x |
| bench_bytes_replace | yes | 0.048902 | 0.010233 | 4.78x | yes | 0.087862 | 8.59x | 0.56x |
| bench_channel_throughput | yes | 0.519328 | 0.019661 | 26.41x | yes | 0.864098 | 43.95x | 0.60x |
| bench_csv_parse | yes | 0.070785 | 0.142315 | 0.50x | yes | 0.276049 | 1.94x | 0.26x |
| bench_csv_parse_wide | yes | 0.122600 | 0.465612 | 0.26x | yes | 0.902511 | 1.94x | 0.14x |
| bench_deeply_nested_loop | yes | 0.880638 | 2.831630 | 0.31x | yes | 4.254881 | 1.50x | 0.21x |
| bench_descriptor_property | yes | 0.124652 | 0.284706 | 0.44x | yes | 1.089573 | 3.83x | 0.11x |
| bench_dict_ops | yes | 0.048143 | 0.017562 | 2.74x | yes | 0.103402 | 5.89x | 0.47x |
| bench_dict_views | yes | 0.046201 | 0.022852 | 2.02x | yes | 0.112216 | 4.91x | 0.41x |
| bench_fib | yes | 0.147676 | 0.304091 | 0.49x | yes | 0.394515 | 1.30x | 0.37x |
| bench_generator_iter | yes | 0.042677 | 0.038060 | 1.12x | yes | 0.149528 | 3.93x | 0.29x |
| bench_list_ops | yes | 0.045684 | 0.016912 | 2.70x | yes | 0.102159 | 6.04x | 0.45x |
| bench_list_slice | yes | 0.049119 | 0.022706 | 2.16x | yes | 0.105124 | 4.63x | 0.47x |
| bench_matrix_math | yes | 0.096549 | 0.020691 | 4.67x | yes | 0.106051 | 5.13x | 0.91x |
| bench_max_list | yes | 0.139206 | 0.027883 | 4.99x | yes | 0.134084 | 4.81x | 1.04x |
| bench_memoryview_tobytes | yes | 0.041417 | 0.009668 | 4.28x | yes | 0.087591 | 9.06x | 0.47x |
| bench_min_list | yes | 0.140171 | 0.028244 | 4.96x | yes | 0.134515 | 4.76x | 1.04x |
| bench_parse_msgpack | yes | 0.116035 | 0.030953 | 3.75x | yes | 0.117500 | 3.80x | 0.99x |
| bench_prod_list | yes | 0.094156 | 0.015243 | 6.18x | yes | 0.094200 | 6.18x | 1.00x |
| bench_ptr_registry | yes | 1.232048 | 0.105268 | 11.70x | yes | 0.215980 | 2.05x | 5.70x |
| bench_range_iter | yes | 0.066879 | 0.056297 | 1.19x | yes | 0.164848 | 2.93x | 0.41x |
| bench_str_count | yes | 0.045224 | 0.008305 | 5.45x | yes | 0.084374 | 10.16x | 0.54x |
| bench_str_count_unicode | yes | 0.044208 | 0.021466 | 2.06x | yes | 0.091121 | 4.24x | 0.49x |
| bench_str_count_unicode_warm | yes | 0.094585 | 0.023316 | 4.06x | yes | 0.092834 | 3.98x | 1.02x |
| bench_str_endswith | yes | 0.042838 | 0.008629 | 4.96x | yes | 0.083920 | 9.73x | 0.51x |
| bench_str_find | yes | 0.046905 | 0.010568 | 4.44x | yes | 0.098384 | 9.31x | 0.48x |
| bench_str_find_unicode | yes | 0.050809 | 0.010190 | 4.99x | yes | 0.104713 | 10.28x | 0.49x |
| bench_str_find_unicode_warm | yes | 0.042708 | 0.008429 | 5.07x | yes | 0.097624 | 11.58x | 0.44x |
| bench_str_join | yes | 0.071755 | 0.077290 | 0.93x | yes | 0.181531 | 2.35x | 0.40x |
| bench_str_replace | yes | 0.043863 | 0.009549 | 4.59x | yes | 0.082749 | 8.67x | 0.53x |
| bench_str_split | yes | 0.069162 | 0.039164 | 1.77x | yes | 0.115722 | 2.95x | 0.60x |
| bench_str_startswith | yes | 0.042880 | 0.008457 | 5.07x | yes | 0.083598 | 9.89x | 0.51x |
| bench_struct | yes | 0.355121 | 1.768211 | 0.20x | yes | 4.138281 | 2.34x | 0.09x |
| bench_sum | yes | 1.580829 | 0.007107 | 222.42x | yes | 0.076071 | 10.70x | 20.78x |
| bench_sum_list | yes | 0.180828 | 0.028467 | 6.35x | yes | 0.132674 | 4.66x | 1.36x |
| bench_sum_list_hints | yes | 0.188600 | 0.027551 | 6.85x | yes | 0.132779 | 4.82x | 1.42x |
| bench_try_except | yes | 0.072029 | 0.082298 | 0.88x | yes | 0.202421 | 2.46x | 0.36x |
| bench_tuple_index | yes | 0.116831 | 0.275005 | 0.42x | yes | 0.496786 | 1.81x | 0.24x |
| bench_tuple_pack | yes | 0.133168 | 0.315874 | 0.42x | yes | 0.526670 | 1.67x | 0.25x |
| bench_tuple_slice | yes | 0.050853 | 0.026415 | 1.93x | yes | 0.112107 | 4.24x | 0.45x |

Generated by `tools/bench_report.py`.
