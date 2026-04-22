#!/usr/bin/env python3
"""Individual benchmark runner with daemon isolation.

Runs each benchmark in complete isolation by killing all molt-backend
processes between runs.  This avoids the cascade-failure problem where
a daemon crash causes all subsequent benchmarks to fail.

Usage:
    python tools/bench_individual.py
    python tools/bench_individual.py --samples 5 --json-out results.json
    python tools/bench_individual.py --bench bench_fib.py --bench bench_sum.py
    python tools/bench_individual.py --skip bench_startup.py
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# ---------------------------------------------------------------------------
# Benchmark list (mirrors bench.py)
# ---------------------------------------------------------------------------

BENCHMARKS = [
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
]

MOLT_ARGS_BY_BENCH = {
    "tests/benchmarks/bench_sum_list_hints.py": ["--type-hints", "trust"],
}

# ---------------------------------------------------------------------------
# Daemon management
# ---------------------------------------------------------------------------


def _kill_all_molt_backends() -> int:
    """Kill every molt-backend daemon process.  Returns number killed."""
    if os.name != "posix":
        return 0
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return 0

    killed = 0
    pattern = re.compile(r"^\s*(\d+)\s+(.*)$")
    for line in result.stdout.splitlines():
        match = pattern.match(line)
        if match is None:
            continue
        pid = int(match.group(1))
        cmd = match.group(2)
        if "molt-backend" not in cmd:
            continue
        try:
            os.kill(pid, signal.SIGKILL)
            killed += 1
        except (ProcessLookupError, PermissionError):
            pass
    return killed


def _ensure_clean_slate(quiet: bool = False) -> None:
    """Kill all backends and wait for cleanup."""
    killed = _kill_all_molt_backends()
    if killed and not quiet:
        print(f"  [cleanup] killed {killed} molt-backend process(es)", file=sys.stderr)
    # Give the OS time to release sockets and clean up
    if killed:
        time.sleep(2)


# ---------------------------------------------------------------------------
# Build helpers
# ---------------------------------------------------------------------------


def _molt_build_cmd() -> list[str]:
    """Command prefix for the Molt compiler via uv."""
    return ["uv", "run", "--python", "3.12", "python3"]


def _resolve_molt_output(payload: dict) -> Path | None:
    output_str = payload.get("data", {}).get("output") or payload.get("output")
    if not output_str:
        return None
    output_path = Path(output_str)
    if output_path.exists():
        return output_path
    fallback = output_path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def molt_build(
    script: str,
    out_dir: Path,
    timeout_s: float,
    extra_args: list[str] | None = None,
) -> tuple[Path | None, float, str]:
    """Build a benchmark with Molt.

    Returns (binary_path_or_None, build_time_seconds, error_message).
    """
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"

    args = [
        *_molt_build_cmd(),
        "-m",
        "molt.cli",
        "build",
        "--trusted",
        "--json",
        "--out-dir",
        str(out_dir),
    ]
    if extra_args:
        args.extend(extra_args)
    args.append(script)

    start = time.perf_counter()
    try:
        res = subprocess.run(
            args,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired:
        return None, time.perf_counter() - start, f"build timed out after {timeout_s}s"
    build_s = time.perf_counter() - start

    if res.returncode != 0:
        err = (res.stderr or res.stdout or "").strip()[:500]
        return None, build_s, f"build failed (rc={res.returncode}): {err}"

    try:
        payload = json.loads(res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        return None, build_s, "build produced invalid JSON"

    binary = _resolve_molt_output(payload)
    if binary is None:
        return None, build_s, "build succeeded but no output binary found"

    return binary, build_s, ""


# ---------------------------------------------------------------------------
# Run helpers
# ---------------------------------------------------------------------------


def run_binary(binary: Path, timeout_s: float) -> tuple[bool, float, str]:
    """Run a compiled binary.  Returns (ok, elapsed_s, stdout_stripped)."""
    start = time.perf_counter()
    try:
        res = subprocess.run(
            [str(binary)],
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired:
        return False, time.perf_counter() - start, ""
    elapsed = time.perf_counter() - start
    if res.returncode != 0:
        return False, elapsed, ""
    return True, elapsed, (res.stdout or "").strip()


def run_cpython(script: str, timeout_s: float) -> tuple[bool, float, str]:
    """Run a script with CPython.  Returns (ok, elapsed_s, stdout_stripped)."""
    start = time.perf_counter()
    try:
        res = subprocess.run(
            [sys.executable, script],
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired:
        return False, time.perf_counter() - start, ""
    elapsed = time.perf_counter() - start
    if res.returncode != 0:
        return False, elapsed, ""
    return True, elapsed, (res.stdout or "").strip()


# ---------------------------------------------------------------------------
# Single benchmark
# ---------------------------------------------------------------------------


def bench_one(
    script: str,
    samples: int,
    timeout_build: float,
    timeout_run: float,
) -> dict:
    """Run a single benchmark with full daemon isolation.

    Returns a result dict for the JSON report.
    """
    name = Path(script).name
    extra_args = MOLT_ARGS_BY_BENCH.get(script)

    result: dict = {
        "build_ok": False,
        "build_time_s": None,
        "run_ok": False,
        "molt_time_s": None,
        "cpython_time_s": None,
        "speedup": None,
        "output_match": None,
        "molt_output": None,
        "cpython_output": None,
        "error": None,
    }

    # --- Kill all backends for isolation ---
    _ensure_clean_slate()

    # --- Build with Molt ---
    tmp = tempfile.TemporaryDirectory(prefix="molt-iso-bench-")
    out_dir = Path(tmp.name)
    binary, build_s, build_err = molt_build(
        script,
        out_dir,
        timeout_build,
        extra_args=extra_args,
    )
    result["build_time_s"] = round(build_s, 4)

    if binary is None:
        result["error"] = build_err
        print(f"  BUILD FAIL: {build_err}", file=sys.stderr)
        # Still try CPython
        cp_ok, cp_time, cp_out = run_cpython(script, timeout_run)
        if cp_ok:
            result["cpython_time_s"] = round(cp_time, 4)
            result["cpython_output"] = cp_out
        tmp.cleanup()
        return result

    result["build_ok"] = True

    # --- Run Molt (multiple samples, take median) ---
    molt_times: list[float] = []
    molt_output = ""
    for i in range(samples):
        ok, elapsed, output = run_binary(binary, timeout_run)
        if ok:
            molt_times.append(elapsed)
            if i == 0:
                molt_output = output
        else:
            print(
                f"  Molt run sample {i + 1}/{samples} failed for {name}",
                file=sys.stderr,
            )

    if molt_times:
        result["run_ok"] = True
        result["molt_time_s"] = round(statistics.median(molt_times), 6)
        result["molt_output"] = molt_output

    # --- Run CPython (multiple samples, take median) ---
    cpython_times: list[float] = []
    cpython_output = ""
    for i in range(samples):
        ok, elapsed, output = run_cpython(script, timeout_run)
        if ok:
            cpython_times.append(elapsed)
            if i == 0:
                cpython_output = output

    if cpython_times:
        result["cpython_time_s"] = round(statistics.median(cpython_times), 6)
        result["cpython_output"] = cpython_output

    # --- Output match ---
    if molt_output and cpython_output:
        result["output_match"] = molt_output == cpython_output
    elif molt_output or cpython_output:
        result["output_match"] = False

    # --- Speedup ---
    if result["molt_time_s"] and result["cpython_time_s"] and result["molt_time_s"] > 0:
        result["speedup"] = round(result["cpython_time_s"] / result["molt_time_s"], 1)

    tmp.cleanup()
    return result


# ---------------------------------------------------------------------------
# Summary printer
# ---------------------------------------------------------------------------


def print_summary(results: dict[str, dict]) -> None:
    """Print an aligned summary table to stdout."""
    header = f"{'Benchmark':<42} {'Build':>7} {'Molt(s)':>10} {'CPy(s)':>10} {'Speedup':>8} {'Match':>6}"
    sep = "-" * len(header)
    print()
    print(sep)
    print(header)
    print(sep)

    pass_count = 0
    fail_count = 0
    total = len(results)

    for name, r in results.items():
        build_str = "OK" if r["build_ok"] else "FAIL"
        molt_str = f"{r['molt_time_s']:.4f}" if r["molt_time_s"] is not None else "-"
        cpy_str = (
            f"{r['cpython_time_s']:.4f}" if r["cpython_time_s"] is not None else "-"
        )
        speedup_str = f"{r['speedup']:.1f}x" if r["speedup"] is not None else "-"

        if r["output_match"] is True:
            match_str = "YES"
        elif r["output_match"] is False:
            match_str = "NO"
        else:
            match_str = "-"

        if r["build_ok"] and r["run_ok"]:
            pass_count += 1
        else:
            fail_count += 1

        print(
            f"{name:<42} {build_str:>7} {molt_str:>10} {cpy_str:>10} {speedup_str:>8} {match_str:>6}"
        )

    print(sep)
    print(f"Total: {total}  |  Pass: {pass_count}  |  Fail: {fail_count}")
    print(sep)
    print()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run Molt benchmarks with per-benchmark daemon isolation.",
    )
    parser.add_argument(
        "--samples",
        type=int,
        default=3,
        help="Number of run samples per benchmark; takes median (default: 3)",
    )
    parser.add_argument(
        "--json-out",
        type=str,
        default=None,
        help="Path to write JSON results",
    )
    parser.add_argument(
        "--bench",
        action="append",
        default=None,
        help="Run only specific benchmark(s) by filename (repeatable)",
    )
    parser.add_argument(
        "--skip",
        action="append",
        default=None,
        help="Skip specific benchmark(s) by filename (repeatable)",
    )
    parser.add_argument(
        "--timeout-build",
        type=float,
        default=120,
        help="Build timeout in seconds (default: 120)",
    )
    parser.add_argument(
        "--timeout-run",
        type=float,
        default=60,
        help="Run timeout in seconds (default: 60)",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()

    # Filter benchmarks
    benchmarks = list(BENCHMARKS)

    if args.bench:
        selected = set(args.bench)
        benchmarks = [
            b
            for b in benchmarks
            if Path(b).name in selected or Path(b).stem in selected or b in selected
        ]
        if not benchmarks:
            print(f"No benchmarks matched: {args.bench}", file=sys.stderr)
            sys.exit(1)

    if args.skip:
        skip_set = set(args.skip)
        benchmarks = [
            b
            for b in benchmarks
            if Path(b).name not in skip_set
            and Path(b).stem not in skip_set
            and b not in skip_set
        ]

    total = len(benchmarks)
    print(f"Running {total} benchmark(s) with {args.samples} sample(s) each")
    print(f"Build timeout: {args.timeout_build}s  |  Run timeout: {args.timeout_run}s")
    print()

    results: dict[str, dict] = {}

    for idx, script in enumerate(benchmarks, 1):
        name = Path(script).name
        print(f"[{idx}/{total}] {name}")
        result = bench_one(
            script,
            samples=args.samples,
            timeout_build=args.timeout_build,
            timeout_run=args.timeout_run,
        )
        results[name] = result

        # Quick inline status
        if result["build_ok"] and result["run_ok"]:
            speedup = f" ({result['speedup']:.1f}x)" if result["speedup"] else ""
            print(
                f"  -> OK  molt={result['molt_time_s']:.4f}s  cpython={result['cpython_time_s']:.4f}s{speedup}"
            )
        elif result["build_ok"]:
            print("  -> BUILD OK, RUN FAIL")
        else:
            print("  -> BUILD FAIL")

    # Summary
    print_summary(results)

    # JSON output
    report = {"benchmarks": results}
    if args.json_out:
        out_path = Path(args.json_out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Results written to {out_path}")
    else:
        # Also dump to a default location
        default_out = Path("bench_individual_results.json")
        default_out.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Results written to {default_out}")

    # Final cleanup
    _ensure_clean_slate(quiet=True)


if __name__ == "__main__":
    main()
