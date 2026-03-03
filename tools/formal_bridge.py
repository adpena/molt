#!/usr/bin/env python3
"""Formal bridge: compare Molt compiler IR output against formal model.

Takes a Molt IR JSON file (from `--emit-ir`) and validates its structure
against the formal Lean model. Reports which IR constructs have formal
counterparts and which are outside the formalized subset.

Usage:
  python3 tools/formal_bridge.py <ir.json>
  python3 tools/formal_bridge.py --dir tests/differential/basic/core_types/
  python3 tools/formal_bridge.py --summary    # aggregated coverage stats

Integration with molt_diff.py:
  MOLT_FORMAL_BRIDGE=1 uv run python3 tests/molt_diff.py tests/differential/basic
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# ── Formal model vocabulary ──────────────────────────────────────
# These correspond to the Lean Expr/BinOp/UnOp/Terminator types
FORMAL_BINOPS = {
    "add",
    "sub",
    "mul",
    "div",
    "floordiv",
    "mod",
    "pow",
    "eq",
    "ne",
    "lt",
    "le",
    "gt",
    "ge",
    "bit_and",
    "bit_or",
    "bit_xor",
    "lshift",
    "rshift",
}

FORMAL_UNOPS = {"neg", "not", "abs", "invert"}

FORMAL_EXPR_KINDS = {
    "const_int",
    "const_float",
    "const_bool",
    "const_str",
    "const_none",
    "load_var",
    "load_fast",
    "binop",
    "binary_op",
    "unop",
    "unary_op",
}

FORMAL_CONTROL_FLOW = {
    "return",
    "ret",
    "jump",
    "jmp",
    "goto",
    "branch",
    "br",
    "if",
    "if_else",
}

# All recognized op kinds in the formal model
FORMAL_OPS = FORMAL_BINOPS | FORMAL_UNOPS | FORMAL_EXPR_KINDS | FORMAL_CONTROL_FLOW


def classify_op(kind: str) -> tuple[str, bool]:
    """Classify an IR op kind. Returns (category, is_formalized)."""
    k = kind.lower().replace("-", "_")

    if k in FORMAL_BINOPS or k.replace("binary_", "") in FORMAL_BINOPS:
        return "binop", True
    if k in FORMAL_UNOPS or k.replace("unary_", "") in FORMAL_UNOPS:
        return "unop", True
    if k.startswith("const_") or k in {"load_fast", "load_var", "store_var"}:
        return "expr", True
    if k in {
        "return",
        "ret",
        "jump",
        "jmp",
        "goto",
        "branch",
        "br",
        "if",
        "if_else",
        "if_branch",
        "end_if",
    }:
        return "control", True

    # Not in formal model
    if k.startswith("call_") or k.startswith("invoke_"):
        return "call", False
    if k.startswith("build_") or k.startswith("unpack_"):
        return "aggregate", False
    if k.startswith("get_") or k.startswith("set_") or k.startswith("del_"):
        return "access", False
    if k.startswith("import_"):
        return "import", False
    return "other", False


def analyze_ir_json(ir_data: dict) -> dict:
    """Analyze an IR JSON blob for formal coverage."""
    stats = {
        "total_ops": 0,
        "formalized_ops": 0,
        "unformalized_ops": 0,
        "op_kinds": {},
        "formalized_kinds": set(),
        "unformalized_kinds": set(),
        "blocks": 0,
        "functions": 0,
    }

    functions = ir_data if isinstance(ir_data, list) else ir_data.get("functions", [])
    if isinstance(ir_data, dict) and "ops" in ir_data:
        functions = [ir_data]

    for func in functions:
        stats["functions"] += 1
        ops = func.get("ops", [])
        for op in ops:
            kind = op.get("kind", "unknown")
            stats["total_ops"] += 1
            category, is_formal = classify_op(kind)
            if is_formal:
                stats["formalized_ops"] += 1
                stats["formalized_kinds"].add(kind)
            else:
                stats["unformalized_ops"] += 1
                stats["unformalized_kinds"].add(kind)

            if kind not in stats["op_kinds"]:
                stats["op_kinds"][kind] = {
                    "count": 0,
                    "category": category,
                    "formalized": is_formal,
                }
            stats["op_kinds"][kind]["count"] += 1

            # Count block boundaries
            if kind in {"label", "block", "entry"}:
                stats["blocks"] += 1

    # Convert sets to sorted lists for JSON
    stats["formalized_kinds"] = sorted(stats["formalized_kinds"])
    stats["unformalized_kinds"] = sorted(stats["unformalized_kinds"])

    return stats


def analyze_ir_file(ir_path: Path) -> dict | None:
    """Analyze a single IR JSON file."""
    try:
        data = json.loads(ir_path.read_text())
        result = analyze_ir_json(data)
        result["file"] = str(ir_path)
        return result
    except (json.JSONDecodeError, OSError) as e:
        print(f"  error reading {ir_path}: {e}", file=sys.stderr)
        return None


def compile_and_analyze(py_path: Path, build_profile: str = "dev") -> dict | None:
    """Compile a Python file with --emit-ir and analyze the output."""
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        ir_path = Path(f.name)

    try:
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                build_profile,
                "--emit-ir",
                str(ir_path),
                str(py_path),
            ],
            capture_output=True,
            text=True,
            timeout=60,
            env={**__import__("os").environ, "PYTHONPATH": str(ROOT / "src")},
        )
        if result.returncode != 0:
            return None
        if not ir_path.exists() or ir_path.stat().st_size == 0:
            return None
        return analyze_ir_file(ir_path)
    except (subprocess.TimeoutExpired, OSError):
        return None
    finally:
        ir_path.unlink(missing_ok=True)


def aggregate_stats(all_stats: list[dict]) -> dict:
    """Aggregate stats across multiple files."""
    agg = {
        "files_analyzed": len(all_stats),
        "total_ops": 0,
        "formalized_ops": 0,
        "unformalized_ops": 0,
        "total_functions": 0,
        "total_blocks": 0,
        "all_formalized_kinds": set(),
        "all_unformalized_kinds": set(),
    }
    for s in all_stats:
        agg["total_ops"] += s["total_ops"]
        agg["formalized_ops"] += s["formalized_ops"]
        agg["unformalized_ops"] += s["unformalized_ops"]
        agg["total_functions"] += s["functions"]
        agg["total_blocks"] += s["blocks"]
        agg["all_formalized_kinds"].update(s["formalized_kinds"])
        agg["all_unformalized_kinds"].update(s["unformalized_kinds"])

    agg["coverage_pct"] = round(
        agg["formalized_ops"] / max(agg["total_ops"], 1) * 100, 1
    )
    agg["all_formalized_kinds"] = sorted(agg["all_formalized_kinds"])
    agg["all_unformalized_kinds"] = sorted(agg["all_unformalized_kinds"])
    return agg


def main() -> int:
    parser = argparse.ArgumentParser(description="Formal bridge: IR ↔ Lean model")
    parser.add_argument("ir_files", nargs="*", help="IR JSON files to analyze")
    parser.add_argument("--dir", help="Directory of .py files to compile and analyze")
    parser.add_argument("--summary", action="store_true", help="Summary only")
    parser.add_argument("--json", action="store_true", help="JSON output")
    parser.add_argument(
        "--build-profile",
        default="dev",
        help="Build profile for compilation (default: dev)",
    )
    args = parser.parse_args()

    all_stats: list[dict] = []

    # Analyze pre-existing IR JSON files
    for ir_file in args.ir_files:
        path = Path(ir_file)
        if path.is_file():
            result = analyze_ir_file(path)
            if result:
                all_stats.append(result)
        elif path.is_dir():
            for json_file in sorted(path.rglob("*.json")):
                result = analyze_ir_file(json_file)
                if result:
                    all_stats.append(result)

    # Compile Python files and analyze
    if args.dir:
        dir_path = Path(args.dir)
        py_files = sorted(dir_path.rglob("*.py"))
        for py_file in py_files:
            if py_file.name.startswith("_"):
                continue
            print(f"  compiling {py_file.relative_to(ROOT)} ...", file=sys.stderr)
            result = compile_and_analyze(py_file, args.build_profile)
            if result:
                all_stats.append(result)

    if not all_stats:
        print("No IR files analyzed. Provide --dir or IR JSON paths.", file=sys.stderr)
        return 1

    agg = aggregate_stats(all_stats)

    if args.json:
        output = {"aggregate": agg, "files": all_stats} if not args.summary else agg
        # Convert sets for JSON
        print(json.dumps(output, indent=2, default=str))
        return 0

    # Human-readable output
    print("=" * 60)
    print("Formal Bridge: IR Coverage Report")
    print("=" * 60)
    print(f"\n  Files analyzed:     {agg['files_analyzed']}")
    print(f"  Total operations:   {agg['total_ops']}")
    print(f"  Formalized ops:     {agg['formalized_ops']}  ({agg['coverage_pct']}%)")
    print(f"  Unformalized ops:   {agg['unformalized_ops']}")
    print(f"  Functions:          {agg['total_functions']}")
    print(f"  Blocks:             {agg['total_blocks']}")

    if agg["all_formalized_kinds"]:
        print(f"\n  Formalized op kinds: {', '.join(agg['all_formalized_kinds'])}")
    if agg["all_unformalized_kinds"]:
        print(f"\n  Unformalized op kinds: {', '.join(agg['all_unformalized_kinds'])}")

    if not args.summary:
        print("\n--- Per-file Details ---")
        for s in all_stats:
            cov = round(s["formalized_ops"] / max(s["total_ops"], 1) * 100, 1)
            fname = Path(s.get("file", "?")).name
            print(f"  {fname:40s}  {s['formalized_ops']}/{s['total_ops']}  ({cov}%)")

    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
