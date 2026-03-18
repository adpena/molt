"""Benchmark: Molt-compiled generator.py vs CPython execution.

Runs the Vertigo world-engine generator.py under both CPython (via subprocess)
and the Molt-compiled native binary, collecting wall-clock time and peak RSS
memory over multiple iterations.  Outputs a comparison table.

Usage:
    uv run python tests/benchmarks/bench_generator.py [--iterations N] [--molt-binary PATH]
"""

import argparse
import json
import os
import re
import subprocess
import sys
import statistics
import time
from pathlib import Path


# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parents[2]  # molt/
VERTIGO_ROOT = REPO_ROOT.parent / "vertigo"
GENERATOR_PY = VERTIGO_ROOT / "site" / "world_engine" / "generator.py"
DEFAULT_MOLT_BINARY = Path("/tmp/generator_molt")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _parse_time_l(stderr: str) -> dict:
    """Parse macOS /usr/bin/time -l output for peak RSS and wall time."""
    result = {}
    for line in stderr.splitlines():
        line = line.strip()
        # Wall clock — format:  N.NN real  N.NN user  N.NN sys
        m = re.match(r"^\s*([\d.]+)\s+real\s+([\d.]+)\s+user\s+([\d.]+)\s+sys", line)
        if m:
            result["wall_s"] = float(m.group(1))
            result["user_s"] = float(m.group(2))
            result["sys_s"] = float(m.group(3))
        # Peak RSS (bytes on macOS)
        if "maximum resident set size" in line:
            nums = re.findall(r"\d+", line)
            if nums:
                result["peak_rss_bytes"] = int(nums[0])
    return result


def run_once_cpython(python: str, script: Path) -> dict:
    """Run generator.py under CPython, timed with /usr/bin/time -l."""
    cmd = ["/usr/bin/time", "-l", python, str(script)]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if proc.returncode != 0:
        raise RuntimeError(f"CPython run failed:\n{proc.stderr}")
    metrics = _parse_time_l(proc.stderr)
    # Validate output is valid JSON
    json.loads(proc.stdout)
    metrics["output_len"] = len(proc.stdout)
    return metrics


def run_once_molt(binary: Path) -> dict:
    """Run Molt-compiled binary, timed with /usr/bin/time -l."""
    cmd = ["/usr/bin/time", "-l", str(binary)]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if proc.returncode != 0:
        raise RuntimeError(f"Molt run failed (exit {proc.returncode}):\n{proc.stderr}")
    metrics = _parse_time_l(proc.stderr)
    # Try to validate output (Molt may or may not produce identical JSON)
    stdout = proc.stdout.strip()
    if stdout:
        try:
            json.loads(stdout)
        except json.JSONDecodeError:
            pass
    metrics["output_len"] = len(stdout)
    return metrics


def fmt_time(seconds: float) -> str:
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} us"
    if seconds < 1.0:
        return f"{seconds * 1_000:.1f} ms"
    return f"{seconds:.3f} s"


def fmt_mem(rss_bytes: int) -> str:
    mb = rss_bytes / (1024 * 1024)
    return f"{mb:.1f} MB"


def collect_runs(label: str, runner, iterations: int, **kwargs) -> list[dict]:
    """Run the runner N times, printing progress."""
    results = []
    for i in range(iterations):
        sys.stdout.write(f"  {label} run {i + 1}/{iterations}...")
        sys.stdout.flush()
        m = runner(**kwargs)
        results.append(m)
        wall = m.get("wall_s")
        print(f" {fmt_time(wall) if wall else '?'}")
    return results


def summarize(results: list[dict]) -> dict:
    walls = [r["wall_s"] for r in results if "wall_s" in r]
    users = [r["user_s"] for r in results if "user_s" in r]
    rss = [r["peak_rss_bytes"] for r in results if "peak_rss_bytes" in r]
    summary = {}
    if walls:
        summary["wall_mean"] = statistics.mean(walls)
        summary["wall_min"] = min(walls)
        summary["wall_max"] = max(walls)
        summary["wall_stdev"] = statistics.stdev(walls) if len(walls) > 1 else 0.0
    if users:
        summary["user_mean"] = statistics.mean(users)
    if rss:
        summary["rss_mean"] = statistics.mean(rss)
        summary["rss_min"] = min(rss)
        summary["rss_max"] = max(rss)
    return summary


