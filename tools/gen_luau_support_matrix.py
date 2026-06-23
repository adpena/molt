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
    ROOT / "docs" / "spec" / "areas" / "compiler" / "luau_support_matrix.generated.md"
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
    "builtin_type": "Modeled as named Luau type metadata for the admitted subset.",
    "class_new": "Modeled as Luau table/metatable object for the admitted subset.",
    "class_apply_set_name": "__set_name__ hooks dispatch over Luau class-table snapshots for the admitted subset.",
    "classmethod_new": "Modeled as Luau descriptor metadata for the admitted subset.",
    "class_layout_version": "Modeled as Luau class-table layout metadata for the admitted subset.",
    "class_merge_layout": "Maintains Luau class-table layout metadata for the admitted subset.",
    "class_set_layout_version": "Modeled as Luau class-table layout metadata for the admitted subset.",
    "call_method": "Uses descriptor-aware Luau table/metatable dispatch for the admitted subset.",
    "alloc": "Modeled as Luau table allocation for the admitted subset.",
    "alloc_task": "Generator/listcomp tasks use coroutine collection paths.",
    "dataclass_new": "Modeled as Luau table object for the admitted subset.",
    "dataclass_get": "Modeled as Luau field/index access for the admitted subset.",
    "dataclass_set": "Modeled as Luau field assignment for the admitted subset.",
    "dataclass_set_class": "Modeled as Luau field assignment for the admitted subset.",
    "del_attr_generic_obj": "Uses descriptor-aware Luau table/metatable deletion for the admitted subset.",
    "del_attr_generic_ptr": "Uses descriptor-aware Luau table/metatable deletion for the admitted subset.",
    "del_attr_name": "Uses descriptor-aware Luau table/metatable deletion for the admitted subset.",
    "get_attr_generic_obj": "Uses descriptor-aware Luau table/metatable lookup for the admitted subset.",
    "get_attr_generic_ptr": "Uses descriptor-aware Luau table/metatable lookup for the admitted subset.",
    "get_attr_name": "Uses descriptor-aware Luau table/metatable lookup for the admitted subset.",
    "get_attr_name_default": "Uses descriptor-aware Luau table/metatable lookup for the admitted subset.",
    "get_attr_special_obj": "Uses descriptor-aware Luau table/metatable lookup for the admitted subset.",
    "getargv": "Luau has no process argv surface; materializes an empty argv list.",
    "getframe": "Luau has no Python frame-object introspection surface; materializes None for fallback-aware stdlib paths.",
    "has_attr_name": "Uses descriptor-aware Luau table/metatable lookup for the admitted subset.",
    "id": "Uses string identity representation, not CPython object address identity.",
    "intarray_from_seq": "Modeled as a copied dense Luau integer table for vector consumers.",
    "is_native_awaitable": "Luau has no Molt native poll-function object representation; target values are non-native awaitables.",
    "isinstance": "Uses Luau type metadata and metatable inheritance for the admitted subset.",
    "issubclass": "Uses Luau type metadata and metatable inheritance for the admitted subset.",
    "property_new": "Modeled as Luau descriptor metadata for the admitted subset.",
    "set_attr_generic_obj": "Uses descriptor-aware Luau table/metatable assignment for the admitted subset.",
    "set_attr_generic_ptr": "Uses descriptor-aware Luau table/metatable assignment for the admitted subset.",
    "set_attr_name": "Uses descriptor-aware Luau table/metatable assignment for the admitted subset.",
    "staticmethod_new": "Modeled as Luau descriptor metadata for the admitted subset.",
    "dict_popitem": "Luau table iteration order is not CPython insertion order.",
    "set_pop": "Luau table iteration order is not CPython set pop order.",
    "sys_executable": "Luau has no executable path surface; materializes an empty string.",
}

CAPABILITY_OPS = {
    "file_open",
    "file_read",
    "file_write",
    "file_close",
    "file_flush",
}

IMPLEMENTED_WITH_MALFORMED_IR_ERRORS = {
    "br_if": "Valid labeled conditional branch lowers to Luau goto; missing target labels fail closed.",
    "branch": "Valid labeled conditional branch lowers to Luau goto; missing target labels fail closed.",
    "branch_false": "Valid labeled false-branch lowers to Luau goto; missing target labels fail closed.",
}


@dataclass(frozen=True)
class Row:
    op: str
    status: str
    note: str


_ARM_START_RE = re.compile(
    r'^\s*(?:_\s*=>|(?:\|\s*)?(?:"[^"]+"|kind if kind\.starts_with\())'
)
_STRING_RE = re.compile(r'"([^"]+)"')
_STARTS_WITH_RE = re.compile(r'kind\.starts_with\("([^"]+)"\)')


