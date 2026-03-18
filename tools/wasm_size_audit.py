#!/usr/bin/env python3
"""Analyse WASM module size breakdown by section (MOL-211).

Parses the binary section headers of a ``.wasm`` file and reports per-section
sizes, identifies the largest contributors, and suggests optimisation
opportunities.

Includes a configurable size-budget gate (MOL-183/MOL-186) that can fail CI
when a linked WASM artifact exceeds the budget — preventing V8 OOM regressions.

Usage::

    python tools/wasm_size_audit.py path/to/module.wasm
    python tools/wasm_size_audit.py path/to/module.wasm --json
    python tools/wasm_size_audit.py path/to/module.wasm --budget 8MB
    python tools/wasm_size_audit.py path/to/module.wasm --budget 8MB --budget-code 4MB
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

# WASM well-known section IDs.
SECTION_NAMES: dict[int, str] = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "data_count",
}

WASM_MAGIC = b"\x00asm"

# Default size budgets (overridable via CLI or env vars).
# These are chosen to stay well below V8's compilation memory limits:
# V8 allocates ~10x the module size during TurboFan compilation, so a
# 16 MB module needs ~160 MB of compilation memory.  With --liftoff-only
# the multiplier drops to ~3x, but we still want headroom.
DEFAULT_TOTAL_BUDGET_MB = 16.0
DEFAULT_CODE_BUDGET_MB = 10.0
DEFAULT_DATA_BUDGET_MB = 4.0


# ---------------------------------------------------------------------------
# LEB128 helper
# ---------------------------------------------------------------------------

def _read_leb128_u32(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128 value. Returns (value, new_offset)."""
    result = 0
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if (byte & 0x80) == 0:
            break
        shift += 7
    return result, offset


def _parse_size_spec(spec: str) -> int:
    """Parse a human-readable size spec like '8MB', '512KB', '4096' into bytes."""
    spec = spec.strip().upper()
    if spec.endswith("GB"):
        return int(float(spec[:-2]) * 1024 * 1024 * 1024)
    if spec.endswith("MB"):
        return int(float(spec[:-2]) * 1024 * 1024)
    if spec.endswith("KB"):
        return int(float(spec[:-2]) * 1024)
    if spec.endswith("B"):
        return int(spec[:-1])
    return int(spec)


# ---------------------------------------------------------------------------
# Section parsing
# ---------------------------------------------------------------------------

class SectionInfo:
    """Metadata for one WASM section."""

    __slots__ = ("id", "name", "offset", "size", "custom_name")

    def __init__(self, *, id: int, name: str, offset: int, size: int, custom_name: str = ""):
        self.id = id
        self.name = name
        self.offset = offset
        self.size = size
        self.custom_name = custom_name

    def to_dict(self) -> dict:
        d: dict = {
            "id": self.id,
            "name": self.name,
            "offset": self.offset,
            "size": self.size,
        }
        if self.custom_name:
            d["custom_name"] = self.custom_name
        return d


def parse_sections(wasm_path: Path) -> list[SectionInfo]:
    """Parse all WASM binary sections and return metadata."""
    data = wasm_path.read_bytes()
    if len(data) < 8 or data[:4] != WASM_MAGIC:
        raise ValueError(f"{wasm_path} is not a valid WASM binary")

    offset = 8  # skip magic + version
    sections: list[SectionInfo] = []

    while offset < len(data):
        sec_id = data[offset]
        offset += 1
        sec_size, offset = _read_leb128_u32(data, offset)
        sec_start = offset

        name = SECTION_NAMES.get(sec_id, f"unknown({sec_id})")
        custom_name = ""

        # For custom sections, read the name field
        if sec_id == 0 and sec_size > 0:
            try:
                name_len, name_offset = _read_leb128_u32(data, sec_start)
                custom_name = data[name_offset : name_offset + name_len].decode(
                    "utf-8", errors="replace"
                )
            except (IndexError, UnicodeDecodeError):
                custom_name = "<unparseable>"

        sections.append(
            SectionInfo(
                id=sec_id,
                name=name,
                offset=sec_start,
                size=sec_size,
                custom_name=custom_name,
            )
        )
        offset += sec_size

    return sections


