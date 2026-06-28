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

import os
import json
import shlex
import sys
import time
from pathlib import Path

BENCH_DIR = Path(__file__).parent
REPO_ROOT = BENCH_DIR.parent.parent
TOOLS_ROOT = REPO_ROOT / "tools"
SRC_ROOT = REPO_ROOT / "src"
RESULTS_DIR = REPO_ROOT / "bench" / "results" / "luau"
TMP_ROOT = REPO_ROOT / "tmp" / "bench" / "luau"
DEFAULT_RESULTS_PATH = RESULTS_DIR / "results.json"

sys.path.insert(0, str(TOOLS_ROOT))
sys.path.insert(0, str(SRC_ROOT))

import harness_memory_guard  # noqa: E402
import perf_authority  # noqa: E402
from molt.dx import development_artifact_env  # noqa: E402

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


def _base_env() -> dict[str, str]:
    env = development_artifact_env(
        REPO_ROOT,
        os.environ,
        session_prefix="bench-luau",
        session_id=os.environ.get("MOLT_SESSION_ID")
        or f"bench-luau-{os.getpid()}",
        create_dirs=True,
    )
    env.update(
        {
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": str(REPO_ROOT / "src"),
        }
    )
    return env


def _command_parts(command: str) -> list[str]:
    return shlex.split(command) if command.strip() else []


def _run_guarded(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    timeout: float | None = 120,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> harness_memory_guard.GuardedCompletedProcess:
    run_env = env if env is not None else _base_env()
    resolved_limits = limits or harness_memory_guard.limits_from_env(
        "MOLT_BENCH",
        run_env,
    )
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_BENCH",
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=run_env,
        limits=resolved_limits,
    )


def _elapsed_ms(
    result: harness_memory_guard.GuardedCompletedProcess,
    fallback_start: float,
) -> float:
    if result.elapsed_s is not None:
        return result.elapsed_s * 1000
    return (time.perf_counter() - fallback_start) * 1000


def _luau_output_path(bench_file: Path) -> Path:
    TMP_ROOT.mkdir(parents=True, exist_ok=True)
    return TMP_ROOT / f"{bench_file.stem}.luau"