def _find_matching_brace(text: str, open_idx: int) -> int:
    depth = 0
    in_string = False
    in_char = False
    in_line_comment = False
    escaped = False

    for idx in range(open_idx, len(text)):
        ch = text[idx]
        nxt = text[idx + 1] if idx + 1 < len(text) else ""

        if in_line_comment:
            if ch == "\n":
                in_line_comment = False
            continue

        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            continue

        if in_char:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == "'":
                in_char = False
            continue

        if ch == "/" and nxt == "/":
            in_line_comment = True
            continue
        if ch == '"':
            in_string = True
            continue
        if ch == "'":
            in_char = True
            continue
        if ch == "{":
            depth += 1
            continue
        if ch == "}":
            depth -= 1
            if depth == 0:
                return idx

    raise ValueError("could not find closing brace for Luau emit_op match")


def _strip_rust_strings_and_comments(line: str) -> str:
    out: list[str] = []
    in_string = False
    in_char = False
    escaped = False
    idx = 0

    while idx < len(line):
        ch = line[idx]
        nxt = line[idx + 1] if idx + 1 < len(line) else ""

        if in_string:
            out.append(" ")
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            idx += 1
            continue

        if in_char:
            out.append(" ")
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == "'":
                in_char = False
            idx += 1
            continue

        if ch == "/" and nxt == "/":
            break
        if ch == '"':
            in_string = True
            out.append(" ")
            idx += 1
            continue
        if ch == "'":
            in_char = True
            out.append(" ")
            idx += 1
            continue

        out.append(ch)
        idx += 1

    return "".join(out)


def _brace_delta(line: str) -> int:
    code = _strip_rust_strings_and_comments(line)
    return code.count("{") - code.count("}")


def _extract_emit_op_match(text: str) -> str:
    marker = "fn emit_op(&mut self, op: &OpIR)"
    start = text.find(marker)
    if start < 0:
        raise ValueError("could not find LuauBackend::emit_op")
    match_start = text.find("match op.kind.as_str()", start)
    if match_start < 0:
        raise ValueError("could not find emit_op match on op.kind")
    open_idx = text.find("{", match_start)
    if open_idx < 0:
        raise ValueError("could not find emit_op match body")
    close_idx = _find_matching_brace(text, open_idx)
    return text[open_idx + 1 : close_idx]


def _ops_from_pattern(pattern: str) -> list[str]:
    starts_with = _STARTS_WITH_RE.findall(pattern)
    if starts_with:
        return [f"{prefix}*" for prefix in starts_with]
    return _STRING_RE.findall(pattern)


def _iter_arms(match_text: str) -> list[tuple[list[str], str]]:
    arms: list[tuple[list[str], str]] = []
    pending_pattern: list[str] = []
    current_ops: list[str] | None = None
    current_body: list[str] = []
    body_depth = 0
    body_started = False

    for line in match_text.splitlines():
        if current_ops is None:
            if not pending_pattern and not _ARM_START_RE.match(line):
                continue
            pending_pattern.append(line)
            code = _strip_rust_strings_and_comments(line)
            if "=>" not in code:
                continue
            pattern = "\n".join(pending_pattern)
            current_ops = _ops_from_pattern(pattern)
            current_body = [line]
            body_depth = _brace_delta(line)
            body_started = bool(code.split("=>", 1)[1].strip())
            pending_pattern = []
            if body_started and body_depth == 0:
                arms.append((current_ops, "\n".join(current_body)))
                current_ops = None
                current_body = []
            continue

        current_body.append(line)
        body_depth += _brace_delta(line)
        body_started = body_started or bool(_strip_rust_strings_and_comments(line).strip())
        if body_started and body_depth == 0:
            arms.append((current_ops, "\n".join(current_body)))
            current_ops = None
            current_body = []

    if current_ops is not None:
        arms.append((current_ops, "\n".join(current_body)))
    if pending_pattern:
        raise ValueError(f"unterminated Luau emit_op match arm pattern: {pending_pattern!r}")
    return arms


def _classify(op: str, body: str) -> Row:
    if op in IMPLEMENTED_WITH_MALFORMED_IR_ERRORS and "missing target label" in body:
        return Row(op, "implemented-exact", IMPLEMENTED_WITH_MALFORMED_IR_ERRORS[op])
    if "-- [unsupported op:" in body or 'error(\\"[unsupported op:' in body:
        return Row(
            op, "compile-error", "Checked Luau emission rejects unsupported markers."
        )
    if op in CAPABILITY_OPS:
        return Row(
            op,
            "runtime-capability-error",
            "Roblox/Luau filesystem capability is unavailable.",
        )

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
        allowed = (
            "-- [exception_last]",
            "-- [exception_message]",
            "-- [missing]",
            "-- [vectorized:",
        )
        if not any(marker in body for marker in allowed):
            return Row(
                op,
                "not-admitted",
                "Checked Luau emission rejects semantic stub markers.",
            )

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
    source_display = source.relative_to(ROOT) if source.is_relative_to(ROOT) else source
    lines: list[str] = [
        "# Luau Backend OpIR Support Matrix",
        "",
        "**Status:** Generated",
        f"**Source:** `{source_display.as_posix()}`",
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
            print(
                f"generated Luau support matrix is stale: {args.output}",
                file=sys.stderr,
            )
            return 1
        print(f"generated Luau support matrix is current: {args.output}")
        return 0
    print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
