#!/usr/bin/env python3
"""Benchmark: Molt-Luau (via Lune) vs CPython on arbitrary benchmark files.

Usage:
    # Run specific benchmark(s):
    uv run python tools/benchmark_luau_vs_cpython.py tests/benchmarks/bench_sum.py

    # Run all benchmarks from tests/benchmarks/bench_*.py:
    uv run python tools/benchmark_luau_vs_cpython.py --all

    # Default (built-in zone generator) when no args given:
    uv run python tools/benchmark_luau_vs_cpython.py

    # CPython-only (no Molt/Lune needed):
    uv run python tools/benchmark_luau_vs_cpython.py --cpython-only

    # Generate markdown report:
    uv run python tools/benchmark_luau_vs_cpython.py --all --report

Environment:
    MOLT_EXT_ROOT=<artifact-root>   # optional; defaults repo-local
    CARGO_TARGET_DIR=<artifact-root>/target
    RUSTC_WRAPPER=""
    PYTHONPATH=src
"""

import argparse
import glob as globmod
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def _artifact_root() -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        return Path(configured).expanduser()
    return REPO_ROOT


GENERATOR_SOURCE = """\
def hash_float(x, y, z, seed):
    h = seed + x * 374761 + y * 668265 + z * 224682
    h = (h * 3266489) % 2147483647
    return h / 2147483647.0

def lerp(a, b, t):
    return a + (b - a) * t

def generate_platforms(seed, base_y, depth, step):
    platforms = []
    crystals = []
    anchors = []
    y = base_y
    while y < base_y + depth:
        x = -48
        while x <= 48:
            z = -48
            while z <= 48:
                density = hash_float(x, y, z, seed)
                if density > 0.55:
                    platforms.append([x, y, z, step])
                    crystal_roll = hash_float(x, y, z, seed + 1)
                    if crystal_roll > 0.94:
                        glow = 0.6 + hash_float(x, y, z, seed + 2) * 0.4
                        crystals.append([x, y + step, z, glow])
                    if crystal_roll < 0.03:
                        anchors.append([x, y + step + 2, z])
                z = z + step
            x = x + step
        y = y + step
    return [platforms, crystals, anchors]

result = generate_platforms(1337, -120, 64, 8)
print(len(result[0]))
print(len(result[1]))
print(len(result[2]))
"""

DEFAULT_ITERATIONS = 10

# Benchmarks expected to work with Luau (no stdlib deps)
LUAU_COMPATIBLE_BENCHMARKS = [
    "bench_sum.py",
    "bench_fib.py",
    "bench_deeply_nested_loop.py",
    "bench_sum_list.py",
    "bench_min_list.py",
    "bench_max_list.py",
    "bench_dict_ops.py",
    "bench_list_ops.py",
    "bench_range_iter.py",
]


