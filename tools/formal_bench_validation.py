#!/usr/bin/env python3
"""Formal benchmark validation.

Compiles benchmark programs with --emit-ir, runs the formal bridge
analysis, and reports coverage of formal model vs real benchmark IR.

Usage:
  python3 tools/formal_bench_validation.py
  python3 tools/formal_bench_validation.py --json-out bench/results/formal_coverage.json
  python3 tools/formal_bench_validation.py --bench-dir bench/
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def find_bench_files(bench_dir: Path) -> list[Path]:
    """Find all benchmark Python files."""
    files = []
    for pattern in ["bench_*.py", "*.py"]:
        for f in sorted(bench_dir.glob(pattern)):
            if f.name.startswith("_") or f.name.startswith("."):
                continue
            # Skip non-benchmark helper files
            if f.name in {"conftest.py", "setup.py"}:
                continue
            files.append(f)
    return files


def compile_with_ir(py_path: Path, profile: str = "dev") -> tuple[Path | None, str]:
    """Compile a Python file with --emit-ir. Returns (ir_path, error)."""
    ir_file = tempfile.NamedTemporaryFile(suffix=".json", delete=False)
    ir_path = Path(ir_file.name)
    ir_file.close()

    try:
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                profile,
                "--emit-ir",
                str(ir_path),
                str(py_path),
            ],
            capture_output=True,
            text=True,
            timeout=120,
            env={**__import__("os").environ, "PYTHONPATH": str(ROOT / "src")},
        )
        if result.returncode != 0:
            ir_path.unlink(missing_ok=True)
            return None, result.stderr[:200]
        if not ir_path.exists() or ir_path.stat().st_size == 0:
            ir_path.unlink(missing_ok=True)
            return None, "empty IR output"
        return ir_path, ""
    except subprocess.TimeoutExpired:
        ir_path.unlink(missing_ok=True)
        return None, "timeout"
    except OSError as e:
        ir_path.unlink(missing_ok=True)
        return None, str(e)


def analyze_ir(ir_path: Path) -> dict:
    """Run formal bridge analysis on an IR file."""
    # Import from formal_bridge
    sys.path.insert(0, str(ROOT / "tools"))
    from formal_bridge import analyze_ir_file

    result = analyze_ir_file(ir_path)
    return result or {}


def main() -> int:
    parser = argparse.ArgumentParser(description="Formal benchmark validation")
    parser.add_argument(
        "--bench-dir", default="bench", help="Benchmark directory (default: bench/)"
    )
    parser.add_argument("--json-out", help="Write JSON results to file")
    parser.add_argument("--profile", default="dev", help="Build profile (default: dev)")
    args = parser.parse_args()

    bench_dir = ROOT / args.bench_dir
    if not bench_dir.exists():
        print(f"Benchmark directory not found: {bench_dir}", file=sys.stderr)
        return 1

    bench_files = find_bench_files(bench_dir)
    if not bench_files:
        print(f"No benchmark files found in {bench_dir}", file=sys.stderr)
        return 1

    print(f"Found {len(bench_files)} benchmark files in {bench_dir}")

    results = []
    total_ops = 0
    formal_ops = 0
    compile_failures = 0

    for py_file in bench_files:
        rel = py_file.relative_to(ROOT)
        print(f"  {rel} ... ", end="", flush=True)

        ir_path, error = compile_with_ir(py_file, args.profile)
        if ir_path is None:
            print(f"SKIP ({error[:40]})")
            compile_failures += 1
            results.append(
                {
                    "file": str(rel),
                    "status": "compile_failed",
                    "error": error,
                }
            )
            continue

        try:
            stats = analyze_ir(ir_path)
            if not stats:
                print("SKIP (empty analysis)")
                compile_failures += 1
                continue

            cov = round(stats["formalized_ops"] / max(stats["total_ops"], 1) * 100, 1)
            total_ops += stats["total_ops"]
            formal_ops += stats["formalized_ops"]
            print(f"{stats['formalized_ops']}/{stats['total_ops']} ops ({cov}% formal)")

            results.append(
                {
                    "file": str(rel),
                    "status": "ok",
                    "total_ops": stats["total_ops"],
                    "formalized_ops": stats["formalized_ops"],
                    "coverage_pct": cov,
                    "functions": stats["functions"],
                    "unformalized_kinds": stats["unformalized_kinds"],
                }
            )
        finally:
            ir_path.unlink(missing_ok=True)

    # Summary
    overall_cov = round(formal_ops / max(total_ops, 1) * 100, 1)
    print(f"\n{'=' * 60}")
    print("Formal Benchmark Validation Summary")
    print(f"{'=' * 60}")
    print(
        f"  Benchmarks analyzed: {len(bench_files) - compile_failures}"
        f"/{len(bench_files)}"
    )
    print(f"  Total operations:    {total_ops}")
    print(f"  Formalized:          {formal_ops} ({overall_cov}%)")
    print(f"  Compile failures:    {compile_failures}")

    report = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "bench_dir": str(bench_dir),
        "benchmarks": results,
        "summary": {
            "total_benchmarks": len(bench_files),
            "analyzed": len(bench_files) - compile_failures,
            "compile_failures": compile_failures,
            "total_ops": total_ops,
            "formalized_ops": formal_ops,
            "overall_coverage_pct": overall_cov,
        },
    }

    if args.json_out:
        out_path = Path(args.json_out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(report, indent=2))
        print(f"\n  Results written to: {out_path}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
