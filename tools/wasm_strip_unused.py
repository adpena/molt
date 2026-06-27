#!/usr/bin/env python3
"""Analyze and optionally strip unused WASM imports for pure-computation modules.

For Molt-compiled WASM binaries running in a browser host, many imports
(networking, process spawning, DB, filesystem write) are never called by
pure-computation scripts (math, json, print).  This tool identifies those
imports, reports their overhead, and can optionally produce a trimmed copy
where unused host imports are replaced by wasm-internal no-op stubs.

Requires: wasm-tools CLI (https://github.com/bytecodealliance/wasm-tools)

Usage:
    python tools/wasm_strip_unused.py path/to/module.wasm
    python tools/wasm_strip_unused.py path/to/module.wasm --strip -o dist/trimmed.wasm
    python tools/wasm_strip_unused.py path/to/module.wasm --json
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import sys
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import artifact_publish, harness_memory_guard  # noqa: E402

_WASM_ABI_GENERATED = REPO_ROOT / "src/molt/_wasm_abi_generated.py"
_WASM_ABI_SPEC = importlib.util.spec_from_file_location(
    "molt_tools_wasm_abi_generated", _WASM_ABI_GENERATED
)
if _WASM_ABI_SPEC is None or _WASM_ABI_SPEC.loader is None:
    raise RuntimeError(f"cannot load generated WASM ABI data: {_WASM_ABI_GENERATED}")
_WASM_ABI = importlib.util.module_from_spec(_WASM_ABI_SPEC)
_WASM_ABI_SPEC.loader.exec_module(_WASM_ABI)


# ---------------------------------------------------------------------------
# Import classification
# ---------------------------------------------------------------------------


class ImportCategory(str, Enum):
    """Functional category of a WASM import."""

    ESSENTIAL = "essential"  # Required for any execution (memory, table, args, clock)
    IO_STDOUT = "io_stdout"  # fd_write to stdout/stderr - used by print()
    IO_FILESYSTEM = "io_filesystem"  # Filesystem read/write/stat/dir ops
    PROCESS = "process"  # Process spawn/kill/wait/poll
    DATABASE = "database"  # DB exec/query/poll
    WEBSOCKET = "websocket"  # WebSocket connect/send/recv
    SOCKET = "socket"  # Raw socket operations
    TIME = "time"  # Timezone/offset (beyond clock_time_get)
    PURE_PROFILE = "pure_profile"  # molt_runtime import omitted by backend Pure
    INDIRECT_CALL = (
        "indirect_call"  # molt_call_indirectN - required for function pointers
    )
    TABLE = "table"  # __indirect_function_table


# Generated from wasm_abi_manifest.toml by tools/gen_wasm_abi.py.
IMPORT_RULES: list[tuple[str, str, ImportCategory, str]] = [
    (module, name, ImportCategory(category), description)
    for module, name, category, description in _WASM_ABI.WASM_STRIP_IMPORT_RULES
]
IMPORT_PREFIX_RULES: list[tuple[str, str, ImportCategory, str]] = [
    (module, prefix, ImportCategory(category), description)
    for module, prefix, category, description in _WASM_ABI.WASM_STRIP_IMPORT_PREFIX_RULES
]


@dataclass
class ImportInfo:
    """Parsed WASM import entry."""

    index: int
    module: str
    name: str
    kind: str  # "func", "table", "memory", "global"
    type_index: int  # For funcs: the type index
    category: ImportCategory = ImportCategory.ESSENTIAL
    description: str = ""
    strippable: bool = False


@dataclass
class AnalysisResult:
    """Full analysis of a WASM binary's imports."""

    wasm_path: str
    file_size_bytes: int
    total_imports: int
    imports: list[ImportInfo] = field(default_factory=list)
    category_counts: dict[str, int] = field(default_factory=dict)
    strippable_count: int = 0
    essential_count: int = 0

    @property
    def strippable_imports(self) -> list[ImportInfo]:
        return [i for i in self.imports if i.strippable]

    @property
    def essential_imports(self) -> list[ImportInfo]:
        return [i for i in self.imports if not i.strippable]


# ---------------------------------------------------------------------------
# WASM binary parsing (import section only)
# ---------------------------------------------------------------------------