# ---------------------------------------------------------------------------
# Analysis and suggestions
# ---------------------------------------------------------------------------

def suggest_optimisations(
    sections: list[SectionInfo], total_bytes: int
) -> list[str]:
    """Return a list of optimisation suggestions based on section sizes."""
    suggestions: list[str] = []
    code_size = sum(s.size for s in sections if s.name == "code")
    data_size = sum(s.size for s in sections if s.name == "data")
    custom_size = sum(s.size for s in sections if s.name == "custom")

    # Find custom section names
    custom_names = [s.custom_name for s in sections if s.name == "custom" and s.custom_name]

    if code_size > total_bytes * 0.5:
        suggestions.append(
            f"Code section is {code_size / 1024:.0f} KB "
            f"({code_size / total_bytes * 100:.1f}% of total). "
            "Run wasm-opt -O2 or -Oz to shrink generated code."
        )

    if data_size > total_bytes * 0.2:
        suggestions.append(
            f"Data section is {data_size / 1024:.0f} KB "
            f"({data_size / total_bytes * 100:.1f}% of total). "
            "Consider compressing constant data or lazy-loading."
        )

    if custom_size > 64 * 1024:
        suggestions.append(
            f"Custom sections total {custom_size / 1024:.0f} KB. "
            "Strip debug/name sections with: wasm-opt --strip-debug --strip-producers"
        )
        if any("name" in n for n in custom_names):
            suggestions.append(
                "A 'name' custom section is present. Strip it for production builds."
            )

    linking_sections = [s for s in sections if s.custom_name == "linking"]
    if linking_sections:
        link_size = sum(s.size for s in linking_sections)
        suggestions.append(
            f"Linking section is {link_size / 1024:.0f} KB. "
            "This is expected for relocatable modules; it is removed after linking."
        )

    reloc_sections = [s for s in sections if s.custom_name.startswith("reloc.")]
    if reloc_sections:
        reloc_size = sum(s.size for s in reloc_sections)
        suggestions.append(
            f"Relocation sections total {reloc_size / 1024:.0f} KB. "
            "Removed after linking; not present in final binaries."
        )

    # V8 OOM risk analysis (MOL-183/MOL-186)
    estimated_v8_mem_mb = total_bytes / 1024 / 1024 * 10  # ~10x for TurboFan
    estimated_liftoff_mem_mb = total_bytes / 1024 / 1024 * 3  # ~3x for Liftoff
    if estimated_v8_mem_mb > 512:
        suggestions.append(
            f"V8 OOM RISK: Module is {total_bytes / 1024 / 1024:.1f} MB. "
            f"Estimated TurboFan compilation memory: ~{estimated_v8_mem_mb:.0f} MB. "
            f"Liftoff-only: ~{estimated_liftoff_mem_mb:.0f} MB. "
            "Use --liftoff-only flag or reduce module size."
        )
    elif estimated_v8_mem_mb > 256:
        suggestions.append(
            f"V8 memory warning: Module is {total_bytes / 1024 / 1024:.1f} MB. "
            f"Estimated compilation memory: ~{estimated_v8_mem_mb:.0f} MB. "
            "Consider stripping debug sections and running wasm-opt."
        )

    if not suggestions:
        suggestions.append("No obvious size issues detected.")

    return suggestions


# ---------------------------------------------------------------------------
# Size budget gate (MOL-183/MOL-186)
# ---------------------------------------------------------------------------