def print_table(cpython_summary: dict, molt_summary: dict, iterations: int):
    """Print a clean comparison table."""
    cp = cpython_summary
    mo = molt_summary

    speedup = cp.get("wall_mean", 0) / mo["wall_mean"] if mo.get("wall_mean") else 0
    mem_ratio = cp.get("rss_mean", 0) / mo["rss_mean"] if mo.get("rss_mean") else 0

    header = f"{'Metric':<28} {'CPython':>14} {'Molt Native':>14} {'Ratio':>10}"
    sep = "-" * len(header)
    rows = []

    def row(label, cp_val, mo_val, fmt_fn, ratio=None):
        cp_s = fmt_fn(cp_val) if cp_val else "N/A"
        mo_s = fmt_fn(mo_val) if mo_val else "N/A"
        r_s = ""
        if ratio is not None:
            r_s = f"{ratio:.2f}x"
        rows.append(f"{label:<28} {cp_s:>14} {mo_s:>14} {r_s:>10}")

    row("Wall time (mean)", cp.get("wall_mean"), mo.get("wall_mean"), fmt_time, speedup)
    row("Wall time (min)", cp.get("wall_min"), mo.get("wall_min"), fmt_time)
    row("Wall time (max)", cp.get("wall_max"), mo.get("wall_max"), fmt_time)
    row("Wall time (stdev)", cp.get("wall_stdev"), mo.get("wall_stdev"), fmt_time)
    row("User time (mean)", cp.get("user_mean"), mo.get("user_mean"), fmt_time)
    row("Peak RSS (mean)", cp.get("rss_mean"), mo.get("rss_mean"), fmt_mem, mem_ratio)
    row("Peak RSS (min)", cp.get("rss_min"), mo.get("rss_min"), fmt_mem)
    row("Peak RSS (max)", cp.get("rss_max"), mo.get("rss_max"), fmt_mem)

    print()
    print(f"=== Generator Benchmark: CPython vs Molt ({iterations} iterations) ===")
    print()
    print(header)
    print(sep)
    for r in rows:
        print(r)
    print(sep)
    if speedup:
        print(
            f"\nMolt is {speedup:.2f}x {'faster' if speedup > 1 else 'slower'} than CPython (wall-clock mean)"
        )
    if mem_ratio:
        dir_label = "less" if mem_ratio > 1 else "more"
        print(
            f"Molt uses {1 / mem_ratio:.2f}x {dir_label} memory than CPython (peak RSS mean)"
        )
    print()


