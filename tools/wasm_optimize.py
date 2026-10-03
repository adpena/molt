#!/usr/bin/env python3
"""Post-build WASM optimization via Binaryen's wasm-opt (MOL-211).

Runs ``wasm-opt -O2`` on a Molt-generated ``.wasm`` module to shrink binary
size without changing semantics.  Designed to be called standalone or
integrated into ``molt build --emit wasm --optimize``.

Usage::

    python tools/wasm_optimize.py path/to/module.wasm
    python tools/wasm_optimize.py path/to/module.wasm -o optimized.wasm
    python tools/wasm_optimize.py path/to/module.wasm --level Oz  # size-focused
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from collections.abc import Sequence
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from tools import harness_memory_guard  # noqa: E402
from molt import artifact_publication  # noqa: E402
from tools.wasm_metrics import wasm_metrics  # noqa: E402
from molt.wasm_artifact import parse_wasm_exports  # noqa: E402
from molt.wasm_optimization import (  # noqa: E402
    WASM_OPT_LEVELS,
    wasm_opt_pipeline,
)
from molt.wasm_optimizer_identity import (  # noqa: E402
    WasmOptimizerExecutableIdentity,
    WasmOptimizerIdentityError,
    find_wasm_opt,
    wasm_optimizer_executable_identity,
)

VALID_LEVELS = frozenset(WASM_OPT_LEVELS)


def _export_names(path: Path) -> frozenset[str]:
    """Read one exact export contract through the shared WASM parser."""

    return frozenset(export.name for export in parse_wasm_exports(path.read_bytes()))


def _optimizer_failure(
    *,
    status: str,
    error: str,
    input_bytes: int,
    output_path: Path | None = None,
    output_bytes: int = 0,
    elapsed_s: float = 0.0,
    pipeline: Sequence[str] = (),
    peak_rss_kb: int | None = None,
    peak_total_rss_kb: int | None = None,
    wasm_opt_path: Path | None = None,
    wasm_opt_sha256: str | None = None,
) -> dict[str, object]:
    """Return the one typed failure shape for every optimizer exit."""

    return {
        "ok": False,
        "status": status,
        "input_bytes": input_bytes,
        "output_bytes": output_bytes,
        "reduction_bytes": 0,
        "reduction_pct": 0.0,
        "elapsed_s": elapsed_s,
        "peak_rss_kb": peak_rss_kb,
        "peak_total_rss_kb": peak_total_rss_kb,
        "output_path": str(output_path) if output_path is not None else None,
        "wasm_opt_path": str(wasm_opt_path) if wasm_opt_path is not None else None,
        "wasm_opt_sha256": wasm_opt_sha256,
        "pipeline": list(pipeline),
        "error": error,
    }


def optimize(
    input_path: Path,
    output_path: Path | None = None,
    level: str = "O2",
    extra_passes: Sequence[str] | None = None,
    *,
    converge: bool | None = None,
    required_exports: set[str] | frozenset[str] | None = None,
    apply_level: bool = True,
    preserve_debug: bool = False,
    timeout: float | None = None,
    optimizer_identity: WasmOptimizerExecutableIdentity | None = None,
) -> dict[str, object]:
    """Run ``wasm-opt`` on *input_path*.

    Parameters:
        input_path   – path to the ``.wasm`` file to optimise.
        output_path  – where to write the result (default: ``<input>.opt.wasm``).
        level        – optimisation level flag (e.g. ``O2``, ``Oz``, ``O3``).
        extra_passes – additional wasm-opt pass flags to append after the level
                       flag (e.g. ``["--dce", "--vacuum", "--inlining"]``).

    Returns a dict with:
        ok              – bool, True if optimisation succeeded
        input_bytes     – original file size
        output_bytes    – optimised file size  (0 on failure)
        reduction_bytes – bytes saved           (0 on failure)
        reduction_pct   – percentage saved      (0.0 on failure)
        elapsed_s       – wall-clock time for wasm-opt
        output_path     – Path to the optimised file
        error           – error message (empty on success)
    """
    if level not in VALID_LEVELS:
        return _optimizer_failure(
            status="invalid-level",
            input_bytes=input_path.stat().st_size,
            error=f"Invalid optimization level: {level!r} (valid: {VALID_LEVELS})",
        )

    selected = (
        str(optimizer_identity.path)
        if optimizer_identity is not None
        else find_wasm_opt()
    )
    if selected is None:
        return _optimizer_failure(
            status="unavailable",
            input_bytes=input_path.stat().st_size if input_path.exists() else 0,
            error="wasm-opt not found (install or provision Binaryen)",
        )
    try:
        selected_path = Path(selected).expanduser().resolve(strict=True)
        if optimizer_identity is None:
            executable_identity = wasm_optimizer_executable_identity(selected_path)
        else:
            if not optimizer_identity.matches_path(selected_path):
                raise WasmOptimizerIdentityError(
                    "wasm-opt no longer matches the invocation-scoped identity"
                )
            executable_identity = optimizer_identity
    except (OSError, WasmOptimizerIdentityError) as exc:
        return _optimizer_failure(
            status="identity-error",
            input_bytes=input_path.stat().st_size if input_path.exists() else 0,
            wasm_opt_path=Path(selected).expanduser(),
            error=str(exc),
        )
    wasm_opt_path = executable_identity.path
    executable_sha256 = executable_identity.sha256
    binaryen_version = executable_identity.binaryen_version
    wasm_opt = str(wasm_opt_path)

    if output_path is None:
        output_path = input_path.with_suffix(".opt.wasm")

    input_bytes = input_path.stat().st_size
    try:
        before = wasm_metrics(input_path)
    except (OSError, ValueError) as exc:
        return _optimizer_failure(
            status="invalid-input",
            input_bytes=input_bytes,
            output_path=output_path,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"failed to profile optimizer input: {exc}",
        )
    try:
        input_exports = _export_names(input_path)
    except (OSError, UnicodeError, ValueError) as exc:
        return _optimizer_failure(
            status="invalid-input",
            input_bytes=input_bytes,
            output_path=output_path,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"failed to read optimizer input export contract: {exc}",
        )
    export_contract = (
        input_exports if required_exports is None else frozenset(required_exports)
    )
    if any(not isinstance(name, str) or not name for name in export_contract):
        return _optimizer_failure(
            status="invalid-contract",
            input_bytes=input_bytes,
            output_path=output_path,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error="required wasm exports must be exact non-empty strings",
        )
    absent_input_exports = sorted(export_contract - input_exports)
    if absent_input_exports:
        return _optimizer_failure(
            status="invalid-contract",
            input_bytes=input_bytes,
            output_path=output_path,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=(
                "required wasm exports are absent before optimization: "
                + ", ".join(absent_input_exports)
            ),
        )

    pipeline = wasm_opt_pipeline(
        level,
        extra_passes=extra_passes or (),
        converge=converge,
        apply_level=apply_level,
        preserve_debug=preserve_debug,
    )
    staged_output = artifact_publication.staged_output_path(
        output_path,
        purpose="wasm-opt",
        suffix=".wasm",
    )
    staged_output.touch(exist_ok=False)
    cmd = [wasm_opt, *pipeline]
    cmd.extend([str(input_path), "-o", str(staged_output)])

    t0 = time.monotonic()
    guard_prefix = "MOLT_WASM_OPT"
    timeout_s = harness_memory_guard.timeout_from_env(
        guard_prefix,
        os.environ,
        explicit=timeout,
        default=300.0,
    )
    limits = harness_memory_guard.limits_from_env(guard_prefix)
    try:
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=guard_prefix,
            capture_output=True,
            text=True,
            timeout=timeout_s,
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        timeout_label = "disabled" if timeout_s is None else f"{timeout_s:g}s"
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="timeout",
            input_bytes=input_bytes,
            output_path=output_path,
            elapsed_s=time.monotonic() - t0,
            pipeline=pipeline,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"wasm-opt timed out after {timeout_label}",
        )
    except (OSError, ValueError) as exc:
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="failed",
            input_bytes=input_bytes,
            output_path=output_path,
            elapsed_s=time.monotonic() - t0,
            pipeline=pipeline,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"failed to execute wasm-opt: {exc}",
        )
    elapsed = time.monotonic() - t0
    peak = getattr(proc, "peak", None)
    peak_total = getattr(proc, "peak_total", None)
    peak_rss_kb = getattr(peak, "rss_kb", None)
    peak_total_rss_kb = getattr(peak_total, "rss_kb", None)

    if getattr(proc, "timed_out", False):
        timeout_label = "disabled" if timeout_s is None else f"{timeout_s:g}s"
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="timeout",
            input_bytes=input_bytes,
            output_path=output_path,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"wasm-opt timed out after {timeout_label}",
        )

    if proc.returncode != 0:
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="failed",
            input_bytes=input_bytes,
            output_path=output_path,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=(proc.stderr or proc.stdout)[:500],
        )

    if not executable_identity.matches_path(wasm_opt_path):
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="identity-error",
            input_bytes=input_bytes,
            output_path=output_path,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error="wasm-opt executable identity changed during execution",
        )

    try:
        exports = _export_names(staged_output)
    except (OSError, ValueError) as exc:
        staged_bytes = staged_output.stat().st_size if staged_output.exists() else 0
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="invalid-output",
            input_bytes=input_bytes,
            output_path=output_path,
            output_bytes=staged_bytes,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"failed to verify optimized output: {exc}",
        )
    missing = sorted(export_contract - exports)
    if missing:
        staged_bytes = staged_output.stat().st_size if staged_output.exists() else 0
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="invalid-output",
            input_bytes=input_bytes,
            output_path=output_path,
            output_bytes=staged_bytes,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error="optimized wasm missing required exports: " + ", ".join(missing),
        )

    output_bytes = staged_output.stat().st_size
    reduction = input_bytes - output_bytes
    pct = (reduction / input_bytes * 100) if input_bytes > 0 else 0.0
    try:
        after = wasm_metrics(staged_output)
    except (OSError, ValueError) as exc:
        staged_output.unlink(missing_ok=True)
        return _optimizer_failure(
            status="invalid-output",
            input_bytes=input_bytes,
            output_path=output_path,
            output_bytes=output_bytes,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"failed to profile optimized output: {exc}",
        )
    try:
        artifact_publication.publish_validated_outputs([(staged_output, output_path)])
    except (OSError, ValueError) as exc:
        return _optimizer_failure(
            status="publication-failed",
            input_bytes=input_bytes,
            output_path=output_path,
            output_bytes=output_bytes,
            elapsed_s=elapsed,
            pipeline=pipeline,
            peak_rss_kb=peak_rss_kb,
            peak_total_rss_kb=peak_total_rss_kb,
            wasm_opt_path=wasm_opt_path,
            wasm_opt_sha256=executable_sha256,
            error=f"failed to publish optimized wasm atomically: {exc}",
        )
    finally:
        staged_output.unlink(missing_ok=True)

    return {
        "ok": True,
        "status": "success",
        "input_bytes": input_bytes,
        "output_bytes": output_bytes,
        "reduction_bytes": reduction,
        "reduction_pct": round(pct, 2),
        "elapsed_s": round(elapsed, 3),
        "peak_rss_kb": peak_rss_kb,
        "peak_total_rss_kb": peak_total_rss_kb,
        "output_path": str(output_path),
        "wasm_opt_path": str(wasm_opt_path),
        "wasm_opt_sha256": executable_sha256,
        "binaryen_version": binaryen_version,
        "pipeline": list(pipeline),
        "before": before,
        "after": after,
        "error": "",
    }


def print_report(result: dict[str, object]) -> None:
    """Print a human-readable optimisation report."""
    if not result["ok"]:
        print(f"Optimisation FAILED: {result['error']}", file=sys.stderr)
        return

    inp = result["input_bytes"]
    out = result["output_bytes"]
    red = result["reduction_bytes"]
    pct = result["reduction_pct"]
    sec = result["elapsed_s"]

    print(f"Input:     {inp:>12,} bytes  ({inp / 1024:.1f} KB)")  # type: ignore[operator]
    print(f"Output:    {out:>12,} bytes  ({out / 1024:.1f} KB)")  # type: ignore[operator]
    print(f"Reduction: {red:>12,} bytes  ({pct}%)")
    print(f"Time:      {sec}s")
    print(f"Output:    {result['output_path']}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Optimise a Molt-compiled WASM module via wasm-opt."
    )
    parser.add_argument("wasm", type=Path, help="Input .wasm file")
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Output path (default: <input>.opt.wasm)",
    )
    parser.add_argument(
        "--level",
        default="O2",
        choices=sorted(VALID_LEVELS),
        help="Optimisation level (default: O2)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Machine-readable JSON output",
    )
    parser.add_argument(
        "--extra-passes",
        nargs="*",
        default=None,
        help="Additional wasm-opt pass flags (e.g. --dce --vacuum).",
    )
    args = parser.parse_args()

    if not args.wasm.is_file():
        print(f"ERROR: {args.wasm} not found", file=sys.stderr)
        sys.exit(1)
    try:
        required_exports = _export_names(args.wasm)
    except (OSError, UnicodeError, ValueError) as exc:
        print(f"ERROR: invalid WebAssembly input: {exc}", file=sys.stderr)
        sys.exit(1)

    result = optimize(
        args.wasm,
        output_path=args.output,
        level=args.level,
        extra_passes=args.extra_passes,
        required_exports=required_exports,
    )

    if args.json_output:
        import json

        print(json.dumps(result, indent=2, default=str))
    else:
        print_report(result)

    sys.exit(0 if result["ok"] else 1)


if __name__ == "__main__":
    main()
