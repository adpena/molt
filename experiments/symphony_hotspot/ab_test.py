"""A/B test: CPython vs Molt-compiled normalize_issue hotspot.

Usage:
    uv run --python 3.12 python3 experiments/symphony_hotspot/ab_test.py

Compiles the hotspot with Molt, then runs both versions and compares.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
HOTSPOT = REPO_ROOT / "experiments" / "symphony_hotspot" / "normalize_issue.py"
EXT_ROOT = Path(os.environ.get("MOLT_EXT_ROOT", "/Volumes/APDataStore/Molt"))
CACHE_DIR = EXT_ROOT / "molt_cache"
BUILD_DIR = EXT_ROOT / "experiments" / "symphony_hotspot"


def run_cpython(iterations: int = 5000, samples: int = 10) -> list[float]:
    """Run the hotspot under CPython and return wall-clock times."""
    times: list[float] = []
    for i in range(samples):
        start = time.perf_counter()
        result = subprocess.run(
            ["uv", "run", "--python", "3.12", "python3", str(HOTSPOT)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
        )
        elapsed = time.perf_counter() - start
        if result.returncode != 0:
            print(f"CPython run {i} FAILED: {result.stderr[:200]}")
            continue
        times.append(elapsed)
        # Verify output
        if "Processed 5000 issues" not in result.stdout:
            print(f"CPython run {i} unexpected output: {result.stdout[:200]}")
    return times


def compile_molt() -> Path | None:
    """Compile the hotspot with Molt, return binary path or None on failure."""
    BUILD_DIR.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["MOLT_CACHE"] = str(CACHE_DIR)
    env["CARGO_TARGET_DIR"] = str(EXT_ROOT / "cargo-target")
    env["PYTHONPATH"] = str(REPO_ROOT / "src")

    print("Compiling with Molt (--profile dev)...")
    result = subprocess.run(
        [
            "uv", "run", "--python", "3.12", "python3", "-m", "molt.cli",
            "build", "--profile", "dev", str(HOTSPOT),
        ],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        env=env,
        timeout=300,
    )
    print(f"Molt compile stdout: {result.stdout[-500:]}")
    if result.returncode != 0:
        print(f"Molt compile FAILED (exit {result.returncode}):")
        print(result.stderr[-1000:])
        return None

    # Find the binary — Molt outputs it in the build cache or current dir
    # Check common output locations
    for candidate in [
        REPO_ROOT / "normalize_issue",
        REPO_ROOT / "normalize_issue.bin",
        BUILD_DIR / "normalize_issue",
        Path(result.stdout.strip().split("\n")[-1]) if result.stdout.strip() else Path("/dev/null"),
    ]:
        if candidate.exists() and candidate.is_file():
            return candidate

    # Search for it
    import glob
    for pattern in [
        str(REPO_ROOT / "normalize_issue*"),
        str(EXT_ROOT / "**" / "normalize_issue*"),
    ]:
        matches = glob.glob(pattern, recursive=True)
        for m in matches:
            p = Path(m)
            if p.is_file() and p.suffix not in {".py", ".pyc"}:
                return p

    print("Could not find Molt binary output. Compile output was:")
    print(result.stdout)
    return None


def run_molt(binary: Path, samples: int = 10) -> list[float]:
    """Run the Molt-compiled binary and return wall-clock times."""
    times: list[float] = []
    for i in range(samples):
        start = time.perf_counter()
        result = subprocess.run(
            [str(binary)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
        )
        elapsed = time.perf_counter() - start
        if result.returncode != 0:
            print(f"Molt run {i} FAILED (exit {result.returncode}): {result.stderr[:200]}")
            continue
        times.append(elapsed)
        if "Processed 5000 issues" not in result.stdout:
            print(f"Molt run {i} unexpected output: {result.stdout[:200]}")
    return times


def stats(times: list[float]) -> dict[str, float]:
    """Compute basic statistics."""
    if not times:
        return {"mean": 0.0, "min": 0.0, "max": 0.0, "median": 0.0, "samples": 0}
    sorted_t = sorted(times)
    n = len(sorted_t)
    mean = sum(sorted_t) / n
    median = sorted_t[n // 2] if n % 2 == 1 else (sorted_t[n // 2 - 1] + sorted_t[n // 2]) / 2
    return {
        "mean": round(mean * 1000, 2),
        "min": round(sorted_t[0] * 1000, 2),
        "max": round(sorted_t[-1] * 1000, 2),
        "median": round(median * 1000, 2),
        "samples": n,
    }


def main() -> int:
    print("=" * 60)
    print("Symphony Hotspot A/B Test: CPython vs Molt")
    print("=" * 60)
    print(f"Hotspot: {HOTSPOT}")
    print("Iterations per run: 5000 normalize_issue calls")
    print()

    # Phase 1: CPython baseline
    print("--- Phase 1: CPython baseline (10 samples) ---")
    cpython_times = run_cpython(samples=10)
    cpython_stats = stats(cpython_times)
    print(f"CPython: {cpython_stats}")
    print()

    # Phase 2: Molt compilation
    print("--- Phase 2: Molt compilation ---")
    binary = compile_molt()
    if binary is None:
        print("\nMolt compilation failed. Recording failure for triage.")
        print("This itself is valuable data — identifies what Molt needs to support.")
        # Dump diagnostic info
        result = {
            "experiment": "symphony_hotspot_normalize_issue",
            "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "cpython_ms": cpython_stats,
            "molt_compiled": False,
            "molt_failure": "compilation_failed",
            "verdict": "MOLT_NEEDS_WORK",
        }
        report_path = REPO_ROOT / "experiments" / "symphony_hotspot" / "ab_result.json"
        report_path.write_text(json.dumps(result, indent=2) + "\n")
        print(f"\nResult saved to {report_path}")
        return 1

    print(f"Molt binary: {binary} ({binary.stat().st_size} bytes)")
    print()

    # Phase 3: Molt runtime
    print("--- Phase 3: Molt runtime (10 samples) ---")
    molt_times = run_molt(binary, samples=10)
    molt_stats = stats(molt_times)
    print(f"Molt:    {molt_stats}")
    print()

    # Phase 4: Comparison
    print("--- Phase 4: A/B Comparison ---")
    if cpython_stats["mean"] > 0 and molt_stats["mean"] > 0:
        speedup = cpython_stats["mean"] / molt_stats["mean"]
        print(f"CPython mean: {cpython_stats['mean']:.2f} ms")
        print(f"Molt mean:    {molt_stats['mean']:.2f} ms")
        print(f"Speedup:      {speedup:.2f}x")
        if speedup > 1.0:
            verdict = f"MOLT_FASTER ({speedup:.1f}x)"
        elif speedup > 0.9:
            verdict = "PARITY (within 10%)"
        else:
            verdict = f"CPYTHON_FASTER ({1/speedup:.1f}x)"
        print(f"Verdict:      {verdict}")
    else:
        verdict = "INSUFFICIENT_DATA"
        speedup = 0.0

    # Save result
    result = {
        "experiment": "symphony_hotspot_normalize_issue",
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "iterations_per_run": 5000,
        "samples": 10,
        "cpython_ms": cpython_stats,
        "molt_ms": molt_stats,
        "speedup": round(speedup, 3),
        "verdict": verdict,
        "binary_size_bytes": binary.stat().st_size if binary else 0,
    }
    report_path = REPO_ROOT / "experiments" / "symphony_hotspot" / "ab_result.json"
    report_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nResult saved to {report_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
