#!/usr/bin/env python3
"""Benchmark harness: Molt-Luau vs CPython.

Compiles each benchmark Python file to Luau via molt, runs both
the original Python and the transpiled Luau (via Lune), and compares
execution time and output correctness.

Usage:
    python bench/luau/run_benchmarks.py [--molt-path PATH] [--lune-path PATH]

Environment variables:
    MOLT_PATH       Path to molt binary (default: searches PATH)
    LUNE_PATH       Path to lune binary (default: searches PATH)
    MOLT_EXT_ROOT   Artifact root for molt compilation
"""
import subprocess
import time
import sys
import os
import json
from pathlib import Path

BENCH_DIR = Path(__file__).parent
REPO_ROOT = BENCH_DIR.parent.parent

BENCHMARKS = [
    "bench_fibonacci.py",
    "bench_nbody.py",
    "bench_mandelbrot.py",
    "bench_spectral_norm.py",
    "bench_sieve.py",
    "bench_matrix_multiply.py",
    "bench_string_ops.py",
    # Depyler-ported benchmarks (https://github.com/paiml/depyler)
    "bench_depyler_compute.py",
    "bench_depyler_factorial.py",
    "bench_depyler_binary_search.py",
    "bench_depyler_bubble_sort.py",
    "bench_depyler_prime_sieve.py",
    "bench_depyler_gcd.py",
    "bench_depyler_quicksort.py",
    "bench_depyler_list_ops.py",
    "bench_depyler_string_ops.py",
    "bench_depyler_digits.py",
    "bench_depyler_power.py",
]


def run_cpython(bench_file: Path, runs: int = 3) -> tuple:
    """Run benchmark with CPython, return (avg_time_ms, output)."""
    times = []
    output = ""
    for _ in range(runs):
        start = time.perf_counter()
        result = subprocess.run(
            [sys.executable, str(bench_file)],
            capture_output=True, text=True, timeout=120
        )
        elapsed = (time.perf_counter() - start) * 1000
        if result.returncode != 0:
            raise RuntimeError(f"CPython failed: {result.stderr[:200]}")
        times.append(elapsed)
        output = result.stdout.strip()
    return sum(times) / len(times), output


def compile_to_luau(bench_file: Path, molt_path: str) -> Path:
    """Compile Python to Luau via molt, return output path or None."""
    out_path = bench_file.with_suffix(".luau")
    artifact_root = os.environ.get("MOLT_EXT_ROOT", str(REPO_ROOT))
    env = {
        **os.environ,
        "MOLT_EXT_ROOT": artifact_root,
        "CARGO_TARGET_DIR": os.environ.get(
            "CARGO_TARGET_DIR", os.path.join(artifact_root, "target")
        ),
        "RUSTC_WRAPPER": "",
        "PYTHONPATH": str(REPO_ROOT / "src"),
    }
    result = subprocess.run(
        [
            molt_path, "build",
            str(bench_file),
            "--target", "luau",
            "--output", str(out_path),
        ],
        capture_output=True, text=True, timeout=120,
        env=env, cwd=str(REPO_ROOT),
    )
    if result.returncode != 0:
        print(f"  COMPILE FAILED: {result.stderr[:200]}")
        return None
    return out_path


def run_lune(luau_file: Path, lune_path: str, runs: int = 3) -> tuple:
    """Run Luau benchmark via Lune, return (avg_time_ms, output)."""
    times = []
    output = ""
    for _ in range(runs):
        start = time.perf_counter()
        result = subprocess.run(
            [lune_path, "run", str(luau_file)],
            capture_output=True, text=True, timeout=120
        )
        elapsed = (time.perf_counter() - start) * 1000
        if result.returncode != 0:
            raise RuntimeError(f"Lune failed: {result.stderr[:200]}")
        times.append(elapsed)
        output = result.stdout.strip()
    return sum(times) / len(times), output


def resolve_molt_path() -> str:
    """Find the molt CLI, preferring uv run."""
    custom = os.environ.get("MOLT_PATH", "").strip()
    if custom:
        return custom
    return "uv run python -m molt.cli"


def resolve_lune_path() -> str:
    """Find the lune binary."""
    custom = os.environ.get("LUNE_PATH", "").strip()
    if custom:
        return custom
    aftman = os.path.expanduser("~/.aftman/bin/lune")
    if os.path.exists(aftman):
        return aftman
    return "lune"


