"""Canonical benchmark suite and Molt argument metadata.

Benchmark runners should import these constants instead of maintaining local
copies.  Per-benchmark Molt args are keyed by the repo-relative benchmark path;
helpers normalize direct script paths and basenames at call sites.
"""

from __future__ import annotations

from pathlib import Path

BENCHMARKS: tuple[str, ...] = (
    "tests/benchmarks/bench_fib.py",
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_sum_list.py",
    "tests/benchmarks/bench_sum_list_hints.py",
    "tests/benchmarks/bench_min_list.py",
    "tests/benchmarks/bench_max_list.py",
    "tests/benchmarks/bench_prod_list.py",
    "tests/benchmarks/bench_struct.py",
    "tests/benchmarks/bench_attr_access.py",
    "tests/benchmarks/bench_descriptor_property.py",
    "tests/benchmarks/bench_dict_ops.py",
    "tests/benchmarks/bench_dict_views.py",
    "tests/benchmarks/bench_counter_words.py",
    "tests/benchmarks/bench_etl_orders.py",
    "tests/benchmarks/bench_list_ops.py",
    "tests/benchmarks/bench_list_slice.py",
    "tests/benchmarks/bench_tuple_index.py",
    "tests/benchmarks/bench_tuple_slice.py",
    "tests/benchmarks/bench_tuple_pack.py",
    "tests/benchmarks/bench_range_iter.py",
    "tests/benchmarks/bench_try_except.py",
    "tests/benchmarks/bench_generator_iter.py",
    "tests/benchmarks/bench_async_await.py",
    "tests/benchmarks/bench_channel_throughput.py",
    "tests/benchmarks/bench_ptr_registry.py",
    "tests/benchmarks/bench_deeply_nested_loop.py",
    "tests/benchmarks/bench_csv_parse.py",
    "tests/benchmarks/bench_csv_parse_wide.py",
    "tests/benchmarks/bench_matrix_math.py",
    "tests/benchmarks/bench_bytes_find.py",
    "tests/benchmarks/bench_bytes_find_only.py",
    "tests/benchmarks/bench_bytes_replace.py",
    "tests/benchmarks/bench_bytearray_find.py",
    "tests/benchmarks/bench_bytearray_replace.py",
    "tests/benchmarks/bench_str_find.py",
    "tests/benchmarks/bench_str_find_unicode.py",
    "tests/benchmarks/bench_str_find_unicode_warm.py",
    "tests/benchmarks/bench_str_split.py",
    "tests/benchmarks/bench_str_replace.py",
    "tests/benchmarks/bench_str_count.py",
    "tests/benchmarks/bench_str_count_unicode.py",
    "tests/benchmarks/bench_str_count_unicode_warm.py",
    "tests/benchmarks/bench_str_join.py",
    "tests/benchmarks/bench_str_startswith.py",
    "tests/benchmarks/bench_str_endswith.py",
    "tests/benchmarks/bench_memoryview_tobytes.py",
    "tests/benchmarks/bench_parse_msgpack.py",
    "tests/benchmarks/bench_json_roundtrip.py",
    "tests/benchmarks/bench_startup.py",
    "tests/benchmarks/bench_gc_pressure.py",
    "tests/benchmarks/bench_class_hierarchy.py",
    "tests/benchmarks/bench_set_ops.py",
    "tests/benchmarks/bench_exception_heavy.py",
    "tests/benchmarks/bench_dict_comprehension.py",
    "tests/benchmarks/bench_procedural_gen.py",
    "tests/benchmarks/bench_import_time.py",
)

SMOKE_BENCHMARKS: tuple[str, ...] = (
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_bytes_find.py",
)

WS_BENCHMARKS: tuple[str, ...] = ("tests/benchmarks/bench_ws_wait.py",)

DYNAMIC_BUILTIN_SLICES: tuple[str, ...] = (
    "tests/benchmarks/bench_builtin_locals_slice.py",
    "tests/benchmarks/bench_builtin_dir_slice.py",
    "tests/benchmarks/bench_builtin_import_slice.py",
    "tests/benchmarks/bench_builtin_delattr_slice.py",
)

MOLT_ARGS_BY_BENCH: dict[str, list[str]] = {
    "tests/benchmarks/bench_sum_list_hints.py": ["--type-hints", "trust"],
    "tests/benchmarks/bench_parse_msgpack.py": ["--stdlib-profile", "full"],
}

_BENCHMARK_ROOT = "tests/benchmarks/"
_MOLT_ARGS_BY_BASENAME = {
    Path(path).name: tuple(args) for path, args in MOLT_ARGS_BY_BENCH.items()
}


def canonical_benchmark_key(script: str | Path) -> str:
    """Return a repo-relative benchmark key when a benchmark path is recognizable."""
    raw = str(script).replace("\\", "/")
    while raw.startswith("./"):
        raw = raw[2:]
    if _BENCHMARK_ROOT in raw:
        return _BENCHMARK_ROOT + raw.rsplit(_BENCHMARK_ROOT, 1)[1]
    return Path(raw).name


def molt_args_for_benchmark(script: str | Path) -> list[str]:
    """Return a fresh Molt CLI arg list for a benchmark script."""
    key = canonical_benchmark_key(script)
    args = MOLT_ARGS_BY_BENCH.get(key)
    if args is not None:
        return list(args)
    return list(_MOLT_ARGS_BY_BASENAME.get(Path(key).name, ()))