class BudgetViolation:
    """A single budget violation."""

    __slots__ = ("category", "actual_bytes", "budget_bytes")

    def __init__(self, category: str, actual_bytes: int, budget_bytes: int):
        self.category = category
        self.actual_bytes = actual_bytes
        self.budget_bytes = budget_bytes

    def __str__(self) -> str:
        over = self.actual_bytes - self.budget_bytes
        return (
            f"{self.category}: {self.actual_bytes / 1024 / 1024:.2f} MB "
            f"exceeds budget of {self.budget_bytes / 1024 / 1024:.2f} MB "
            f"(over by {over / 1024:.1f} KB)"
        )

    def to_dict(self) -> dict:
        return {
            "category": self.category,
            "actual_bytes": self.actual_bytes,
            "budget_bytes": self.budget_bytes,
            "over_bytes": self.actual_bytes - self.budget_bytes,
        }


def check_budget(
    sections: list[SectionInfo],
    total_bytes: int,
    *,
    total_budget: int | None = None,
    code_budget: int | None = None,
    data_budget: int | None = None,
) -> list[BudgetViolation]:
    """Check if the module exceeds any size budgets.

    Returns a list of violations (empty if all budgets pass).
    Budgets are specified in bytes.  Pass None to skip a check.
    """
    violations: list[BudgetViolation] = []

    if total_budget is not None and total_bytes > total_budget:
        violations.append(BudgetViolation("total", total_bytes, total_budget))

    code_size = sum(s.size for s in sections if s.name == "code")
    if code_budget is not None and code_size > code_budget:
        violations.append(BudgetViolation("code", code_size, code_budget))

    data_size = sum(s.size for s in sections if s.name == "data")
    if data_budget is not None and data_size > data_budget:
        violations.append(BudgetViolation("data", data_size, data_budget))

    return violations


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

def print_report(sections: list[SectionInfo], total_bytes: int) -> None:
    """Print a human-readable size audit."""
    print("=" * 72)
    print(f"WASM Module Size Audit -- {total_bytes:,} bytes ({total_bytes / 1024 / 1024:.2f} MB)")
    print("=" * 72)

    # Group by section type
    by_type: dict[str, int] = {}
    for sec in sections:
        key = sec.name
        if sec.custom_name:
            key = f"custom:{sec.custom_name}"
        by_type[key] = by_type.get(key, 0) + sec.size

    # Sort by size descending
    sorted_types = sorted(by_type.items(), key=lambda kv: -kv[1])

    print(f"\n{'Section':<35s} {'Size':>12s}  {'%':>6s}")
    print("-" * 57)
    for name, size in sorted_types:
        pct = size / total_bytes * 100 if total_bytes > 0 else 0
        bar = "#" * int(pct / 2)
        print(f"  {name:<33s} {size:>10,}  {pct:>5.1f}%  {bar}")

    # Header overhead (8 bytes magic+version + section headers)
    accounted = sum(s.size for s in sections)
    overhead = total_bytes - accounted
    if overhead > 0:
        print(f"  {'<headers/padding>':<33s} {overhead:>10,}  {overhead / total_bytes * 100:>5.1f}%")

    print("-" * 57)
    print(f"  {'TOTAL':<33s} {total_bytes:>10,}")

    # Suggestions
    suggestions = suggest_optimisations(sections, total_bytes)
    print("\n--- Optimisation Suggestions ---")
    for i, s in enumerate(suggestions, 1):
        print(f"  {i}. {s}")
    print()


