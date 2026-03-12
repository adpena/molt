#!/usr/bin/env python3
"""Dump Molt IR at various compilation stages.

Hooks into the Molt compilation pipeline to capture and display TIR
(Typed IR) at different stages of the midend optimization pipeline.

Stages:
  pre-midend   — After TIR construction, before optimization passes
  post-midend  — After all optimization passes complete
  all          — Dump at every pass boundary (pre/post each pass)

Output modes:
  text (default) — Human-readable op listing
  json           — Machine-readable JSON for automated comparison

Usage:
    uv run --python 3.12 python3 tools/ir_dump.py examples/hello.py
    uv run --python 3.12 python3 tools/ir_dump.py --stage=pre-midend examples/hello.py
    uv run --python 3.12 python3 tools/ir_dump.py --stage=post-midend examples/hello.py
    uv run --python 3.12 python3 tools/ir_dump.py --stage=all --json examples/hello.py
    uv run --python 3.12 python3 tools/ir_dump.py --stage=all --json --out-dir /tmp/ir_dumps examples/hello.py
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import sys
import textwrap
import time
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Molt import bootstrap
# ---------------------------------------------------------------------------

_REPO_ROOT = Path(__file__).resolve().parent.parent
_SRC_DIR = _REPO_ROOT / "src"
if str(_SRC_DIR) not in sys.path:
    sys.path.insert(0, str(_SRC_DIR))

try:
    from molt.frontend import compile_to_tir

    _MOLT_AVAILABLE = True
except ImportError as exc:
    _MOLT_AVAILABLE = False
    _MOLT_IMPORT_ERROR = str(exc)


# ---------------------------------------------------------------------------
# Stage definitions
# ---------------------------------------------------------------------------

VALID_STAGES = {"pre-midend", "post-midend", "all"}

# The midend pass names as recorded by _record_midend_pass_sample in the
# fixed-point loop (see src/molt/frontend/__init__.py).
MIDEND_PASS_NAMES = [
    "cfg_precanonicalize",
    "simplify",
    "sccp_edge_thread",
    "join_canonicalize",
    "guard_hoist",
    "licm",
    "prune",
    "verifier",
    "dce",
    "cse",
]


# ---------------------------------------------------------------------------
# IR snapshot helpers
# ---------------------------------------------------------------------------


def _ops_fingerprint(ops_json: list[dict[str, Any]]) -> str:
    """Deterministic hash of a JSON ops list for quick equality checks."""
    canonical = json.dumps(ops_json, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()[:16]


def _format_ops_text(ops_json: list[dict[str, Any]], *, indent: int = 2) -> str:
    """Human-readable text representation of a TIR ops list."""
    lines: list[str] = []
    prefix = " " * indent
    for i, op in enumerate(ops_json):
        kind = op.get("kind", "?")
        result = op.get("result")
        value = op.get("value")
        parts = [f"{prefix}{i:4d}  {kind}"]
        if result is not None:
            parts.append(f" -> {result}")
        if value is not None:
            val_str = str(value)
            if len(val_str) > 60:
                val_str = val_str[:57] + "..."
            parts.append(f"  [{val_str}]")
        lines.append("".join(parts))
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Instrumented compilation
# ---------------------------------------------------------------------------


def compile_with_ir_snapshots(
    source: str,
    *,
    stage: str = "all",
) -> dict[str, Any]:
    """Compile source and capture IR snapshots at requested stages.

    Returns a dict with:
      - "functions": list of function dicts from final TIR
      - "snapshots": list of {"stage": str, "functions": [...]} dicts
      - "timing_ms": total compilation time
      - "error": optional error string
    """
    if not _MOLT_AVAILABLE:
        return {
            "functions": [],
            "snapshots": [],
            "timing_ms": 0.0,
            "error": f"molt not importable: {_MOLT_IMPORT_ERROR}",
        }

    snapshots: list[dict[str, Any]] = []
    t0 = time.perf_counter()

    # Strategy: compile twice if needed.
    # - pre-midend: compile with MOLT_MIDEND_DISABLE=1
    # - post-midend: compile normally
    # - all: do both and diff

    try:
        if stage in ("pre-midend", "all"):
            prev_val = os.environ.get("MOLT_MIDEND_DISABLE")
            os.environ["MOLT_MIDEND_DISABLE"] = "1"
            try:
                pre_tir = compile_to_tir(source)
            finally:
                if prev_val is None:
                    os.environ.pop("MOLT_MIDEND_DISABLE", None)
                else:
                    os.environ["MOLT_MIDEND_DISABLE"] = prev_val
            snapshots.append(
                {
                    "stage": "pre-midend",
                    "functions": pre_tir.get("functions", []),
                }
            )

        if stage in ("post-midend", "all"):
            prev_val = os.environ.get("MOLT_MIDEND_DISABLE")
            os.environ.pop("MOLT_MIDEND_DISABLE", None)
            try:
                post_tir = compile_to_tir(source)
            finally:
                if prev_val is not None:
                    os.environ["MOLT_MIDEND_DISABLE"] = prev_val
            snapshots.append(
                {
                    "stage": "post-midend",
                    "functions": post_tir.get("functions", []),
                }
            )

        elapsed = (time.perf_counter() - t0) * 1000.0
        final_funcs = snapshots[-1]["functions"] if snapshots else []
        return {
            "functions": final_funcs,
            "snapshots": snapshots,
            "timing_ms": round(elapsed, 3),
            "error": None,
        }
    except Exception as exc:
        elapsed = (time.perf_counter() - t0) * 1000.0
        return {
            "functions": [],
            "snapshots": snapshots,
            "timing_ms": round(elapsed, 3),
            "error": str(exc),
        }


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def print_snapshots(
    result: dict[str, Any],
    *,
    as_json: bool = False,
    out_dir: Path | None = None,
    source_path: str = "<stdin>",
) -> None:
    """Print or write IR snapshots."""
    if result.get("error"):
        print(f"ERROR: {result['error']}", file=sys.stderr)
        if not result["snapshots"]:
            return

    if as_json:
        payload = {
            "source": source_path,
            "timing_ms": result["timing_ms"],
            "snapshots": [],
        }
        for snap in result["snapshots"]:
            entry: dict[str, Any] = {"stage": snap["stage"], "functions": []}
            for func in snap["functions"]:
                ops = func.get("ops", [])
                entry["functions"].append(
                    {
                        "name": func.get("name", "?"),
                        "op_count": len(ops),
                        "fingerprint": _ops_fingerprint(ops),
                        "ops": ops,
                    }
                )
            payload["snapshots"].append(entry)

        if out_dir is not None:
            out_dir.mkdir(parents=True, exist_ok=True)
            stem = Path(source_path).stem
            out_path = out_dir / f"{stem}_ir_dump.json"
            out_path.write_text(json.dumps(payload, indent=2) + "\n")
            print(f"Wrote: {out_path}")
        else:
            print(json.dumps(payload, indent=2))
        return

    # Text mode
    print(f"=== IR Dump: {source_path} ({result['timing_ms']:.1f} ms) ===\n")
    for snap in result["snapshots"]:
        print(f"--- Stage: {snap['stage']} ---")
        for func in snap["functions"]:
            ops = func.get("ops", [])
            name = func.get("name", "?")
            fp = _ops_fingerprint(ops)
            print(f"  Function: {name}  ({len(ops)} ops, fingerprint={fp})")
            print(_format_ops_text(ops))
            print()
        print()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Dump Molt IR at various compilation stages.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=textwrap.dedent("""\
            Stages:
              pre-midend   IR after TIR construction, before optimization
              post-midend  IR after all optimization passes
              all          Both pre-midend and post-midend snapshots

            Environment variables:
              MOLT_MIDEND_DISABLE  Set to 1 to skip all midend passes
              MOLT_MIDEND_MAX_ROUNDS  Cap fixed-point iteration rounds
        """),
    )
    p.add_argument(
        "source",
        help="Python source file to compile",
    )
    p.add_argument(
        "--stage",
        choices=sorted(VALID_STAGES),
        default="all",
        help="Which compilation stage(s) to dump (default: all)",
    )
    p.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Output as JSON instead of human-readable text",
    )
    p.add_argument(
        "--out-dir",
        type=str,
        default=None,
        help="Write JSON output to this directory instead of stdout",
    )
    return p


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    source_path = args.source
    if not Path(source_path).is_file():
        print(f"ERROR: {source_path} is not a file", file=sys.stderr)
        return 1

    source = Path(source_path).read_text(encoding="utf-8")

    # Quick syntax check
    try:
        ast.parse(source)
    except SyntaxError as exc:
        print(f"SyntaxError in {source_path}: {exc}", file=sys.stderr)
        return 1

    result = compile_with_ir_snapshots(source, stage=args.stage)

    out_dir = Path(args.out_dir) if args.out_dir else None
    print_snapshots(
        result,
        as_json=args.json_output,
        out_dir=out_dir,
        source_path=source_path,
    )

    return 1 if result.get("error") else 0


if __name__ == "__main__":
    sys.exit(main())
