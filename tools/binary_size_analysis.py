#!/usr/bin/env python3
"""Binary size breakdown analysis for Molt native and WASM binaries.

Analyses what contributes to binary size — runtime, stdlib, user code — and
reports the largest symbols/sections.  Supports a ``--compare`` mode for
measuring the effect of optimisations across two builds.

Usage::

    # Native (Mach-O / ELF) binary
    python tools/binary_size_analysis.py .molt_cache/home/bin/bench_sum_molt

    # WASM binary
    python tools/binary_size_analysis.py target/wasm32-wasip1/release/bench_sum.wasm

    # Compare two builds
    python tools/binary_size_analysis.py --compare before.bin after.bin

    # JSON output
    python tools/binary_size_analysis.py --json .molt_cache/home/bin/bench_sum_molt

    # Custom size budget
    python tools/binary_size_analysis.py --budget 25MB .molt_cache/home/bin/bench_sum_molt
"""

from __future__ import annotations

import argparse
import json
import struct
import subprocess
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Symbol prefix categories for native binaries.
CATEGORIES: list[tuple[str, str, list[str]]] = [
    # (display_name, category_key, prefixes)
    ("Molt runtime", "molt_runtime", ["_molt_", "molt_", "_ZN4molt", "_ZN12molt_runtime", "_ZN11molt_python", "_ZN14molt_obj_model", "_ZN12molt_backend"]),
    ("Rust std/core", "rust_std", ["_ZN3std", "_ZN4core", "_ZN5alloc", "_ZN3syn", "_ZN5serde", "_ZN9hashbrown"]),
    ("RustPython parser", "rustpython", ["_ZN12rustpython"]),
    ("LLVM/codegen", "llvm_codegen", ["_ZN4llvm", "_ZN7craneli", "_ZN6wasmti", "_ZN6regall"]),
    ("C runtime/system", "c_runtime", ["_memcpy", "_memset", "_memmove", "_bzero", "___stack_chk", "_malloc", "_free", "_realloc"]),
]

# WASM section IDs.
WASM_SECTION_NAMES: dict[int, str] = {
    0: "custom", 1: "type", 2: "import", 3: "function", 4: "table",
    5: "memory", 6: "global", 7: "export", 8: "start", 9: "element",
    10: "code", 11: "data", 12: "data_count",
}

WASM_MAGIC = b"\x00asm"

# Default size budget (native ~30MB, WASM ~17MB — allow some headroom).
DEFAULT_BUDGET_NATIVE_MB = 35.0
DEFAULT_BUDGET_WASM_MB = 20.0


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _fmt_bytes(n: int) -> str:
    """Human-readable byte size."""
    if n >= 1024 * 1024:
        return f"{n / 1024 / 1024:.2f} MB"
    if n >= 1024:
        return f"{n / 1024:.1f} KB"
    return f"{n} B"


def _pct(part: int, total: int) -> str:
    if total == 0:
        return "0.0%"
    return f"{part / total * 100:.1f}%"


def _parse_size_spec(spec: str) -> int:
    """Parse '25MB', '512KB', etc. into bytes."""
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


