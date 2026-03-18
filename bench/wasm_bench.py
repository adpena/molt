#!/usr/bin/env python3
"""WASM benchmark runner for Molt (MOL-211).

Compiles a set of benchmark programs to both native and WASM, measures compile
time and binary size, and writes a JSON report suitable for CI consumption.

Usage::

    uv run python bench/wasm_bench.py
    uv run python bench/wasm_bench.py --out bench/wasm_baseline.json
    uv run python bench/wasm_bench.py --samples 5 --programs examples/hello.py
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]

# Make tools/ importable for wasm_optimize
sys.path.insert(0, str(ROOT / "tools"))

DEFAULT_PROGRAMS: list[str] = [
    "examples/hello.py",
    "examples/simple_ret.py",
    "tests/benchmarks/bench_fib.py",
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_deeply_nested_loop.py",
    "tests/benchmarks/bench_matrix_math.py",
    "tests/benchmarks/bench_str_find.py",
    "tests/benchmarks/bench_str_count.py",
    "tests/benchmarks/bench_bytes_find.py",
    "tests/benchmarks/bench_struct.py",
]


@dataclass
class CompileResult:
    ok: bool
    elapsed_s: float = 0.0
    size_bytes: int = 0
    error: str = ""


@dataclass
class OptimizeResult:
    """Result of running wasm-opt on a module."""
    ok: bool = False
    input_bytes: int = 0
    output_bytes: int = 0
    reduction_pct: float = 0.0
    elapsed_s: float = 0.0
    error: str = ""


@dataclass
class BenchEntry:
    name: str
    source: str
    wasm_samples: list[CompileResult] = field(default_factory=list)
    native_result: CompileResult | None = None
    optimize_result: OptimizeResult | None = None

    def wasm_ok(self) -> bool:
        return all(s.ok for s in self.wasm_samples) and len(self.wasm_samples) > 0

    def native_ok(self) -> bool:
        return self.native_result is not None and self.native_result.ok

    def wasm_median_s(self) -> float:
        times = [s.elapsed_s for s in self.wasm_samples if s.ok]
        return statistics.median(times) if times else 0.0

    def wasm_size_kb(self) -> float:
        sizes = [s.size_bytes for s in self.wasm_samples if s.ok]
        return (sizes[-1] / 1024) if sizes else 0.0

    def native_size_kb(self) -> float:
        if self.native_result and self.native_result.ok:
            return self.native_result.size_bytes / 1024
        return 0.0

    def size_ratio(self) -> float | None:
        if self.wasm_ok() and self.native_ok():
            ns = self.native_result.size_bytes  # type: ignore[union-attr]
            ws = self.wasm_samples[-1].size_bytes
            return ws / ns if ns > 0 else None
        return None

    def compile_speedup(self) -> float | None:
        if self.wasm_ok() and self.native_ok():
            ws = self.wasm_median_s()
            ns = self.native_result.elapsed_s  # type: ignore[union-attr]
            return ns / ws if ws > 0 else None
        return None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"source": self.source}
        if self.wasm_ok():
            d["wasm_ok"] = True
            d["wasm_compile_s_median"] = round(self.wasm_median_s(), 3)
            d["wasm_compile_s_samples"] = [
                round(s.elapsed_s, 3) for s in self.wasm_samples
            ]
            d["wasm_size_bytes"] = self.wasm_samples[-1].size_bytes
            d["wasm_size_kb"] = round(self.wasm_size_kb(), 1)
        else:
            d["wasm_ok"] = False
            errors = [s.error for s in self.wasm_samples if not s.ok and s.error]
            if errors:
                d["wasm_error"] = errors[0][:300]
        if self.native_ok():
            d["native_ok"] = True
            d["native_compile_s"] = round(self.native_result.elapsed_s, 3)  # type: ignore[union-attr]
            d["native_size_bytes"] = self.native_result.size_bytes  # type: ignore[union-attr]
            d["native_size_kb"] = round(self.native_size_kb(), 1)
        else:
            d["native_ok"] = False
        ratio = self.size_ratio()
        if ratio is not None:
            d["size_ratio_wasm_native"] = round(ratio, 3)
        speedup = self.compile_speedup()
        if speedup is not None:
            d["compile_speedup_wasm_over_native"] = round(speedup, 2)
        if self.optimize_result is not None and self.optimize_result.ok:
            d["optimized_size_bytes"] = self.optimize_result.output_bytes
            d["optimized_size_kb"] = round(self.optimize_result.output_bytes / 1024, 1)
            d["optimize_reduction_pct"] = self.optimize_result.reduction_pct
            d["optimize_elapsed_s"] = round(self.optimize_result.elapsed_s, 3)
        return d


def _compile_wasm(src: Path, out_dir: Path) -> CompileResult:
    out_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["MOLT_WASM_LINKED"] = "0"
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    t0 = time.monotonic()
    try:
        r = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                str(src),
                "--target",
                "wasm",
                "--emit",
                "wasm",
                "--out-dir",
                str(out_dir),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            env=env,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return CompileResult(ok=False, error="timeout")
    elapsed = time.monotonic() - t0
    wasm = out_dir / "output.wasm"
    if r.returncode != 0 or not wasm.exists():
        return CompileResult(
            ok=False, elapsed_s=elapsed, error=(r.stderr or r.stdout)[:300]
        )
    return CompileResult(ok=True, elapsed_s=elapsed, size_bytes=wasm.stat().st_size)


def _compile_native(src: Path, out_dir: Path) -> CompileResult:
    out_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    t0 = time.monotonic()
    try:
        r = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                str(src),
                "--out-dir",
                str(out_dir),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            env=env,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return CompileResult(ok=False, error="timeout")
    elapsed = time.monotonic() - t0
    if r.returncode != 0:
        return CompileResult(
            ok=False, elapsed_s=elapsed, error=(r.stderr or r.stdout)[:300]
        )
    for entry in out_dir.iterdir():
        if entry.is_file() and not entry.name.endswith(".json"):
            return CompileResult(
                ok=True, elapsed_s=elapsed, size_bytes=entry.stat().st_size
            )
    return CompileResult(ok=False, elapsed_s=elapsed, error="no output binary found")


def _optimize_wasm(wasm_path: Path, out_dir: Path) -> OptimizeResult:
    """Run wasm-opt on a compiled WASM module."""
    from wasm_optimize import optimize as wasm_opt_optimize

    opt_path = out_dir / "output.opt.wasm"
    result = wasm_opt_optimize(wasm_path, output_path=opt_path)
    return OptimizeResult(
        ok=bool(result["ok"]),
        input_bytes=int(result["input_bytes"]),  # type: ignore[arg-type]
        output_bytes=int(result["output_bytes"]),  # type: ignore[arg-type]
        reduction_pct=float(result["reduction_pct"]),  # type: ignore[arg-type]
        elapsed_s=float(result["elapsed_s"]),  # type: ignore[arg-type]
        error=str(result["error"]),
    )


def run_benchmarks(
    programs: list[str],
    samples: int = 3,
    skip_native: bool = False,
    do_optimize: bool = False,
) -> list[BenchEntry]:
    import tempfile

    entries: list[BenchEntry] = []
    for prog_path in programs:
        src = ROOT / prog_path
        if not src.exists():
            print(f"  SKIP {prog_path} (file not found)", file=sys.stderr)
            continue
        name = src.stem
        entry = BenchEntry(name=name, source=prog_path)
        print(f"  {name}: ", end="", flush=True)

        # WASM samples
        for i in range(samples):
            with tempfile.TemporaryDirectory(prefix=f"molt_wasm_{name}_") as td:
                result = _compile_wasm(src, Path(td))
                entry.wasm_samples.append(result)
        if entry.wasm_ok():
            print(
                f"wasm={entry.wasm_size_kb():.1f}KB "
                f"({entry.wasm_median_s():.2f}s) ",
                end="",
                flush=True,
            )
        else:
            print("wasm=FAIL ", end="", flush=True)

        # Optimization pass
        if do_optimize and entry.wasm_ok():
            with tempfile.TemporaryDirectory(prefix=f"molt_opt_{name}_") as td:
                # Re-build once to get a fresh module for optimisation
                last_ok = [s for s in entry.wasm_samples if s.ok][-1]
                # Re-compile to get the .wasm in this temp dir
                opt_result = _compile_wasm(src, Path(td))
                if opt_result.ok:
                    wasm_file = Path(td) / "output.wasm"
                    entry.optimize_result = _optimize_wasm(wasm_file, Path(td))
                    if entry.optimize_result.ok:
                        print(
                            f"opt={entry.optimize_result.output_bytes / 1024:.1f}KB "
                            f"(-{entry.optimize_result.reduction_pct}%) ",
                            end="",
                            flush=True,
                        )
                    else:
                        print(f"opt=FAIL({entry.optimize_result.error[:40]}) ", end="", flush=True)

        # Native
        if not skip_native:
            with tempfile.TemporaryDirectory(prefix=f"molt_native_{name}_") as td:
                entry.native_result = _compile_native(src, Path(td))
            if entry.native_ok():
                print(f"native={entry.native_size_kb():.1f}KB", end="")
            else:
                print("native=FAIL", end="")

            ratio = entry.size_ratio()
            speedup = entry.compile_speedup()
            if ratio is not None:
                print(f" ratio={ratio:.3f}", end="")
            if speedup is not None:
                print(f" speedup={speedup:.1f}x", end="")

        print()
        entries.append(entry)
    return entries


def build_report(entries: list[BenchEntry]) -> dict[str, Any]:
    git_rev = ""
    try:
        r = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            cwd=ROOT,
        )
        if r.returncode == 0:
            git_rev = r.stdout.strip()
    except OSError:
        pass

    return {
        "schema_version": 1,
        "created_at": time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime()),
        "git_rev": git_rev,
        "system": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "benchmarks": {e.name: e.to_dict() for e in entries},
        "notes": {
            "wasm_mode": "unlinked (MOLT_WASM_LINKED=0)",
            "native_mode": "default (Cranelift AOT)",
            "wasm_compile_s_median": "median of N samples",
            "size_ratio_wasm_native": "wasm_size / native_size (lower = smaller wasm)",
            "compile_speedup_wasm_over_native": (
                "native_time / wasm_time (higher = wasm compiles faster)"
            ),
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Molt WASM benchmark runner")
    parser.add_argument(
        "--out",
        type=Path,
        default=ROOT / "bench" / "wasm_baseline.json",
        help="Output JSON path (default: bench/wasm_baseline.json)",
    )
    parser.add_argument(
        "--samples",
        type=int,
        default=3,
        help="Number of WASM compile samples per program (default: 3)",
    )
    parser.add_argument(
        "--programs",
        nargs="*",
        default=None,
        help="Override the default program list",
    )
    parser.add_argument(
        "--skip-native",
        action="store_true",
        help="Skip native compilation (WASM-only mode)",
    )
    parser.add_argument(
        "--optimize",
        action="store_true",
        help="Run wasm-opt on each WASM output and report size reduction",
    )
    args = parser.parse_args()

    programs = args.programs if args.programs else DEFAULT_PROGRAMS
    print(f"Running WASM benchmarks ({len(programs)} programs, {args.samples} samples)")
    entries = run_benchmarks(
        programs,
        samples=args.samples,
        skip_native=args.skip_native,
        do_optimize=args.optimize,
    )

    report = build_report(entries)
    out_path: Path = args.out
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"\nReport written to {out_path}")

    # Summary
    ok_count = sum(1 for e in entries if e.wasm_ok())
    fail_count = len(entries) - ok_count
    print(f"Results: {ok_count} OK, {fail_count} FAIL out of {len(entries)} programs")


if __name__ == "__main__":
    main()