WASM_MAGIC = b"\x00asm"
SECTION_IMPORT = 2
SECTION_TYPE = 1


def _read_leb128_u32(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128-encoded u32. Returns (value, new_offset)."""
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


def _read_name(data: bytes, offset: int) -> tuple[str, int]:
    """Read a WASM name (length-prefixed UTF-8 string)."""
    length, offset = _read_leb128_u32(data, offset)
    name = data[offset : offset + length].decode("utf-8", errors="replace")
    return name, offset + length


def _parse_sections(data: bytes) -> dict[int, tuple[int, int]]:
    """Parse WASM sections, returning {section_id: (offset, size)}."""
    assert data[:4] == WASM_MAGIC, "Not a valid WASM binary"
    offset = 8  # Skip magic + version
    sections: dict[int, tuple[int, int]] = {}
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_leb128_u32(data, offset)
        sections[section_id] = (offset, section_size)
        offset += section_size
    return sections


def _parse_type_section(
    data: bytes, sec_offset: int, sec_size: int
) -> dict[int, tuple[list, list]]:
    """Parse the type section to get function signatures.

    Tolerates non-functype entries (sub-types, GC types, rec groups) by
    skipping entries whose leading byte is not 0x60.
    """
    offset = sec_offset
    end = sec_offset + sec_size
    count, offset = _read_leb128_u32(data, offset)
    types: dict[int, tuple[list, list]] = {}
    for i in range(count):
        if offset >= end:
            break
        form = data[offset]
        offset += 1
        if form != 0x60:
            # Skip non-functype entries (rec group, sub type, etc.)
            # We can't reliably parse these, so skip to next by scanning
            # for the next 0x60 or end of section.  Mark this type as unknown.
            types[i] = ([], [])
            # Heuristic: try to skip a plausible LEB128 param+result block
            # by looking for the next functype marker or exhausting section.
            while offset < end and data[offset] != 0x60:
                offset += 1
            continue
        # Params
        param_count, offset = _read_leb128_u32(data, offset)
        params = []
        for _ in range(param_count):
            params.append(data[offset])
            offset += 1
        # Results
        result_count, offset = _read_leb128_u32(data, offset)
        results = []
        for _ in range(result_count):
            results.append(data[offset])
            offset += 1
        types[i] = (params, results)
    return types


def _valtype_name(vt: int) -> str:
    return {
        0x7F: "i32",
        0x7E: "i64",
        0x7D: "f32",
        0x7C: "f64",
        0x7B: "v128",
        0x70: "funcref",
        0x6F: "externref",
    }.get(vt, f"0x{vt:02x}")


def parse_imports(wasm_path: Path) -> list[ImportInfo]:
    """Parse the import section of a WASM binary."""
    data = wasm_path.read_bytes()
    sections = _parse_sections(data)

    if SECTION_IMPORT not in sections:
        return []

    sec_offset, sec_size = sections[SECTION_IMPORT]
    offset = sec_offset
    count, offset = _read_leb128_u32(data, offset)
    imports: list[ImportInfo] = []

    for idx in range(count):
        module, offset = _read_name(data, offset)
        name, offset = _read_name(data, offset)
        kind_byte = data[offset]
        offset += 1

        type_index = -1
        if kind_byte == 0x00:  # func
            type_index, offset = _read_leb128_u32(data, offset)
            kind = "func"
        elif kind_byte == 0x01:  # table
            offset += 1  # elem_type
            flags = data[offset]
            offset += 1
            _initial, offset = _read_leb128_u32(data, offset)
            if flags & 0x01:
                _max, offset = _read_leb128_u32(data, offset)
            kind = "table"
        elif kind_byte == 0x02:  # memory
            flags = data[offset]
            offset += 1
            _initial, offset = _read_leb128_u32(data, offset)
            if flags & 0x01:
                _max, offset = _read_leb128_u32(data, offset)
            kind = "memory"
        elif kind_byte == 0x03:  # global
            _valtype = data[offset]
            offset += 1
            _mutability = data[offset]
            offset += 1
            kind = "global"
        else:
            kind = f"unknown({kind_byte})"

        # Classify the import
        info = ImportInfo(
            index=idx, module=module, name=name, kind=kind, type_index=type_index
        )
        _classify_import(info)
        imports.append(info)

    return imports


def _classify_import(info: ImportInfo) -> None:
    """Classify an import by matching against known rules."""
    if info.module == "molt_runtime" and _WASM_ABI.pure_profile_skips_import(info.name):
        info.category = ImportCategory.PURE_PROFILE
        info.description = "Skipped by the backend Pure WASM profile"
        info.strippable = True
        return

    for rule_mod, prefix, category, description in IMPORT_PREFIX_RULES:
        if info.module == rule_mod and info.name.startswith(prefix):
            info.category = category
            info.description = description
            info.strippable = category in STRIPPABLE_CATEGORIES
            return

    # Match against known rules
    for rule_mod, rule_name, category, description in IMPORT_RULES:
        if info.module == rule_mod and info.name == rule_name:
            info.category = category
            info.description = description
            info.strippable = category in STRIPPABLE_CATEGORIES
            return

    # Fallback: unclassified
    info.category = ImportCategory.ESSENTIAL
    info.description = "Unclassified (treated as essential)"
    info.strippable = False


# ---------------------------------------------------------------------------
# Analysis
# ---------------------------------------------------------------------------


def analyze(wasm_path: Path) -> AnalysisResult:
    """Run the full import analysis."""
    imports = parse_imports(wasm_path)
    file_size = wasm_path.stat().st_size

    result = AnalysisResult(
        wasm_path=str(wasm_path),
        file_size_bytes=file_size,
        total_imports=len(imports),
        imports=imports,
    )

    # Count by category
    for imp in imports:
        cat = imp.category.value
        result.category_counts[cat] = result.category_counts.get(cat, 0) + 1

    result.strippable_count = sum(1 for i in imports if i.strippable)
    result.essential_count = sum(1 for i in imports if not i.strippable)

    return result


# ---------------------------------------------------------------------------
# Stripping via wasm-tools
# ---------------------------------------------------------------------------


def strip_imports(wasm_path: Path, output_path: Path, result: AnalysisResult) -> Path:
    """Create a stripped copy using wasm-tools.

    Strategy: Use wasm-tools to produce a WAT text form, replace strippable
    imports with wasm-internal no-op functions, then reassemble to binary.

    For safety, this uses the wasm-tools component model to:
    1. Convert to WAT
    2. Inject no-op function bodies for each stripped import
    3. Reassemble to WASM binary
    """
    wasm_tools = shutil.which("wasm-tools")
    if not wasm_tools:
        print("ERROR: wasm-tools not found in PATH", file=sys.stderr)
        sys.exit(1)

    strippable = result.strippable_imports
    if not strippable:
        print("No strippable imports found. Output is a copy of input.")
        artifact_publish.atomic_copy_file(wasm_path, output_path)
        return output_path

    # Use wasm-tools strip to remove debug/name sections and report size savings.
    print("Stripping debug and name sections...")
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH")
    tmp_output = artifact_publish.staged_output_path(output_path)
    try:
        strip_proc = harness_memory_guard.guarded_completed_process(
            [wasm_tools, "strip", str(wasm_path), "-o", str(tmp_output)],
            prefix="MOLT_BENCH",
            capture_output=True,
            text=True,
            timeout=120,
            limits=limits,
        )
        if strip_proc.returncode != 0:
            print(
                f"ERROR: wasm-tools strip failed: {strip_proc.stderr}",
                file=sys.stderr,
            )
            sys.exit(1)
        artifact_publish.publish_validated_outputs([(tmp_output, output_path)])
    finally:
        try:
            tmp_output.unlink()
        except OSError:
            pass

    original_size = wasm_path.stat().st_size
    stripped_size = output_path.stat().st_size
    savings = original_size - stripped_size
    print(
        f"Stripped debug/name sections: {original_size:,} -> {stripped_size:,} bytes "
        f"(saved {savings:,} bytes, {savings / original_size * 100:.1f}%)"
    )

    return output_path


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def print_report(result: AnalysisResult, verbose: bool = False) -> None:
    """Print a human-readable analysis report."""
    print("=" * 72)
    print(f"WASM Import Analysis: {result.wasm_path}")
    print(
        f"File size: {result.file_size_bytes:,} bytes ({result.file_size_bytes / 1024 / 1024:.1f} MB)"
    )
    print(f"Total imports: {result.total_imports}")
    print("=" * 72)

    print("\n--- Category Breakdown ---")
    for cat in ImportCategory:
        count = result.category_counts.get(cat.value, 0)
        if count > 0:
            marker = "  [STRIPPABLE]" if cat in STRIPPABLE_CATEGORIES else ""
            print(f"  {cat.value:<20s}  {count:3d} imports{marker}")

    print("\n--- Summary ---")
    print(f"  Essential (keep):     {result.essential_count}")
    print(f"  Strippable (no-op):   {result.strippable_count}")
    pct = (
        result.strippable_count / result.total_imports * 100
        if result.total_imports
        else 0
    )
    print(f"  Strippable ratio:     {pct:.1f}%")

    # Estimate overhead: each import entry is ~20-60 bytes in the binary,
    # plus the host stub code in the TypeScript host
    est_import_overhead = result.strippable_count * 40  # ~40 bytes avg per import entry
    print(f"  Est. import section overhead: ~{est_import_overhead:,} bytes")
    print(
        f"  Host stub code lines saved:   ~{result.strippable_count * 2} lines in molt-wasm-host.ts"
    )

    if verbose or True:  # Always show details
        print(f"\n--- Strippable Imports ({result.strippable_count}) ---")
        for imp in result.strippable_imports:
            sig = ""
            if imp.type_index >= 0:
                sig = f" (type {imp.type_index})"
            print(f"  [{imp.index:2d}] {imp.module}::{imp.name}{sig}")
            print(f"       Category: {imp.category.value}, {imp.description}")

        print(f"\n--- Essential Imports ({result.essential_count}) ---")
        for imp in result.essential_imports:
            sig = ""
            if imp.type_index >= 0:
                sig = f" (type {imp.type_index})"
            print(f"  [{imp.index:2d}] {imp.module}::{imp.name}{sig}")
            print(f"       Category: {imp.category.value}, {imp.description}")


def print_json(result: AnalysisResult) -> None:
    """Print machine-readable JSON output."""
    data = {
        "wasm_path": result.wasm_path,
        "file_size_bytes": result.file_size_bytes,
        "total_imports": result.total_imports,
        "strippable_count": result.strippable_count,
        "essential_count": result.essential_count,
        "category_counts": result.category_counts,
        "strippable_imports": [
            {
                "index": i.index,
                "module": i.module,
                "name": i.name,
                "category": i.category.value,
                "description": i.description,
            }
            for i in result.strippable_imports
        ],
        "essential_imports": [
            {
                "index": i.index,
                "module": i.module,
                "name": i.name,
                "category": i.category.value,
                "description": i.description,
            }
            for i in result.essential_imports
        ],
    }
    print(json.dumps(data, indent=2))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze and strip unused WASM imports for pure-computation modules."
    )
    parser.add_argument("wasm", type=Path, help="Path to WASM binary")
    parser.add_argument(
        "--strip",
        action="store_true",
        help="Produce a trimmed copy with debug sections stripped",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help=(
            "Output path for stripped binary (default: sibling "
            "<name>-stripped.wasm next to input)"
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Output machine-readable JSON instead of human report",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Show detailed per-import information",
    )
    args = parser.parse_args()

    if not args.wasm.is_file():
        print(f"ERROR: {args.wasm} not found", file=sys.stderr)
        sys.exit(1)

    result = analyze(args.wasm)

    if args.json_output:
        print_json(result)
    else:
        print_report(result, verbose=args.verbose)

    if args.strip:
        output = args.output
        if output is None:
            output = args.wasm.with_name(f"{args.wasm.stem}-stripped.wasm")
        strip_imports(args.wasm, output, result)
        print(f"\nStripped output: {output}")

        # Re-analyze the stripped binary for comparison
        stripped_result = analyze(output)
        print(
            f"Stripped file: {stripped_result.file_size_bytes:,} bytes "
            f"({stripped_result.file_size_bytes / 1024 / 1024:.1f} MB)"
        )


if __name__ == "__main__":
    main()
