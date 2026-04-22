#!/usr/bin/env python3
"""Full WASM optimization pipeline for Molt (MOL-211).

Orchestrates the complete compile -> link -> optimize pipeline:

1. Compile Python source to unlinked WASM via ``molt build --emit wasm``
2. Link with ``wasm-ld --gc-sections`` to strip dead code (via wasm_link.py)
3. Optimize with ``wasm-opt -O2`` to shrink binary (via wasm_optimize.py)
4. Optionally run with ``wasmtime`` (for WASI-compatible modules)
5. Report size at each stage

Usage::

    python tools/wasm_pipeline.py path/to/script.py
    python tools/wasm_pipeline.py path/to/script.py --opt-level Oz
    python tools/wasm_pipeline.py path/to/script.py --run
    python tools/wasm_pipeline.py path/to/script.py --benchmark  # run 5 programs
    python tools/wasm_pipeline.py path/to/script.py --json
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Project layout
# ---------------------------------------------------------------------------

MOLT_ROOT = Path(__file__).resolve().parent.parent
TOOLS_DIR = MOLT_ROOT / "tools"
SRC_DIR = MOLT_ROOT / "src"


def _wasm_runtime_root() -> Path:
    env_root = os.environ.get("MOLT_WASM_RUNTIME_DIR")
    if env_root:
        return Path(env_root).expanduser()
    artifact_root = os.environ.get("MOLT_EXT_ROOT", "").strip()
    if artifact_root:
        return Path(artifact_root).expanduser() / "wasm"
    return MOLT_ROOT / "wasm"


RUNTIME_DIR = _wasm_runtime_root()
RUNTIME_WASM = RUNTIME_DIR / "molt_runtime.wasm"
RUNTIME_RELOC = RUNTIME_DIR / "molt_runtime_reloc.wasm"


def _ensure_repo_pythonpath() -> None:
    src = str(SRC_DIR)
    current = os.environ.get("PYTHONPATH", "")
    if current:
        parts = current.split(os.pathsep)
        if src not in parts:
            os.environ["PYTHONPATH"] = src + os.pathsep + current
    else:
        os.environ["PYTHONPATH"] = src


# Benchmark programs (relative to MOLT_ROOT)
BENCHMARK_PROGRAMS = [
    ("hello", 'print("hello world")'),
    (
        "fib",
        """\
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(20))
""",
    ),
    (
        "sum_range",
        """\
total = 0
for i in range(1000):
    total += i
print(total)
""",
    ),
    (
        "list_ops",
        """\
xs = list(range(100))
xs.reverse()
print(sum(xs))
""",
    ),
    (
        "nested_loop",
        """\
total = 0
for i in range(50):
    for j in range(50):
        total += i * j