def run_cpython_bench(source_path: str, iterations: int) -> dict:
    """Benchmark CPython execution."""
    times = []
    output = None
    for i in range(iterations):
        t0 = time.perf_counter()
        try:
            proc = subprocess.run(
                [sys.executable, source_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except subprocess.TimeoutExpired:
            return {"error": "CPython execution timed out (30s)"}
        elapsed = time.perf_counter() - t0
        if proc.returncode != 0:
            print(f"  CPython error: {proc.stderr.strip()}", file=sys.stderr)
            return {"error": proc.stderr.strip()}
        times.append(elapsed)
        if output is None:
            output = proc.stdout.strip()
    return {
        "runtime": "CPython",
        "version": sys.version.split()[0],
        "iterations": iterations,
        "times_ms": [round(t * 1000, 2) for t in times],
        "mean_ms": round(sum(times) / len(times) * 1000, 2),
        "min_ms": round(min(times) * 1000, 2),
        "max_ms": round(max(times) * 1000, 2),
        "output": output,
    }


def compile_to_luau(source_path: str, output_path: str) -> tuple[bool, float]:
    """Compile Python source to Luau via Molt. Returns (success, compile_time_s)."""
    artifact_root = _artifact_root()
    env = {
        **os.environ,
        "MOLT_EXT_ROOT": str(artifact_root),
        "CARGO_TARGET_DIR": os.environ.get("CARGO_TARGET_DIR", str(artifact_root / "target")),
        "RUSTC_WRAPPER": "",
        "PYTHONPATH": "src",
    }
    t0 = time.perf_counter()
    try:
        proc = subprocess.run(
            [
                "uv",
                "run",
                "python",
                "-m",
                "molt.cli",
                "build",
                source_path,
                "--target",
                "luau",
                "--output",
                output_path,
            ],
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
            cwd=str(REPO_ROOT),
        )
    except subprocess.TimeoutExpired:
        elapsed = time.perf_counter() - t0
        return False, elapsed
    elapsed = time.perf_counter() - t0
    if proc.returncode != 0:
        print(f"  Molt compile error: {proc.stderr.strip()}", file=sys.stderr)
        return False, elapsed
    return True, elapsed


def run_lune_bench(luau_path: str, iterations: int) -> dict:
    """Benchmark Lune (Luau VM) execution."""
    lune = os.path.expanduser("~/.aftman/bin/lune")
    if not os.path.exists(lune):
        lune = "lune"

    times = []
    output = None
    for i in range(iterations):
        t0 = time.perf_counter()
        try:
            proc = subprocess.run(
                [lune, "run", luau_path],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except subprocess.TimeoutExpired:
            return {"error": "Lune execution timed out (30s)"}
        elapsed = time.perf_counter() - t0
        if proc.returncode != 0:
            print(f"  Lune error: {proc.stderr.strip()}", file=sys.stderr)
            return {"error": proc.stderr.strip()}
        times.append(elapsed)
        if output is None:
            output = proc.stdout.strip()
    return {
        "runtime": "Lune (Luau VM)",
        "iterations": iterations,
        "times_ms": [round(t * 1000, 2) for t in times],
        "mean_ms": round(sum(times) / len(times) * 1000, 2),
        "min_ms": round(min(times) * 1000, 2),
        "max_ms": round(max(times) * 1000, 2),
        "output": output,
    }


def run_single_benchmark(
    bench_name: str,
    source_path: str,
    iterations: int,
    cpython_only: bool,
    tmp_dir: str,
) -> dict:
    """Run a single benchmark and return structured results."""
    result = {
        "name": bench_name,
        "source": source_path,
        "cpython": None,
        "luau": None,
        "compile_time_ms": None,
        "luau_output_bytes": None,
        "output_match": None,
        "ratio": None,
        "error": None,
    }

    print(f"--- {bench_name} ---")

    # CPython
    print("  Running CPython benchmark...")
    cpython_result = run_cpython_bench(source_path, iterations)
    result["cpython"] = cpython_result
    if "error" not in cpython_result:
        print(f"    Mean: {cpython_result['mean_ms']:.2f} ms")
    else:
        print(f"    ERROR: {cpython_result['error'][:120]}")
        result["error"] = f"CPython: {cpython_result['error'][:200]}"

    if cpython_only:
        return result

    # Molt -> Luau compilation
    luau_path = os.path.join(tmp_dir, Path(source_path).stem + ".luau")
    print("  Compiling to Luau via Molt...")
    ok, compile_time = compile_to_luau(source_path, luau_path)
    result["compile_time_ms"] = round(compile_time * 1000, 0)

    if not ok:
        result["error"] = "Transpilation failed"
        print(f"    Transpilation FAILED ({result['compile_time_ms']:.0f} ms)")
        return result

    luau_size = os.path.getsize(luau_path)
    result["luau_output_bytes"] = luau_size
    print(f"    Compile time: {result['compile_time_ms']:.0f} ms  ({luau_size} bytes)")

    # Lune (Luau VM)
    print("  Running Lune (Luau VM) benchmark...")
    lune_result = run_lune_bench(luau_path, iterations)
    result["luau"] = lune_result

    if "error" in lune_result:
        result["error"] = f"Lune: {lune_result['error'][:200]}"
        print(f"    Lune FAILED: {lune_result['error'][:120]}")
        return result

    print(f"    Mean: {lune_result['mean_ms']:.2f} ms")

    # Comparison
    if "error" not in cpython_result and "error" not in lune_result:
        match = cpython_result["output"] == lune_result["output"]
        result["output_match"] = match
        if lune_result["mean_ms"] > 0:
            ratio = cpython_result["mean_ms"] / lune_result["mean_ms"]
            result["ratio"] = round(ratio, 2)
            tag = "Luau faster" if ratio > 1 else "CPython faster"
            print(f"    Ratio: {ratio:.2f}x ({tag})  Match: {'YES' if match else 'NO'}")
        else:
            print(f"    Match: {'YES' if match else 'NO'}")

    return result


def generate_markdown_report(results: list[dict], iterations: int) -> str:
    """Generate a markdown table summarizing all benchmark results."""
    lines = [
        "# Molt Luau Transpiler Benchmark Report",
        "",
        f"**Iterations per benchmark:** {iterations}  ",
        f"**Date:** {time.strftime('%Y-%m-%d %H:%M:%S')}  ",
        f"**CPython:** {sys.version.split()[0]}  ",
        "",
        "| Benchmark | Transpile (ms) | CPython (ms) | Luau (ms) | Ratio | Output Match | Status |",
        "|-----------|---------------|-------------|----------|-------|-------------|--------|",
    ]

    for r in results:
        name = r["name"]
        compile_ms = f"{r['compile_time_ms']:.0f}" if r["compile_time_ms"] is not None else "-"

        if r["cpython"] and "error" not in r["cpython"]:
            cpython_ms = f"{r['cpython']['mean_ms']:.2f}"
        else:
            cpython_ms = "ERR"

        if r["luau"] and "error" not in r["luau"]:
            luau_ms = f"{r['luau']['mean_ms']:.2f}"
        else:
            luau_ms = "ERR"

        if r["ratio"] is not None:
            ratio = f"{r['ratio']:.2f}x"
        else:
            ratio = "-"

        if r["output_match"] is True:
            match = "YES"
        elif r["output_match"] is False:
            match = "NO"
        else:
            match = "-"

        if r["error"]:
            status = f"FAIL: {r['error'][:40]}"
        else:
            status = "OK"

        lines.append(f"| {name} | {compile_ms} | {cpython_ms} | {luau_ms} | {ratio} | {match} | {status} |")

    lines.append("")
    return "\n".join(lines)


def discover_benchmarks(pattern: str = "bench_*.py") -> list[Path]:
    """Discover benchmark files in the tests/benchmarks directory."""
    bench_dir = REPO_ROOT / "tests" / "benchmarks"
    paths = sorted(bench_dir.glob(pattern))
    return paths


def main():
    parser = argparse.ArgumentParser(
        description="Benchmark Molt-Luau vs CPython",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""\
examples:
  %(prog)s                                          # built-in zone generator
  %(prog)s tests/benchmarks/bench_sum.py            # single benchmark
  %(prog)s tests/benchmarks/bench_sum.py bench_fib.py  # multiple benchmarks
  %(prog)s --all                                    # all tests/benchmarks/bench_*.py
  %(prog)s --all --report                           # all + markdown report
  %(prog)s --cpython-only tests/benchmarks/bench_fib.py
""",
    )
    parser.add_argument(
        "benchmarks",
        nargs="*",
        help="Benchmark .py file(s) to run. Defaults to built-in zone generator.",
    )
    parser.add_argument("--cpython-only", action="store_true", help="Skip Molt/Lune, CPython only")
    parser.add_argument("--iterations", type=int, default=DEFAULT_ITERATIONS)
    parser.add_argument(
        "--all",
        action="store_true",
        help="Run all benchmarks from tests/benchmarks/bench_*.py",
    )
    parser.add_argument(
        "--luau-compat",
        action="store_true",
        help="With --all, only run benchmarks expected to work with Luau",
    )
    parser.add_argument(
        "--report",
        action="store_true",
        help="Generate a markdown table of all results",
    )
    args = parser.parse_args()

    # Resolve which benchmarks to run
    bench_files: list[tuple[str, str]] = []  # (name, path)

    if args.all:
        all_paths = discover_benchmarks()
        if args.luau_compat:
            all_paths = [p for p in all_paths if p.name in LUAU_COMPATIBLE_BENCHMARKS]
        for p in all_paths:
            bench_files.append((p.stem, str(p)))
    elif args.benchmarks:
        for b in args.benchmarks:
            p = Path(b)
            if not p.is_absolute():
                p = REPO_ROOT / p
            if not p.exists():
                # Try resolving relative to tests/benchmarks/
                p2 = REPO_ROOT / "tests" / "benchmarks" / b
                if p2.exists():
                    p = p2
            if not p.exists():
                print(f"WARNING: benchmark file not found: {b}", file=sys.stderr)
                continue
            bench_files.append((p.stem, str(p)))
    else:
        # Default: use the built-in zone generator
        bench_files = []  # handled below

    with tempfile.TemporaryDirectory(prefix="molt_luau_bench_") as tmp_dir:
        results: list[dict] = []

        if not bench_files:
            # Legacy mode: built-in zone generator
            py_path = os.path.join(tmp_dir, "zone_generator.py")
            with open(py_path, "w") as f:
                f.write(GENERATOR_SOURCE)
            bench_files = [("zone_generator", py_path)]

        print(f"=== Molt Luau Benchmark Suite ===")
        print(f"Benchmarks: {len(bench_files)}  Iterations: {args.iterations}")
        if args.cpython_only:
            print("Mode: CPython-only")
        print()

        for name, path in bench_files:
            try:
                result = run_single_benchmark(
                    bench_name=name,
                    source_path=path,
                    iterations=args.iterations,
                    cpython_only=args.cpython_only,
                    tmp_dir=tmp_dir,
                )
                results.append(result)
            except Exception as exc:
                print(f"  UNEXPECTED ERROR: {exc}", file=sys.stderr)
                results.append({
                    "name": name,
                    "source": path,
                    "cpython": None,
                    "luau": None,
                    "compile_time_ms": None,
                    "luau_output_bytes": None,
                    "output_match": None,
                    "ratio": None,
                    "error": f"Exception: {exc}",
                })
            print()

        # Summary
        print("=== Summary ===")
        ok_count = sum(1 for r in results if not r["error"])
        fail_count = sum(1 for r in results if r["error"])
        print(f"  Passed: {ok_count}  Failed: {fail_count}  Total: {len(results)}")

        if not args.cpython_only:
            matched = [r for r in results if r["output_match"] is True]
            if matched:
                ratios = [r["ratio"] for r in matched if r["ratio"] is not None]
                if ratios:
                    avg_ratio = sum(ratios) / len(ratios)
                    print(f"  Average ratio (Luau vs CPython): {avg_ratio:.2f}x")
        print()

        # Markdown report
        if args.report:
            report = generate_markdown_report(results, args.iterations)
            report_path = os.path.join(str(REPO_ROOT), "bench_luau_report.md")
            with open(report_path, "w") as f:
                f.write(report)
            print(f"Report written to: {report_path}")
            print()
            print(report)

        # JSON dump
        print(json.dumps(results, indent=2, default=str))


if __name__ == "__main__":
    main()
