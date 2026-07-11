#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import tomllib
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "config" / "cpython_coverage.toml"
STABLE_ABI = ROOT / "config" / "cpython_stable_abi_3_12.toml"
MATRIX = ROOT / "runtime" / "molt-cpython-abi" / "cpython_abi_coverage.generated.json"
DOC = ROOT / "docs" / "spec" / "areas" / "compat" / "surfaces" / "cpython_abi_coverage.generated.md"
AUDIT = ROOT / "docs" / "spec" / "areas" / "compat" / "surfaces" / "cpython_version_assumptions.generated.md"
SOURCE_ROOT = ROOT / "runtime" / "molt-cpython-abi"

VERSION_PATTERN = re.compile(r"3\.(?:12|13|14)|sys\.version_info|PY_VERSION_HEX|0x030[CDEcde]")
DECL_PATTERN = re.compile(
    r"#\[unsafe\(no_mangle\)\]\s*"
    r"(?:pub\s+)?(?:(?:unsafe)\s+)?(?:extern\s+\"C\"\s+)?"
    r"(fn|static(?:\s+mut)?)\s+([A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)


def _load_config() -> dict[str, object]:
    return tomllib.loads(CONFIG.read_text(encoding="utf-8"))


def _stable_abi_symbols() -> set[str]:
    data = tomllib.loads(STABLE_ABI.read_text(encoding="utf-8"))
    return {
        str(name)
        for kind in ("function", "data")
        for name in data.get(kind, {})
    }


def _stability(symbol: str, stable_abi: set[str] | None = None) -> str:
    if symbol.startswith("_Py"):
        return "private"
    if symbol.startswith("PyUnstable_"):
        return "unstable"
    if symbol.startswith("molt_"):
        return "molt-private"
    if symbol in (_stable_abi_symbols() if stable_abi is None else stable_abi):
        return "stable-abi"
    return "public-version-specific"


def _surface(symbol: str, declaration: str) -> str:
    if declaration.startswith("static"):
        if symbol.endswith("Type") or symbol.endswith("_Type"):
            return "type-object"
        return "data"
    return "function"


def coverage_complete(matrix_symbols: set[str], live_symbols: set[str]) -> bool:
    return matrix_symbols == live_symbols


def _exports() -> list[dict[str, object]]:
    rows: dict[str, dict[str, object]] = {}
    stable_abi = _stable_abi_symbols()
    for path in sorted((SOURCE_ROOT / "src").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        for match in DECL_PATTERN.finditer(text):
            declaration = match.group(1)
            symbol = match.group(2)
            rows[symbol] = {
                "symbol": symbol,
                "surface": _surface(symbol, declaration),
                "verified_at": "3.12",
                "stability": _stability(symbol, stable_abi),
                "source": path.relative_to(ROOT).as_posix(),
            }
    exports_file = SOURCE_ROOT / "shims" / "pyarg_variadic.exports"
    for raw in exports_file.read_text(encoding="utf-8").splitlines():
        symbol = raw.strip()
        if symbol and not symbol.startswith("#"):
            rows[symbol] = {
                "symbol": symbol,
                "surface": "function",
                "verified_at": "3.12",
                "stability": _stability(symbol, stable_abi),
                "source": exports_file.relative_to(ROOT).as_posix(),
            }
    return [rows[name] for name in sorted(rows)]


def _matrix(config: dict[str, object]) -> dict[str, object]:
    divergences = list(config.get("divergence", []))
    by_surface = {str(row["surface"]): row for row in divergences}
    symbols = _exports()
    for row in symbols:
        divergence = by_surface.get(str(row["symbol"]))
        if divergence:
            row["known_divergence"] = divergence
    structural = [
        {"symbol": "PyObject.layout", "surface": "layout", "verified_at": "3.12", "stability": "version-specific-public", "source": "runtime/molt-cpython-abi/src/abi_types.rs"},
        {"symbol": "PyTypeObject.layout", "surface": "layout", "verified_at": "3.12", "stability": "version-specific-public", "source": "runtime/molt-cpython-abi/src/abi_types.rs"},
        {"symbol": "Py_TPFLAGS", "surface": "flags", "verified_at": "3.12", "stability": "stable-values-version-specific-layout", "source": "runtime/molt-cpython-abi/include/Python.h"},
        {"symbol": "PyModuleDef_Slot", "surface": "module-slot", "verified_at": "3.12", "stability": "stable-abi", "source": "runtime/molt-cpython-abi/include/Python.h", "known_divergence": by_surface["PyModuleDef_Slot.Py_mod_gil"]},
        {"symbol": "object-header.immortal-refcount", "surface": "layout", "verified_at": "3.12", "stability": "private-representation", "source": "runtime/molt-cpython-abi/src/abi_types.rs", "known_divergence": by_surface["object-header.immortal-refcount"]},
    ]
    counts = Counter(str(row["stability"]) for row in symbols + structural)
    return {
        "schema_version": 1,
        "authority": "TargetPythonVersion",
        "verified_tuples": config.get("verified", []),
        "candidate_tuples": config.get("candidate", []),
        "counts": {"total": len(symbols) + len(structural), **dict(sorted(counts.items()))},
        "symbols": symbols,
        "structural_surfaces": structural,
        "future_divergences": divergences,
    }


def _render_doc(matrix: dict[str, object]) -> str:
    counts = matrix["counts"]
    lines = ["# CPython ABI Coverage (generated)", "", "Generated by `tools/gen_cpython_coverage.py`; do not edit by hand.", "", "## Verified boundary", ""]
    for row in matrix["verified_tuples"]:
        lines.append(f"- CPython {row['cpython']} on {row['platform']}: {row['evidence']}")
    lines.extend(["", "## Counts", "", f"- Total surfaces: {counts['total']}"])
    for key, value in counts.items():
        if key != "total":
            lines.append(f"- `{key}`: {value}")
    lines.extend(["", "## Contract", "", "Every exported C symbol discovered from `#[unsafe(no_mangle)]` declarations and the variadic shim export list is present in the JSON matrix. Structural layout, flag, and module-slot surfaces are included separately. Public symbols are conservatively classified as version-specific unless a stronger stable-ABI contract is explicitly represented; `_Py*` is private and `PyUnstable_*` is unstable.", "", "The matrix records 3.13/3.14 divergence surfaces for future work; it does not claim support for those versions."])
    return "\n".join(lines) + "\n"


def _audit() -> str:
    rows: list[tuple[str, int, str, str]] = []
    for base in (ROOT / "src", ROOT / "runtime", ROOT / "config", ROOT / "tools"):
        for path in sorted(base.rglob("*")):
            if path.suffix not in {".py", ".rs", ".c", ".h", ".toml"}:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            matches = list(VERSION_PATTERN.finditer(text))
            if not matches:
                continue
            routed = "TargetPythonVersion" in text or "target_python" in text or path == CONFIG
            operational = "uv run --python" in text or "PYTHON_VERSION" in text or "sys.version_info" in text
            classification = "authority-routed" if routed else ("operational-tooling" if operational else "scattered-assumption")
            rows.append((path.relative_to(ROOT).as_posix(), len(matches), classification, matches[0].group(0)))
    totals = Counter(row[2] for row in rows)
    lines = ["# CPython Version-Assumption Audit (generated)", "", "Generated by `tools/gen_cpython_coverage.py`; counts are files containing version literals.", "", "## Counts", ""]
    for key, value in sorted(totals.items()):
        lines.append(f"- `{key}` files: {value}")
    lines.extend(["", "## Files", "", "| File | Literals | Classification | First match |", "|---|---:|---|---|"])
    lines.extend(f"| `{path}` | {count} | {kind} | `{first}` |" for path, count, kind, first in rows)
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    config = _load_config()
    matrix = _matrix(config)
    outputs = {MATRIX: json.dumps(matrix, indent=2, sort_keys=True) + "\n", DOC: _render_doc(matrix), AUDIT: _audit()}
    stale = [path for path, content in outputs.items() if not path.exists() or path.read_text(encoding="utf-8") != content]
    if args.check:
        if stale:
            print("stale CPython coverage outputs: " + ", ".join(str(path.relative_to(ROOT)) for path in stale))
            return 1
        print("CPython coverage outputs are synchronized")
        return 0
    for path, content in outputs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