def print_json_report(
    sections: list[SectionInfo],
    total_bytes: int,
    violations: list[BudgetViolation] | None = None,
) -> None:
    """Print a machine-readable JSON audit."""
    by_type: dict[str, int] = {}
    for sec in sections:
        key = sec.name
        if sec.custom_name:
            key = f"custom:{sec.custom_name}"
        by_type[key] = by_type.get(key, 0) + sec.size

    data: dict = {
        "total_bytes": total_bytes,
        "total_kb": round(total_bytes / 1024, 1),
        "total_mb": round(total_bytes / 1024 / 1024, 2),
        "sections": [s.to_dict() for s in sections],
        "by_type": dict(sorted(by_type.items(), key=lambda kv: -kv[1])),
        "suggestions": suggest_optimisations(sections, total_bytes),
    }
    if violations is not None:
        data["budget_violations"] = [v.to_dict() for v in violations]
        data["budget_ok"] = len(violations) == 0
    print(json.dumps(data, indent=2))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Audit WASM module size by section")
    parser.add_argument("wasm", type=Path, help="Path to .wasm file")
    parser.add_argument("--json", action="store_true", dest="json_output", help="JSON output")
    parser.add_argument(
        "--budget",
        type=str,
        default=None,
        help=(
            "Total size budget (e.g. '16MB', '8192KB', '16777216'). "
            "Exit non-zero if exceeded. "
            f"Default when --budget-check is used: {DEFAULT_TOTAL_BUDGET_MB:.0f}MB. "
            "Override with MOLT_WASM_BUDGET env var."
        ),
    )
    parser.add_argument(
        "--budget-code",
        type=str,
        default=None,
        help="Code section size budget (e.g. '10MB'). Override with MOLT_WASM_BUDGET_CODE env var.",
    )
    parser.add_argument(
        "--budget-data",
        type=str,
        default=None,
        help="Data section size budget (e.g. '4MB'). Override with MOLT_WASM_BUDGET_DATA env var.",
    )
    parser.add_argument(
        "--budget-check",
        action="store_true",
        help="Enable budget checking with default limits (can be overridden by --budget* flags)",
    )
    args = parser.parse_args()

    if not args.wasm.is_file():
        print(f"ERROR: {args.wasm} not found", file=sys.stderr)
        sys.exit(1)

    total_bytes = args.wasm.stat().st_size
    sections = parse_sections(args.wasm)

    # Resolve budgets: CLI > env var > default (if --budget-check)
    total_budget = None
    code_budget = None
    data_budget = None

    if args.budget:
        total_budget = _parse_size_spec(args.budget)
    elif os.environ.get("MOLT_WASM_BUDGET"):
        total_budget = _parse_size_spec(os.environ["MOLT_WASM_BUDGET"])
    elif args.budget_check:
        total_budget = int(DEFAULT_TOTAL_BUDGET_MB * 1024 * 1024)

    if args.budget_code:
        code_budget = _parse_size_spec(args.budget_code)
    elif os.environ.get("MOLT_WASM_BUDGET_CODE"):
        code_budget = _parse_size_spec(os.environ["MOLT_WASM_BUDGET_CODE"])
    elif args.budget_check:
        code_budget = int(DEFAULT_CODE_BUDGET_MB * 1024 * 1024)

    if args.budget_data:
        data_budget = _parse_size_spec(args.budget_data)
    elif os.environ.get("MOLT_WASM_BUDGET_DATA"):
        data_budget = _parse_size_spec(os.environ["MOLT_WASM_BUDGET_DATA"])
    elif args.budget_check:
        data_budget = int(DEFAULT_DATA_BUDGET_MB * 1024 * 1024)

    # Run budget checks
    violations: list[BudgetViolation] | None = None
    any_budget = total_budget is not None or code_budget is not None or data_budget is not None
    if any_budget:
        violations = check_budget(
            sections,
            total_bytes,
            total_budget=total_budget,
            code_budget=code_budget,
            data_budget=data_budget,
        )

    if args.json_output:
        print_json_report(sections, total_bytes, violations)
    else:
        print_report(sections, total_bytes)
        if violations:
            print("--- SIZE BUDGET VIOLATIONS ---")
            for v in violations:
                print(f"  FAIL: {v}")
            print()
        elif any_budget:
            print("--- Size budget: PASS ---\n")

    # Exit non-zero on budget violations
    if violations:
        sys.exit(1)


if __name__ == "__main__":
    main()
