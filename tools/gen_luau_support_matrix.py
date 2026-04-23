#!/usr/bin/env python3
"""Generate the Luau backend OpIR support matrix.

The matrix is derived from `runtime/molt-backend/src/luau.rs` so support
claims stay tied to the actual emitter. It classifies each `emit_op` match arm
into a small set of gateable statuses.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SOURCE = ROOT / "runtime" / "molt-backend" / "src" / "luau.rs"
DEFAULT_OUTPUT = (
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compiler"
    / "luau_support_matrix.generated.md"
)

STATUSES = {
    "implemented-exact",
    "implemented-target-limited",
    "compile-error",
    "runtime-capability-error",
    "not-admitted",
}

TARGET_LIMITED_OPS = {
    "const_bigint": "Luau numbers are IEEE-754 doubles; arbitrary precision is not represented.",
    "is": "Non-None identity currently lowers through equality on Luau values.",
    "is_not": "Non-None identity currently lowers through inequality on Luau values.",
    "module_import": "Only known module bridges are materialized in Luau.",
    "module_cache_get": "Only known module bridges are materialized in Luau.",
    "module_get_global": "Dynamic module lookup depends on Luau module cache entries.",
    "module_get_name": "Dynamic module lookup depends on Luau module cache entries.",
    "module_get_attr": "Known module bridges are direct; unknown attrs return nil unless rejected by checked output.",
    "object_new": "Modeled as Luau table object for the admitted subset.",
    "module_new": "Modeled as Luau table object for the admitted subset.",
    "builtin_type": "Modeled as Luau table object for the admitted subset.",
    "class_new": "Modeled as Luau table/metatable object for the admitted subset.",
    "alloc": "Modeled as Luau table allocation for the admitted subset.",
    "alloc_task": "Generator/listcomp tasks use coroutine collection paths.",
    "dataclass_new": "Modeled as Luau table object for the admitted subset.",
    "dataclass_get": "Modeled as Luau field/index access for the admitted subset.",
    "dataclass_set": "Modeled as Luau field assignment for the admitted subset.",
    "dataclass_set_class": "Modeled as Luau field assignment for the admitted subset.",
    "id": "Uses string identity representation, not CPython object address identity.",
    "dict_popitem": "Luau table iteration order is not CPython insertion order.",
    "set_pop": "Luau table iteration order is not CPython set pop order.",
}

CAPABILITY_OPS = {
    "file_open",
    "file_read",
    "file_write",
    "file_close",
    "file_flush",
}


@dataclass(frozen=True)
class Row:
    op: str
    status: str
    note: str


_ARM_RE = re.compile(
    r'^\s*(?P<pat>(?:"[^"]+"\s*(?:\|\s*"[^"]+"\s*)*|kind if kind\.starts_with\("[^"]+"\).*?))\s*=>'
)
_STRING_RE = re.compile(r'"([^"]+)"')
_STARTS_WITH_RE = re.compile(r'kind\.starts_with\("([^"]+)"\)')


def _extract_emit_op_match(text: str) -> str:
    marker = "fn emit_op(&mut self, op: &OpIR)"
    start = text.find(marker)
    if start < 0:
        raise ValueError("could not find LuauBackend::emit_op")
    match_start = text.find("match op.kind.as_str()", start)
    if match_start < 0:
        raise ValueError("could not find emit_op match on op.kind")
    helper_start = text.find("\n    // --- helper: binary op ---", match_start)
    if helper_start < 0:
        helper_start = len(text)
    return text[match_start:helper_start]


def _ops_from_pattern(pattern: str) -> list[str]:
    starts_with = _STARTS_WITH_RE.search(pattern)
    if starts_with:
        return [f"{starts_with.group(1)}*"]
    return _STRING_RE.findall(pattern)


def _iter_arms(match_text: str) -> list[tuple[list[str], str]]:
    arms: list[tuple[list[str], str]] = []
    current_ops: list[str] | None = None
    current_body: list[str] = []

    for line in match_text.splitlines():
        arm = _ARM_RE.match(line)
        if arm:
            if current_ops is not None:
                arms.append((current_ops, "\n".join(current_body)))
            pattern = arm.group("pat")
            current_ops = _ops_from_pattern(pattern)
            current_body = [line]
            continue
        if current_ops is not None:
            if re.match(r"^\s*_ =>", line):
                arms.append((current_ops, "\n".join(current_body)))
                current_ops = None
                current_body = []
                continue
            current_body.append(line)

    if current_ops is not None:
        arms.append((current_ops, "\n".join(current_body)))
    return arms


def _classify(op: str, body: str) -> Row:
    if "-- [unsupported op:" in body or 'error(\\"[unsupported op:' in body:
        return Row(op, "compile-error", "Checked Luau emission rejects unsupported markers.")
    if op in CAPABILITY_OPS:
        return Row(op, "runtime-capability-error", "Roblox/Luau filesystem capability is unavailable.")

    semantic_markers = (
        "-- [async:",
        "-- [context:",
        "-- [internal:",
        "-- [stub:",
        "-- [class op:",
        "-- [try_start]",
        "-- [try_end]",
        "-- [",
    )
    if any(marker in body for marker in semantic_markers):
        allowed = ("-- [exception_last]", "-- [exception_message]", "-- [missing]", "-- [vectorized:")
        if not any(marker in body for marker in allowed):
            return Row(op, "not-admitted", "Checked Luau emission rejects semantic stub markers.")

    if op in TARGET_LIMITED_OPS:
        return Row(op, "implemented-target-limited", TARGET_LIMITED_OPS[op])

    return Row(op, "implemented-exact", "Lowered without checked-output stub markers.")


def collect_rows_from_text(text: str) -> list[Row]:
    match_text = _extract_emit_op_match(text)
    by_op: dict[str, Row] = {}
    for ops, body in _iter_arms(match_text):
        for op in ops:
            by_op[op] = _classify(op, body)

    rows = sorted(by_op.values(), key=lambda row: row.op)
    bad_statuses = sorted({row.status for row in rows} - STATUSES)
    if bad_statuses:
        raise ValueError(f"unknown statuses produced: {', '.join(bad_statuses)}")
    return rows


def _render(rows: list[Row], source: Path) -> str:
    counts = Counter(row.status for row in rows)
    lines: list[str] = [
        "# Luau Backend OpIR Support Matrix",
        "",
        "**Status:** Generated",
        f"**Source:** `{source.relative_to(ROOT) if source.is_relative_to(ROOT) else source}`",
        "**Target:** current/future Luau surface; Molt does not add legacy Lua compatibility shims.",
        "",
        "## Summary",
        "",
    ]
    for status in sorted(STATUSES):
        lines.append(f"- `{status}`: `{counts.get(status, 0)}`")
    lines.extend(
        [
            f"- `total`: `{len(rows)}`",
            "",
            "## Matrix",
            "",
            "| OpIR kind | Status | Note |",
            "| --- | --- | --- |",
        ]
    )
    for row in rows:
        lines.append(f"| `{row.op}` | `{row.status}` | {row.note} |")
    lines.extend(
        [
            "",
            "## Status Definitions",
            "",
            "- `implemented-exact`: emitted without known Luau target limitation or checked-output stub marker.",
            "- `implemented-target-limited`: emitted for an admitted subset with an explicit Luau/Python semantic limit.",
            "- `compile-error`: checked Luau emission rejects this unsupported operation.",
            "- `runtime-capability-error`: operation requires a target capability unavailable in Roblox/Luau.",
            "- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.",
            "",
        ]
    )
    return "\n".join(lines)


def build_output(source: Path) -> str:
    text = source.read_text(encoding="utf-8")
    return _render(collect_rows_from_text(text), source)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)

    output = build_output(args.source)
    if args.write:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output, encoding="utf-8")
        print(f"wrote {args.output}")
        return 0
    if args.check:
        try:
            current = args.output.read_text(encoding="utf-8")
        except FileNotFoundError:
            print(f"missing generated file: {args.output}", file=sys.stderr)
            return 1
        if current != output:
            print(f"generated Luau support matrix is stale: {args.output}", file=sys.stderr)
            return 1
        print(f"generated Luau support matrix is current: {args.output}")
        return 0
    print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
