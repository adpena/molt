"""Benchmark audit tool for Project TITAN Phase 0.

Runs each benchmark through both Molt (native) and CPython, computes speedup,
and classifies results as Green/Yellow/Red.

Usage:
    python3 tools/bench_audit.py [--samples N] [--timeout S] [--out FILE]
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Re-use infrastructure from bench.py where possible
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).parent.parent.resolve()

# Import bench.py helpers directly
sys.path.insert(0, str(REPO_ROOT / "tools"))
try:
    import bench as _bench

    prepare_molt_binary = _bench.prepare_molt_binary
    measure_molt_run = _bench.measure_molt_run
    BENCHMARKS = _bench.BENCHMARKS
    MOLT_ARGS_BY_BENCH = _bench.MOLT_ARGS_BY_BENCH
except ImportError as _err:
    print(f"Warning: could not import bench.py helpers: {_err}", file=sys.stderr)
    prepare_molt_binary = None  # type: ignore[assignment]
    measure_molt_run = None  # type: ignore[assignment]
    BENCHMARKS = []
    MOLT_ARGS_BY_BENCH = {}

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

DEFAULT_SAMPLES = 5
DEFAULT_TIMEOUT_S = 60.0

# Speedup thresholds
GREEN_THRESHOLD = 1.0   # Molt is at least as fast as CPython
YELLOW_THRESHOLD = 0.5  # Molt is within 2x of CPython

RESULTS_DIR = REPO_ROOT / "benchmarks" / "results"
DEFAULT_OUTPUT = RESULTS_DIR / "audit_baseline.json"


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

@dataclass
class BenchResult:
    name: str
    molt_s: Optional[float]
    cpython_s: Optional[float]
    speedup: Optional[float]
    classification: str  # "Green", "Yellow", "Red", "Error"
    error: Optional[str] = None
    molt_build_s: Optional[float] = None
    perf_data: Optional[dict] = None


# ---------------------------------------------------------------------------
# Timing helpers
# ---------------------------------------------------------------------------

def _median_time(
    cmd: list[str],
    samples: int,
    timeout_s: float,
    *,
    label: str = "",
) -> Optional[float]:
    """Run *cmd* *samples* times and return the median elapsed time.

    Returns None if any run fails or times out.
    """
    times: list[float] = []
    for _ in range(samples):
        start = time.perf_counter()
        try:
            res = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=timeout_s,
            )
        except subprocess.TimeoutExpired:
            print(
                f"  [timeout] {label or ' '.join(cmd[:3])} timed out after {timeout_s:.1f}s",
                file=sys.stderr,
            )
            return None
        elapsed = time.perf_counter() - start
        if res.returncode != 0:
            err_snippet = (res.stderr or res.stdout or "").strip()[:200]
            print(
                f"  [error] {label or cmd[0]} exited {res.returncode}: {err_snippet}",
                file=sys.stderr,
            )
            return None
        times.append(elapsed)
    return statistics.median(times)


def _cpython_median(
    script: str,
    samples: int,
    timeout_s: float,
) -> Optional[float]:
    python_exe = sys.executable  # same interpreter that's running us
    return _median_time(
        [python_exe, script],
        samples,
        timeout_s,
        label=f"cpython:{Path(script).name}",
    )


def _molt_median(
    script: str,
    samples: int,
    timeout_s: float,
    extra_args: Optional[list[str]] = None,
    *,
    skip_molt: bool = False,
) -> tuple[Optional[float], Optional[float]]:
    """Build the Molt binary once, then time *samples* runs.

    Returns (median_run_s, build_s).  Both are None on failure.
    """
    if skip_molt or prepare_molt_binary is None or measure_molt_run is None:
        return None, None

    binary_info = prepare_molt_binary(script, extra_args=extra_args)
    if binary_info is None:
        return None, None

    build_s = binary_info.build_s
    times: list[float] = []
    label = f"molt:{Path(script).name}"
    for _ in range(samples):
        t = measure_molt_run(binary_info.path, label=label, timeout_s=timeout_s)
        if t is None:
            return None, build_s
        times.append(t)

    # Cleanup temp dir
    try:
        binary_info.temp_dir.cleanup()
    except Exception:
        pass

    return statistics.median(times), build_s


# ---------------------------------------------------------------------------
# perf counter collection (Linux only)
# ---------------------------------------------------------------------------

def _collect_perf_data(binary_path: Path) -> Optional[dict]:
    """Collect instruction count and cache-miss data via `perf stat`."""
    if platform.system() != "Linux":
        return None
    perf = shutil.which("perf")
    if perf is None:
        return None
    events = "instructions,cache-misses,cache-references"
    try:
        res = subprocess.run(
            [perf, "stat", "-e", events, "--", str(binary_path)],
            capture_output=True,
            text=True,
            timeout=30.0,
        )
    except Exception:
        return None
    if res.returncode != 0:
        return None
    # Parse the perf stat stderr output
    data: dict = {}
    for line in res.stderr.splitlines():
        line = line.strip()
        for key, token in [
            ("instructions", "instructions"),
            ("cache_misses", "cache-misses"),
            ("cache_references", "cache-references"),
        ]:
            if token in line:
                parts = line.split()
                if parts:
                    try:
                        data[key] = int(parts[0].replace(",", "").replace(".", ""))
                    except ValueError:
                        pass
    return data or None


# ---------------------------------------------------------------------------
# Classification
# ---------------------------------------------------------------------------

def _classify(speedup: Optional[float]) -> str:
    if speedup is None:
        return "Error"
    if speedup >= GREEN_THRESHOLD:
        return "Green"
    if speedup >= YELLOW_THRESHOLD:
        return "Yellow"
    return "Red"


# ---------------------------------------------------------------------------
# Core audit loop
# ---------------------------------------------------------------------------

def audit_benchmarks(
    benchmarks: list[str],
    samples: int = DEFAULT_SAMPLES,
    timeout_s: float = DEFAULT_TIMEOUT_S,
    verbose: bool = False,
    skip_molt: bool = False,
) -> list[BenchResult]:
    results: list[BenchResult] = []

    for i, bench_path in enumerate(benchmarks, 1):
        # Resolve relative to repo root
        script = str(REPO_ROOT / bench_path) if not os.path.isabs(bench_path) else bench_path
        name = Path(script).stem
        extra_args = MOLT_ARGS_BY_BENCH.get(bench_path)

        print(f"[{i:2d}/{len(benchmarks)}] {name} ...", flush=True)

        # --- CPython timing ---
        cpython_s = _cpython_median(script, samples, timeout_s)

        # --- Molt timing ---
        molt_s, build_s = _molt_median(
            script, samples, timeout_s, extra_args=extra_args, skip_molt=skip_molt
        )

        # --- Speedup ---
        speedup: Optional[float] = None
        if molt_s is not None and cpython_s is not None and molt_s > 0:
            speedup = cpython_s / molt_s

        if skip_molt and cpython_s is not None:
            classification = "Skipped"
        else:
            classification = _classify(speedup)
        color_tag = {"Green": "\033[32m", "Yellow": "\033[33m", "Red": "\033[31m"}.get(
            classification, "\033[90m"
        )
        reset = "\033[0m"

        if speedup is not None:
            print(
                f"  CPython: {cpython_s:.4f}s  Molt: {molt_s:.4f}s  "
                f"Speedup: {speedup:.2f}x  {color_tag}{classification}{reset}"
            )
        elif skip_molt and cpython_s is not None:
            print(f"  CPython: {cpython_s:.4f}s  (Molt skipped)")
        else:
            reason_parts = []
            if cpython_s is None:
                reason_parts.append("CPython failed")
            if molt_s is None and not skip_molt:
                reason_parts.append("Molt failed")
            print(f"  {color_tag}{classification}{reset}: {', '.join(reason_parts) or 'unknown'}")

        perf_data: Optional[dict] = None
        if (
            not skip_molt
            and platform.system() == "Linux"
            and shutil.which("perf")
            and molt_s is not None
        ):
            # We need the binary path — re-build for perf collection
            if prepare_molt_binary is not None:
                bin_info = prepare_molt_binary(script, extra_args=extra_args)
                if bin_info is not None:
                    perf_data = _collect_perf_data(bin_info.path)
                    try:
                        bin_info.temp_dir.cleanup()
                    except Exception:
                        pass
        elif platform.system() == "Darwin":
            if verbose:
                print(
                    "  [note] macOS: perf counters require Instruments (not automated)"
                )

        results.append(
            BenchResult(
                name=name,
                molt_s=molt_s,
                cpython_s=cpython_s,
                speedup=speedup,
                classification=classification,
                error=None,
                molt_build_s=build_s,
                perf_data=perf_data,
            )
        )

    return results


# ---------------------------------------------------------------------------
# Output / summary
# ---------------------------------------------------------------------------

def _to_json_record(r: BenchResult) -> dict:
    record: dict = {
        "name": r.name,
        "molt_s": round(r.molt_s, 6) if r.molt_s is not None else None,
        "cpython_s": round(r.cpython_s, 6) if r.cpython_s is not None else None,
        "speedup": round(r.speedup, 4) if r.speedup is not None else None,
        "class": r.classification,
    }
    if r.molt_build_s is not None:
        record["molt_build_s"] = round(r.molt_build_s, 4)
    if r.error is not None:
        record["error"] = r.error
    if r.perf_data:
        record["perf"] = r.perf_data
    return record


def print_summary(results: list[BenchResult]) -> None:
    green = [r for r in results if r.classification == "Green"]
    yellow = [r for r in results if r.classification == "Yellow"]
    red = [r for r in results if r.classification == "Red"]
    error = [r for r in results if r.classification == "Error"]
    skipped = [r for r in results if r.classification == "Skipped"]

    print()
    print("=" * 72)
    print("BENCHMARK AUDIT SUMMARY")
    print("=" * 72)
    print(f"  Total:   {len(results)}")
    print(f"  \033[32mGreen\033[0m (>= 1x faster):  {len(green)}")
    print(f"  \033[33mYellow\033[0m (0.5x–1x):       {len(yellow)}")
    print(f"  \033[31mRed\033[0m (< 0.5x):           {len(red)}")
    print(f"  Error:               {len(error)}")
    if skipped:
        print(f"  Skipped (--cpython-only): {len(skipped)}")
    print()

    if green:
        print("Green benchmarks (Molt wins):")
        for r in sorted(green, key=lambda x: -(x.speedup or 0)):
            print(f"  {r.name:<40s} {r.speedup:.2f}x")

    if yellow:
        print()
        print("Yellow benchmarks (within 2x):")
        for r in sorted(yellow, key=lambda x: -(x.speedup or 0)):
            print(f"  {r.name:<40s} {r.speedup:.2f}x")

    if red:
        print()
        print("Red benchmarks (Molt > 2x slower):")
        for r in sorted(red, key=lambda x: (x.speedup or 0)):
            print(f"  {r.name:<40s} {r.speedup:.2f}x")

    if error:
        print()
        print("Errors / skipped:")
        for r in error:
            msg = r.error or "(no details)"
            print(f"  {r.name:<40s} {msg}")

    if skipped:
        print()
        print("CPython-only baseline (Molt skipped):")
        for r in skipped:
            t = f"{r.cpython_s:.4f}s" if r.cpython_s is not None else "failed"
            print(f"  {r.name:<40s} CPython: {t}")

    print("=" * 72)

    # Geometric mean of speedups for benchmarks that completed
    completed = [r for r in results if r.speedup is not None]
    if completed:
        import math
        log_sum = sum(math.log(r.speedup) for r in completed)
        geo_mean = math.exp(log_sum / len(completed))
        print(f"Geometric mean speedup (n={len(completed)}): {geo_mean:.3f}x")

    print()


def save_results(results: list[BenchResult], output_path: Path) -> None:
    output_path.parent.mkdir(parents=True, exist_ok=True)
    records = [_to_json_record(r) for r in results]
    output_path.write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    print(f"Results saved to {output_path}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Benchmark audit tool: compare Molt vs CPython on all benchmarks.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument(
        "--samples",
        type=int,
        default=DEFAULT_SAMPLES,
        metavar="N",
        help=f"Number of timing samples per benchmark (default: {DEFAULT_SAMPLES})",
    )
    p.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_S,
        metavar="SECONDS",
        help=f"Per-run timeout in seconds (default: {DEFAULT_TIMEOUT_S:.0f})",
    )
    p.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUTPUT,
        metavar="FILE",
        help=f"JSON output path (default: {DEFAULT_OUTPUT})",
    )
    p.add_argument(
        "--filter",
        metavar="PATTERN",
        help="Only run benchmarks whose name contains PATTERN",
    )
    p.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Print extra diagnostic information",
    )
    p.add_argument(
        "--cpython-only",
        action="store_true",
        help="Skip Molt runs (useful for baseline collection without the compiler)",
    )
    p.add_argument(
        "--list",
        action="store_true",
        help="List available benchmarks and exit",
    )
    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    # Build the list of benchmark paths
    benchmarks = list(BENCHMARKS)
    if not benchmarks:
        # Fallback: discover from tests/benchmarks/
        bench_dir = REPO_ROOT / "tests" / "benchmarks"
        benchmarks = sorted(
            str(p.relative_to(REPO_ROOT))
            for p in bench_dir.glob("bench_*.py")
        )

    if args.filter:
        benchmarks = [b for b in benchmarks if args.filter in Path(b).stem]

    if args.list:
        print(f"Available benchmarks ({len(benchmarks)}):")
        for b in benchmarks:
            print(f"  {b}")
        return 0

    print(f"Benchmark audit: {len(benchmarks)} benchmarks, {args.samples} samples each")
    print(f"Platform: {platform.system()} {platform.machine()}")
    print(f"CPython: {sys.version.split()[0]}")

    if platform.system() == "Darwin":
        print("Note: perf counters require Instruments on macOS (not automated here)")
    elif platform.system() == "Linux" and shutil.which("perf"):
        print("Note: perf counters will be collected via 'perf stat'")

    print()

    if args.cpython_only:
        print("--cpython-only mode: Molt runs will be skipped")

    results = audit_benchmarks(
        benchmarks,
        samples=args.samples,
        timeout_s=args.timeout,
        verbose=args.verbose,
        skip_molt=args.cpython_only,
    )

    print_summary(results)
    save_results(results, args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