print(total)
""",
    ),
]


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------


@dataclass
class StageResult:
    """Result of a single pipeline stage."""

    name: str
    ok: bool
    size_bytes: int = 0
    elapsed_s: float = 0.0
    output_path: str = ""
    error: str = ""

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "name": self.name,
            "ok": self.ok,
            "size_bytes": self.size_bytes,
        }
        if self.elapsed_s:
            d["elapsed_s"] = round(self.elapsed_s, 3)
        if self.output_path:
            d["output_path"] = self.output_path
        if self.error:
            d["error"] = self.error
        return d


@dataclass
class PipelineResult:
    """Result of the full pipeline run."""

    source: str
    stages: list[StageResult] = field(default_factory=list)
    run_output: str = ""
    run_ok: bool | None = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "source": self.source,
            "stages": [s.to_dict() for s in self.stages],
        }
        if self.run_ok is not None:
            d["run_ok"] = self.run_ok
            d["run_output"] = self.run_output
        # Summary
        sizes = {s.name: s.size_bytes for s in self.stages if s.ok and s.size_bytes > 0}
        if len(sizes) >= 2:
            first_size = next(iter(sizes.values()))
            last_size = list(sizes.values())[-1]
            if first_size > 0:
                d["total_reduction_pct"] = round((1 - last_size / first_size) * 100, 1)
        d["sizes"] = sizes
        return d


# ---------------------------------------------------------------------------
# Tool finders
# ---------------------------------------------------------------------------


def _find_tool(name: str) -> str | None:
    return shutil.which(name)


def _molt_build_cmd() -> list[str]:
    """Return the command prefix for invoking ``molt build``."""
    _ensure_repo_pythonpath()
    # Try the installed binary first
    molt = _find_tool("molt")
    if molt:
        return [molt, "build"]
    # Fall back to running through uv/python with PYTHONPATH
    uv = _find_tool("uv")
    if uv:
        return [uv, "run", "python", "-m", "molt.cli", "build"]
    return [sys.executable, "-m", "molt.cli", "build"]


# ---------------------------------------------------------------------------
# Pipeline stages
# ---------------------------------------------------------------------------


def stage_compile(
    source: Path,
    out_dir: Path,
    *,
    linked: bool = False,
    verbose: bool = False,
) -> StageResult:
    """Stage 1: Compile Python to WASM via molt build."""
    t0 = time.monotonic()
    cmd = _molt_build_cmd() + [
        str(source),
        "--emit",
        "wasm",
        "--target",
        "wasm",
        "--out-dir",
        str(out_dir),
    ]

    env = os.environ.copy()

    if linked:
        cmd.append("--linked")
    else:
        # Produce unlinked output so we can link separately
        cmd.append("--no-linked")
        env["MOLT_WASM_LINKED"] = "0"

    if verbose:
        cmd.append("--verbose")

    proc = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=300,
        cwd=MOLT_ROOT,
        env=env,
    )
    elapsed = time.monotonic() - t0

    output_wasm = out_dir / "output.wasm"
    linked_wasm = out_dir / "output_linked.wasm"

    if proc.returncode != 0:
        err = proc.stderr.strip() or proc.stdout.strip()
        return StageResult(
            name="compile",
            ok=False,
            elapsed_s=elapsed,
            error=err[:500],
        )

    # Prefer linked output if it exists
    result_path = linked_wasm if linked_wasm.exists() else output_wasm
    if not result_path.exists():
        return StageResult(
            name="compile",
            ok=False,
            elapsed_s=elapsed,
            error="Compilation succeeded but no .wasm output found",
        )

    size = result_path.stat().st_size
    return StageResult(
        name="compile",
        ok=True,
        size_bytes=size,
        elapsed_s=elapsed,
        output_path=str(result_path),
    )


def stage_link(
    input_wasm: Path,
    output_wasm: Path,
    *,
    runtime: Path | None = None,
) -> StageResult:
    """Stage 2: Link with wasm-ld via tools/wasm_link.py.

    If the output module is already a final (non-relocatable) module,
    wasm-ld cannot link it further. In that case, this stage is skipped
    gracefully.
    """
    if runtime is None:
        runtime = RUNTIME_RELOC if RUNTIME_RELOC.exists() else RUNTIME_WASM

    if not runtime.exists():
        return StageResult(
            name="link",
            ok=False,
            error=f"Runtime WASM not found: {runtime}",
        )

    link_tool = TOOLS_DIR / "wasm_link.py"
    if not link_tool.exists():
        return StageResult(
            name="link",
            ok=False,
            error="tools/wasm_link.py not found",
        )

    t0 = time.monotonic()
    proc = subprocess.run(
        [
            sys.executable,
            str(link_tool),
            "--runtime",
            str(runtime),
            "--input",
            str(input_wasm),
            "--output",
            str(output_wasm),
        ],
        capture_output=True,
        text=True,
        timeout=300,
        cwd=MOLT_ROOT,
    )
    elapsed = time.monotonic() - t0

    if proc.returncode != 0:
        err = proc.stderr.strip() or proc.stdout.strip()
        # Non-relocatable files cannot be linked — this is expected for
        # modules compiled without MOLT_WASM_LINK=1.
        if "not a relocatable wasm file" in err:
            return StageResult(
                name="link",
                ok=False,
                elapsed_s=elapsed,
                error="Input is not relocatable (skipped — compile with --linked for wasm-ld linking)",
            )
        return StageResult(
            name="link",
            ok=False,
            elapsed_s=elapsed,
            error=err[:500],
        )

    if not output_wasm.exists():
        return StageResult(
            name="link",
            ok=False,
            elapsed_s=elapsed,
            error="wasm-ld produced no output",
        )

    size = output_wasm.stat().st_size
    return StageResult(
        name="link",
        ok=True,
        size_bytes=size,
        elapsed_s=elapsed,
        output_path=str(output_wasm),
    )


def stage_optimize(
    input_wasm: Path,
    output_wasm: Path,
    *,
    level: str = "O2",
) -> StageResult:
    """Stage 3: Optimize with wasm-opt."""
    wasm_opt = _find_tool("wasm-opt")
    if not wasm_opt:
        return StageResult(
            name="optimize",
            ok=False,
            error="wasm-opt not found (install binaryen: brew install binaryen)",
        )

    t0 = time.monotonic()
    proc = subprocess.run(
        [
            wasm_opt,
            f"-{level}",
            # Explicit features — avoid --all-features which enables
            # --enable-custom-descriptors, causing `exact` heap types
            # that Cloudflare Workers' V8 rejects.
            "--enable-bulk-memory",
            "--enable-mutable-globals",
            "--enable-sign-ext",
            "--enable-nontrapping-float-to-int",
            "--enable-simd",
            "--enable-multivalue",
            "--enable-reference-types",
            "--enable-gc",
            "--enable-tail-call",
            "--disable-custom-descriptors",
            str(input_wasm),
            "-o",
            str(output_wasm),
        ],
        capture_output=True,
        text=True,
        timeout=300,
    )
    elapsed = time.monotonic() - t0

    if proc.returncode != 0:
        err = proc.stderr.strip() or proc.stdout.strip()
        return StageResult(
            name="optimize",
            ok=False,
            elapsed_s=elapsed,
            error=err[:500],
        )

    if not output_wasm.exists():
        return StageResult(
            name="optimize",
            ok=False,
            elapsed_s=elapsed,
            error="wasm-opt produced no output",
        )

    size = output_wasm.stat().st_size
    return StageResult(
        name="optimize",
        ok=True,
        size_bytes=size,
        elapsed_s=elapsed,
        output_path=str(output_wasm),
    )


def stage_run(wasm_path: Path, *, timeout: int = 30) -> tuple[bool, str]:
    """Stage 4 (optional): Run with wasmtime."""
    wasmtime = _find_tool("wasmtime")
    if not wasmtime:
        return False, "wasmtime not found (brew install wasmtime)"

    try:
        proc = subprocess.run(
            [wasmtime, str(wasm_path)],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        output = proc.stdout.strip()
        if proc.returncode != 0:
            err = proc.stderr.strip()
            return False, f"Exit {proc.returncode}: {err[:300]}"
        return True, output
    except subprocess.TimeoutExpired:
        return False, f"Timed out after {timeout}s"


# ---------------------------------------------------------------------------
# Full pipeline
# ---------------------------------------------------------------------------


def run_pipeline(
    source: Path,
    *,
    opt_level: str = "O2",
    do_run: bool = False,
    linked: bool = False,
    verbose: bool = False,
) -> PipelineResult:
    """Run the full compile -> link -> optimize pipeline."""
    result = PipelineResult(source=str(source))

    with tempfile.TemporaryDirectory(prefix="molt-pipeline-") as tmpdir:
        work = Path(tmpdir)

        # Stage 1: Compile
        compile_out = work / "compile"
        compile_out.mkdir()
        compile_result = stage_compile(
            source, compile_out, linked=linked, verbose=verbose
        )
        result.stages.append(compile_result)
        if not compile_result.ok:
            return result

        current_wasm = Path(compile_result.output_path)

        # Stage 2: Link (only if compiled as relocatable with --linked)
        if linked:
            linked_wasm = work / "output_linked.wasm"
            link_result = stage_link(current_wasm, linked_wasm)
            result.stages.append(link_result)
            if link_result.ok:
                current_wasm = Path(link_result.output_path)
            # If link fails due to non-relocatable, continue with compile output
        else:
            # Try linking anyway — it will gracefully report skip if non-relocatable
            linked_wasm = work / "output_linked.wasm"
            link_result = stage_link(current_wasm, linked_wasm)
            result.stages.append(link_result)
            if link_result.ok:
                current_wasm = Path(link_result.output_path)

        # Stage 3: Optimize
        optimized_wasm = work / "output_optimized.wasm"
        opt_result = stage_optimize(current_wasm, optimized_wasm, level=opt_level)
        result.stages.append(opt_result)
        if opt_result.ok:
            current_wasm = Path(opt_result.output_path)

        # Stage 4: Optionally run
        if do_run:
            run_ok, run_output = stage_run(current_wasm)
            result.run_ok = run_ok
            result.run_output = run_output

    return result


# ---------------------------------------------------------------------------
# Benchmark suite
# ---------------------------------------------------------------------------


def run_benchmark_suite(
    *,
    opt_level: str = "O2",
    verbose: bool = False,
    json_output: bool = False,
) -> list[PipelineResult]:
    """Run the pipeline on several benchmark programs and compare sizes."""
    results: list[PipelineResult] = []

    with tempfile.TemporaryDirectory(prefix="molt-bench-") as tmpdir:
        for name, code in BENCHMARK_PROGRAMS:
            src_path = Path(tmpdir) / f"{name}.py"
            src_path.write_text(code)

            if not json_output:
                print(f"\n{'=' * 60}")
                print(f"  Benchmark: {name}")
                print(f"{'=' * 60}")

            result = run_pipeline(
                src_path,
                opt_level=opt_level,
                verbose=verbose,
            )
            results.append(result)

            if not json_output:
                _print_pipeline_report(result)

    if not json_output:
        _print_comparison_table(results)

    return results


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def _fmt_size(size_bytes: int) -> str:
    if size_bytes == 0:
        return "—"
    if size_bytes < 1024:
        return f"{size_bytes} B"
    if size_bytes < 1024 * 1024:
        return f"{size_bytes / 1024:.1f} KB"
    return f"{size_bytes / 1024 / 1024:.2f} MB"


def _print_pipeline_report(result: PipelineResult) -> None:
    """Print a human-readable report for a single pipeline run."""
    for stage in result.stages:
        status = "OK" if stage.ok else "FAIL"
        size_str = _fmt_size(stage.size_bytes) if stage.size_bytes else "—"
        time_str = f"{stage.elapsed_s:.2f}s" if stage.elapsed_s else ""
        print(f"  [{status:>4s}] {stage.name:<12s}  {size_str:>12s}  {time_str}")
        if not stage.ok and stage.error:
            # Truncate for readability
            err = stage.error
            if len(err) > 120:
                err = err[:117] + "..."
            print(f"         {err}")

    # Size reduction summary
    sizes = [s.size_bytes for s in result.stages if s.ok and s.size_bytes > 0]
    if len(sizes) >= 2:
        reduction = (1 - sizes[-1] / sizes[0]) * 100
        print(
            f"  Total reduction: {reduction:.1f}% ({_fmt_size(sizes[0])} -> {_fmt_size(sizes[-1])})"
        )

    if result.run_ok is not None:
        status = "OK" if result.run_ok else "FAIL"
        print(
            f"  [{'OK' if result.run_ok else 'FAIL':>4s}] run           {result.run_output[:80]}"
        )


def _print_comparison_table(results: list[PipelineResult]) -> None:
    """Print a comparison table across all benchmark programs."""
    print(f"\n{'=' * 72}")
    print("  BENCHMARK COMPARISON")
    print(f"{'=' * 72}")
    print(
        f"  {'Program':<16s} {'Compiled':>12s} {'Linked':>12s} {'Optimized':>12s} {'Reduction':>10s}"
    )
    print(f"  {'-' * 16} {'-' * 12} {'-' * 12} {'-' * 12} {'-' * 10}")

    for result in results:
        name = Path(result.source).stem
        sizes: dict[str, int] = {}
        for stage in result.stages:
            if stage.ok and stage.size_bytes > 0:
                sizes[stage.name] = stage.size_bytes

        compiled = _fmt_size(sizes.get("compile", 0))
        linked = _fmt_size(sizes.get("link", 0))
        optimized = _fmt_size(sizes.get("optimize", 0))

        first = sizes.get("compile", 0)
        last = list(sizes.values())[-1] if sizes else 0
        if first > 0 and last > 0:
            reduction = f"{(1 - last / first) * 100:.1f}%"
        else:
            reduction = "—"

        print(
            f"  {name:<16s} {compiled:>12s} {linked:>12s} {optimized:>12s} {reduction:>10s}"
        )

    print()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Full WASM optimization pipeline for Molt-compiled modules."
    )
    parser.add_argument(
        "source",
        nargs="?",
        type=Path,
        help="Python source file to compile",
    )
    parser.add_argument(
        "--opt-level",
        default="O2",
        choices=["O1", "O2", "O3", "O4", "Os", "Oz"],
        help="wasm-opt optimization level (default: O2)",
    )
    parser.add_argument(
        "--run",
        action="store_true",
        help="Run the final WASM with wasmtime after optimization",
    )
    parser.add_argument(
        "--linked",
        action="store_true",
        help="Compile in relocatable mode and link with wasm-ld",
    )
    parser.add_argument(
        "--benchmark",
        action="store_true",
        help="Run pipeline on 5 benchmark programs and compare sizes",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Machine-readable JSON output",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Verbose compilation output",
    )
    args = parser.parse_args()

    if args.benchmark:
        results = run_benchmark_suite(
            opt_level=args.opt_level,
            verbose=args.verbose,
            json_output=args.json_output,
        )
        if args.json_output:
            print(json.dumps([r.to_dict() for r in results], indent=2))
        return

    if not args.source:
        parser.error("Provide a source file or use --benchmark")

    if not args.source.is_file():
        print(f"ERROR: {args.source} not found", file=sys.stderr)
        sys.exit(1)

    result = run_pipeline(
        args.source,
        opt_level=args.opt_level,
        do_run=args.run,
        linked=args.linked,
        verbose=args.verbose,
    )

    if args.json_output:
        print(json.dumps(result.to_dict(), indent=2))
    else:
        _print_pipeline_report(result)

    # Exit non-zero if compile stage failed
    if not result.stages or not result.stages[0].ok:
        sys.exit(1)


if __name__ == "__main__":
    main()
