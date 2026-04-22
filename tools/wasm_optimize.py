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
import shutil
import subprocess
import sys
import time
from pathlib import Path

# Optimization levels supported by wasm-opt.
VALID_LEVELS = {"O1", "O2", "O3", "O4", "Os", "Oz"}
# Explicit feature set instead of --all-features.  Binaryen's --all-features
# enables --enable-custom-descriptors which rewrites typed function references
# into `exact` heap types — rejected by Cloudflare Workers' V8 and other
# engines that haven't shipped the custom-descriptors proposal yet.
_DEFAULT_FEATURE_FLAGS = [
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
]


def find_wasm_opt() -> str | None:
    """Return the path to ``wasm-opt`` if it is on ``$PATH``."""
    return shutil.which("wasm-opt")


def _read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("unexpected EOF while reading varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 63:
            raise ValueError("varuint too large")


def _read_string(data: bytes, offset: int) -> tuple[str, int]:
    size, offset = _read_varuint(data, offset)
    end = offset + size
    if end > len(data):
        raise ValueError("unexpected EOF while reading string")
    return data[offset:end].decode("utf-8"), end


def _collect_function_exports(path: Path) -> set[str]:
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\0\0\0":
        return set()
    offset = 8
    exports: set[str] = set()
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_varuint(data, offset)
        end = offset + section_size
        if end > len(data):
            raise ValueError("unexpected EOF while reading section")
        payload = data[offset:end]
        offset = end
        if section_id != 7:
            continue
        cursor = 0
        count, cursor = _read_varuint(payload, cursor)
        for _ in range(count):
            name, cursor = _read_string(payload, cursor)
            if cursor >= len(payload):
                raise ValueError("unexpected EOF while reading export kind")
            kind = payload[cursor]
            cursor += 1
            _, cursor = _read_varuint(payload, cursor)
            if kind == 0:
                exports.add(name)
        break
    return exports


def optimize(
    input_path: Path,
    output_path: Path | None = None,
    level: str = "O2",
    extra_passes: list[str] | None = None,
    *,
    converge: bool = True,
    required_exports: set[str] | frozenset[str] | None = None,
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
        return {
            "ok": False,
            "input_bytes": input_path.stat().st_size,
            "output_bytes": 0,
            "reduction_bytes": 0,
            "reduction_pct": 0.0,
            "elapsed_s": 0.0,
            "output_path": None,
            "error": f"Invalid optimization level: {level!r} (valid: {VALID_LEVELS})",
        }

    wasm_opt = find_wasm_opt()
    if wasm_opt is None:
        return {
            "ok": False,
            "input_bytes": input_path.stat().st_size if input_path.exists() else 0,
            "output_bytes": 0,
            "reduction_bytes": 0,
            "reduction_pct": 0.0,
            "elapsed_s": 0.0,
            "output_path": None,
            "error": "wasm-opt not found in PATH (install Binaryen)",
        }

    if output_path is None:
        output_path = input_path.with_suffix(".opt.wasm")

    input_bytes = input_path.stat().st_size

    cmd = [wasm_opt, f"-{level}", *_DEFAULT_FEATURE_FLAGS, "--strip-producers"]
    if converge:
        cmd.append("--converge")
    if extra_passes:
        cmd.extend(extra_passes)
    cmd.extend([str(input_path), "-o", str(output_path)])

    t0 = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except subprocess.TimeoutExpired:
        return {
            "ok": False,
            "input_bytes": input_bytes,
            "output_bytes": 0,
            "reduction_bytes": 0,
            "reduction_pct": 0.0,
            "elapsed_s": time.monotonic() - t0,
            "output_path": str(output_path),
            "error": "wasm-opt timed out after 300s",
        }
    elapsed = time.monotonic() - t0

    if proc.returncode != 0:
        return {
            "ok": False,
            "input_bytes": input_bytes,
            "output_bytes": 0,
            "reduction_bytes": 0,
            "reduction_pct": 0.0,
            "elapsed_s": elapsed,
            "output_path": str(output_path),
            "error": (proc.stderr or proc.stdout)[:500],
        }

    if required_exports:
        try:
            exports = _collect_function_exports(output_path)
        except (OSError, ValueError) as exc:
            return {
                "ok": False,
                "input_bytes": input_bytes,
                "output_bytes": output_path.stat().st_size
                if output_path.exists()
                else 0,
                "reduction_bytes": 0,
                "reduction_pct": 0.0,
                "elapsed_s": elapsed,
                "output_path": str(output_path),
                "error": f"failed to verify optimized exports: {exc}",
            }
        missing = sorted(set(required_exports) - exports)
        if missing:
            return {
                "ok": False,
                "input_bytes": input_bytes,
                "output_bytes": output_path.stat().st_size
                if output_path.exists()
                else 0,
                "reduction_bytes": 0,
                "reduction_pct": 0.0,
                "elapsed_s": elapsed,
                "output_path": str(output_path),
                "error": "optimized wasm missing required exports: "
                + ", ".join(missing),
            }

    output_bytes = output_path.stat().st_size
    reduction = input_bytes - output_bytes
    pct = (reduction / input_bytes * 100) if input_bytes > 0 else 0.0

    return {
        "ok": True,
        "input_bytes": input_bytes,
        "output_bytes": output_bytes,
        "reduction_bytes": reduction,
        "reduction_pct": round(pct, 2),
        "elapsed_s": round(elapsed, 3),
        "output_path": str(output_path),
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

    result = optimize(
        args.wasm,
        output_path=args.output,
        level=args.level,
        extra_passes=args.extra_passes,
    )

    if args.json_output:
        import json

        print(json.dumps(result, indent=2, default=str))
    else:
        print_report(result)

    sys.exit(0 if result["ok"] else 1)


if __name__ == "__main__":
    main()
