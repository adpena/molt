#!/usr/bin/env python3
"""Generate the Luau backend OpIR support matrix.

The matrix is derived from the Luau backend's `op_emitter.rs` so support claims
stay tied to the actual emitter. It classifies each `emit_op` match arm into a
small set of gateable statuses.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

from generator_io import generated_file_matches, write_generated_text


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SOURCE = ROOT / "runtime" / "molt-backend-luau" / "src" / "luau"
DEFAULT_OUTPUT = (
    ROOT / "docs" / "spec" / "areas" / "compiler" / "luau_support_matrix.generated.md"
)

STATUSES = {
    "implemented-exact",
    "implemented-target-limited",
    "compile-error",
    "not-admitted",
}

_OP_KIND_TABLE = tomllib.loads(
    (ROOT / "runtime" / "molt-ir" / "src" / "tir" / "op_kinds.toml").read_text(
        encoding="utf-8"
    )
)


def _kind_set(*keys: str) -> frozenset[str]:
    return frozenset(kind for key in keys for kind in _OP_KIND_TABLE.get(key, []))


_PRE_SOURCE_NOT_ADMITTED = _kind_set(
    "simpleir_dynamic_divmod_semantics_kinds",
    "simpleir_dynamic_power_semantics_kinds",
    "simpleir_integer_only_semantics_kinds",
    "simpleir_integer_producer_semantics_kinds",
    "simpleir_identity_semantics_kinds",
    "simpleir_tuple_semantics_kinds",
    "simpleir_exception_semantics_kinds",
    "simpleir_deterministic_lifetime_semantics_kinds",
    "simpleir_frame_introspection_semantics_kinds",
    "simpleir_format_protocol_semantics_kinds",
    "simpleir_iterable_protocol_semantics_kinds",
    "simpleir_object_model_semantics_kinds",
    "simpleir_truthiness_semantics_kinds",
    "simpleir_comparison_semantics_kinds",
    "simpleir_fallible_protocol_semantics_kinds",
    "simpleir_async_runtime_semantics_kinds",
    "simpleir_unstructured_control_semantics_kinds",
    "simpleir_host_capability_semantics_kinds",
)
_PRE_SOURCE_LITERAL_LIMITED = _kind_set(
    "simpleir_integer_literal_semantics_kinds",
)
_PRE_SOURCE_TYPE_LIMITED = _kind_set(
    "simpleir_dynamic_add_semantics_kinds",
    "simpleir_dynamic_numeric_semantics_kinds",
    "simpleir_dynamic_true_div_semantics_kinds",
    "simpleir_dynamic_unary_numeric_semantics_kinds",
)
_REGISTERED_SIMPLEIR_KINDS = {
    spelling
    for row in _OP_KIND_TABLE.get("kind", [])
    for spelling in (row["canonical"], *row.get("aliases", []))
}
_REGISTERED_SIMPLEIR_KINDS.update(
    _kind_set(
        "simpleir_execution_frame_semantics_kinds",
        "simpleir_frame_introspection_semantics_kinds",
    )
)
for _table in ("simpleir_control_kind", "frontend_effect_kind"):
    _REGISTERED_SIMPLEIR_KINDS.update(
        row["kind"] for row in _OP_KIND_TABLE.get(_table, [])
    )
_REGISTERED_SIMPLEIR_KINDS.update(_PRE_SOURCE_NOT_ADMITTED)
_REGISTERED_SIMPLEIR_KINDS.update(_PRE_SOURCE_LITERAL_LIMITED)
_REGISTERED_SIMPLEIR_KINDS.update(_PRE_SOURCE_TYPE_LIMITED)
_REGISTERED_SIMPLEIR_KINDS.update(
    _kind_set("simpleir_runtime_neutral_semantics_kinds")
)


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
_EMIT_OP_FN_RE = re.compile(
    r"\bfn\s+emit[A-Za-z0-9_]*\s*\(\s*&mut\s+self\s*,\s*op:\s*&OpIR\s*\)"
)


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


def _extract_emit_op_matches(text: str) -> list[str]:
    """Return op-kind match bodies from Luau emitter functions.

    The Luau backend is decomposed by op family. The support matrix is an
    authority over that whole emitter cluster, so it must follow every
    `emit_*_op(&mut self, op: &OpIR)` function rather than a single façade.
    """
    matches: list[str] = []
    for fn_match in _EMIT_OP_FN_RE.finditer(text):
        fn_open_idx = text.find("{", fn_match.end())
        if fn_open_idx < 0:
            continue
        fn_close_idx = _find_matching_brace(text, fn_open_idx)
        cursor = fn_open_idx
        while True:
            match_start = text.find("match op.kind.as_str()", cursor, fn_close_idx)
            if match_start < 0:
                break
            open_idx = text.find("{", match_start, fn_close_idx)
            if open_idx < 0:
                break
            close_idx = _find_matching_brace(text, open_idx)
            matches.append(text[open_idx + 1 : close_idx])
            cursor = close_idx + 1
    return matches


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
        body_started = body_started or bool(
            _strip_rust_strings_and_comments(line).strip()
        )
        if body_started and body_depth == 0:
            arms.append((current_ops, "\n".join(current_body)))
            current_ops = None
            current_body = []

    if current_ops is not None:
        arms.append((current_ops, "\n".join(current_body)))
    if pending_pattern:
        raise ValueError(
            f"unterminated Luau emit_op match arm pattern: {pending_pattern!r}"
        )
    return arms


def _classify(op: str, body: str) -> Row:
    if op not in _REGISTERED_SIMPLEIR_KINDS:
        return Row(
            op,
            "not-admitted",
            "Operation is unclassified in the generated target-contract authority.",
        )
    if op in _PRE_SOURCE_NOT_ADMITTED:
        return Row(
            op,
            "not-admitted",
            "Shared generated target contract rejects this semantic family before source generation.",
        )
    if op in _PRE_SOURCE_LITERAL_LIMITED:
        return Row(
            op,
            "implemented-target-limited",
            "Shared target contract admits only concrete integer literals exactly representable by Luau's numeric carrier.",
        )
    if op in _PRE_SOURCE_TYPE_LIMITED:
        return Row(
            op,
            "implemented-target-limited",
            "Shared target contract admits only representation-proven non-integer scalar domains.",
        )
    if "-- [unsupported op:" in body or 'error(\\"[unsupported op:' in body:
        return Row(
            op, "compile-error", "Checked Luau emission rejects unsupported markers."
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

    return Row(
        op,
        "implemented-exact",
        "Lowered and outside every generated target-contract limitation.",
    )


def collect_rows_from_text(text: str) -> list[Row]:
    by_op: dict[str, Row] = {}
    match_texts = _extract_emit_op_matches(text)
    if not match_texts:
        raise ValueError("could not find Luau emitter op-kind match arms")
    for match_text in match_texts:
        for ops, body in _iter_arms(match_text):
            for op in ops:
                by_op[op] = _classify(op, body)

    rows = sorted(by_op.values(), key=lambda row: row.op)
    bad_statuses = sorted({row.status for row in rows} - STATUSES)
    if bad_statuses:
        raise ValueError(f"unknown statuses produced: {', '.join(bad_statuses)}")
    return rows


def _source_files(source: Path) -> list[Path]:
    if source.is_dir():
        return sorted(
            path
            for path in source.rglob("*.rs")
            if "tests" not in path.relative_to(source).parts
        )
    return [source]


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
            "- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.",
            "",
        ]
    )
    return "\n".join(lines)


def build_output(source: Path) -> str:
    text = "\n".join(path.read_text(encoding="utf-8") for path in _source_files(source))
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
        write_generated_text(args.output, output)
        print(f"wrote {args.output}")
        return 0
    if args.check:
        if not generated_file_matches(args.output, output):
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
