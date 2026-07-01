#!/usr/bin/env python3
"""Benchmark: Molt Rust transpiler performance — transpile time, compile time,
runtime vs CPython.

Measures the full Molt -> Rust -> native pipeline:
  1. Transpile:  molt build --target rust
  2. Compile:    rustc -O --edition=2024
  3. Run:        native binary vs CPython

Reports: transpile time, compile time, output size, runtime, speedup vs CPython,
and output correctness (stdout match).

NOTE: The Rust backend currently emits a self-contained .rs file with an inline
MoltValue prelude AND the full stdlib bootstrap (~130K+ lines, ~200MB+). This
makes rustc compilation very slow or impractical for file-based transpilation.
The test suite (tests/rust/test_molt_rust_correctness.py) works around this by
passing small inline snippets that produce compact output. Once the backend
supports tree-shaking or crate-based compilation (--use-crate mode), this
benchmark will produce practical end-to-end timings.

Usage:
    # Run all default benchmarks:
    uv run python tools/bench_rust_transpile.py

    # Run specific benchmark files:
    uv run python tools/bench_rust_transpile.py tests/benchmarks/bench_sum.py

    # CPython-only (no Molt/rustc needed):
    uv run python tools/bench_rust_transpile.py --cpython-only

    # Adjust iterations / output JSON to file:
    uv run python tools/bench_rust_transpile.py --iterations 5 --json results.json

Environment:
    MOLT_EXT_ROOT=<artifact-root>   # optional; defaults to repo root
    CARGO_TARGET_DIR=<artifact-root>/target/sessions/$MOLT_SESSION_ID
    RUSTC_WRAPPER=""
    PYTHONPATH=src
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from tools import harness_memory_guard  # noqa: E402
from tools import perf_authority  # noqa: E402
from molt.dx import development_artifact_env  # noqa: E402

DEFAULT_BENCHMARKS = [
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_fib.py",
    "tests/benchmarks/bench_deeply_nested_loop.py",
    "tests/benchmarks/bench_list_ops.py",
]

ITERATIONS = 10


def _artifact_root() -> Path:
    configured = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if configured:
        return Path(configured).expanduser()
    return REPO_ROOT


def _find_rustc() -> str | None:
    """Return rustc path, or None if unavailable."""
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    for candidate in ("rustc", os.path.expanduser("~/.cargo/bin/rustc")):
        try:
            r = harness_memory_guard.guarded_completed_process(
                [candidate, "--version"],
                prefix="MOLT_BENCH",
                capture_output=True,
                text=True,
                timeout=15,
                limits=limits,
            )
            if r.returncode == 0:
                return candidate
        except (FileNotFoundError, subprocess.TimeoutExpired):
            pass
    return None


def _find_cpython() -> str:
    """Return a working CPython executable."""
    candidates = [
        sys.executable,
        getattr(sys, "_base_executable", ""),
        shutil.which("python3") or "",
        shutil.which("python") or "",
    ]
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    for candidate in candidates:
        if not candidate:
            continue
        try:
            probe = harness_memory_guard.guarded_completed_process(
                [candidate, "-c", "import sys; print(sys.version_info[0])"],
                prefix="MOLT_BENCH",
                capture_output=True,
                text=True,
                timeout=5,
                limits=limits,
            )
            if probe.returncode == 0 and probe.stdout.strip() == "3":
                return candidate
        except (FileNotFoundError, subprocess.TimeoutExpired):
            continue
    print("ERROR: CPython not found", file=sys.stderr)
    sys.exit(1)


def _molt_env() -> dict[str, str]:
    """Build environment dict for molt CLI invocations."""
    env = development_artifact_env(
        REPO_ROOT,
        os.environ,
        session_prefix="bench-rust-transpile",
        session_id=os.environ.get("MOLT_SESSION_ID")
        or f"bench-rust-transpile-{os.getpid()}",
        create_dirs=True,
    )
    env.update(
        {
            "MOLT_USE_SCCACHE": "0",
            "MOLT_BACKEND_DAEMON": "0",
            "RUSTC_WRAPPER": "",
            "PYTHONPATH": str(REPO_ROOT / "src"),
            "UV_LINK_MODE": os.environ.get("UV_LINK_MODE", "copy"),
            "UV_NO_SYNC": os.environ.get("UV_NO_SYNC", "1"),
        }
    )
    return env


def transpile_to_rust(py_path: str, rs_path: str) -> tuple[bool, float, str]:
    """Transpile Python → Rust via molt CLI.

    Returns (success, elapsed_seconds, error_message).
    """
    env = _molt_env()
    py_exec = sys.executable or _find_cpython()
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", env)

    t0 = time.perf_counter()
    try:
        result = harness_memory_guard.guarded_completed_process(
            [
                py_exec,
                "-m",
                "molt.cli",
                "build",
                py_path,
                "--target",
                "rust",
                "--output",
                rs_path,
            ],
            prefix="MOLT_BENCH",
            capture_output=True,
            text=True,
            timeout=120,
            env=env,
            cwd=str(REPO_ROOT),
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return False, time.perf_counter() - t0, "transpile timed out (120s)"
    elapsed = (
        result.elapsed_s if result.elapsed_s is not None else time.perf_counter() - t0
    )

    if result.returncode != 0:
        return False, elapsed, result.stderr.strip() or result.stdout.strip()
    return True, elapsed, ""


def compile_rust(
    rustc: str, rs_path: str, bin_path: str, optimize: bool = True
) -> tuple[bool, float, str]:
    """Compile .rs file with rustc.

    Returns (success, elapsed_seconds, error_message).
    """
    allow_lints = ["unused_mut", "unused_variables", "dead_code", "non_snake_case"]
    compile_timeout = (
        600  # Molt emits large self-contained files; allow generous compile time
    )
    cmd = [
        rustc,
        rs_path,
        "-o",
        bin_path,
        "--edition=2024",
        *(["-O"] if optimize else []),
        *[flag for lint in allow_lints for flag in ("-A", lint)],
    ]

    t0 = time.perf_counter()
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    try:
        result = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix="MOLT_BENCH",
            capture_output=True,
            text=True,
            timeout=compile_timeout,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return False, time.perf_counter() - t0, f"rustc timed out ({compile_timeout}s)"
    elapsed = (
        result.elapsed_s if result.elapsed_s is not None else time.perf_counter() - t0
    )

    if result.returncode != 0:
        return False, elapsed, result.stderr.strip()
    return True, elapsed, ""


def run_binary(bin_path: str, iterations: int) -> dict:
    """Run a compiled binary multiple times and collect timings."""
    times = []
    output = None
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    for _ in range(iterations):
        t0 = time.perf_counter()
        try:
            proc = harness_memory_guard.guarded_completed_process(
                [bin_path],
                prefix="MOLT_BENCH",
                capture_output=True,
                text=True,
                timeout=60,
                limits=limits,
            )
        except subprocess.TimeoutExpired:
            return {"error": "binary timed out (60s)"}
        elapsed = (
            proc.elapsed_s if proc.elapsed_s is not None else time.perf_counter() - t0
        )
        if proc.returncode != 0:
            return {
                "error": f"binary exit code {proc.returncode}: {proc.stderr.strip()}"
            }
        times.append(elapsed)
        if output is None:
            output = proc.stdout.strip()
    return {
        "runtime": "Rust (rustc -O)",
        "iterations": iterations,
        "times_ms": [round(t * 1000, 2) for t in times],
        "mean_ms": round(sum(times) / len(times) * 1000, 2),
        "min_ms": round(min(times) * 1000, 2),
        "max_ms": round(max(times) * 1000, 2),
        "output": output,
    }


def run_cpython(py_path: str, iterations: int) -> dict:
    """Run Python file with CPython multiple times and collect timings."""
    cpython = _find_cpython()
    times = []
    output = None
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    for _ in range(iterations):
        t0 = time.perf_counter()
        try:
            proc = harness_memory_guard.guarded_completed_process(
                [cpython, py_path],
                prefix="MOLT_BENCH",
                capture_output=True,
                text=True,
                timeout=60,
                limits=limits,
            )
        except subprocess.TimeoutExpired:
            return {"error": "CPython timed out (60s)"}
        elapsed = (
            proc.elapsed_s if proc.elapsed_s is not None else time.perf_counter() - t0
        )
        if proc.returncode != 0:
            return {"error": f"CPython error: {proc.stderr.strip()}"}
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


def bench_one(
    py_path: str,
    rustc: str | None,
    iterations: int,
    cpython_only: bool,
) -> dict:
    """Run a full benchmark for one .py file.

    Returns a result dict with transpile/compile/run metrics.
    """
    name = Path(py_path).stem
    result: dict = {"name": name, "source": str(py_path)}

    # --- CPython baseline ---
    print(f"  CPython ({iterations} iterations)...")
    cpython_res = run_cpython(py_path, iterations)
    result["cpython"] = cpython_res
    if "error" not in cpython_res:
        print(
            f"    mean: {cpython_res['mean_ms']:.2f} ms  "
            f"min: {cpython_res['min_ms']:.2f} ms"
        )
    else:
        print(f"    ERROR: {cpython_res['error']}")

    if cpython_only or rustc is None:
        return result

    # --- Transpile ---
    with tempfile.TemporaryDirectory(prefix="molt_bench_") as tmpdir:
        rs_path = os.path.join(tmpdir, f"{name}.rs")
        bin_path = os.path.join(tmpdir, name)

        print("  Transpile (molt build --target rust)...")
        ok, transpile_time, err = transpile_to_rust(py_path, rs_path)
        result["transpile_ms"] = round(transpile_time * 1000, 1)
        if not ok:
            result["transpile_error"] = err
            print(f"    FAILED ({transpile_time * 1000:.0f} ms): {err[:120]}")
            return result

        rs_size = os.path.getsize(rs_path)
        result["rs_size_bytes"] = rs_size
        print(f"    OK in {transpile_time * 1000:.0f} ms  ({rs_size} bytes)")

        # --- Compile ---
        print("  Compile (rustc -O)...")
        ok, compile_time, err = compile_rust(rustc, rs_path, bin_path, optimize=True)
        result["compile_ms"] = round(compile_time * 1000, 1)
        if not ok:
            result["compile_error"] = err
            print(f"    FAILED ({compile_time * 1000:.0f} ms): {err[:200]}")
            return result
        print(f"    OK in {compile_time * 1000:.0f} ms")

        # --- Run ---
        print(f"  Rust binary ({iterations} iterations)...")
        rust_res = run_binary(bin_path, iterations)
        result["rust"] = rust_res
        if "error" not in rust_res:
            print(
                f"    mean: {rust_res['mean_ms']:.2f} ms  "
                f"min: {rust_res['min_ms']:.2f} ms"
            )
        else:
            print(f"    ERROR: {rust_res['error']}")

    # --- Comparison ---
    if (
        "error" not in cpython_res
        and "rust" in result
        and "error" not in result.get("rust", {})
    ):
        rust_res = result["rust"]
        # Route through the single guarded authority (SPEEDUP direction:
        # baseline/candidate, >1 = candidate faster) so a degenerate
        # (None/0/NaN) rust time can never render a finite OR infinite ratio.
        speedup = perf_authority.signed_ratio_value(
            cpython_res["mean_ms"],
            rust_res["mean_ms"],
            direction=perf_authority.RatioDirection.SPEEDUP,
        )
        result["speedup_vs_cpython"] = (
            round(speedup, 2) if speedup is not None else None
        )
        result["ratio_directions"] = {
            "speedup_vs_cpython": perf_authority.RatioDirection.SPEEDUP.value,
        }
        output_match = cpython_res["output"] == rust_res["output"]
        result["output_match"] = output_match
        speedup_cell = "n/a" if speedup is None else f"{speedup:.1f}x"
        print(
            f"  Speedup: {speedup_cell}  "
            f"Output match: {'YES' if output_match else 'NO'}"
        )

    return result


def main():
    parser = argparse.ArgumentParser(
        description="Benchmark Molt Rust transpiler: transpile + compile + runtime vs CPython"
    )
    parser.add_argument(
        "benchmarks",
        nargs="*",
        help="Python benchmark files to run (default: built-in list)",
    )
    parser.add_argument(
        "--cpython-only",
        action="store_true",
        help="Only run CPython baseline (no Molt/rustc)",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=ITERATIONS,
        help=f"Number of runtime iterations (default: {ITERATIONS})",
    )
    parser.add_argument(
        "--json", type=str, default=None, help="Write JSON results to this file"
    )
    args = parser.parse_args()

    benchmarks = args.benchmarks
    if not benchmarks:
        benchmarks = [str(REPO_ROOT / b) for b in DEFAULT_BENCHMARKS]
    else:
        # Resolve relative paths against CWD
        benchmarks = [str(Path(b).resolve()) for b in benchmarks]

    # Validate inputs
    for b in benchmarks:
        if not os.path.isfile(b):
            print(f"ERROR: benchmark file not found: {b}", file=sys.stderr)
            sys.exit(1)

    # Find rustc
    rustc = None
    if not args.cpython_only:
        rustc = _find_rustc()
        if rustc is None:
            print(
                "WARNING: rustc not found — running CPython-only mode", file=sys.stderr
            )

    print("=" * 60)
    print("Molt Rust Transpiler Benchmark Suite")
    print("=" * 60)
    print(f"Benchmarks:  {len(benchmarks)}")
    print(f"Iterations:  {args.iterations}")
    print(f"rustc:       {rustc or 'N/A'}")
    print(f"CPython:     {sys.version.split()[0]}")
    print()

    results = []
    for bench_path in benchmarks:
        name = Path(bench_path).stem
        print(f"--- {name} ---")
        res = bench_one(bench_path, rustc, args.iterations, args.cpython_only)
        results.append(res)
        print()

    # --- Summary table ---
    print("=" * 60)
    print("SUMMARY")
    print("=" * 60)
    header = f"{'Benchmark':<30} {'Transpile':>10} {'Compile':>10} {'Rust':>10} {'CPython':>10} {'Speedup':>8} {'Match':>6}"
    print(header)
    print("-" * len(header))

    for r in results:
        name = r["name"]
        transpile = f"{r['transpile_ms']:.0f}ms" if "transpile_ms" in r else "N/A"
        compile_t = f"{r['compile_ms']:.0f}ms" if "compile_ms" in r else "N/A"

        rust_mean = "N/A"
        if "rust" in r and "error" not in r["rust"]:
            rust_mean = f"{r['rust']['mean_ms']:.1f}ms"
        elif "transpile_error" in r or "compile_error" in r:
            rust_mean = "FAIL"

        cpython_mean = "N/A"
        if "cpython" in r and "error" not in r["cpython"]:
            cpython_mean = f"{r['cpython']['mean_ms']:.1f}ms"

        speedup = (
            f"{r['speedup_vs_cpython']:.1f}x" if "speedup_vs_cpython" in r else "N/A"
        )
        match = (
            "YES" if r.get("output_match") else ("NO" if "output_match" in r else "N/A")
        )

        print(
            f"{name:<30} {transpile:>10} {compile_t:>10} {rust_mean:>10} {cpython_mean:>10} {speedup:>8} {match:>6}"
        )

    # --- JSON output ---
    report = {
        "tool": "bench_rust_transpile",
        "iterations": args.iterations,
        "cpython_version": sys.version.split()[0],
        "rustc": rustc,
        "benchmarks": results,
    }

    if args.json:
        with open(args.json, "w") as f:
            json.dump(report, f, indent=2)
        print(f"\nJSON results written to {args.json}")
    else:
        print()
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
