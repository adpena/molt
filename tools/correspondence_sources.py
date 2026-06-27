"""Shared source parsers for Molt formal/code correspondence checks."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Mapping


def read_source(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    return ""


def read_required_source(path: Path) -> str:
    text = read_source(path)
    if not text:
        raise FileNotFoundError(f"Source file not found: {path}")
    return text


def parse_lean_hex_constants(text: str) -> dict[str, int]:
    result: dict[str, int] = {}
    for match in re.finditer(
        r"def\s+(\w+)\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F_]+)",
        text,
    ):
        result[match.group(1)] = int(match.group(2).replace("_", ""), 16)
    return result


def parse_lean_inductive_variants(text: str, type_name: str) -> list[str]:
    match = re.search(rf"inductive\s+{type_name}\s+where", text)
    if not match:
        return []

    variants: list[str] = []
    for line in text[match.end() :].split("\n"):
        stripped = line.strip()
        if not stripped or stripped.startswith("--"):
            continue
        if stripped.startswith("deriving"):
            break
        if re.match(
            r"^(inductive|def|theorem|structure|namespace|end|abbrev|section)\b",
            stripped,
        ):
            break
        for variant_match in re.finditer(r"\|\s*\.?(\w+)", stripped):
            name = normalize_lean_variant_name(variant_match.group(1))
            if name not in variants:
                variants.append(name)
    return variants


def normalize_lean_variant_name(name: str) -> str:
    return name[:-1] if name.endswith("_") else name


def parse_lean_builtin_mappings(text: str) -> list[tuple[str, str]]:
    return re.findall(r'\("(\w+)",\s*"([^"]+)"\)', text)


def parse_lean_eval_binop_rules(text: str) -> list[tuple[str, str, str]]:
    rules: list[tuple[str, str, str]] = []
    for match in re.finditer(
        r"\|\s*\.(\w+),\s*\.(\w+)\s+\w+,\s*\.(\w+)\s+\w+\s*=>",
        text,
    ):
        rules.append((match.group(1), match.group(2), match.group(3)))
    return rules


def parse_lean_eval_unop_rules(text: str) -> list[tuple[str, str]]:
    rules: list[tuple[str, str]] = []
    for match in re.finditer(r"\|\s*\.(\w+),\s*\.(\w+)\s+\w+\s*=>", text):
        rules.append((match.group(1), match.group(2)))
    return rules


def parse_rust_unsigned_constant_expressions(text: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for match in re.finditer(
        r"(?:pub\s+)?const\s+(\w+):\s*u(?:32|64)\s*=\s*(.+?);",
        text,
    ):
        result[match.group(1)] = match.group(2).strip()
    return result


def parse_rust_unsigned_constants(text: str, *, strict: bool = False) -> dict[str, int]:
    raw = parse_rust_unsigned_constant_expressions(text)
    resolved: dict[str, int] = {}
    for name, expr in raw.items():
        try:
            resolved[name] = resolve_rust_unsigned_expr(expr, raw)
        except ValueError:
            if strict:
                raise
    return resolved


def resolve_rust_or_hex_int(expr: str, *, rust_text: str = "") -> int:
    raw = parse_rust_unsigned_constant_expressions(rust_text) if rust_text else {}
    return resolve_rust_unsigned_expr(expr, raw)


def resolve_rust_unsigned_expr(
    expr: str,
    raw_consts: Mapping[str, str] | None = None,
) -> int:
    raw_consts = raw_consts or {}
    expr = expr.strip().rstrip(";")

    cast = re.fullmatch(r"(\w+)\s+as\s+u(?:32|64)", expr)
    if cast:
        expr = cast.group(1)

    if re.fullmatch(r"0x[0-9a-fA-F_]+", expr):
        return int(expr.replace("_", ""), 16)
    if expr.isdigit():
        return int(expr)
    if re.fullmatch(r"\w+", expr) and expr in raw_consts:
        return resolve_rust_unsigned_expr(raw_consts[expr], raw_consts)

    shifted_mask = re.fullmatch(r"\(1u64\s*<<\s*(.+)\)\s*-\s*1", expr)
    if shifted_mask:
        return (1 << _resolve_rust_shift_width(shifted_mask.group(1), raw_consts)) - 1

    shift = re.fullmatch(r"1(?:u64)?\s*<<\s*(.+)", expr)
    if shift:
        return 1 << _resolve_rust_shift_width(shift.group(1), raw_consts)

    if re.fullmatch(r"[0-9a-fA-F_]+", expr):
        return int(expr.replace("_", ""), 16)
    raise ValueError(f"Cannot parse Rust unsigned constant expression: {expr}")


def _resolve_rust_shift_width(expr: str, raw_consts: Mapping[str, str]) -> int:
    expr = expr.strip()
    if expr.startswith("(") and expr.endswith(")"):
        expr = expr[1:-1].strip()
    match = re.fullmatch(r"(\w+)(?:\s*-\s*(\d+))?", expr)
    if not match:
        raise ValueError(f"Cannot parse Rust shift width: {expr}")

    base, decrement = match.group(1), match.group(2)
    if base.isdigit():
        width = int(base)
    elif base in raw_consts:
        width = resolve_rust_unsigned_expr(raw_consts[base], raw_consts)
    else:
        raise ValueError(f"Cannot resolve Rust shift variable: {base}")

    if decrement:
        width -= int(decrement)
    return width
