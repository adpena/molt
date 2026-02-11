#!/usr/bin/env python3
"""Report fast_int lowering coverage for arithmetic/compare ops.

Example:
  UV_NO_SYNC=1 uv run --python 3.12 python3 tools/fast_int_report.py examples/hello.py
"""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

from molt.frontend import compile_to_tir

TRACKED_KINDS = {
    "add",
    "inplace_add",
    "sub",
    "inplace_sub",
    "mul",
    "inplace_mul",
    "div",
    "floordiv",
    "mod",
    "bit_or",
    "inplace_bit_or",
    "bit_and",
    "inplace_bit_and",
    "bit_xor",
    "inplace_bit_xor",
    "lshift",
    "rshift",
    "lt",
    "le",
    "gt",
    "ge",
    "eq",
    "ne",
}


def collect_ops(ir: dict[str, Any]) -> list[dict[str, Any]]:
    ops: list[dict[str, Any]] = []
    for fn in ir.get("functions", []):
        for op in fn.get("ops", []):
            kind = op.get("kind")
            if kind in TRACKED_KINDS:
                ops.append(op)
    return ops


def analyze_file(path: Path, type_hints: str) -> tuple[Counter[str], Counter[str]]:
    source = path.read_text(encoding="utf-8")
    ir = compile_to_tir(source, type_hint_policy=type_hints)
    total: Counter[str] = Counter()
    fast: Counter[str] = Counter()
    for op in collect_ops(ir):
        kind = str(op["kind"])
        total[kind] += 1
        if op.get("fast_int") is True:
            fast[kind] += 1
    return total, fast


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path, help="Python source files")
    parser.add_argument(
        "--type-hints",
        choices=("ignore", "trust", "check"),
        default="check",
        help="Frontend type hint policy (default: check)",
    )
    args = parser.parse_args()

    agg_total: Counter[str] = Counter()
    agg_fast: Counter[str] = Counter()
    per_file: dict[Path, tuple[Counter[str], Counter[str]]] = {}
    for path in args.paths:
        total, fast = analyze_file(path, args.type_hints)
        per_file[path] = (total, fast)
        agg_total.update(total)
        agg_fast.update(fast)

    print(f"type_hints={args.type_hints}")
    print("== Per-file ==")
    for path in args.paths:
        total, fast = per_file[path]
        total_ops = sum(total.values())
        fast_ops = sum(fast.values())
        ratio = (100.0 * fast_ops / total_ops) if total_ops else 0.0
        print(f"{path}: fast_int={fast_ops}/{total_ops} ({ratio:.1f}%)")

    print("\n== Aggregate ==")
    rows = sorted(agg_total)
    for kind in rows:
        total = agg_total[kind]
        fast = agg_fast[kind]
        ratio = (100.0 * fast / total) if total else 0.0
        print(f"{kind:16} {fast:5d}/{total:<5d} {ratio:5.1f}%")

    by_gap: dict[int, list[str]] = defaultdict(list)
    for kind, total in agg_total.items():
        by_gap[total - agg_fast[kind]].append(kind)
    missing = [k for gap, kinds in by_gap.items() if gap > 0 for k in sorted(kinds)]
    if missing:
        print("\nOps with remaining import-path exposure:")
        for kind in missing:
            print(f"- {kind}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