def main():
    import argparse

    parser = argparse.ArgumentParser(description="Molt-Luau Benchmark Suite")
    parser.add_argument("--runs", type=int, default=3, help="Number of runs per benchmark")
    parser.add_argument("--cpython-only", action="store_true", help="Only run CPython (skip Luau)")
    parser.add_argument("--benchmarks", nargs="*", help="Specific benchmark files to run")
    args = parser.parse_args()

    molt_cmd = resolve_molt_path()
    lune_path = resolve_lune_path()

    bench_list = args.benchmarks if args.benchmarks else BENCHMARKS

    results = []
    print("=" * 70)
    print("Molt-Luau Benchmark Suite")
    print(f"Runs per benchmark: {args.runs}")
    print(f"CPython: {sys.version.split()[0]}")
    print("=" * 70)

    for bench_name in bench_list:
        bench_file = BENCH_DIR / bench_name
        if not bench_file.exists():
            print(f"\n[SKIP] {bench_name} -- file not found")
            continue

        print(f"\n--- {bench_name} ---")

        # CPython
        try:
            cpython_time, cpython_output = run_cpython(bench_file, args.runs)
            print(f"  CPython:    {cpython_time:8.1f} ms")
        except Exception as e:
            print(f"  CPython:    FAILED ({e})")
            results.append({
                "name": bench_name,
                "cpython_ms": None,
                "luau_ms": None,
                "correct": False,
                "error": str(e),
            })
            continue

        if args.cpython_only:
            results.append({
                "name": bench_name,
                "cpython_ms": round(cpython_time, 1),
                "luau_ms": None,
                "correct": None,
            })
            continue

        # Compile to Luau
        # Use uv run if molt_cmd contains spaces (i.e. "uv run python -m molt.cli")
        if " " in molt_cmd:
            cmd_parts = molt_cmd.split()
        else:
            cmd_parts = [molt_cmd]

        out_path = bench_file.with_suffix(".luau")
        artifact_root = os.environ.get("MOLT_EXT_ROOT", str(REPO_ROOT))
        env = {
            **os.environ,
            "MOLT_EXT_ROOT": artifact_root,
            "CARGO_TARGET_DIR": os.environ.get(
                "CARGO_TARGET_DIR", os.path.join(artifact_root, "target")
            ),
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": str(REPO_ROOT / "src"),
        }

        compile_start = time.perf_counter()
        compile_result = subprocess.run(
            cmd_parts + [
                "build", str(bench_file),
                "--target", "luau",
                "--output", str(out_path),
            ],
            capture_output=True, text=True, timeout=120,
            env=env, cwd=str(REPO_ROOT),
        )
        compile_time = (time.perf_counter() - compile_start) * 1000

        if compile_result.returncode != 0:
            print(f"  COMPILE FAILED ({compile_time:.0f} ms): {compile_result.stderr[:200]}")
            results.append({
                "name": bench_name,
                "cpython_ms": round(cpython_time, 1),
                "luau_ms": None,
                "correct": False,
                "compile_ms": round(compile_time, 1),
                "error": "compile failed",
            })
            continue

        luau_lines = len(out_path.read_text().splitlines())
        print(f"  Compiled:   {compile_time:8.0f} ms  ({luau_lines} lines of Luau)")

        # Run Luau via Lune
        try:
            luau_time, luau_output = run_lune(out_path, lune_path, args.runs)
            correct = luau_output == cpython_output
            speedup = cpython_time / luau_time if luau_time > 0 else 0

            status = "PASS" if correct else "FAIL (output mismatch)"
            print(f"  Luau:       {luau_time:8.1f} ms  ({speedup:.2f}x vs CPython) [{status}]")

            if not correct:
                print(f"  Expected: {cpython_output[:80]}")
                print(f"  Got:      {luau_output[:80]}")

            results.append({
                "name": bench_name,
                "cpython_ms": round(cpython_time, 1),
                "luau_ms": round(luau_time, 1),
                "speedup": round(speedup, 2),
                "correct": correct,
                "compile_ms": round(compile_time, 1),
                "luau_lines": luau_lines,
            })
        except Exception as e:
            print(f"  Luau:       FAILED ({e})")
            results.append({
                "name": bench_name,
                "cpython_ms": round(cpython_time, 1),
                "luau_ms": None,
                "correct": False,
                "error": str(e),
            })

    # Summary table
    print("\n" + "=" * 70)
    print("Summary")
    print("=" * 70)
    print(f"{'Benchmark':<25} {'CPython (ms)':>12} {'Luau (ms)':>12} {'Speedup':>10} {'Correct':>8}")
    print("-" * 70)
    for r in results:
        cp_str = f"{r['cpython_ms']:>12.1f}" if r.get('cpython_ms') is not None else "      FAILED"
        luau_str = f"{r['luau_ms']:>12.1f}" if r.get('luau_ms') is not None else "      FAILED"
        speedup_str = f"{r.get('speedup', 0):>9.2f}x" if r.get('luau_ms') is not None else "         -"
        if r.get('correct') is True:
            correct_str = "PASS"
        elif r.get('correct') is False:
            correct_str = "FAIL"
        else:
            correct_str = "-"
        print(f"{r['name']:<25} {cp_str} {luau_str} {speedup_str} {correct_str:>8}")

    # Write JSON results
    json_path = BENCH_DIR / "results.json"
    with open(json_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nResults written to {json_path}")


if __name__ == "__main__":
    main()
