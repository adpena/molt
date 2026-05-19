from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import tools.bench_suites as bench_suites


ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = ROOT / "tools"


def _load_tool(name: str) -> ModuleType:
    if str(TOOLS_ROOT) not in sys.path:
        sys.path.insert(0, str(TOOLS_ROOT))
    spec = importlib.util.spec_from_file_location(
        f"molt_test_{name.removesuffix('.py')}",
        TOOLS_ROOT / name,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_benchmark_runner_tables_use_canonical_suite() -> None:
    bench = _load_tool("bench.py")
    bench_individual = _load_tool("bench_individual.py")
    bench_wasm = _load_tool("bench_wasm.py")

    assert tuple(bench.BENCHMARKS) == bench_suites.BENCHMARKS
    assert tuple(bench_individual.BENCHMARKS) == bench_suites.BENCHMARKS
    assert tuple(bench_wasm.BENCHMARKS) == bench_suites.BENCHMARKS
    assert "tests/benchmarks/bench_import_time.py" in bench_individual.BENCHMARKS
    assert "tests/benchmarks/bench_procedural_gen.py" in bench_individual.BENCHMARKS

    assert tuple(bench.SMOKE_BENCHMARKS) == bench_suites.SMOKE_BENCHMARKS
    assert tuple(bench_wasm.SMOKE_BENCHMARKS) == bench_suites.SMOKE_BENCHMARKS
    assert tuple(bench.WS_BENCHMARKS) == bench_suites.WS_BENCHMARKS
    assert tuple(bench_wasm.WS_BENCHMARKS) == bench_suites.WS_BENCHMARKS
    assert tuple(bench.DYNAMIC_BUILTIN_SLICES) == bench_suites.DYNAMIC_BUILTIN_SLICES


def test_molt_benchmark_args_are_canonicalized_for_all_call_shapes() -> None:
    expected_msgpack = ["--stdlib-profile", "full"]
    assert (
        bench_suites.molt_args_for_benchmark("tests/benchmarks/bench_parse_msgpack.py")
        == expected_msgpack
    )
    assert (
        bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
        == expected_msgpack
    )
    assert (
        bench_suites.molt_args_for_benchmark(
            ROOT / "tests" / "benchmarks" / "bench_parse_msgpack.py"
        )
        == expected_msgpack
    )
    assert bench_suites.molt_args_for_benchmark("tests/benchmarks/bench_sum.py") == []

    first = bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
    first.append("--mutated")
    assert (
        bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
        == expected_msgpack
    )


def test_msgpack_full_stdlib_args_reach_wasm_and_profile_tools() -> None:
    bench = _load_tool("bench.py")
    bench_individual = _load_tool("bench_individual.py")
    bench_wasm = _load_tool("bench_wasm.py")
    profile = _load_tool("profile.py")

    expected = ["--stdlib-profile", "full"]
    script = "tests/benchmarks/bench_parse_msgpack.py"

    assert bench.molt_args_for_benchmark(script) == expected
    assert bench_individual.molt_args_for_benchmark(script) == expected
    assert bench_wasm.molt_args_for_benchmark(script) == expected
    assert profile.molt_args_for_benchmark("bench_parse_msgpack.py") == expected
    assert bench_wasm.MOLT_ARGS_BY_BENCH[script] == expected