def _read_leb128_u32(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128 value.  Returns (value, new_offset)."""
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


# ---------------------------------------------------------------------------
# Binary format detection
# ---------------------------------------------------------------------------

def detect_format(path: Path) -> str:
    """Return 'wasm', 'macho', 'elf', or 'unknown'."""
    with open(path, "rb") as f:
        magic = f.read(8)
    if len(magic) < 4:
        return "unknown"
    if magic[:4] == WASM_MAGIC:
        return "wasm"
    # Mach-O: 0xFEEDFACE (32-bit), 0xFEEDFACF (64-bit), or fat binary 0xCAFEBABE
    if magic[:4] in (b"\xfe\xed\xfa\xce", b"\xfe\xed\xfa\xcf",
                      b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe",
                      b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"):
        return "macho"
    # ELF
    if magic[:4] == b"\x7fELF":
        return "elf"
    return "unknown"


# ---------------------------------------------------------------------------
# Native binary analysis (Mach-O / ELF)
# ---------------------------------------------------------------------------

class SymbolInfo:
    __slots__ = ("name", "size", "kind")

    def __init__(self, name: str, size: int, kind: str = ""):
        self.name = name
        self.size = size
        self.kind = kind


def _categorise_symbol(name: str) -> tuple[str, str]:
    """Return (display_name, category_key) for a symbol name."""
    for display, key, prefixes in CATEGORIES:
        for p in prefixes:
            if name.startswith(p):
                return display, key
    return "Other / user code", "user_code"


def _parse_symbols_gnu(nm_output: str) -> list[SymbolInfo]:
    """Parse GNU nm --print-size output: <addr> <size> <type> <name>."""
    symbols: list[SymbolInfo] = []
    for line in nm_output.splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        try:
            size = int(parts[1], 16)
        except ValueError:
            continue
        kind = parts[2]
        name = parts[3]
        if size > 0:
            symbols.append(SymbolInfo(name, size, kind))
    return symbols


def _parse_symbols_from_addresses(nm_output: str) -> list[SymbolInfo]:
    """Compute symbol sizes from address-sorted nm output (macOS workaround).

    macOS nm always reports size=0 with --print-size, so we use ``nm -n``
    (numeric sort) and compute each symbol's size as the delta to the next
    symbol's address.
    """
    entries: list[tuple[int, str, str]] = []  # (addr, kind, name)
    for line in nm_output.splitlines():
        parts = line.split()
        if len(parts) < 3:
            continue
        try:
            addr = int(parts[0], 16)
        except ValueError:
            continue
        kind = parts[1]
        name = parts[2]
        entries.append((addr, kind, name))

    # Sort by address (should already be sorted from nm -n, but be safe).
    entries.sort(key=lambda e: e[0])

    symbols: list[SymbolInfo] = []
    for i, (addr, kind, name) in enumerate(entries):
        if i + 1 < len(entries):
            size = entries[i + 1][0] - addr
        else:
            size = 0
        if size > 0:
            symbols.append(SymbolInfo(name, size, kind))

    return symbols


def _parse_macho_segments(path: Path) -> list[dict]:
    """Parse ``size -m`` output for Mach-O segment/section breakdown."""
    try:
        result = subprocess.run(
            ["size", "-m", str(path)],
            capture_output=True, text=True, timeout=30,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return []

    segments: list[dict] = []
    current_seg: dict | None = None

    for line in result.stdout.splitlines():
        line = line.rstrip()
        if line.startswith("Segment "):
            # e.g. "Segment __TEXT: 28819456"
            rest = line[len("Segment "):]
            parts = rest.split(":")
            if len(parts) >= 2:
                seg_name = parts[0].strip()
                size_str = parts[1].strip().split()[0]
                try:
                    seg_size = int(size_str)
                except ValueError:
                    seg_size = 0
                current_seg = {"name": seg_name, "size": seg_size, "sections": []}
                segments.append(current_seg)
        elif line.strip().startswith("Section ") and current_seg is not None:
            # e.g. "	Section __text: 26191752"
            rest = line.strip()[len("Section "):]
            parts = rest.split(":")
            if len(parts) >= 2:
                sec_name = parts[0].strip()
                size_str = parts[1].strip().split()[0]
                try:
                    sec_size = int(size_str)
                except ValueError:
                    sec_size = 0
                current_seg["sections"].append({"name": sec_name, "size": sec_size})

    return segments


def analyse_native(path: Path) -> dict:
    """Analyse a native (Mach-O/ELF) binary via nm and size."""
    total_bytes = path.stat().st_size
    fmt = detect_format(path)

    # First try GNU nm with --print-size (works on Linux).
    symbols: list[SymbolInfo] = []
    try:
        result = subprocess.run(
            ["nm", "--print-size", "--size-sort", "--reverse-sort", str(path)],
            capture_output=True, text=True, timeout=60,
        )
        # Check if sizes are actually non-zero (macOS always gives 0).
        symbols = _parse_symbols_gnu(result.stdout)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # If GNU nm yielded no sized symbols, fall back to address-delta method.
    if not symbols:
        try:
            result = subprocess.run(
                ["nm", "-n", str(path)],
                capture_output=True, text=True, timeout=60,
            )
            symbols = _parse_symbols_from_addresses(result.stdout)
        except (FileNotFoundError, subprocess.TimeoutExpired):
            print("ERROR: 'nm' not found on PATH", file=sys.stderr)
            sys.exit(1)

    # Sort by size descending.
    symbols.sort(key=lambda s: -s.size)

    # Categorise symbols.
    cat_totals: dict[str, int] = {}
    cat_display: dict[str, str] = {}
    for sym in symbols:
        display, key = _categorise_symbol(sym.name)
        cat_totals[key] = cat_totals.get(key, 0) + sym.size
        cat_display[key] = display

    symbol_total = sum(s.size for s in symbols)

    # Parse Mach-O segment info if available.
    segments: list[dict] = []
    if fmt == "macho":
        segments = _parse_macho_segments(path)

    return {
        "format": "native",
        "path": str(path),
        "total_bytes": total_bytes,
        "symbol_total": symbol_total,
        "symbols": symbols,
        "category_totals": cat_totals,
        "category_display": cat_display,
        "segments": segments,
    }


def print_native_report(analysis: dict) -> None:
    total = analysis["total_bytes"]
    sym_total = analysis["symbol_total"]
    symbols = analysis["symbols"]
    cat_totals = analysis["category_totals"]
    cat_display = analysis["category_display"]
    segments = analysis.get("segments", [])

    print("=" * 76)
    print(f"Binary Size Analysis -- {analysis['path']}")
    print(f"Total file size: {_fmt_bytes(total)} ({total:,} bytes)")
    print("=" * 76)

    # Mach-O segment breakdown (if available)
    if segments:
        print(f"\n--- Mach-O Segments ---")
        print(f"{'Segment / Section':<40s} {'Size':>12s}  {'% of file':>9s}")
        print("-" * 65)
        for seg in segments:
            if seg["name"] == "__PAGEZERO":
                continue  # virtual, not on disk
            print(f"  {seg['name']:<38s} {_fmt_bytes(seg['size']):>12s}  {_pct(seg['size'], total):>9s}")
            for sec in seg.get("sections", []):
                label = f"    {sec['name']}"
                print(f"  {label:<38s} {_fmt_bytes(sec['size']):>12s}  {_pct(sec['size'], total):>9s}")
        print()

    # Category breakdown from symbol analysis
    print(f"--- Symbol Category Breakdown ---")
    print(f"{'Category':<30s} {'Size':>12s}  {'% of syms':>9s}  {'% of file':>9s}")
    print("-" * 65)
    for key, size in sorted(cat_totals.items(), key=lambda kv: -kv[1]):
        display = cat_display[key]
        print(f"  {display:<28s} {_fmt_bytes(size):>12s}  {_pct(size, sym_total):>9s}  {_pct(size, total):>9s}")

    unaccounted = total - sym_total
    if unaccounted > 0:
        print(f"  {'<headers/debug/unmapped>':<28s} {_fmt_bytes(unaccounted):>12s}  {'':>9s}  {_pct(unaccounted, total):>9s}")
    print("-" * 65)
    print(f"  {'Symbol total':<28s} {_fmt_bytes(sym_total):>12s}  {'100.0%':>9s}  {_pct(sym_total, total):>9s}")
    print(f"  {'File total':<28s} {_fmt_bytes(total):>12s}")

    # Top 50 largest symbols
    print(f"\n--- Top 50 Largest Symbols ---")
    print(f"{'#':>4s}  {'Size':>12s}  {'Category':<22s}  {'Symbol'}")
    print("-" * 76)
    for i, sym in enumerate(symbols[:50], 1):
        display, _ = _categorise_symbol(sym.name)
        # Demangle-friendly: truncate long names
        name = sym.name
        if len(name) > 80:
            name = name[:77] + "..."
        print(f"{i:>4d}  {_fmt_bytes(sym.size):>12s}  {display:<22s}  {name}")

    print()


# ---------------------------------------------------------------------------
# WASM binary analysis
# ---------------------------------------------------------------------------

class WasmSectionInfo:
    __slots__ = ("id", "name", "offset", "size", "custom_name")

    def __init__(self, *, id: int, name: str, offset: int, size: int, custom_name: str = ""):
        self.id = id
        self.name = name
        self.offset = offset
        self.size = size
        self.custom_name = custom_name


def _parse_wasm_sections(path: Path) -> list[WasmSectionInfo]:
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != WASM_MAGIC:
        raise ValueError(f"{path} is not a valid WASM binary")

    offset = 8
    sections: list[WasmSectionInfo] = []

    while offset < len(data):
        sec_id = data[offset]
        offset += 1
        sec_size, offset = _read_leb128_u32(data, offset)
        sec_start = offset

        name = WASM_SECTION_NAMES.get(sec_id, f"unknown({sec_id})")
        custom_name = ""

        if sec_id == 0 and sec_size > 0:
            try:
                name_len, name_offset = _read_leb128_u32(data, sec_start)
                custom_name = data[name_offset:name_offset + name_len].decode("utf-8", errors="replace")
            except (IndexError, UnicodeDecodeError):
                custom_name = "<unparseable>"

        sections.append(WasmSectionInfo(
            id=sec_id, name=name, offset=sec_start, size=sec_size, custom_name=custom_name,
        ))
        offset += sec_size

    return sections


def _count_wasm_functions(path: Path) -> tuple[int, int]:
    """Return (function_count, code_section_size) from the code section."""
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != WASM_MAGIC:
        return 0, 0

    offset = 8
    while offset < len(data):
        sec_id = data[offset]
        offset += 1
        sec_size, offset = _read_leb128_u32(data, offset)
        sec_start = offset

        if sec_id == 10:  # code section
            try:
                func_count, _ = _read_leb128_u32(data, sec_start)
                return func_count, sec_size
            except (IndexError, ValueError):
                return 0, sec_size

        offset += sec_size

    return 0, 0


def analyse_wasm(path: Path) -> dict:
    total_bytes = path.stat().st_size
    sections = _parse_wasm_sections(path)
    func_count, code_size = _count_wasm_functions(path)
    avg_func_size = code_size // func_count if func_count > 0 else 0

    by_type: dict[str, int] = {}
    for sec in sections:
        key = sec.name
        if sec.custom_name:
            key = f"custom:{sec.custom_name}"
        by_type[key] = by_type.get(key, 0) + sec.size

    return {
        "format": "wasm",
        "path": str(path),
        "total_bytes": total_bytes,
        "sections": sections,
        "by_type": by_type,
        "function_count": func_count,
        "code_size": code_size,
        "avg_function_size": avg_func_size,
    }


def print_wasm_report(analysis: dict) -> None:
    total = analysis["total_bytes"]
    by_type = analysis["by_type"]
    func_count = analysis["function_count"]
    avg_func = analysis["avg_function_size"]

    print("=" * 76)
    print(f"WASM Binary Size Analysis — {analysis['path']}")
    print(f"Total file size: {_fmt_bytes(total)} ({total:,} bytes)")
    print("=" * 76)

    print(f"\n{'Section':<35s} {'Size':>12s}  {'%':>6s}")
    print("-" * 57)
    for name, size in sorted(by_type.items(), key=lambda kv: -kv[1]):
        pct = size / total * 100 if total > 0 else 0
        bar = "#" * int(pct / 2)
        print(f"  {name:<33s} {_fmt_bytes(size):>12s}  {pct:>5.1f}%  {bar}")

    accounted = sum(s.size for s in analysis["sections"])
    overhead = total - accounted
    if overhead > 0:
        print(f"  {'<headers/padding>':<33s} {_fmt_bytes(overhead):>12s}  {overhead / total * 100:>5.1f}%")

    print("-" * 57)
    print(f"  {'TOTAL':<33s} {_fmt_bytes(total):>12s}")

    print(f"\n--- Code Section Details ---")
    print(f"  Function count:        {func_count:,}")
    print(f"  Code section size:     {_fmt_bytes(analysis['code_size'])}")
    print(f"  Avg function size:     {_fmt_bytes(avg_func)}")

    # Runtime vs user code estimate based on code vs data ratio
    code_total = by_type.get("code", 0)
    data_total = by_type.get("data", 0)
    custom_total = sum(v for k, v in by_type.items() if k.startswith("custom:"))
    structural = total - code_total - data_total - custom_total

    print(f"\n--- Estimated Contribution ---")
    print(f"  Code (functions):      {_fmt_bytes(code_total):>12s}  {_pct(code_total, total)}")
    print(f"  Data (constants/heap): {_fmt_bytes(data_total):>12s}  {_pct(data_total, total)}")
    print(f"  Custom sections:       {_fmt_bytes(custom_total):>12s}  {_pct(custom_total, total)}")
    print(f"  Structural/metadata:   {_fmt_bytes(structural):>12s}  {_pct(structural, total)}")
    print()


# ---------------------------------------------------------------------------
# Comparison mode
# ---------------------------------------------------------------------------

def compare_binaries(path_a: Path, path_b: Path) -> dict:
    """Compare two binaries and compute deltas."""
    fmt_a = detect_format(path_a)
    fmt_b = detect_format(path_b)

    size_a = path_a.stat().st_size
    size_b = path_b.stat().st_size
    delta = size_b - size_a

    result: dict = {
        "before": str(path_a),
        "after": str(path_b),
        "before_bytes": size_a,
        "after_bytes": size_b,
        "delta_bytes": delta,
        "delta_pct": (delta / size_a * 100) if size_a > 0 else 0,
    }

    # If both are same format, do deeper comparison
    if fmt_a == fmt_b == "wasm":
        a = analyse_wasm(path_a)
        b = analyse_wasm(path_b)

        section_delta: dict[str, dict] = {}
        all_keys = set(a["by_type"]) | set(b["by_type"])
        for key in sorted(all_keys):
            sa = a["by_type"].get(key, 0)
            sb = b["by_type"].get(key, 0)
            section_delta[key] = {"before": sa, "after": sb, "delta": sb - sa}

        result["section_deltas"] = section_delta
        result["function_count_before"] = a["function_count"]
        result["function_count_after"] = b["function_count"]

    elif fmt_a in ("macho", "elf") and fmt_b in ("macho", "elf"):
        a = analyse_native(path_a)
        b = analyse_native(path_b)

        cat_delta: dict[str, dict] = {}
        all_cats = set(a["category_totals"]) | set(b["category_totals"])
        for cat in sorted(all_cats):
            sa = a["category_totals"].get(cat, 0)
            sb = b["category_totals"].get(cat, 0)
            display = a["category_display"].get(cat) or b["category_display"].get(cat, cat)
            cat_delta[cat] = {"display": display, "before": sa, "after": sb, "delta": sb - sa}

        result["category_deltas"] = cat_delta

    return result


def print_comparison(comp: dict) -> None:
    delta = comp["delta_bytes"]
    sign = "+" if delta >= 0 else ""

    print("=" * 76)
    print("Binary Size Comparison")
    print("=" * 76)
    print(f"  Before: {comp['before']:<50s} {_fmt_bytes(comp['before_bytes'])}")
    print(f"  After:  {comp['after']:<50s} {_fmt_bytes(comp['after_bytes'])}")
    print(f"  Delta:  {sign}{_fmt_bytes(delta)} ({sign}{comp['delta_pct']:.1f}%)")
    print()

    if "section_deltas" in comp:
        print(f"{'Section':<30s} {'Before':>10s}  {'After':>10s}  {'Delta':>12s}")
        print("-" * 66)
        for key, d in sorted(comp["section_deltas"].items(), key=lambda kv: -abs(kv[1]["delta"])):
            dd = d["delta"]
            s = "+" if dd >= 0 else ""
            print(f"  {key:<28s} {_fmt_bytes(d['before']):>10s}  {_fmt_bytes(d['after']):>10s}  {s}{_fmt_bytes(dd):>10s}")
        print()
        print(f"  Functions: {comp.get('function_count_before', '?')} -> {comp.get('function_count_after', '?')}")

    if "category_deltas" in comp:
        print(f"{'Category':<30s} {'Before':>10s}  {'After':>10s}  {'Delta':>12s}")
        print("-" * 66)
        for key, d in sorted(comp["category_deltas"].items(), key=lambda kv: -abs(kv[1]["delta"])):
            dd = d["delta"]
            s = "+" if dd >= 0 else ""
            print(f"  {d['display']:<28s} {_fmt_bytes(d['before']):>10s}  {_fmt_bytes(d['after']):>10s}  {s}{_fmt_bytes(dd):>10s}")

    print()


# ---------------------------------------------------------------------------
# JSON output
# ---------------------------------------------------------------------------

def to_json(analysis: dict) -> dict:
    """Convert an analysis result to JSON-serialisable dict."""
    out = dict(analysis)
    # Remove non-serialisable objects
    if "symbols" in out:
        out["top_50_symbols"] = [
            {"name": s.name, "size": s.size, "kind": s.kind}
            for s in out.pop("symbols")[:50]
        ]
    if "sections" in out:
        out["sections"] = [
            {"id": s.id, "name": s.name, "size": s.size, "custom_name": s.custom_name}
            for s in out.pop("sections")
        ]
    # segments are already JSON-serialisable dicts
    return out


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyse binary size breakdown for Molt native and WASM binaries",
    )
    parser.add_argument("binary", type=Path, nargs="?", help="Path to binary file")
    parser.add_argument("--compare", nargs=2, metavar=("BEFORE", "AFTER"),
                        help="Compare two binaries and show deltas")
    parser.add_argument("--json", action="store_true", dest="json_output",
                        help="Output as JSON")
    parser.add_argument("--budget", type=str, default=None,
                        help=f"Size budget (e.g. '30MB'). Default: {DEFAULT_BUDGET_NATIVE_MB:.0f}MB native, {DEFAULT_BUDGET_WASM_MB:.0f}MB WASM")
    args = parser.parse_args()

    # Compare mode
    if args.compare:
        path_a, path_b = Path(args.compare[0]), Path(args.compare[1])
        for p in (path_a, path_b):
            if not p.is_file():
                print(f"ERROR: {p} not found", file=sys.stderr)
                sys.exit(1)
        comp = compare_binaries(path_a, path_b)
        if args.json_output:
            print(json.dumps(comp, indent=2))
        else:
            print_comparison(comp)
        return

    # Single binary mode
    if args.binary is None:
        parser.error("binary path is required (unless using --compare)")

    if not args.binary.is_file():
        print(f"ERROR: {args.binary} not found", file=sys.stderr)
        sys.exit(1)

    fmt = detect_format(args.binary)

    if fmt == "wasm":
        analysis = analyse_wasm(args.binary)
        if args.json_output:
            print(json.dumps(to_json(analysis), indent=2))
        else:
            print_wasm_report(analysis)
    elif fmt in ("macho", "elf"):
        analysis = analyse_native(args.binary)
        if args.json_output:
            print(json.dumps(to_json(analysis), indent=2))
        else:
            print_native_report(analysis)
    else:
        print(f"ERROR: Unrecognised binary format for {args.binary}", file=sys.stderr)
        sys.exit(1)

    # Budget check
    total = analysis["total_bytes"]
    default_mb = DEFAULT_BUDGET_WASM_MB if fmt == "wasm" else DEFAULT_BUDGET_NATIVE_MB
    budget_bytes = int(default_mb * 1024 * 1024)
    if args.budget:
        budget_bytes = _parse_size_spec(args.budget)

    if total > budget_bytes:
        over = total - budget_bytes
        print(f"BUDGET EXCEEDED: {_fmt_bytes(total)} > {_fmt_bytes(budget_bytes)} "
              f"(over by {_fmt_bytes(over)})")
        sys.exit(1)
    else:
        remaining = budget_bytes - total
        print(f"Budget OK: {_fmt_bytes(total)} / {_fmt_bytes(budget_bytes)} "
              f"({_fmt_bytes(remaining)} remaining)")


if __name__ == "__main__":
    main()