def generate_markdown(
    cpython_summary: dict,
    molt_summary: dict,
    iterations: int,
    cpython_results: list[dict],
    molt_results: list[dict],
    python_version: str,
) -> str:
    """Generate a markdown report."""
    cp = cpython_summary
    mo = molt_summary
    speedup = cp.get("wall_mean", 0) / mo["wall_mean"] if mo.get("wall_mean") else 0
    mem_ratio = cp.get("rss_mean", 0) / mo["rss_mean"] if mo.get("rss_mean") else 0

    lines = []
    lines.append("# Generator Benchmark: Molt-compiled vs CPython")
    lines.append("")
    lines.append(f"**Date:** {time.strftime('%Y-%m-%d %H:%M')}")
    lines.append(
        "**Workload:** `site/world_engine/generator.py` (3D noise procedural zone generation)"
    )
    lines.append(f"**Iterations:** {iterations}")
    lines.append(f"**CPython:** {python_version}")
    lines.append(f"**Platform:** {sys.platform} ({os.uname().machine})")
    lines.append("")
    lines.append("## Results")
    lines.append("")
    lines.append("| Metric | CPython | Molt Native | Ratio |")
    lines.append("|--------|--------:|------------:|------:|")

    def md_row(label, cp_val, mo_val, fmt_fn, ratio=None):
        cp_s = fmt_fn(cp_val) if cp_val else "N/A"
        mo_s = fmt_fn(mo_val) if mo_val else "N/A"
        r_s = f"{ratio:.2f}x" if ratio is not None else ""
        lines.append(f"| {label} | {cp_s} | {mo_s} | {r_s} |")

    md_row(
        "Wall time (mean)", cp.get("wall_mean"), mo.get("wall_mean"), fmt_time, speedup
    )
    md_row("Wall time (min)", cp.get("wall_min"), mo.get("wall_min"), fmt_time)
    md_row("Wall time (max)", cp.get("wall_max"), mo.get("wall_max"), fmt_time)
    md_row("User time (mean)", cp.get("user_mean"), mo.get("user_mean"), fmt_time)
    md_row(
        "Peak RSS (mean)", cp.get("rss_mean"), mo.get("rss_mean"), fmt_mem, mem_ratio
    )
    md_row("Peak RSS (min)", cp.get("rss_min"), mo.get("rss_min"), fmt_mem)
    md_row("Peak RSS (max)", cp.get("rss_max"), mo.get("rss_max"), fmt_mem)

    lines.append("")
    lines.append("## Summary")
    lines.append("")
    if speedup:
        lines.append(
            f"- Molt is **{speedup:.2f}x {'faster' if speedup > 1 else 'slower'}** than CPython (wall-clock mean)"
        )
    if mem_ratio:
        lines.append(
            f"- Molt uses **{1 / mem_ratio:.2f}x** the memory of CPython (peak RSS mean)"
        )
    lines.append("")
    lines.append("## Raw Data")
    lines.append("")
    lines.append("### CPython runs")
    lines.append("```")
    for i, r in enumerate(cpython_results):
        lines.append(
            f"  run {i + 1}: wall={fmt_time(r.get('wall_s', 0))}  rss={fmt_mem(r.get('peak_rss_bytes', 0))}"
        )
    lines.append("```")
    lines.append("")
    lines.append("### Molt runs")
    lines.append("```")
    for i, r in enumerate(molt_results):
        lines.append(
            f"  run {i + 1}: wall={fmt_time(r.get('wall_s', 0))}  rss={fmt_mem(r.get('peak_rss_bytes', 0))}"
        )
    lines.append("```")
    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser(
        description="Benchmark Molt vs CPython on generator.py"
    )
    parser.add_argument(
        "--iterations",
        "-n",
        type=int,
        default=5,
        help="Number of runs per engine (default: 5)",
    )
    parser.add_argument(
        "--molt-binary",
        type=str,
        default=str(DEFAULT_MOLT_BINARY),
        help="Path to Molt-compiled generator binary",
    )
    parser.add_argument(
        "--generator",
        type=str,
        default=str(GENERATOR_PY),
        help="Path to generator.py source",
    )
    parser.add_argument(
        "--output-md",
        type=str,
        default=None,
        help="Path to write markdown results (default: docs/benchmarks/generator-perf.md)",
    )
    args = parser.parse_args()

    molt_binary = Path(args.molt_binary)
    generator = Path(args.generator)
    iterations = args.iterations
    md_path = (
        Path(args.output_md)
        if args.output_md
        else REPO_ROOT / "docs" / "benchmarks" / "generator-perf.md"
    )

    # Validate prerequisites
    if not generator.exists():
        print(f"ERROR: generator.py not found at {generator}", file=sys.stderr)
        sys.exit(1)
    if not molt_binary.exists():
        print(f"ERROR: Molt binary not found at {molt_binary}", file=sys.stderr)
        print("Build it first with:", file=sys.stderr)
        print(
            f"  cd {REPO_ROOT} && ARTIFACT_ROOT=${{MOLT_EXT_ROOT:-$PWD}} "
            f"MOLT_EXT_ROOT=$ARTIFACT_ROOT CARGO_TARGET_DIR=$ARTIFACT_ROOT/target "
            f"RUSTC_WRAPPER='' PYTHONPATH=src uv run python -m molt.cli build "
            f"{generator} --output {molt_binary}",
            file=sys.stderr,
        )
        sys.exit(1)

    python = sys.executable
    py_version = subprocess.run(
        [python, "--version"], capture_output=True, text=True
    ).stdout.strip()

    print(f"Generator:  {generator}")
    print(f"Molt bin:   {molt_binary}")
    print(f"CPython:    {py_version} ({python})")
    print(f"Iterations: {iterations}")
    print()

    # Warmup (1 run each, discarded)
    print("Warming up...")
    try:
        run_once_cpython(python, generator)
    except Exception as e:
        print(f"  CPython warmup failed: {e}", file=sys.stderr)
        sys.exit(1)
    try:
        run_once_molt(molt_binary)
    except Exception as e:
        print(f"  Molt warmup failed: {e}", file=sys.stderr)
        sys.exit(1)
    print()

    # Collect
    print("CPython runs:")
    cpython_results = collect_runs(
        "CPython", run_once_cpython, iterations, python=python, script=generator
    )
    print()
    print("Molt runs:")
    molt_results = collect_runs("Molt", run_once_molt, iterations, binary=molt_binary)

    # Summarize
    cp_summary = summarize(cpython_results)
    mo_summary = summarize(molt_results)

    # Print table
    print_table(cp_summary, mo_summary, iterations)

    # Write markdown
    md_path.parent.mkdir(parents=True, exist_ok=True)
    md_content = generate_markdown(
        cp_summary, mo_summary, iterations, cpython_results, molt_results, py_version
    )
    md_path.write_text(md_content)
    print(f"Markdown report written to: {md_path}")


if __name__ == "__main__":
    main()