def run_cpython(
    bench_file: Path,
    runs: int = 3,
    *,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> tuple[float, str]:
    """Run benchmark with CPython, return (avg_time_ms, output)."""
    times = []
    output = ""
    for _ in range(runs):
        start = time.perf_counter()
        result = _run_guarded(
            [sys.executable, str(bench_file)],
            env=_base_env(),
            timeout=120,
            limits=limits,
        )
        elapsed = _elapsed_ms(result, start)
        if result.returncode != 0:
            raise RuntimeError(f"CPython failed: {result.stderr[:200]}")
        times.append(elapsed)
        output = result.stdout.strip()
    return sum(times) / len(times), output


def compile_to_luau(
    bench_file: Path,
    molt_cmd: str,
    *,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> tuple[Path | None, float, str]:
    """Compile Python to Luau via molt, return output path or None."""
    out_path = _luau_output_path(bench_file)
    cmd_parts = _command_parts(molt_cmd)
    if not cmd_parts:
        return None, 0.0, "empty Molt command"
    env = _base_env()
    start = time.perf_counter()
    result = _run_guarded(
        cmd_parts
        + [
            "build",
            str(bench_file),
            "--target",
            "luau",
            "--output",
            str(out_path),
        ],
        env=env,
        timeout=120,
        limits=limits,
    )
    elapsed_ms = _elapsed_ms(result, start)
    if result.returncode != 0:
        return None, elapsed_ms, (result.stderr or result.stdout)[:200]
    if not out_path.exists():
        return None, elapsed_ms, f"missing Luau output: {out_path}"
    return out_path, elapsed_ms, ""


def run_lune(
    luau_file: Path,
    lune_cmd: str,
    runs: int = 3,
    *,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> tuple[float, str]:
    """Run Luau benchmark via Lune, return (avg_time_ms, output)."""
    cmd_parts = _command_parts(lune_cmd)
    if not cmd_parts:
        raise RuntimeError("empty Lune command")
    times = []
    output = ""
    for _ in range(runs):
        start = time.perf_counter()
        result = _run_guarded(
            cmd_parts + ["run", str(luau_file)],
            env=_base_env(),
            timeout=120,
            limits=limits,
        )
        elapsed = _elapsed_ms(result, start)
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
    parser.add_argument(
        "--runs", type=int, default=3, help="Number of runs per benchmark"
    )
    parser.add_argument(
        "--cpython-only", action="store_true", help="Only run CPython (skip Luau)"
    )
    parser.add_argument(
        "--benchmarks", nargs="*", help="Specific benchmark files to run"
    )
    args = parser.parse_args()
    if args.runs <= 0:
        parser.error("--runs must be positive")

    molt_cmd = resolve_molt_path()
    lune_cmd = resolve_lune_path()
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    TMP_ROOT.mkdir(parents=True, exist_ok=True)
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    bench_list = args.benchmarks if args.benchmarks else BENCHMARKS

    results = []
    print("=" * 70)
    print("Molt-Luau Benchmark Suite")
    print(f"Runs per benchmark: {args.runs}")
    print(f"CPython: {sys.version.split()[0]}")
    print("=" * 70)

    with harness_memory_guard.repo_process_sentinel(
        repo_root=REPO_ROOT,
        artifact_root=REPO_ROOT / "tmp" / "bench",
        label="luau_run_benchmarks",
        limits=limits,
    ):
        for bench_name in bench_list:
            bench_file = BENCH_DIR / bench_name
            if not bench_file.exists():
                print(f"\n[SKIP] {bench_name} -- file not found")
                continue

            print(f"\n--- {bench_name} ---")

            # CPython
            try:
                cpython_time, cpython_output = run_cpython(
                    bench_file,
                    args.runs,
                    limits=limits,
                )
                print(f"  CPython:    {cpython_time:8.1f} ms")
            except Exception as e:
                print(f"  CPython:    FAILED ({e})")
                results.append(
                    {
                        "name": bench_name,
                        "cpython_ms": None,
                        "luau_ms": None,
                        "correct": False,
                        "error": str(e),
                    }
                )
                continue

            if args.cpython_only:
                results.append(
                    {
                        "name": bench_name,
                        "cpython_ms": round(cpython_time, 1),
                        "luau_ms": None,
                        "correct": None,
                    }
                )
                continue

            out_path, compile_time, compile_error = compile_to_luau(
                bench_file,
                molt_cmd,
                limits=limits,
            )
            if out_path is None:
                print(f"  COMPILE FAILED ({compile_time:.0f} ms): {compile_error}")
                results.append(
                    {
                        "name": bench_name,
                        "cpython_ms": round(cpython_time, 1),
                        "luau_ms": None,
                        "correct": False,
                        "compile_ms": round(compile_time, 1),
                        "error": "compile failed",
                    }
                )
                continue

            luau_lines = len(out_path.read_text().splitlines())
            print(f"  Compiled:   {compile_time:8.0f} ms  ({luau_lines} lines of Luau)")

            # Run Luau via Lune
            try:
                luau_time, luau_output = run_lune(
                    out_path,
                    lune_cmd,
                    args.runs,
                    limits=limits,
                )
                correct = luau_output == cpython_output
                # Route through the single guarded authority (SPEEDUP
                # direction: baseline/candidate, >1 = candidate faster) so a
                # degenerate (None/0) luau time can never render a finite ratio.
                speedup = perf_authority.signed_ratio_value(
                    cpython_time,
                    luau_time,
                    direction=perf_authority.RatioDirection.SPEEDUP,
                )
                speedup_cell = "n/a" if speedup is None else f"{speedup:.2f}x"

                status = "PASS" if correct else "FAIL (output mismatch)"
                print(
                    f"  Luau:       {luau_time:8.1f} ms  "
                    f"({speedup_cell} vs CPython) [{status}]"
                )

                if not correct:
                    print(f"  Expected: {cpython_output[:80]}")
                    print(f"  Got:      {luau_output[:80]}")

                results.append(
                    {
                        "name": bench_name,
                        "cpython_ms": round(cpython_time, 1),
                        "luau_ms": round(luau_time, 1),
                        "speedup": round(speedup, 2) if speedup is not None else None,
                        "ratio_directions": {
                            "speedup": perf_authority.RatioDirection.SPEEDUP.value,
                        },
                        "correct": correct,
                        "compile_ms": round(compile_time, 1),
                        "luau_lines": luau_lines,
                    }
                )
            except Exception as e:
                print(f"  Luau:       FAILED ({e})")
                results.append(
                    {
                        "name": bench_name,
                        "cpython_ms": round(cpython_time, 1),
                        "luau_ms": None,
                        "correct": False,
                        "error": str(e),
                    }
                )

    # Summary table
    print("\n" + "=" * 70)
    print("Summary")
    print("=" * 70)
    print(
        f"{'Benchmark':<25} {'CPython (ms)':>12} {'Luau (ms)':>12} {'Speedup':>10} {'Correct':>8}"
    )
    print("-" * 70)
    for r in results:
        cp_str = (
            f"{r['cpython_ms']:>12.1f}"
            if r.get("cpython_ms") is not None
            else "      FAILED"
        )
        luau_str = (
            f"{r['luau_ms']:>12.1f}" if r.get("luau_ms") is not None else "      FAILED"
        )
        speedup_str = (
            f"{r.get('speedup', 0):>9.2f}x"
            if r.get("luau_ms") is not None
            else "         -"
        )
        if r.get("correct") is True:
            correct_str = "PASS"
        elif r.get("correct") is False:
            correct_str = "FAIL"
        else:
            correct_str = "-"
        print(f"{r['name']:<25} {cp_str} {luau_str} {speedup_str} {correct_str:>8}")

    # Write JSON results
    json_path = DEFAULT_RESULTS_PATH
    with open(json_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nResults written to {json_path}")


if __name__ == "__main__":
    main()
